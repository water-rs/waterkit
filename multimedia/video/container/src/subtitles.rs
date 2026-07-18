use std::{path::Path, time::Duration};

use quick_xml::{
    Reader, XmlVersion,
    escape::resolve_predefined_entity,
    events::{BytesRef, Event},
};
use subtp::{
    srt::{SrtTimestamp, SubRip},
    vtt::{VttBlock, VttTimestamp, WebVtt},
};
use waterkit_video_core::Error;

const MPEG_TIMESTAMP_TIMESCALE: i128 = 90_000;
const MPEG_TIMESTAMP_WRAP: i128 = 1_i128 << 33;

/// One decoded sidecar or segmented subtitle cue.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubtitleCue {
    /// Inclusive cue start time.
    pub start: Duration,
    /// Exclusive cue end time.
    pub end: Duration,
    /// Cue text payload.
    pub text: String,
}

impl SubtitleCue {
    /// Shifts this cue forward by a presentation offset.
    ///
    /// # Errors
    ///
    /// Returns a container error when the shifted interval overflows.
    pub fn shift_by(self, offset: Duration) -> Result<Self, Error> {
        self.shift_nanos(duration_nanos(offset)?)
    }

    fn shift_nanos(mut self, offset: i128) -> Result<Self, Error> {
        self.start = shifted_duration(self.start, offset)?;
        self.end = shifted_duration(self.end, offset)?;
        Ok(self)
    }
}

/// Reads and parses a `WebVTT`, `SubRip`, or TTML sidecar subtitle file.
///
/// # Errors
///
/// Returns a container error when the file cannot be read or its subtitle
/// syntax is invalid.
pub fn parse_subtitles_from_path(path: &Path) -> Result<Vec<SubtitleCue>, Error> {
    let document = std::fs::read_to_string(path).map_err(|error| {
        Error::Container(format!(
            "failed to read subtitle file {}: {error}",
            path.display()
        ))
    })?;
    let normalized = normalize_document(&document);
    match path
        .extension()
        .and_then(|extension| extension.to_str())
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("vtt") => parse_webvtt_document(&normalized),
        Some("srt") => parse_subrip_document(&normalized),
        Some("ttml" | "xml" | "dfxp") => parse_ttml_document(&normalized),
        _ if normalized.trim_start().starts_with("WEBVTT") => parse_webvtt_document(&normalized),
        _ if normalized.trim_start().starts_with('<') => parse_ttml_document(&normalized),
        _ => parse_subrip_document(&normalized).map_err(|_| {
            Error::Unsupported(format!(
                "unsupported subtitle format for {}: expected WebVTT, SubRip, or TTML",
                path.display()
            ))
        }),
    }
}

/// Parses a complete `WebVTT` document into presentation cues.
///
/// # Errors
///
/// Returns a container error for invalid UTF-8-normalized `WebVTT` syntax or cue timing.
pub fn parse_webvtt_document(document: &str) -> Result<Vec<SubtitleCue>, Error> {
    let mut sanitized = document
        .lines()
        .filter(|line| !line.trim_start().starts_with("X-TIMESTAMP-MAP="))
        .collect::<Vec<_>>()
        .join("\n");
    while !sanitized.ends_with("\n\n") {
        sanitized.push('\n');
    }
    let parsed = WebVtt::parse(&sanitized)
        .map_err(|error| Error::Container(format!("invalid WebVTT document: {error}")))?;
    parsed
        .blocks
        .into_iter()
        .filter_map(|block| match block {
            VttBlock::Que(cue) => Some(cue),
            VttBlock::Comment(_) | VttBlock::Style(_) | VttBlock::Region(_) => None,
        })
        .map(|cue| {
            let start = vtt_timestamp(cue.timings.start)?;
            let end = vtt_timestamp(cue.timings.end)?;
            validate_cue(start, end, cue.payload.join("\n"))
        })
        .collect()
}

