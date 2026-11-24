use crate::drivers::ac97::{self, AC97_DRIVER};
use crate::input;
use crate::storage::vfs::{self, VfsError};

#[derive(Clone, Copy)]
pub struct WavInfo {
    pub audio_format: u16,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits_per_sample: u16,
    pub data_offset: u32,
    pub data_size: u32,
    pub duration_ms: u32,
}

#[derive(Clone, Copy)]
pub enum WavError {
    Vfs(VfsError),
    NotAudioReady,
    InvalidHeader,
    UnsupportedFormat,
}

impl WavError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Vfs(err) => err.message(),
            Self::NotAudioReady => "Intel AC'97 audio controller is not initialized",
            Self::InvalidHeader => "Invalid RIFF/WAVE file header",
            Self::UnsupportedFormat => "Only uncompressed PCM (8/16-bit, mono/stereo) WAV is supported",
        }
    }
}

pub fn inspect_wav(path: &str) -> Result<WavInfo, WavError> {
    let stat = vfs::with_vfs(|state| state.stat(path)).map_err(WavError::Vfs)?;
    if stat.size < 44 {
        return Err(WavError::InvalidHeader);
    }

    let mut header = [0u8; 12];
    let n = vfs::with_vfs(|state| state.read_file_chunk(path, 0, &mut header)).map_err(WavError::Vfs)?;
    if n < 12 || &header[0..4] != b"RIFF" || &header[8..12] != b"WAVE" {
        return Err(WavError::InvalidHeader);
    }

    let mut offset = 12u32;
    let mut found_fmt = false;
    let mut audio_format = 0u16;
    let mut channels = 0u16;
    let mut sample_rate = 0u32;
    let mut bits_per_sample = 0u16;
    let mut data_offset = 0u32;
    let mut data_size = 0u32;

    while offset + 8 <= stat.size {
        let mut chunk_hdr = [0u8; 8];
        let read_n = vfs::with_vfs(|state| state.read_file_chunk(path, offset, &mut chunk_hdr))
            .map_err(WavError::Vfs)?;
        if read_n < 8 {
            break;
        }
        let chunk_id = [chunk_hdr[0], chunk_hdr[1], chunk_hdr[2], chunk_hdr[3]];
        let chunk_size = u32::from_le_bytes([chunk_hdr[4], chunk_hdr[5], chunk_hdr[6], chunk_hdr[7]]);

        if &chunk_id == b"fmt " {
            if chunk_size < 16 {
                return Err(WavError::InvalidHeader);
            }
            let mut fmt_buf = [0u8; 16];
            let fmt_n = vfs::with_vfs(|state| state.read_file_chunk(path, offset + 8, &mut fmt_buf))
                .map_err(WavError::Vfs)?;
            if fmt_n < 16 {
                return Err(WavError::InvalidHeader);
            }
            audio_format = u16::from_le_bytes([fmt_buf[0], fmt_buf[1]]);
            channels = u16::from_le_bytes([fmt_buf[2], fmt_buf[3]]);
            sample_rate = u32::from_le_bytes([fmt_buf[4], fmt_buf[5], fmt_buf[6], fmt_buf[7]]);
            bits_per_sample = u16::from_le_bytes([fmt_buf[14], fmt_buf[15]]);
            found_fmt = true;
        } else if &chunk_id == b"data" {
            data_offset = offset + 8;
            data_size = chunk_size.min(stat.size.saturating_sub(data_offset));
            break;
        }

        // Chunks are padded to even byte boundaries.
        let padded = chunk_size.saturating_add(chunk_size & 1);
        offset = match offset.checked_add(8 + padded) {
            Some(next) if next > offset => next,
            _ => break,
        };
    }

    if !found_fmt || data_offset == 0 {
        return Err(WavError::InvalidHeader);
    }

    if audio_format != 1
        || (channels != 1 && channels != 2)
        || (bits_per_sample != 8 && bits_per_sample != 16)
        || sample_rate < 4000
        || sample_rate > 96000
    {
        return Err(WavError::UnsupportedFormat);
    }

    let bytes_per_sec = sample_rate as u64 * channels as u64 * (bits_per_sample as u64 / 8);
    let duration_ms = if bytes_per_sec > 0 {
        ((data_size as u64 * 1000) / bytes_per_sec) as u32
    } else {
        0
    };

    Ok(WavInfo {
        audio_format,
        channels,
        sample_rate,
        bits_per_sample,
        data_offset,
        data_size,
        duration_ms,
    })
}

