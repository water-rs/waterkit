use crate::VideoError;
use byteorder::{BigEndian, WriteBytesExt};
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

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
    /// Returns [`VideoError::Io`] if writing to the file fails.
    #[allow(clippy::too_many_lines, clippy::cast_possible_truncation)]
    pub fn finish(self) -> Result<(), VideoError> {
        if self.codec_config.is_none() {
            eprintln!("Warning: No codec config provided. File may be invalid.");
        }
        
        let mut w = self.file;
        
        // 1. Write ftyp
        w.write_u32::<BigEndian>(20)?; // Size
        w.write_all(b"ftyp")?;
        w.write_all(b"qt  ")?; // Major brand
        w.write_u32::<BigEndian>(20_050_300)?; // Minor version
        w.write_all(b"qt  ")?; // Compatible brands
        
        // 2. Write mdat
        // Calculate mdat size
        let mdat_data_size: u64 = self.samples.iter().map(|(d, _)| d.len() as u64).sum();
        let mdat_box_size = 8 + mdat_data_size;
        
        // We use 64-bit size for safety if large, but standard uses 32-bit if < 4GB.
        // For simplicity, let's stick to 32-bit for now, assuming < 4GB.
        // If > 4GB, we should use size=1 and 64-bit large size.
        // Let's assume < 4GB for test.
        w.write_u32::<BigEndian>(mdat_box_size as u32)?;
        w.write_all(b"mdat")?;
        
        let mut sample_sizes = Vec::with_capacity(self.samples.len());
        let mut sample_offsets = Vec::with_capacity(self.samples.len());
        let mut sync_samples = Vec::new();
        let mut current_offset = 20 + 8; // ftyp + mdat header
        
        for (i, (data, is_keyframe)) in self.samples.iter().enumerate() {
            w.write_all(data)?;
            sample_sizes.push(data.len() as u32);
            sample_offsets.push(current_offset as u32);
            current_offset += data.len() as u64;
            
            if *is_keyframe {
                sync_samples.push((i + 1) as u32); // 1-based index
            }
        }
        
        // 3. Write moov
        // Note: write_box_header is moved to the top level
        
        // We need to buffer moov content or calculate size recursively.
        // Buffering is easier.
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
                mw.write_u32::<BigEndian>(self.fps)?; // Timescale
                mw.write_u32::<BigEndian>(self.samples.len() as u32)?; // Duration (assuming 1 unit per frame with timescale=fps)
                mw.write_u32::<BigEndian>(0x0001_0000)?; // Rate (1.0)
               mw.write_u16::<BigEndian>(0x0100)?; // Volume (1.0)
                mw.write_all(&[0u8; 10])?; // Reserved
                // Matrix (unity)
                mw.write_u32::<BigEndian>(0x0001_0000)?; mw.write_u32::<BigEndian>(0)?; mw.write_u32::<BigEndian>(0)?;
                mw.write_u32::<BigEndian>(0)?; mw.write_u32::<BigEndian>(0x0001_0000)?; mw.write_u32::<BigEndian>(0)?;
                mw.write_u32::<BigEndian>(0)?; mw.write_u32::<BigEndian>(0)?; mw.write_u32::<BigEndian>(0x4000_0000)?;
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
                    thw.write_u32::<BigEndian>(self.samples.len() as u32)?; // Duration
                    thw.write_all(&[0u8; 8])?; // Reserved
                    thw.write_u16::<BigEndian>(0)?; // Layer
                    thw.write_u16::<BigEndian>(0)?; // Alt group
                    thw.write_u16::<BigEndian>(0)?; // Volume
                    thw.write_u16::<BigEndian>(0)?; // Reserved
                    // Matrix (unity)
                    thw.write_all(&[ // Same matrix as mvhd
                        0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0, 0, 0, 0, 0,
                        0, 0, 0, 0, 0x00, 0x01, 0x00, 0x00, 0, 0, 0, 0,
                        0, 0, 0, 0, 0, 0, 0, 0, 0x40, 0x00, 0x00, 0x00
                    ])?;
                    thw.write_u32::<BigEndian>(self.width << 16)?; // Width (fixed point 16.16)
                    thw.write_u32::<BigEndian>(self.height << 16)?; // Height (fixed point 16.16)
                    
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
                        mhw.write_u32::<BigEndian>(self.fps)?; // Timescale
                        mhw.write_u32::<BigEndian>(self.samples.len() as u32)?; // Duration
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
                                ew.write_u16::<BigEndian>(self.width as u16)?;
                                ew.write_u16::<BigEndian>(self.height as u16)?;
                                ew.write_u32::<BigEndian>(0x0048_0000)?; // 72 dpi
                                ew.write_u32::<BigEndian>(0x0048_0000)?; // 72 dpi
                                ew.write_u32::<BigEndian>(0)?; // Reserved
                                ew.write_u16::<BigEndian>(1)?; // Frame count
                                ew.write_u8(0)?; // Compressor name length
                                ew.write_all(&[0u8; 31])?; // Padding
                                ew.write_u16::<BigEndian>(0x0018)?; // Depth
                                ew.write_i16::<BigEndian>(-1)?; // Pre-defined
                                
                                // Codec Config Box (avcC or hvcC)
                                if let Some(config) = &self.codec_config {
                                    // Use 'hvcC' if HEVC, 'avcC' if H264
                                    let tag = if self.codec == CodecType::H265 { b"hvcC" } else { b"avcC" };
                                    
                                    // Wrap config payload in box header
                                    let box_size = 8 + config.len() as u32;
                                    ew.write_u32::<BigEndian>(box_size)?;
                                    ew.write_all(tag)?;
                                    ew.write_all(config)?;
                                }
                                
                                let type_code = if self.codec == CodecType::H265 { b"hev1" } else { b"avc1" };
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
                                stw.write_u32::<BigEndian>(self.samples.len() as u32)?; // Sample count
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
                                ssw.write_u32::<BigEndian>(sync_samples.len() as u32)?; // Entry count
                                for &idx in &sync_samples {
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
                                szw.write_u32::<BigEndian>(sample_sizes.len() as u32)?; // Sample count
                                for &size in &sample_sizes {
                                    szw.write_u32::<BigEndian>(size)?;
                                }
                                
                                write_box_header(sw, b"stsz", stsz.len() as u64)?;
                                sw.write_all(&stsz)?;
                            }
                             
                            // stco (chunk offsets - 32 bit)
                            {
                                let mut stco = Vec::new();
                                let cow = &mut stco;
                                cow.write_u32::<BigEndian>(0)?; // Version/Flags
                                cow.write_u32::<BigEndian>(sample_offsets.len() as u32)?; // Entry count
                                for &offset in &sample_offsets {
                                    cow.write_u32::<BigEndian>(offset)?;
                                }
                                
                                write_box_header(sw, b"stco", stco.len() as u64)?;
                                sw.write_all(&stco)?;
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

#[allow(clippy::cast_possible_truncation)]
fn write_box_header<W: Write>(w: &mut W, type_str: &[u8], size_content: u64) -> std::io::Result<()> {
    w.write_u32::<BigEndian>((8 + size_content) as u32)?;
    w.write_all(type_str)?;
    Ok(())
}