/// Parses one HLS `WebVTT` segment and maps local cue time to presentation time.
///
/// When `X-TIMESTAMP-MAP` is present, the 33-bit MPEG timestamp is unwrapped
/// around `segment_start`. Without a map, cue timestamps are relative to the
/// media-playlist segment start.
///
/// # Errors
///
/// Returns a container error for invalid `WebVTT` or timestamp-map syntax.
pub fn parse_hls_webvtt_segment(
    document: &str,
    segment_start: Duration,
) -> Result<Vec<SubtitleCue>, Error> {
    let cues = parse_webvtt_document(document)?;
    let offset = webvtt_timestamp_map(document)?.map_or_else(
        || duration_nanos(segment_start),
        |(local, mpegts)| {
            let local_ticks = duration_to_mpeg_ticks(local);
            let anchor_ticks = duration_to_mpeg_ticks(segment_start)
                .checked_add(local_ticks)
                .ok_or_else(|| {
                    Error::Container(String::from("WebVTT timestamp anchor overflow"))
                })?;
            let unwrapped = unwrap_mpeg_timestamp(i128::from(mpegts), anchor_ticks);
            let offset_ticks = unwrapped.checked_sub(local_ticks).ok_or_else(|| {
                Error::Container(String::from("WebVTT timestamp-map offset overflow"))
            })?;
            offset_ticks
                .checked_mul(1_000_000_000)
                .map(|nanos| nanos / MPEG_TIMESTAMP_TIMESCALE)
                .ok_or_else(|| Error::Container(String::from("WebVTT timestamp-map overflow")))
        },
    )?;
    cues.into_iter()
        .map(|cue| cue.shift_nanos(offset))
        .collect()
}

/// Parses a complete `SubRip` document into presentation cues.
///
/// # Errors
///
/// Returns a container error for invalid `SubRip` syntax or cue timing.
pub fn parse_subrip_document(document: &str) -> Result<Vec<SubtitleCue>, Error> {
    SubRip::parse(document)
        .map_err(|error| Error::Container(format!("invalid SubRip document: {error}")))?
        .subtitles
        .into_iter()
        .map(|cue| {
            validate_cue(
                srt_timestamp(cue.start)?,
                srt_timestamp(cue.end)?,
                cue.text.join("\n"),
            )
        })
        .collect()
}

/// Parses a TTML/DFXP document into presentation cues.
///
/// The timing parser supports TTML clock, frame, tick, and offset time
/// expressions. Nested timing on `tt`, `body`, and `div` is inherited by `p`
/// cues, while inline spans and line breaks are flattened to readable text.
///
/// # Errors
///
/// Returns a container error for malformed XML, unsupported time expressions,
/// nested cue paragraphs, or cues without a finite positive interval.
pub fn parse_ttml_document(document: &str) -> Result<Vec<SubtitleCue>, Error> {
    let mut reader = Reader::from_str(document);
    reader.config_mut().trim_text(false);
    let mut timing = TtmlTimingParameters::default();
    let mut stack = vec![TtmlInterval::root()];
    let mut cue = None::<PendingTtmlCue>;
    let mut cues = Vec::new();

    loop {
        match reader
            .read_event()
            .map_err(|error| Error::Container(format!("invalid TTML XML: {error}")))?
        {
            Event::Start(element) => {
                if element.local_name().as_ref() == b"tt" {
                    timing = TtmlTimingParameters::from_root(&element, &reader)?;
                }
                let parent = stack.last().copied().ok_or_else(|| {
                    Error::Container(String::from("TTML timing stack lost its root interval"))
                })?;
                let interval = TtmlInterval::from_element(&element, &reader, parent, timing)?;
                if element.local_name().as_ref() == b"p" {
                    if cue.is_some() {
                        return Err(Error::Container(String::from(
                            "TTML cue paragraphs must not be nested",
                        )));
                    }
                    cue = Some(PendingTtmlCue::new(interval)?);
                }
                stack.push(interval);
            }
            Event::Empty(element) => {
                if element.local_name().as_ref() == b"br"
                    && let Some(cue) = cue.as_mut()
                {
                    cue.text.push('\n');
                }
            }
            Event::Text(text) => {
                if let Some(cue) = cue.as_mut() {
                    let decoded = text
                        .xml10_content()
                        .map_err(|error| Error::Container(format!("invalid TTML text: {error}")))?;
                    cue.text.push_str(&decoded);
                }
            }
            Event::CData(text) => {
                if let Some(cue) = cue.as_mut() {
                    let decoded = text.xml10_content().map_err(|error| {
                        Error::Container(format!("invalid TTML CDATA: {error}"))
                    })?;
                    cue.text.push_str(&decoded);
                }
            }
            Event::GeneralRef(reference) => {
                if let Some(cue) = cue.as_mut() {
                    cue.text.push_str(&resolve_ttml_reference(&reference)?);
                }
            }
            Event::End(element) => {
                let interval = stack
                    .pop()
                    .ok_or_else(|| Error::Container(String::from("TTML timing stack underflow")))?;
                if element.local_name().as_ref() == b"p" {
                    let pending = cue.take().ok_or_else(|| {
                        Error::Container(String::from("TTML paragraph closed without a cue"))
                    })?;
                    if pending.interval != interval {
                        return Err(Error::Container(String::from(
                            "TTML paragraph timing stack is inconsistent",
                        )));
                    }
                    let text = normalize_ttml_text(&pending.text);
                    if !text.is_empty() {
                        cues.push(validate_cue(
                            pending.interval.begin,
                            pending.interval.end.ok_or_else(|| {
                                Error::Container(String::from(
                                    "TTML cue requires end or duration timing",
                                ))
                            })?,
                            text,
                        )?);
                    }
                }
            }
            Event::Eof => break,
            Event::Decl(_) | Event::PI(_) | Event::DocType(_) | Event::Comment(_) => {}
        }
    }
    if stack.len() != 1 {
        return Err(Error::Container(String::from(
            "TTML document ended with unclosed elements",
        )));
    }
    Ok(cues)
}