pub fn play_wav_file(path: &str) -> Result<(WavInfo, bool), WavError> {
    if !ac97::is_ready() {
        return Err(WavError::NotAudioReady);
    }

    let info = inspect_wav(path)?;
    let actual_rate = {
        let mut dev = AC97_DRIVER.lock();
        dev.begin_playback(info.sample_rate)
    };

    let bytes_per_frame = (info.channels as usize) * (info.bits_per_sample as usize / 8);
    let mut file_pos = info.data_offset;
    let file_end = info.data_offset.saturating_add(info.data_size);

    let mut disk_buf = [0u8; 2048];
    let mut pcm_out = [0i16; 2048]; // 1024 stereo output frames
    let mut out_len = 0usize; // in i16 samples (always even)

    // Integer fixed-point Q16 resampling accumulator when hardware DAC rate differs.
    let ratio_q16 = (((info.sample_rate as u64) << 16) / (actual_rate.max(1) as u64)) as u32;
    let same_rate = info.sample_rate == actual_rate;
    let mut frac_q16 = 0u32;
    let mut cur_l = 0i16;
    let mut cur_r = 0i16;
    let mut cancelled = false;

    while file_pos < file_end && !cancelled {
        let to_read = ((file_end - file_pos) as usize).min(disk_buf.len());
        let aligned_read = (to_read / bytes_per_frame) * bytes_per_frame;
        if aligned_read == 0 {
            break;
        }

        let read_n = vfs::with_vfs(|state| {
            state.read_file_chunk(path, file_pos, &mut disk_buf[..aligned_read])
        })
        .map_err(WavError::Vfs)?;
        if read_n < bytes_per_frame {
            break;
        }
        let usable = (read_n / bytes_per_frame) * bytes_per_frame;
        file_pos += usable as u32;

        let mut p = 0usize;
        while p + bytes_per_frame <= usable {
            // Check keyboard for Esc (0x01) or 'q' (0x10) cancel.
            while let Some(sc) = input::read_scancode() {
                if sc == 0x01 || sc == 0x10 {
                    cancelled = true;
                    break;
                }
            }
            if cancelled {
                break;
            }

            let (l, r) = decode_frame(&disk_buf[p..p + bytes_per_frame], info.channels, info.bits_per_sample);
            p += bytes_per_frame;

            if same_rate {
                pcm_out[out_len] = l;
                pcm_out[out_len + 1] = r;
                out_len += 2;
                if out_len >= pcm_out.len() {
                    let _ = AC97_DRIVER.lock().queue_pcm_s16le(&pcm_out[..out_len]);
                    out_len = 0;
                }
            } else {
                // Step output frames until frac_q16 advances past 1.0 (0x10000).
                while frac_q16 < 0x10000 {
                    let interp = (frac_q16 >> 8) as i32;
                    let ol = cur_l as i32 + ((((l as i32) - (cur_l as i32)) * interp) >> 8);
                    let or = cur_r as i32 + ((((r as i32) - (cur_r as i32)) * interp) >> 8);
                    pcm_out[out_len] = ol as i16;
                    pcm_out[out_len + 1] = or as i16;
                    out_len += 2;
                    if out_len >= pcm_out.len() {
                        let _ = AC97_DRIVER.lock().queue_pcm_s16le(&pcm_out[..out_len]);
                        out_len = 0;
                    }
                    frac_q16 += ratio_q16;
                }
                frac_q16 -= 0x10000;
                cur_l = l;
                cur_r = r;
            }
        }
    }

    if !cancelled && out_len > 0 {
        let _ = AC97_DRIVER.lock().queue_pcm_s16le(&pcm_out[..out_len]);
    }

    if cancelled {
        use crate::drivers::registry::AudioDevice;
        AC97_DRIVER.lock().stop();
    } else {
        AC97_DRIVER.lock().finish_playback();
    }

    Ok((info, cancelled))
}

#[inline]
fn decode_frame(bytes: &[u8], channels: u16, bits: u16) -> (i16, i16) {
    match (channels, bits) {
        (1, 8) => {
            let s = ((bytes[0] as i16) - 128) << 8;
            (s, s)
        }
        (2, 8) => {
            let l = ((bytes[0] as i16) - 128) << 8;
            let r = ((bytes[1] as i16) - 128) << 8;
            (l, r)
        }
        (1, 16) => {
            let s = i16::from_le_bytes([bytes[0], bytes[1]]);
            (s, s)
        }
        (2, 16) => {
            let l = i16::from_le_bytes([bytes[0], bytes[1]]);
            let r = i16::from_le_bytes([bytes[2], bytes[3]]);
            (l, r)
        }
        _ => (0, 0),
    }
}
