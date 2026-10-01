use byteorder::{BigEndian, WriteBytesExt};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;
use waterkit_video_core::Error;

type VideoError = Error;

/// Payload length of the `ftyp` box this muxer emits.
const FTYP_CONTENT_LEN: u64 = 12;

/// Total length in bytes of the `ftyp` box this muxer emits.
const FTYP_BOX_LEN: u64 = 8 + FTYP_CONTENT_LEN;

/// Video container format.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum VideoFormat {
    /// MP4 container (most compatible).
    #[default]
    Mp4,
    /// MOV container (Apple `QuickTime`).
    Mov,
}

/// Video codec type.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CodecType {
    /// H.264/AVC codec.
    H264,
    /// H.265/HEVC codec.
    #[default]
    H265,
    /// AV1 codec.
    Av1,
}

/// Video writer for creating MP4/MOV files.
///
/// Note: This is a simplified writer. For production use, consider
/// using the full mp4 crate API or `AVFoundation` on Apple platforms.
#[derive(Debug)]
pub struct VideoWriter {
    file: BufWriter<File>,
    width: u32,
    height: u32,
    fps: u32,
    codec: CodecType,
    samples: Vec<(Vec<u8>, bool)>, // (data, is_keyframe)
    codec_config: Option<Vec<u8>>,
}

// Minimal manual MOV muxer to avoid mp4 crate limitations
impl VideoWriter {
    /// Create a new video writer.
    ///
    /// # Arguments
    /// * `path` - Output file path (.mp4 or .mov)
    /// * `width` - Video width in pixels
    /// * `height` - Video height in pixels
    /// * `fps` - Frames per second
    /// * `codec` - Video codec (H264 or H265)
    ///
    /// # Errors
    /// Returns [`VideoError::Io`] if the file cannot be created.
    pub fn new<P: AsRef<Path>>(
        path: P,
        width: u32,
        height: u32,
        fps: u32,
        codec: CodecType,
    ) -> Result<Self, VideoError> {
        let file = File::create(path)?;
        let writer_buf = BufWriter::new(file);

        Ok(Self {
            file: writer_buf,
            width,
            height,
            fps,
            codec,
            samples: Vec::new(),
            codec_config: None,
        })
    }

    /// Set codec configuration (hvcC/avcC atom data).
    pub fn set_codec_config(&mut self, config: Vec<u8>) {
        self.codec_config = Some(config);
    }

    /// Write a video sample (encoded frame).
    ///
    /// # Errors
    /// Returns an error if the sample cannot be written (currently always returns Ok).
    pub fn write_sample(&mut self, data: &[u8], is_keyframe: bool) -> Result<(), VideoError> {
        self.samples.push((data.to_vec(), is_keyframe));
        Ok(())
    }

    /// Finish writing and close the file.
    ///
    /// # Errors
    /// Returns [`VideoError::Io`] if writing to the file fails, or
    /// [`VideoError::Container`] if a value does not fit its container field.
    #[allow(clippy::too_many_lines)]
    pub fn finish(self) -> Result<(), VideoError> {
        let Self {
            file: mut w,
            width,
            height,
            fps,
            codec,
            samples,
            codec_config,
        } = self;

        let Some(codec_config) = codec_config else {
            return Err(VideoError::Container(String::from(
                "codec configuration must be set before finishing the video container",
            )));
        };

        // 1. Write ftyp
        write_ftyp(&mut w)?;

        // 2. Write mdat
        let mdat_data_size = samples
            .iter()
            .try_fold(0_u64, |total, (data, _)| {
                total.checked_add(data.len() as u64)
            })
            .ok_or_else(|| {
                VideoError::Container(String::from("mdat payload size overflows u64"))
            })?;
        write_box_header(&mut w, b"mdat", mdat_data_size)?;

        let mut table = SampleTable {
            sizes: Vec::with_capacity(samples.len()),
            offsets: Vec::with_capacity(samples.len()),
            sync: Vec::new(),
        };
        let mut current_offset = first_sample_offset(mdat_data_size);

        for (index, (data, is_keyframe)) in samples.iter().enumerate() {
            w.write_all(data)?;
            table.sizes.push(u32::try_from(data.len()).map_err(|_| {
                VideoError::Container(String::from("sample size exceeds the u32 stsz field"))
            })?);
            table.offsets.push(current_offset);
            current_offset = current_offset
                .checked_add(data.len() as u64)
                .ok_or_else(|| {
                    VideoError::Container(String::from("sample offset overflows u64"))
                })?;

            if *is_keyframe {
                table.sync.push(u32::try_from(index + 1).map_err(|_| {
                    VideoError::Container(String::from("sample index exceeds the u32 stss field"))
                })?); // 1-based index
            }
        }

        // 3. Write moov
        let moov = build_moov(width, height, fps, codec, &codec_config, &table)?;
        write_box_header(&mut w, b"moov", moov.len() as u64)?;
        w.write_all(&moov)?;

        w.flush()?;
        Ok(())
    }