fn resolve_ttml_reference(reference: &BytesRef<'_>) -> Result<String, Error> {
    if let Some(character) = reference
        .resolve_char_ref()
        .map_err(|error| Error::Container(format!("invalid TTML character reference: {error}")))?
    {
        return Ok(character.to_string());
    }
    let name = reference
        .decode()
        .map_err(|error| Error::Container(format!("invalid TTML entity reference: {error}")))?;
    resolve_predefined_entity(&name)
        .map(ToOwned::to_owned)
        .ok_or_else(|| Error::Container(format!("unsupported TTML entity reference &{name};")))
}

/// Returns the text active at a presentation timestamp.
#[must_use]
pub fn active_subtitle_text(cues: &[SubtitleCue], position: Duration) -> Option<&str> {
    cues.iter()
        .rev()
        .find(|cue| cue.start <= position && position < cue.end)
        .map(|cue| cue.text.as_str())
}

fn normalize_document(document: &str) -> String {
    document
        .strip_prefix('\u{feff}')
        .unwrap_or(document)
        .replace("\r\n", "\n")
        .replace('\r', "\n")
}

fn validate_cue(start: Duration, end: Duration, text: String) -> Result<SubtitleCue, Error> {
    if end <= start {
        return Err(Error::Container(format!(
            "subtitle cue end {end:?} must be after start {start:?}"
        )));
    }
    Ok(SubtitleCue { start, end, text })
}

fn timestamp_duration(
    hours: u8,
    minutes: u8,
    seconds: u8,
    milliseconds: u16,
) -> Result<Duration, Error> {
    let seconds = u64::from(hours)
        .checked_mul(3_600)
        .and_then(|value| value.checked_add(u64::from(minutes) * 60))
        .and_then(|value| value.checked_add(u64::from(seconds)))
        .ok_or_else(|| Error::Container(String::from("subtitle timestamp overflow")))?;
    Ok(Duration::from_secs(seconds).saturating_add(Duration::from_millis(u64::from(milliseconds))))
}

fn vtt_timestamp(value: VttTimestamp) -> Result<Duration, Error> {
    timestamp_duration(
        value.hours,
        value.minutes,
        value.seconds,
        value.milliseconds,
    )
}

fn srt_timestamp(value: SrtTimestamp) -> Result<Duration, Error> {
    timestamp_duration(
        value.hours,
        value.minutes,
        value.seconds,
        value.milliseconds,
    )
}

fn duration_nanos(value: Duration) -> Result<i128, Error> {
    i128::try_from(value.as_nanos())
        .map_err(|_| Error::Container(String::from("subtitle duration exceeds i128 nanoseconds")))
}

