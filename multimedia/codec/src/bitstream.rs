use crate::CodecError;
use waterkit_video_core::EncryptionSubsample;

const START_CODE: [u8; 4] = [0, 0, 0, 1];

/// Converts ISO BMFF length-prefixed H.264/H.265 access units to Annex B.
///
/// The parsed decoder configuration and parameter sets are retained so one
/// instance can adapt every sample in a track without reparsing `avcC`/`hvcC`.
#[derive(Debug)]
pub enum NalStreamConverter {
    /// Input samples already use Annex B start codes.
    AnnexB,
    /// Input samples carry fixed-width big-endian NAL lengths.
    LengthPrefixed {
        /// Width of each NAL length field.
        nal_length_size: usize,
        /// Annex B parameter sets extracted from decoder configuration.
        parameter_sets: Vec<u8>,
        #[cfg(any(test, target_os = "android"))]
        /// Boundary between primary and secondary platform codec data.
        secondary_parameter_set_offset: Option<usize>,
        #[cfg(any(test, target_os = "linux"))]
        /// Whether parameter sets have already been prefixed on Linux.
        sent_parameter_sets: bool,
    },
}

/// Annex B bytes and adjusted CENC subsample boundaries.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConvertedProtectedSample {
    data: Vec<u8>,
    subsamples: Vec<EncryptionSubsample>,
}

impl ConvertedProtectedSample {
    /// Returns the converted compressed access unit.
    #[must_use]
    pub fn data(&self) -> &[u8] {
        &self.data
    }

    /// Returns CENC ranges adjusted for converted start codes.
    #[must_use]
    pub fn subsamples(&self) -> &[EncryptionSubsample] {
        &self.subsamples
    }

    /// Consumes this value and returns its bytes and adjusted subsamples.
    #[must_use]
    pub fn into_parts(self) -> (Vec<u8>, Vec<EncryptionSubsample>) {
        (self.data, self.subsamples)
    }
}

impl NalStreamConverter {
    /// Creates a converter from an optional `avcC` or `hvcC` configuration.
    ///
    /// # Errors
    ///
    /// Returns an initialization error when a recognized configuration record
    /// is truncated or internally inconsistent.
    pub fn new(is_hevc: bool, config: Option<&[u8]>) -> Result<Self, CodecError> {
        let Some(config) = config else {
            return Ok(Self::AnnexB);
        };
        let parsed = if is_hevc {
            hvcc_payload(config).map(parse_h265_hvcc).transpose()?
        } else {
            avcc_payload(config).map(parse_h264_avcc).transpose()?
        };
        let Some(parsed) = parsed else {
            return Ok(Self::AnnexB);
        };
        Ok(Self::LengthPrefixed {
            nal_length_size: parsed.nal_length_size,
            parameter_sets: parsed.parameter_sets,
            #[cfg(any(test, target_os = "android"))]
            secondary_parameter_set_offset: parsed.secondary_parameter_set_offset,
            #[cfg(any(test, target_os = "linux"))]
            sent_parameter_sets: false,
        })
    }

    #[cfg(any(test, target_os = "windows"))]
    /// Returns Annex B parameter sets extracted from decoder configuration.
    #[must_use]
    pub fn parameter_sets(&self) -> &[u8] {
        match self {
            Self::AnnexB => &[],
            Self::LengthPrefixed { parameter_sets, .. } => parameter_sets,
        }
    }

    #[cfg(any(test, target_os = "android"))]
    /// Returns platform codec-specific data split into primary and secondary sets.
    #[must_use]
    pub fn codec_specific_data(&self) -> (Option<&[u8]>, Option<&[u8]>) {
        match self {
            Self::AnnexB => (None, None),
            Self::LengthPrefixed {
                parameter_sets,
                secondary_parameter_set_offset: None,
                ..
            } => (Some(parameter_sets), None),
            Self::LengthPrefixed {
                parameter_sets,
                secondary_parameter_set_offset: Some(offset),
                ..
            } => (
                Some(&parameter_sets[..*offset]),
                Some(&parameter_sets[*offset..]),
            ),
        }
    }

    #[cfg(any(test, target_os = "android", target_os = "windows"))]
    /// Converts one clear access unit to Annex B.
    ///
    /// # Errors
    ///
    /// Returns a decoding error when a length-prefixed sample is malformed.
    pub fn convert_sample(&mut self, sample: &[u8]) -> Result<Vec<u8>, CodecError> {
        match self {
            Self::AnnexB => Ok(sample.to_vec()),
            Self::LengthPrefixed {
                nal_length_size, ..
            } => length_prefixed_to_annex_b(sample, *nal_length_size),
        }
    }