    /// Get the number of frames written.
    #[must_use]
    pub const fn frame_count(&self) -> u64 {
        self.samples.len() as u64
    }

    /// Get video dimensions.
    #[must_use]
    pub const fn dimensions(&self) -> (u32, u32) {
        (self.width, self.height)
    }
}

fn write_ftyp<W: Write>(w: &mut W) -> std::io::Result<()> {
    write_box_header(w, b"ftyp", FTYP_CONTENT_LEN)?;
    w.write_all(b"qt  ")?; // Major brand
    w.write_u32::<BigEndian>(20_050_300)?; // Minor version
    w.write_all(b"qt  ")?; // Compatible brands
    Ok(())
}

/// Per-sample table data collected while streaming `mdat` payloads.
struct SampleTable {
    /// `stsz` entries: each sample's byte length.
    sizes: Vec<u32>,
    /// `stco`/`co64` entries: each sample's absolute file offset.
    offsets: Vec<u64>,
    /// `stss` entries: 1-based indices of sync samples.
    sync: Vec<u32>,
}

/// Build the serialized `moov` box content (everything after its header).
///
/// `table.offsets` are absolute byte offsets into the file; a `co64` box is
/// emitted instead of `stco` as soon as one does not fit 32 bits.
#[allow(clippy::too_many_lines)]
fn build_moov(
    width: u32,
    height: u32,
    fps: u32,
    codec: CodecType,
    codec_config: &[u8],
    table: &SampleTable,
) -> Result<Vec<u8>, VideoError> {
    let sample_count = u32::try_from(table.sizes.len()).map_err(|_| {
        VideoError::Container(String::from("sample count exceeds the u32 table fields"))
    })?;

    let mut moov = Vec::new();
    {
        let w = &mut moov;
        // mvhd
        {
            let mut mvhd = Vec::new();
            let mw = &mut mvhd;
            mw.write_u32::<BigEndian>(0)?; // Version/Flags
            mw.write_u32::<BigEndian>(0)?; // Creation time
            mw.write_u32::<BigEndian>(0)?; // Modification time
            mw.write_u32::<BigEndian>(fps)?; // Timescale
            mw.write_u32::<BigEndian>(sample_count)?; // Duration (assuming 1 unit per frame with timescale=fps)
            mw.write_u32::<BigEndian>(0x0001_0000)?; // Rate (1.0)
            mw.write_u16::<BigEndian>(0x0100)?; // Volume (1.0)
            mw.write_all(&[0u8; 10])?; // Reserved
            // Matrix (unity)
            mw.write_u32::<BigEndian>(0x0001_0000)?;
            mw.write_u32::<BigEndian>(0)?;
            mw.write_u32::<BigEndian>(0)?;
            mw.write_u32::<BigEndian>(0)?;
            mw.write_u32::<BigEndian>(0x0001_0000)?;
            mw.write_u32::<BigEndian>(0)?;
            mw.write_u32::<BigEndian>(0)?;
            mw.write_u32::<BigEndian>(0)?;
            mw.write_u32::<BigEndian>(0x4000_0000)?;
            mw.write_all(&[0u8; 24])?; // Pre-defined
            mw.write_u32::<BigEndian>(2)?; // Next track ID

            write_box_header(w, b"mvhd", mvhd.len() as u64)?;
            w.write_all(&mvhd)?;
        }

        // trak
        {
            let mut trak = Vec::new();
            let tw = &mut trak;

            // tkhd
            {
                let mut tkhd = Vec::new();
                let thw = &mut tkhd;
                thw.write_u32::<BigEndian>(0x0000_0001)?; // Version/Flags (Enabled/InPresentation)
                thw.write_u32::<BigEndian>(0)?; // Creation time
                thw.write_u32::<BigEndian>(0)?; // Modification time
                thw.write_u32::<BigEndian>(1)?; // Track ID
                thw.write_u32::<BigEndian>(0)?; // Reserved
                thw.write_u32::<BigEndian>(sample_count)?; // Duration
                thw.write_all(&[0u8; 8])?; // Reserved
                thw.write_u16::<BigEndian>(0)?; // Layer
                thw.write_u16::<BigEndian>(0)?; // Alt group
                thw.write_u16::<BigEndian>(0)?; // Volume
                thw.write_u16::<BigEndian>(0)?; // Reserved
                // Matrix (unity)
                thw.write_all(&[
                    // Same matrix as mvhd
                    0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x00, 0x01, 0x00,
                    0x00, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0x00, 0x00, 0x00,
                ])?;
                thw.write_u32::<BigEndian>(width.checked_mul(0x1_0000).ok_or_else(|| {
                    VideoError::Container(format!(
                        "video width {width} exceeds the 16.16 fixed-point field"
                    ))
                })?)?; // Width (fixed point 16.16)
                thw.write_u32::<BigEndian>(height.checked_mul(0x1_0000).ok_or_else(|| {
                    VideoError::Container(format!(
                        "video height {height} exceeds the 16.16 fixed-point field"
                    ))
                })?)?; // Height (fixed point 16.16)

                write_box_header(tw, b"tkhd", tkhd.len() as u64)?;
                tw.write_all(&tkhd)?;
            }

            // mdia
            {
                let mut mdia = Vec::new();
                let mw = &mut mdia;

                // mdhd
                {
                    let mut mdhd = Vec::new();
                    let mhw = &mut mdhd;
                    mhw.write_u32::<BigEndian>(0)?; // Version/Flags
                    mhw.write_u32::<BigEndian>(0)?; // Creation time
                    mhw.write_u32::<BigEndian>(0)?; // Modification time
                    mhw.write_u32::<BigEndian>(fps)?; // Timescale
                    mhw.write_u32::<BigEndian>(sample_count)?; // Duration
                    mhw.write_u16::<BigEndian>(0)?; // Language (0)
                    mhw.write_u16::<BigEndian>(0)?; // Pre-defined

                    write_box_header(mw, b"mdhd", mdhd.len() as u64)?;
                    mw.write_all(&mdhd)?;
                }

                // hdlr
                {
                    let mut hdlr = Vec::new();
                    let hw = &mut hdlr;
                    hw.write_u32::<BigEndian>(0)?; // Version/Flags
                    hw.write_u32::<BigEndian>(0)?; // Pre-defined
                    hw.write_all(b"vide")?; // Component sub-type
                    hw.write_all(&[0u8; 12])?; // Reserved
                    hw.write_all(b"VideoHandler\0")?; // Component name

                    write_box_header(mw, b"hdlr", hdlr.len() as u64)?;
                    mw.write_all(&hdlr)?;
                }

                // minf
                {
                    let mut minf = Vec::new();
                    let miw = &mut minf;

                    // vmhd
                    {
                        let mut vmhd = Vec::new();
                        let vmw = &mut vmhd;
                        vmw.write_u32::<BigEndian>(0x0000_0001)?; // Version/Flags
                        vmw.write_u16::<BigEndian>(0)?; // Graphics mode
                        vmw.write_all(&[0u8; 6])?; // Opcolor

                        write_box_header(miw, b"vmhd", vmhd.len() as u64)?;
                        miw.write_all(&vmhd)?;
                    }

                    // dinf
                    {
                        let mut dinf = Vec::new();
                        let dw = &mut dinf;

                        // dref
                        let mut dref = Vec::new();
                        let drw = &mut dref;
                        drw.write_u32::<BigEndian>(0)?; // Version/Flags
                        drw.write_u32::<BigEndian>(1)?; // Entry count

                        // url
                        let mut url = Vec::new();
                        url.write_u32::<BigEndian>(0x0000_0001)?; // Version/Flags (self-contained)
                        write_box_header(drw, b"url ", url.len() as u64)?;
                        drw.write_all(&url)?;

                        write_box_header(dw, b"dref", dref.len() as u64)?;
                        dw.write_all(&dref)?;

                        write_box_header(miw, b"dinf", dinf.len() as u64)?;
                        miw.write_all(&dinf)?;
                    }

                    // stbl
                    {
                        let mut stbl = Vec::new();
                        let sw = &mut stbl;

                        // stsd
                        {
                            let mut stsd = Vec::new();
                            let ssw = &mut stsd;
                            ssw.write_u32::<BigEndian>(0)?; // Version/Flags
                            ssw.write_u32::<BigEndian>(1)?; // Entry count

                            // VisualSampleEntry (hvc1 or avc1)
                            let mut entry = Vec::new();
                            let ew = &mut entry;

                            ew.write_all(&[0u8; 6])?; // Reserved
                            ew.write_u16::<BigEndian>(1)?; // Data ref index
                            ew.write_u16::<BigEndian>(0)?; // Pre-defined
                            ew.write_u16::<BigEndian>(0)?; // Reserved
                            ew.write_all(&[0u8; 12])?; // Pre-defined
                            ew.write_u16::<BigEndian>(u16::try_from(width).map_err(|_| {
                                VideoError::Container(format!(
                                    "video width {width} exceeds the u16 sample-entry field"
                                ))
                            })?)?;
                            ew.write_u16::<BigEndian>(u16::try_from(height).map_err(|_| {
                                VideoError::Container(format!(
                                    "video height {height} exceeds the u16 sample-entry field"
                                ))
                            })?)?;
                            ew.write_u32::<BigEndian>(0x0048_0000)?; // 72 dpi
                            ew.write_u32::<BigEndian>(0x0048_0000)?; // 72 dpi
                            ew.write_u32::<BigEndian>(0)?; // Reserved
                            ew.write_u16::<BigEndian>(1)?; // Frame count
                            ew.write_u8(0)?; // Compressor name length
                            ew.write_all(&[0u8; 31])?; // Padding
                            ew.write_u16::<BigEndian>(0x0018)?; // Depth
                            ew.write_i16::<BigEndian>(-1)?; // Pre-defined

                            // Codec Config Box (avcC, hvcC or av1C)
                            let tag = match codec {
                                CodecType::H264 => b"avcC",
                                CodecType::H265 => b"hvcC",
                                CodecType::Av1 => b"av1C",
                            };
                            write_box_header(ew, tag, codec_config.len() as u64)?;
                            ew.write_all(codec_config)?;

                            let type_code = match codec {
                                CodecType::H264 => b"avc1",
                                CodecType::H265 => b"hev1",
                                CodecType::Av1 => b"av01",
                            };
                            write_box_header(ssw, type_code, entry.len() as u64)?;
                            ssw.write_all(&entry)?;

                            write_box_header(sw, b"stsd", stsd.len() as u64)?;
                            sw.write_all(&stsd)?;
                        }

                        // stts (time to sample)
                        {
                            let mut stts = Vec::new();
                            let stw = &mut stts;
                            stw.write_u32::<BigEndian>(0)?; // Version/Flags
                            stw.write_u32::<BigEndian>(1)?; // Entry count
                            stw.write_u32::<BigEndian>(sample_count)?; // Sample count
                            stw.write_u32::<BigEndian>(1)?; // Sample delta

                            write_box_header(sw, b"stts", stts.len() as u64)?;
                            sw.write_all(&stts)?;
                        }

                        // stsc (sample to chunk)
                        {
                            let mut stsc = Vec::new();
                            let scw = &mut stsc;
                            scw.write_u32::<BigEndian>(0)?; // Version/Flags
                            scw.write_u32::<BigEndian>(1)?; // Entry count

                            // 1 entry: first chunk = 1, samples per chunk = 1, description index = 1
                            // We are writing 1 sample per chunk because we write samples individually in mdat loop
                            // and sample_offsets array corresponds to each sample.
                            // Actually, standard usually chunks them. But 1 sample/chunk is valid (though inefficient overhead).
                            scw.write_u32::<BigEndian>(1)?; // First chunk
                            scw.write_u32::<BigEndian>(1)?; // Samples per chunk
                            scw.write_u32::<BigEndian>(1)?; // Sample description index

                            write_box_header(sw, b"stsc", stsc.len() as u64)?;
                            sw.write_all(&stsc)?;
                        }

                        // stss (sync samples)
                        {
                            let mut stss = Vec::new();
                            let ssw = &mut stss;
                            ssw.write_u32::<BigEndian>(0)?; // Version/Flags
                            ssw.write_u32::<BigEndian>(u32::try_from(table.sync.len()).map_err(
                                |_| {
                                    VideoError::Container(String::from(
                                        "sync sample count exceeds the u32 stss field",
                                    ))
                                },
                            )?)?; // Entry count
                            for &idx in &table.sync {
                                ssw.write_u32::<BigEndian>(idx)?;
                            }

                            write_box_header(sw, b"stss", stss.len() as u64)?;
                            sw.write_all(&stss)?;
                        }

                        // stsz (sample sizes)
                        {
                            let mut stsz = Vec::new();
                            let szw = &mut stsz;
                            szw.write_u32::<BigEndian>(0)?; // Version/Flags
                            szw.write_u32::<BigEndian>(0)?; // Default sample size (0=variable)
                            szw.write_u32::<BigEndian>(sample_count)?; // Sample count
                            for &size in &table.sizes {
                                szw.write_u32::<BigEndian>(size)?;
                            }

                            write_box_header(sw, b"stsz", stsz.len() as u64)?;
                            sw.write_all(&stsz)?;
                        }

                        // Chunk offsets: stco (32 bit) while every offset fits,
                        // co64 (64 bit) as soon as one crosses the boundary.
                        {
                            let use_co64 = table
                                .offsets
                                .iter()
                                .any(|&offset| offset > u64::from(u32::MAX));
                            let mut chunk_offsets = Vec::new();
                            let cow = &mut chunk_offsets;
                            cow.write_u32::<BigEndian>(0)?; // Version/Flags
                            cow.write_u32::<BigEndian>(
                                u32::try_from(table.offsets.len()).map_err(|_| {
                                    VideoError::Container(String::from(
                                        "chunk count exceeds the u32 offset-table field",
                                    ))
                                })?,
                            )?; // Entry count

                            if use_co64 {
                                for &offset in &table.offsets {
                                    cow.write_u64::<BigEndian>(offset)?;
                                }
                                write_box_header(sw, b"co64", chunk_offsets.len() as u64)?;
                            } else {
                                for &offset in &table.offsets {
                                    cow.write_u32::<BigEndian>(u32::try_from(offset).map_err(
                                        |_| {
                                            VideoError::Container(String::from(
                                                "chunk offset does not fit the u32 stco field",
                                            ))
                                        },
                                    )?)?;
                                }
                                write_box_header(sw, b"stco", chunk_offsets.len() as u64)?;
                            }
                            sw.write_all(&chunk_offsets)?;
                        }

                        write_box_header(miw, b"stbl", stbl.len() as u64)?;
                        miw.write_all(&stbl)?;
                    }

                    write_box_header(mw, b"minf", minf.len() as u64)?;
                    mw.write_all(&minf)?;
                }

                write_box_header(tw, b"mdia", mdia.len() as u64)?;
                tw.write_all(&mdia)?;
            }

            write_box_header(w, b"trak", trak.len() as u64)?;
            w.write_all(&trak)?;
        }
    }

    Ok(moov)
}