fn shifted_duration(value: Duration, offset: i128) -> Result<Duration, Error> {
    let nanos = duration_nanos(value)?
        .checked_add(offset)
        .ok_or_else(|| Error::Container(String::from("shifted subtitle timestamp overflow")))?;
    let nanos = u64::try_from(nanos).map_err(|_| {
        Error::Container(String::from(
            "shifted subtitle timestamp is negative or exceeds u64",
        ))
    })?;
    Ok(Duration::from_nanos(nanos))
}

fn duration_to_mpeg_ticks(value: Duration) -> i128 {
    i128::from(value.as_secs()) * MPEG_TIMESTAMP_TIMESCALE
        + i128::from(value.subsec_nanos()) * MPEG_TIMESTAMP_TIMESCALE / 1_000_000_000
}

const fn unwrap_mpeg_timestamp(raw: i128, target: i128) -> i128 {
    let wrap_count = (target - raw).div_euclid(MPEG_TIMESTAMP_WRAP);
    let lower = raw + wrap_count * MPEG_TIMESTAMP_WRAP;
    let upper = lower + MPEG_TIMESTAMP_WRAP;
    if (target - lower).abs() <= (upper - target).abs() {
        lower
    } else {
        upper
    }
}

fn webvtt_timestamp_map(document: &str) -> Result<Option<(Duration, u64)>, Error> {
    let Some(line) = document
        .lines()
        .map(str::trim)
        .find(|line| line.starts_with("X-TIMESTAMP-MAP="))
    else {
        return Ok(None);
    };
    let mut local = None;
    let mut mpegts = None;
    for field in line.trim_start_matches("X-TIMESTAMP-MAP=").split(',') {
        let (name, value) = field.split_once(':').ok_or_else(|| {
            Error::Container(format!("invalid WebVTT timestamp-map field {field:?}"))
        })?;
        match name.trim() {
            "LOCAL" => {
                local = Some(parse_clock_time(
                    value.trim(),
                    TtmlTimingParameters::default(),
                )?);
            }
            "MPEGTS" => {
                let value = value.trim().parse::<u64>().map_err(|error| {
                    Error::Container(format!("invalid WebVTT MPEGTS timestamp: {error}"))
                })?;
                if u128::from(value) >= (1_u128 << 33) {
                    return Err(Error::Container(String::from(
                        "WebVTT MPEGTS timestamp exceeds 33 bits",
                    )));
                }
                mpegts = Some(value);
            }
            other => {
                return Err(Error::Container(format!(
                    "unsupported WebVTT timestamp-map field {other:?}"
                )));
            }
        }
    }
    Ok(Some((
        local.ok_or_else(|| Error::Container(String::from("WebVTT timestamp map has no LOCAL")))?,
        mpegts
            .ok_or_else(|| Error::Container(String::from("WebVTT timestamp map has no MPEGTS")))?,
    )))
}

#[derive(Debug, Clone, Copy)]
struct TtmlTimingParameters {
    frames_per_second: f64,
    subframes_per_frame: f64,
    ticks_per_second: f64,
}

impl Default for TtmlTimingParameters {
    fn default() -> Self {
        Self {
            frames_per_second: 30.0,
            subframes_per_frame: 1.0,
            ticks_per_second: 1.0,
        }
    }
}

impl TtmlTimingParameters {
    fn from_root(
        element: &quick_xml::events::BytesStart<'_>,
        reader: &Reader<&[u8]>,
    ) -> Result<Self, Error> {
        let mut result = Self::default();
        for attribute in element.attributes() {
            let attribute = attribute
                .map_err(|error| Error::Container(format!("invalid TTML attribute: {error}")))?;
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| Error::Container(format!("invalid TTML attribute: {error}")))?;
            match attribute.key.local_name().as_ref() {
                b"frameRate" => {
                    result.frames_per_second = positive_number(&value, "frameRate")?;
                }
                b"subFrameRate" => {
                    result.subframes_per_frame = positive_number(&value, "subFrameRate")?;
                }
                b"tickRate" => {
                    result.ticks_per_second = positive_number(&value, "tickRate")?;
                }
                _ => {}
            }
        }
        Ok(result)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct TtmlInterval {
    begin: Duration,
    end: Option<Duration>,
}

impl TtmlInterval {
    const fn root() -> Self {
        Self {
            begin: Duration::ZERO,
            end: None,
        }
    }

