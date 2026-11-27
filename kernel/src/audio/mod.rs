pub mod mixer;
pub mod wav;

use core::arch::asm;

use crate::drivers;
use crate::interrupts::timer;

// 64-entry quarter/full sine wave LUT scaled to [-18000, +18000] (no floating point in no_std).
const SINE_LUT_64: [i16; 64] = [
    0, 1764, 3511, 5224, 6887, 8485, 10000, 11419,
    12728, 13914, 14966, 15874, 16630, 17225, 17654, 17913,
    18000, 17913, 17654, 17225, 16630, 15874, 14966, 13914,
    12728, 11419, 10000, 8485, 6887, 5224, 3511, 1764,
    0, -1764, -3511, -5224, -6887, -8485, -10000, -11419,
    -12728, -13914, -14966, -15874, -16630, -17225, -17654, -17913,
    -18000, -17913, -17654, -17225, -16630, -15874, -14966, -13914,
    -12728, -11419, -10000, -8485, -6887, -5224, -3511, -1764,
];

#[inline]
pub(crate) fn sample_sine_q32(phase: u32) -> i16 {
    let idx = (phase >> 26) as usize; // top 6 bits -> 0..63
    let next_idx = (idx + 1) & 63;
    let frac = ((phase >> 18) & 0xFF) as i32;
    let a = SINE_LUT_64[idx] as i32;
    let b = SINE_LUT_64[next_idx] as i32;
    (a + (((b - a) * frac) >> 8)) as i16
}

pub fn play_tone(freq_hz: u32, duration_ms: u32) -> Result<bool, &'static str> {
    let freq = freq_hz.clamp(60, 12_000);
    let dur = duration_ms.clamp(20, 5_000);

    if !drivers::audio_is_ready() {
        pc_speaker_beep(freq, dur);
        return Ok(false);
    }

    let rate = drivers::audio_begin_playback(44100);

    let total_frames = ((rate as u64) * (dur as u64) / 1000) as usize;
    let fade_frames = ((rate as u64) * 10 / 1000).max(1) as usize;
    let phase_step = (((freq as u64) << 32) / (rate as u64)) as u32;
    let mut phase = 0u32;

    let mut pcm_buf = [0i16; 2048]; // 1024 stereo frames per chunk
    let mut frame_idx = 0usize;

    while frame_idx < total_frames {
        let chunk_frames = (total_frames - frame_idx).min(1024);
        for i in 0..chunk_frames {
            let cur = frame_idx + i;
            let raw = sample_sine_q32(phase) as i32;
            phase = phase.wrapping_add(phase_step);

            let env = if cur < fade_frames {
                (cur as i32 * 256) / (fade_frames as i32)
            } else if cur + fade_frames > total_frames {
                let rem = total_frames - cur;
                (rem as i32 * 256) / (fade_frames as i32)
            } else {
                256
            };

            let val = ((raw * env) >> 8) as i16;
            pcm_buf[i * 2] = val;
            pcm_buf[i * 2 + 1] = val;
        }

        drivers::audio_queue_pcm_s16le(&pcm_buf[..chunk_frames * 2])?;
        frame_idx += chunk_frames;
    }

    drivers::audio_finish_playback();
    Ok(true)
}

pub fn set_volume(percent: u8) -> Result<(), &'static str> {
    if !drivers::audio_is_ready() {
        return Err("No active hardware audio driver (AC'97/HDA)");
    }
    drivers::audio_set_volume(percent);
    Ok(())
}

pub fn pc_speaker_beep(freq_hz: u32, duration_ms: u32) {
    let freq = freq_hz.clamp(50, 10_000);
    let divisor = (1_193_182 / freq) as u16;

    unsafe {
        asm!("out dx, al", in("dx") 0x43u16, in("al") 0xB6u8, options(nomem, nostack, preserves_flags));
        asm!("out dx, al", in("dx") 0x42u16, in("al") (divisor & 0xFF) as u8, options(nomem, nostack, preserves_flags));
        asm!("out dx, al", in("dx") 0x42u16, in("al") (divisor >> 8) as u8, options(nomem, nostack, preserves_flags));

        let cur: u8;
        asm!("in al, dx", in("dx") 0x61u16, out("al") cur, options(nomem, nostack, preserves_flags));
        asm!("out dx, al", in("dx") 0x61u16, in("al") (cur | 0x03), options(nomem, nostack, preserves_flags));
    }

    timer::sleep_ms(duration_ms as u64);

    unsafe {
        let cur: u8;
        asm!("in al, dx", in("dx") 0x61u16, out("al") cur, options(nomem, nostack, preserves_flags));
        asm!("out dx, al", in("dx") 0x61u16, in("al") (cur & !0x03), options(nomem, nostack, preserves_flags));
    }
}