/// Write an ISO BMFF box header for `size_content` bytes of payload.
///
/// Emits the compact 8-byte header while the whole box fits the 32-bit size
/// field, and the extended 16-byte `largesize` form (`size == 1`) above it.
fn write_box_header<W: Write>(
    w: &mut W,
    type_str: &[u8],
    size_content: u64,
) -> std::io::Result<()> {
    let total = size_content.checked_add(8).ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidInput, "box size overflows u64")
    })?;
    if let Ok(compact) = u32::try_from(total) {
        w.write_u32::<BigEndian>(compact)?;
        w.write_all(type_str)
    } else {
        w.write_u32::<BigEndian>(1)?;
        w.write_all(type_str)?;
        w.write_u64::<BigEndian>(total.checked_add(8).ok_or_else(|| {
            std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "box largesize overflows u64",
            )
        })?)
    }
}

/// Header length [`write_box_header`] emits for `size_content` bytes of
/// payload: 8 bytes for a compact box, 16 for the extended `largesize` form.
fn box_header_len(size_content: u64) -> u64 {
    if size_content <= u64::from(u32::MAX) - 8 {
        8
    } else {
        16
    }
}

/// Absolute byte offset of the first `mdat` payload byte: the `ftyp` box plus
/// the `mdat` header, which widens to 16 bytes once the payload no longer fits
/// the 32-bit box size field.
fn first_sample_offset(mdat_data_size: u64) -> u64 {
    FTYP_BOX_LEN + box_header_len(mdat_data_size)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom};

    /// Largest payload a box can carry with a compact 8-byte header.
    const MAX_COMPACT_CONTENT: u64 = u32::MAX as u64 - 8;

    /// Opaque stand-in for an `hvcC` payload; the muxer copies it verbatim and
    /// the HEVC read path extracts the box without parsing its contents.
    fn fake_codec_config() -> Vec<u8> {
        vec![0x01; 32]
    }

    /// Return the bytes of the first box carrying `tag` inside `haystack`,
    /// including its header.
    fn find_box(haystack: &[u8], tag: [u8; 4]) -> Option<&[u8]> {
        let pos = haystack.windows(4).position(|window| window == tag)?;
        let start = pos.checked_sub(4)?;
        let size =
            usize::try_from(u32::from_be_bytes(haystack[start..pos].try_into().ok()?)).ok()?;
        haystack.get(start..start + size)
    }

    /// Assemble a file exactly the way [`VideoWriter::finish`] does, but with
    /// declared sizes/offsets instead of in-memory sample payloads, so offsets
    /// past 4 GiB stay sparse on disk.
    fn write_sparse_file(
        path: &Path,
        mdat_data_size: u64,
        samples: &[(u64, &[u8])],
        sync_samples: &[u32],
    ) {
        let mut file = File::create(path).expect("create sparse output");
        // NTFS only keeps seek holes for files explicitly marked sparse;
        // without the flag the multi-GiB seeks below would materialize
        // gigabytes on disk. `fsutil` is Windows' own mechanism for it.
        #[cfg(windows)]
        {
            let status = std::process::Command::new("fsutil")
                .args(["sparse", "setflag"])
                .arg(path)
                .status()
                .expect("fsutil must be available on Windows");
            assert!(
                status.success(),
                "fsutil sparse setflag failed for {path:?}"
            );
        }
        write_ftyp(&mut file).expect("write ftyp");
        write_box_header(&mut file, b"mdat", mdat_data_size).expect("write mdat header");
        let mdat_end = first_sample_offset(mdat_data_size) + mdat_data_size;

        for &(offset, data) in samples {
            file.seek(SeekFrom::Start(offset)).expect("seek to sample");
            file.write_all(data).expect("write sample");
        }
        file.seek(SeekFrom::Start(mdat_end))
            .expect("seek past mdat");

        let table = SampleTable {
            sizes: samples
                .iter()
                .map(|(_, data)| u32::try_from(data.len()).expect("sample fits u32"))
                .collect(),
            offsets: samples.iter().map(|(offset, _)| *offset).collect(),
            sync: sync_samples.to_vec(),
        };
        let moov = build_moov(64, 48, 30, CodecType::H265, &fake_codec_config(), &table)
            .expect("build moov");
        write_box_header(&mut file, b"moov", moov.len() as u64).expect("write moov header");
        file.write_all(&moov).expect("write moov");
        file.flush().expect("flush");
    }

    #[test]
    fn box_header_boundary_serialization() {
        // Just below the boundary the compact form is used.
        assert_eq!(box_header_len(MAX_COMPACT_CONTENT), 8);
        let mut buf = Vec::new();
        write_box_header(&mut buf, b"mdat", MAX_COMPACT_CONTENT).expect("compact header");
        assert_eq!(buf.len(), 8);
        assert_eq!(buf[..4], u32::MAX.to_be_bytes());
        assert_eq!(buf[4..8], *b"mdat");

        // Above it the extended largesize form carries the true size.
        assert_eq!(box_header_len(MAX_COMPACT_CONTENT + 1), 16);
        let mut buf = Vec::new();
        write_box_header(&mut buf, b"mdat", MAX_COMPACT_CONTENT + 1).expect("extended header");
        assert_eq!(buf.len(), 16);
        assert_eq!(buf[..4], 1_u32.to_be_bytes());
        assert_eq!(buf[4..8], *b"mdat");
        assert_eq!(buf[8..16], (MAX_COMPACT_CONTENT + 1 + 16).to_be_bytes());
    }

    #[test]
    fn moov_selects_stco_or_co64_from_offsets() {
        let config = fake_codec_config();
        let table = SampleTable {
            sizes: vec![16, 8],
            offsets: vec![36, 52],
            sync: vec![1],
        };
        let moov = build_moov(64, 48, 30, CodecType::H265, &config, &table)
            .expect("moov with small offsets");
        let stco = find_box(&moov, *b"stco").expect("small offsets must emit stco");
        assert!(find_box(&moov, *b"co64").is_none());
        assert_eq!(stco[8..16], [0_u8, 0, 0, 0, 0, 0, 0, 2]); // version/flags + count
        let expected: Vec<u8> = [36_u32, 52]
            .iter()
            .flat_map(|offset| offset.to_be_bytes())
            .collect();
        assert_eq!(stco[16..24], expected[..]);

        let large = u64::from(u32::MAX) + 1;
        let table = SampleTable {
            sizes: vec![16, 8],
            offsets: vec![36, large],
            sync: vec![1],
        };
        let moov = build_moov(64, 48, 30, CodecType::H265, &config, &table)
            .expect("moov with a >32-bit offset");
        assert!(find_box(&moov, *b"stco").is_none());
        let co64 = find_box(&moov, *b"co64").expect("large offset must emit co64");
        assert_eq!(co64[8..16], [0_u8, 0, 0, 0, 0, 0, 0, 2]);
        assert_eq!(co64[16..24], 36_u64.to_be_bytes());
        assert_eq!(co64[24..32], large.to_be_bytes());
    }

    #[test]
    fn small_recording_roundtrips_through_reader() {
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("small.mp4");
        let mut writer =
            VideoWriter::new(&path, 64, 48, 30, CodecType::H265).expect("create writer");
        writer.set_codec_config(fake_codec_config());
        writer
            .write_sample(b"first-sample", true)
            .expect("sample 1");
        writer.write_sample(b"second", false).expect("sample 2");
        writer.finish().expect("finish");

        let mut reader = crate::VideoReader::open(&path).expect("reader opens written file");
        assert_eq!(reader.dimensions(), (64, 48));
        assert_eq!(reader.sample_count(), 2);
        let (data, _, keyframe) = reader
            .read_sample()
            .expect("read sample 1")
            .expect("sample 1 exists");
        assert_eq!(data, b"first-sample");
        assert!(keyframe);
        let (data, _, keyframe) = reader
            .read_sample()
            .expect("read sample 2")
            .expect("sample 2 exists");
        assert_eq!(data, b"second");
        assert!(!keyframe);
    }

    #[test]
    fn sparse_file_at_u32_boundary_stays_compact() {
        // mdat payload sized so the box size field still fits u32, with the
        // last chunk offset exactly at u32::MAX — the largest stco entry.
        let mdat_data_size = MAX_COMPACT_CONTENT;
        let first_offset = first_sample_offset(mdat_data_size);
        assert_eq!(first_offset, FTYP_BOX_LEN + 8);
        let sample_a = b"0123456789abcdef";
        let sample_b = b"at_boundary_offset_!";
        let offset_b = first_offset + mdat_data_size - sample_b.len() as u64;
        assert_eq!(offset_b, u64::from(u32::MAX));

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("boundary.mp4");
        write_sparse_file(
            &path,
            mdat_data_size,
            &[(first_offset, sample_a), (offset_b, sample_b)],
            &[1],
        );

        let mut reader = crate::VideoReader::open(&path).expect("reader opens boundary file");
        assert_eq!(reader.sample_count(), 2);
        let (data, _, _) = reader.read_sample().expect("read").expect("sample 1");
        assert_eq!(data, sample_a);
        let (data, _, _) = reader.read_sample().expect("read").expect("sample 2");
        assert_eq!(data, sample_b);
    }

    #[test]
    fn sparse_file_above_u32_boundary_uses_largesize_and_co64() {
        // mdat payload past the 32-bit box size: extended header and a chunk
        // offset beyond u32::MAX.
        let mdat_data_size = MAX_COMPACT_CONTENT + 16;
        let first_offset = first_sample_offset(mdat_data_size);
        assert_eq!(first_offset, FTYP_BOX_LEN + 16);
        let sample_a = b"0123456789abcdef";
        let sample_b = b"beyond-4gb";
        let offset_b = first_offset + mdat_data_size - sample_b.len() as u64;
        assert!(offset_b > u64::from(u32::MAX));

        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("large.mp4");
        write_sparse_file(
            &path,
            mdat_data_size,
            &[(first_offset, sample_a), (offset_b, sample_b)],
            &[1],
        );

        let mut reader = crate::VideoReader::open(&path).expect("reader opens large file");
        assert_eq!(reader.sample_count(), 2);
        let (data, _, _) = reader.read_sample().expect("read").expect("sample 1");
        assert_eq!(data, sample_a);
        let (data, _, _) = reader.read_sample().expect("read").expect("sample 2");
        assert_eq!(data, sample_b);
    }
}