    fn from_element(
        element: &quick_xml::events::BytesStart<'_>,
        reader: &Reader<&[u8]>,
        parent: Self,
        timing: TtmlTimingParameters,
    ) -> Result<Self, Error> {
        let mut begin = None;
        let mut end = None;
        let mut duration = None;
        for attribute in element.attributes() {
            let attribute = attribute
                .map_err(|error| Error::Container(format!("invalid TTML attribute: {error}")))?;
            let value = attribute
                .decoded_and_normalized_value(XmlVersion::Implicit1_0, reader.decoder())
                .map_err(|error| Error::Container(format!("invalid TTML attribute: {error}")))?;
            match attribute.key.local_name().as_ref() {
                b"begin" => begin = Some(parse_clock_time(&value, timing)?),
                b"end" => end = Some(parse_clock_time(&value, timing)?),
                b"dur" => duration = Some(parse_clock_time(&value, timing)?),
                _ => {}
            }
        }
        let absolute_begin = parent
            .begin
            .checked_add(begin.unwrap_or_default())
            .ok_or_else(|| Error::Container(String::from("TTML begin time overflow")))?;
        let absolute_end = match (end, duration, parent.end) {
            (Some(end), _, _) => Some(
                parent
                    .begin
                    .checked_add(end)
                    .ok_or_else(|| Error::Container(String::from("TTML end time overflow")))?,
            ),
            (None, Some(duration), _) => Some(
                absolute_begin
                    .checked_add(duration)
                    .ok_or_else(|| Error::Container(String::from("TTML duration overflow")))?,
            ),
            (None, None, parent_end) => parent_end,
        };
        Ok(Self {
            begin: absolute_begin,
            end: absolute_end,
        })
    }
}

struct PendingTtmlCue {
    interval: TtmlInterval,
    text: String,
}

impl PendingTtmlCue {
    fn new(interval: TtmlInterval) -> Result<Self, Error> {
        if interval.end.is_some_and(|end| end <= interval.begin) {
            return Err(Error::Container(String::from(
                "TTML cue end must be after its begin",
            )));
        }
        Ok(Self {
            interval,
            text: String::new(),
        })
    }
}

fn positive_number(value: &str, name: &str) -> Result<f64, Error> {
    let number = value
        .parse::<f64>()
        .map_err(|error| Error::Container(format!("invalid TTML {name}: {error}")))?;
    if !number.is_finite() || number <= 0.0 {
        return Err(Error::Container(format!(
            "TTML {name} must be finite and positive"
        )));
    }
    Ok(number)
}

fn parse_clock_time(value: &str, timing: TtmlTimingParameters) -> Result<Duration, Error> {
    let value = value.trim();
    if value.contains(':') {
        let parts = value.split(':').collect::<Vec<_>>();
        if parts.len() != 3 && parts.len() != 4 {
            return Err(Error::Container(format!(
                "unsupported TTML clock expression {value:?}"
            )));
        }
        let hours = parts[0]
            .parse::<f64>()
            .map_err(|error| Error::Container(format!("invalid TTML hours: {error}")))?;
        let minutes = parts[1]
            .parse::<f64>()
            .map_err(|error| Error::Container(format!("invalid TTML minutes: {error}")))?;
        let seconds = parts[2]
            .parse::<f64>()
            .map_err(|error| Error::Container(format!("invalid TTML seconds: {error}")))?;
        let frames = parts
            .get(3)
            .map(|value| {
                let (frames, subframes) = value.split_once('.').unwrap_or((value, "0"));
                let frames = frames.parse::<f64>().map_err(|error| {
                    Error::Container(format!("invalid TTML frame count: {error}"))
                })?;
                let subframes = subframes.parse::<f64>().map_err(|error| {
                    Error::Container(format!("invalid TTML subframe count: {error}"))
                })?;
                Ok::<f64, Error>(
                    (frames + subframes / timing.subframes_per_frame) / timing.frames_per_second,
                )
            })
            .transpose()?
            .unwrap_or_default();
        return checked_seconds(hours.mul_add(3_600.0, minutes.mul_add(60.0, seconds + frames)));
    }

    for (suffix, scale) in [
        ("ms", 0.001),
        ("h", 3_600.0),
        ("m", 60.0),
        ("s", 1.0),
        ("f", 1.0 / timing.frames_per_second),
        ("t", 1.0 / timing.ticks_per_second),
    ] {
        if let Some(number) = value.strip_suffix(suffix) {
            let number = number.parse::<f64>().map_err(|error| {
                Error::Container(format!("invalid TTML offset expression {value:?}: {error}"))
            })?;
            return checked_seconds(number * scale);
        }
    }
    Err(Error::Container(format!(
        "unsupported TTML time expression {value:?}"
    )))
}

fn checked_seconds(seconds: f64) -> Result<Duration, Error> {
    if !seconds.is_finite() || seconds < 0.0 {
        return Err(Error::Container(String::from(
            "subtitle time must be finite and non-negative",
        )));
    }
    Duration::try_from_secs_f64(seconds)
        .map_err(|error| Error::Container(format!("subtitle time is out of range: {error}")))
}

fn normalize_ttml_text(text: &str) -> String {
    text.lines()
        .map(|line| line.split_whitespace().collect::<Vec<_>>().join(" "))
        .filter(|line| !line.is_empty())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::{
        active_subtitle_text, parse_hls_webvtt_segment, parse_subrip_document, parse_ttml_document,
        parse_webvtt_document,
    };
    use std::time::Duration;

    #[test]
    fn parses_webvtt_cues() {
        let cues = parse_webvtt_document("WEBVTT\n\n00:00:01.000 --> 00:00:02.500\nHello\nWorld\n")
            .expect("WebVTT parse must succeed");

        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start, Duration::from_secs(1));
        assert_eq!(cues[0].end, Duration::from_millis(2500));
        assert_eq!(cues[0].text, "Hello\nWorld");
    }