    /// Converts one potentially encrypted access unit while preserving its
    /// exact clear/encrypted byte topology.
    ///
    /// NAL length fields must be clear so their boundaries can be parsed. The
    /// returned CENC subsample list accounts for any size difference between
    /// the original length fields and four-byte Annex B start codes.
    ///
    /// # Errors
    ///
    /// Returns a decoding error for malformed sample boundaries, encrypted NAL
    /// length fields, or subsample counts that do not cover the access unit.
    pub fn convert_protected_sample(
        &mut self,
        sample: &[u8],
        subsamples: &[EncryptionSubsample],
    ) -> Result<ConvertedProtectedSample, CodecError> {
        match self {
            Self::AnnexB => Ok(ConvertedProtectedSample {
                data: sample.to_vec(),
                subsamples: subsamples.to_vec(),
            }),
            Self::LengthPrefixed {
                nal_length_size, ..
            } => length_prefixed_protected_to_annex_b(sample, *nal_length_size, subsamples),
        }
    }

    #[cfg(any(test, target_os = "linux"))]
    /// Converts one access unit and prepends configuration parameter sets once.
    ///
    /// # Errors
    ///
    /// Returns a decoding error when a length-prefixed sample is malformed.
    pub fn convert_sample_with_parameter_sets(
        &mut self,
        sample: &[u8],
    ) -> Result<Vec<u8>, CodecError> {
        match self {
            Self::AnnexB => Ok(sample.to_vec()),
            Self::LengthPrefixed {
                nal_length_size,
                parameter_sets,
                sent_parameter_sets,
                ..
            } => {
                let mut annex_b = length_prefixed_to_annex_b(sample, *nal_length_size)?;
                if *sent_parameter_sets {
                    return Ok(annex_b);
                }
                *sent_parameter_sets = true;
                let mut prefixed = Vec::with_capacity(parameter_sets.len() + annex_b.len());
                prefixed.extend_from_slice(parameter_sets);
                prefixed.append(&mut annex_b);
                Ok(prefixed)
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ProtectionRun {
    encrypted: bool,
    length: usize,
}

fn length_prefixed_protected_to_annex_b(
    sample: &[u8],
    nal_length_size: usize,
    subsamples: &[EncryptionSubsample],
) -> Result<ConvertedProtectedSample, CodecError> {
    let source_runs = protection_runs(sample.len(), subsamples)?;
    let mut source_offset = 0;
    let mut data = Vec::with_capacity(sample.len() + 64);
    let mut output_runs = Vec::new();
    while source_offset + nal_length_size <= sample.len() {
        if !range_is_clear(&source_runs, source_offset, nal_length_size) {
            return Err(CodecError::DecodingFailed(format!(
                "protected NAL length field at byte {source_offset} is encrypted"
            )));
        }
        let nal_len = read_length_field(&sample[source_offset..source_offset + nal_length_size]);
        source_offset += nal_length_size;
        if nal_len == 0 {
            continue;
        }
        let end = source_offset.checked_add(nal_len).ok_or_else(|| {
            CodecError::DecodingFailed("protected NAL size overflowed usize".into())
        })?;
        let nal = sample.get(source_offset..end).ok_or_else(|| {
            CodecError::DecodingFailed("protected sample has a truncated NAL unit".into())
        })?;
        data.extend_from_slice(&START_CODE);
        push_run(&mut output_runs, false, START_CODE.len());
        data.extend_from_slice(nal);
        append_source_runs(&source_runs, source_offset, end, &mut output_runs);
        source_offset = end;
    }
    if source_offset != sample.len() {
        return Err(CodecError::DecodingFailed(
            "protected length-prefixed sample has trailing bytes".into(),
        ));
    }
    Ok(ConvertedProtectedSample {
        data,
        subsamples: runs_to_subsamples(&output_runs)?,
    })
}

fn protection_runs(
    sample_len: usize,
    subsamples: &[EncryptionSubsample],
) -> Result<Vec<ProtectionRun>, CodecError> {
    if subsamples.is_empty() {
        return Ok(vec![ProtectionRun {
            encrypted: true,
            length: sample_len,
        }]);
    }
    let mut runs = Vec::with_capacity(subsamples.len() * 2);
    let mut covered = 0_usize;
    for subsample in subsamples {
        let clear = usize::from(subsample.clear_bytes());
        let encrypted = usize::try_from(subsample.encrypted_bytes()).map_err(|_| {
            CodecError::DecodingFailed("CENC encrypted byte count exceeds usize".into())
        })?;
        covered = covered
            .checked_add(clear)
            .and_then(|value| value.checked_add(encrypted))
            .ok_or_else(|| {
                CodecError::DecodingFailed("CENC subsample coverage overflowed usize".into())
            })?;
        push_run(&mut runs, false, clear);
        push_run(&mut runs, true, encrypted);
    }
    if covered != sample_len {
        return Err(CodecError::DecodingFailed(format!(
            "CENC subsamples cover {covered} bytes but the sample has {sample_len}"
        )));
    }
    Ok(runs)
}

fn push_run(runs: &mut Vec<ProtectionRun>, encrypted: bool, length: usize) {
    if length == 0 {
        return;
    }
    if let Some(last) = runs.last_mut()
        && last.encrypted == encrypted
    {
        last.length = last
            .length
            .checked_add(length)
            .expect("validated sample run length must fit usize");
        return;
    }
    runs.push(ProtectionRun { encrypted, length });
}

fn range_is_clear(runs: &[ProtectionRun], start: usize, length: usize) -> bool {
    let end = start
        .checked_add(length)
        .expect("validated sample range must fit usize");
    let mut offset = 0_usize;
    for run in runs {
        let run_end = offset + run.length;
        if offset < end && run_end > start && run.encrypted {
            return false;
        }
        if run_end >= end {
            return true;
        }
        offset = run_end;
    }
    false
}

fn append_source_runs(
    runs: &[ProtectionRun],
    start: usize,
    end: usize,
    output: &mut Vec<ProtectionRun>,
) {
    let mut offset = 0_usize;
    for run in runs {
        let run_end = offset + run.length;
        let overlap_start = start.max(offset);
        let overlap_end = end.min(run_end);
        if overlap_start < overlap_end {
            push_run(output, run.encrypted, overlap_end - overlap_start);
        }
        if run_end >= end {
            return;
        }
        offset = run_end;
    }
}

fn runs_to_subsamples(runs: &[ProtectionRun]) -> Result<Vec<EncryptionSubsample>, CodecError> {
    let mut subsamples = Vec::new();
    let mut clear = 0_usize;
    for run in runs {
        if run.encrypted {
            push_subsample_chunks(&mut subsamples, clear, run.length)?;
            clear = 0;
        } else {
            clear = clear.checked_add(run.length).ok_or_else(|| {
                CodecError::DecodingFailed("clear CENC run overflowed usize".into())
            })?;
        }
    }
    if clear > 0 {
        push_subsample_chunks(&mut subsamples, clear, 0)?;
    }
    Ok(subsamples)
}

fn push_subsample_chunks(
    output: &mut Vec<EncryptionSubsample>,
    mut clear: usize,
    mut encrypted: usize,
) -> Result<(), CodecError> {
    while clear > usize::from(u16::MAX) {
        output.push(EncryptionSubsample::new(u16::MAX, 0));
        clear -= usize::from(u16::MAX);
    }
    let first_encrypted = encrypted.min(u32::MAX as usize);
    output.push(EncryptionSubsample::new(
        u16::try_from(clear).expect("clear chunk is bounded to u16"),
        u32::try_from(first_encrypted)
            .map_err(|_| CodecError::DecodingFailed("encrypted CENC run exceeds u32".into()))?,
    ));
    encrypted -= first_encrypted;
    while encrypted > 0 {
        let chunk = encrypted.min(u32::MAX as usize);
        output.push(EncryptionSubsample::new(
            0,
            u32::try_from(chunk)
                .map_err(|_| CodecError::DecodingFailed("encrypted CENC run exceeds u32".into()))?,
        ));
        encrypted -= chunk;
    }
    Ok(())
}

struct ParsedConfiguration {
    nal_length_size: usize,
    parameter_sets: Vec<u8>,
    #[cfg(any(test, target_os = "android"))]
    secondary_parameter_set_offset: Option<usize>,
}

fn avcc_payload(config: &[u8]) -> Option<&[u8]> {
    if config.len() >= 8 && &config[4..8] == b"avcC" {
        return Some(&config[8..]);
    }
    (config.len() >= 7 && config[0] == 1).then_some(config)
}

fn hvcc_payload(config: &[u8]) -> Option<&[u8]> {
    if config.len() >= 8 && &config[4..8] == b"hvcC" {
        return Some(&config[8..]);
    }
    (config.len() >= 23 && config[0] == 1).then_some(config)
}

fn parse_h264_avcc(payload: &[u8]) -> Result<ParsedConfiguration, CodecError> {
    if payload.len() < 7 {
        return Err(CodecError::InitializationFailed(
            "invalid avcC payload: too short".into(),
        ));
    }
    let nal_length_size = ((payload[4] & 0x03) + 1) as usize;
    let mut cursor = 6;
    let num_sps = (payload[5] & 0x1f) as usize;
    let mut parameter_sets = Vec::new();
    for _ in 0..num_sps {
        append_parameter_set(payload, &mut cursor, "SPS", &mut parameter_sets)?;
    }
    #[cfg(any(test, target_os = "android"))]
    let secondary_parameter_set_offset = parameter_sets.len();
    let pps_count = *payload.get(cursor).ok_or_else(|| {
        CodecError::InitializationFailed("invalid avcC payload: missing PPS count".into())
    })? as usize;
    cursor += 1;
    for _ in 0..pps_count {
        append_parameter_set(payload, &mut cursor, "PPS", &mut parameter_sets)?;
    }
    Ok(ParsedConfiguration {
        nal_length_size,
        parameter_sets,
        #[cfg(any(test, target_os = "android"))]
        secondary_parameter_set_offset: Some(secondary_parameter_set_offset),
    })
}

fn parse_h265_hvcc(payload: &[u8]) -> Result<ParsedConfiguration, CodecError> {
    if payload.len() < 23 {
        return Err(CodecError::InitializationFailed(
            "invalid hvcC payload: too short".into(),
        ));
    }
    let nal_length_size = ((payload[21] & 0x03) + 1) as usize;
    let mut cursor = 23;
    let num_arrays = payload[22] as usize;
    let mut parameter_sets = Vec::new();
    for _ in 0..num_arrays {
        if cursor + 3 > payload.len() {
            return Err(CodecError::InitializationFailed(
                "invalid hvcC payload: truncated NAL array header".into(),
            ));
        }
        cursor += 1;
        let count = u16::from_be_bytes([payload[cursor], payload[cursor + 1]]) as usize;
        cursor += 2;
        for _ in 0..count {
            append_parameter_set(payload, &mut cursor, "HEVC NAL", &mut parameter_sets)?;
        }
    }
    Ok(ParsedConfiguration {
        nal_length_size,
        parameter_sets,
        #[cfg(any(test, target_os = "android"))]
        secondary_parameter_set_offset: None,
    })
}

fn append_parameter_set(
    data: &[u8],
    cursor: &mut usize,
    label: &str,
    output: &mut Vec<u8>,
) -> Result<(), CodecError> {
    if *cursor + 2 > data.len() {
        return Err(CodecError::InitializationFailed(format!(
            "invalid config payload: missing {label} length"
        )));
    }
    let len = u16::from_be_bytes([data[*cursor], data[*cursor + 1]]) as usize;
    *cursor += 2;
    let end = cursor.checked_add(len).ok_or_else(|| {
        CodecError::InitializationFailed(format!("invalid config payload: {label} overflow"))
    })?;
    let nal = data.get(*cursor..end).ok_or_else(|| {
        CodecError::InitializationFailed(format!("invalid config payload: truncated {label}"))
    })?;
    output.extend_from_slice(&START_CODE);
    output.extend_from_slice(nal);
    *cursor = end;
    Ok(())
}

fn length_prefixed_to_annex_b(
    sample: &[u8],
    nal_length_size: usize,
) -> Result<Vec<u8>, CodecError> {
    let mut offset = 0;
    let mut output = Vec::with_capacity(sample.len() + 64);
    while offset + nal_length_size <= sample.len() {
        let nal_len = read_length_field(&sample[offset..offset + nal_length_size]);
        offset += nal_length_size;
        if nal_len == 0 {
            continue;
        }
        let end = offset.checked_add(nal_len).ok_or_else(|| {
            CodecError::DecodingFailed("length-prefixed NAL size overflowed usize".into())
        })?;
        let nal = sample.get(offset..end).ok_or_else(|| {
            CodecError::DecodingFailed("length-prefixed sample has a truncated NAL unit".into())
        })?;
        output.extend_from_slice(&START_CODE);
        output.extend_from_slice(nal);
        offset = end;
    }
    if offset != sample.len() {
        return Err(CodecError::DecodingFailed(
            "length-prefixed sample has trailing bytes".into(),
        ));
    }
    Ok(output)
}

fn read_length_field(bytes: &[u8]) -> usize {
    match bytes {
        [a] => *a as usize,
        [a, b] => u16::from_be_bytes([*a, *b]) as usize,
        [a, b, c] => ((*a as usize) << 16) | ((*b as usize) << 8) | (*c as usize),
        [a, b, c, d] => u32::from_be_bytes([*a, *b, *c, *d]) as usize,
        _ => unreachable!("NAL length size is encoded in two bits and must be 1..=4"),
    }
}

#[cfg(test)]
mod tests {
    use super::NalStreamConverter;
    use waterkit_video_core::EncryptionSubsample;

    #[test]
    fn converts_length_prefixed_avc_and_prepends_parameter_sets_once() {
        let avcc = [
            1, 100, 0, 40, 0xff, 0xe1, 0, 2, 0x67, 0x64, 1, 0, 2, 0x68, 0xee,
        ];
        let mut converter =
            NalStreamConverter::new(false, Some(&avcc)).expect("valid avcC configuration");
        assert_eq!(
            converter.parameter_sets(),
            [0, 0, 0, 1, 0x67, 0x64, 0, 0, 0, 1, 0x68, 0xee,]
        );
        assert_eq!(
            converter.codec_specific_data(),
            (
                Some(&[0, 0, 0, 1, 0x67, 0x64][..]),
                Some(&[0, 0, 0, 1, 0x68, 0xee][..])
            )
        );
        let sample = converter
            .convert_sample(&[0, 0, 0, 2, 0x65, 0x01])
            .expect("valid length-prefixed sample");
        assert_eq!(sample, [0, 0, 0, 1, 0x65, 0x01]);
        let first = converter
            .convert_sample_with_parameter_sets(&[0, 0, 0, 2, 0x65, 0x01])
            .expect("valid length-prefixed sample");
        assert_eq!(
            first,
            [
                0, 0, 0, 1, 0x67, 0x64, 0, 0, 0, 1, 0x68, 0xee, 0, 0, 0, 1, 0x65, 0x01,
            ]
        );
        let second = converter
            .convert_sample_with_parameter_sets(&[0, 0, 0, 1, 0x09])
            .expect("valid second sample");
        assert_eq!(second, [0, 0, 0, 1, 0x09]);
    }

    #[test]
    fn protected_conversion_preserves_encrypted_payload_and_adjusts_clear_prefixes() {
        let avcc = [
            1, 100, 0, 40, 0xfd, 0xe1, 0, 2, 0x67, 0x64, 1, 0, 2, 0x68, 0xee,
        ];
        let mut converter =
            NalStreamConverter::new(false, Some(&avcc)).expect("valid avcC configuration");
        let sample = [0, 3, 0x65, 0xaa, 0xbb, 0, 2, 0x41, 0xcc];
        let protected_sample = converter
            .convert_protected_sample(
                &sample,
                &[
                    EncryptionSubsample::new(3, 2),
                    EncryptionSubsample::new(3, 1),
                ],
            )
            .expect("clear NAL lengths must be convertible");
        assert_eq!(
            protected_sample.data(),
            [0, 0, 0, 1, 0x65, 0xaa, 0xbb, 0, 0, 0, 1, 0x41, 0xcc]
        );
        assert_eq!(
            protected_sample.subsamples(),
            [
                EncryptionSubsample::new(5, 2),
                EncryptionSubsample::new(5, 1),
            ]
        );
    }

    #[test]
    fn protected_conversion_rejects_encrypted_nal_length() {
        let avcc = [
            1, 100, 0, 40, 0xff, 0xe1, 0, 2, 0x67, 0x64, 1, 0, 2, 0x68, 0xee,
        ];
        let mut converter =
            NalStreamConverter::new(false, Some(&avcc)).expect("valid avcC configuration");
        let error = converter
            .convert_protected_sample(&[0, 0, 0, 2, 0x65, 0xaa], &[EncryptionSubsample::new(0, 6)])
            .expect_err("encrypted framing cannot be parsed");
        assert!(error.to_string().contains("NAL length field"));
    }
}