    #[test]
    fn parses_srt_cues() {
        let cues = parse_subrip_document(
            "1\n00:00:03,000 --> 00:00:05,000\nLine one\n\n2\n00:00:06,000 --> 00:00:07,000\nLine two\n",
        )
        .expect("SubRip parse must succeed");

        assert_eq!(cues.len(), 2);
        assert_eq!(cues[0].text, "Line one");
        assert_eq!(cues[1].start, Duration::from_secs(6));
    }

    #[test]
    fn maps_hls_webvtt_mpeg_timestamp_to_presentation_time() {
        let cues = parse_hls_webvtt_segment(
            "WEBVTT\nX-TIMESTAMP-MAP=LOCAL:00:00:00.000,MPEGTS:900000\n\n00:00:01.000 --> 00:00:02.000\nMapped\n",
            Duration::from_secs(10),
        )
        .expect("HLS WebVTT parse must succeed");

        assert_eq!(cues[0].start, Duration::from_secs(11));
        assert_eq!(cues[0].end, Duration::from_secs(12));
    }

    #[test]
    fn parses_ttml_nested_timing_and_text() {
        let cues = parse_ttml_document(
            "<tt xmlns=\"http://www.w3.org/ns/ttml\"><body begin=\"1s\"><div><p begin=\"500ms\" dur=\"2s\">Hello <span>TTML</span><br/>World</p></div></body></tt>",
        )
        .expect("TTML parse must succeed");

        assert_eq!(cues.len(), 1);
        assert_eq!(cues[0].start, Duration::from_millis(1500));
        assert_eq!(cues[0].end, Duration::from_millis(3500));
        assert_eq!(cues[0].text, "Hello TTML\nWorld");
    }

    #[test]
    fn parses_ttml_predefined_and_numeric_character_references() {
        let cues = parse_ttml_document(
            "<tt><body><div><p begin=\"0s\" dur=\"1s\">A &amp; B &#x2014; C</p></div></body></tt>",
        )
        .expect("TTML entity references must parse");

        assert_eq!(cues[0].text, "A & B — C");
    }

    #[test]
    fn finds_active_cue_for_playback_position() {
        let cues = parse_webvtt_document(
            "WEBVTT\n\n00:00:01.000 --> 00:00:02.500\nHello\n\n00:00:03.000 --> 00:00:04.000\nWorld\n",
        )
        .expect("subtitle parse must succeed");

        assert_eq!(
            active_subtitle_text(&cues, Duration::from_millis(1500)),
            Some("Hello")
        );
        assert_eq!(
            active_subtitle_text(&cues, Duration::from_millis(3500)),
            Some("World")
        );
        assert_eq!(active_subtitle_text(&cues, Duration::from_secs(5)), None);
    }
}
