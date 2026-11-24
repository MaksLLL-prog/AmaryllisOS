use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::task::preempt::PreemptMutex;

use crate::audio::sample_sine_q32;
use crate::audio::wav::{inspect_wav, WavInfo};
use crate::drivers::ac97::{self, AC97_DRIVER};
use crate::drivers::registry::AudioDevice;
use crate::storage::vfs;

const MIX_RATE: u32 = 44_100;
const CHUNK_FRAMES: usize = 1024; // 2048 i16 samples = 4096B (1 BDL entry = ~23.2 ms)
const MAX_CHIME_VOICES: usize = 4;

#[derive(Clone, Copy)]
struct ToneVoice {
    active: bool,
    phase: u32,
    phase_step: u32,
    frame_idx: usize,
    total_frames: usize,
    fade_frames: usize,
    gain_q8: i32,
}

impl ToneVoice {
    const EMPTY: Self = Self {
        active: false,
        phase: 0,
        phase_step: 0,
        frame_idx: 0,
        total_frames: 0,
        fade_frames: 1,
        gain_q8: 256,
    };
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BuiltinTrack {
    StartupChime,
    AmbientArp,
    Reference440,
}

impl BuiltinTrack {
    pub const fn title(self) -> &'static str {
        match self {
            Self::StartupChime => "Amaryllis Startup Chime",
            Self::AmbientArp => "Glass Horizon Arpeggio",
            Self::Reference440 => "440 Hz Reference Sine",
        }
    }

    pub const fn duration_ms(self) -> u32 {
        match self {
            Self::StartupChime => 2400,
            Self::AmbientArp => 6400,
            Self::Reference440 => 3000,
        }
    }
}

enum MusicSource {
    None,
    Builtin {
        kind: BuiltinTrack,
        frame_idx: usize,
        total_frames: usize,
        phase_a: u32,
        phase_b: u32,
    },
    WavFile {
        path: String,
        info: WavInfo,
        file_pos: u32,
        file_end: u32,
        ratio_q16: u32,
        frac_q16: u32,
        cur_l: i16,
        cur_r: i16,
        frames_emitted: u64,
        bytes_per_frame: usize,
        disk_buf: Vec<u8>,
        disk_pos: usize,
        disk_len: usize,
    },
}

#[derive(Clone)]
pub struct MixerSnapshot {
    pub hardware_ready: bool,
    pub playing: bool,
    pub paused: bool,
    pub title: String,
    pub path: String,
    pub sample_rate: u32,
    pub channels: u16,
    pub bits_per_sample: u16,
    pub position_ms: u32,
    pub duration_ms: u32,
    pub volume_percent: u8,
    pub vu_left: u8,
    pub vu_right: u8,
    pub waveform: [i8; 32],
    pub last_error: Option<String>,
}

struct BackgroundMixer {
    voices: [ToneVoice; MAX_CHIME_VOICES],
    music: MusicSource,
    music_title: String,
    music_path: String,
    paused: bool,
    hardware_rate: u32,
    volume_percent: u8,
    vu_left: u8,
    vu_right: u8,
    waveform: [i8; 32],
    last_error: Option<String>,
    pcm_scratch: [i16; CHUNK_FRAMES * 2],
}

impl BackgroundMixer {
    const fn new() -> Self {
        Self {
            voices: [ToneVoice::EMPTY; MAX_CHIME_VOICES],
            music: MusicSource::None,
            music_title: String::new(),
            music_path: String::new(),
            paused: false,
            hardware_rate: MIX_RATE,
            volume_percent: 85,
            vu_left: 0,
            vu_right: 0,
            waveform: [0; 32],
            last_error: None,
            pcm_scratch: [0; CHUNK_FRAMES * 2],
        }
    }

    fn has_active_audio(&self) -> bool {
        let any_voice = self.voices.iter().any(|v| v.active);
        let has_music = !matches!(self.music, MusicSource::None) && !self.paused;
        any_voice || has_music
    }

    fn add_tone(&mut self, freq_hz: u32, duration_ms: u32) {
        let rate = self.hardware_rate.max(8000);
        let freq = freq_hz.clamp(60, 12_000);
        let dur = duration_ms.clamp(20, 8_000);
        let total_frames = ((rate as u64) * (dur as u64) / 1000).max(1) as usize;
        let fade_frames = ((rate as u64) * 10 / 1000).max(1) as usize;
        let phase_step = (((freq as u64) << 32) / (rate as u64)) as u32;

        let slot = self
            .voices
            .iter()
            .position(|v| !v.active)
            .unwrap_or(0);
        self.voices[slot] = ToneVoice {
            active: true,
            phase: 0,
            phase_step,
            frame_idx: 0,
            total_frames,
            fade_frames,
            gain_q8: 170,
        };
    }

    fn start_builtin(&mut self, track: BuiltinTrack) {
        self.last_error = None;
        let rate = self.hardware_rate.max(8000);
        let total_frames = ((rate as u64) * (track.duration_ms() as u64) / 1000) as usize;
        self.music = MusicSource::Builtin {
            kind: track,
            frame_idx: 0,
            total_frames,
            phase_a: 0,
            phase_b: 0,
        };
        self.music_title = String::from(track.title());
        self.music_path = String::from("builtin://synth");
        self.paused = false;
    }

    fn start_wav(&mut self, path: &str) -> Result<WavInfo, String> {
        self.last_error = None;
        let info = inspect_wav(path).map_err(|e| String::from(e.message()))?;
        let actual_rate = self.hardware_rate.max(8000);
        let ratio_q16 = (((info.sample_rate as u64) << 16) / (actual_rate as u64)).max(1) as u32;
        let bytes_per_frame = ((info.channels as usize) * (info.bits_per_sample as usize / 8)).max(1);

        self.music = MusicSource::WavFile {
            path: String::from(path),
            info,
            file_pos: info.data_offset,
            file_end: info.data_offset.saturating_add(info.data_size),
            ratio_q16,
            frac_q16: 0,
            cur_l: 0,
            cur_r: 0,
            frames_emitted: 0,
            bytes_per_frame,
            disk_buf: alloc::vec![0u8; 4096],
            disk_pos: 0,
            disk_len: 0,
        };
        let fname = path.rsplit('/').next().unwrap_or(path);
        self.music_title = String::from(fname);
        self.music_path = String::from(path);
        self.paused = false;
        Ok(info)
    }

    fn stop_music(&mut self) {
        self.music = MusicSource::None;
        self.paused = false;
        self.vu_left = 0;
        self.vu_right = 0;
        self.waveform = [0; 32];
        if !self.voices.iter().any(|v| v.active) {
            if let Some(mut dev) = AC97_DRIVER.try_lock() {
                dev.stop();
            }
        }
    }

    fn render_chunk_into_scratch(&mut self) -> usize {
        for s in self.pcm_scratch.iter_mut() {
            *s = 0;
        }

        let mut max_frames = 0usize;
        let rate = self.hardware_rate.max(8000);

        // 1. Mix primary music source (WAV or Builtin synth)
        if !self.paused {
            match &mut self.music {
                MusicSource::None => {}
                MusicSource::Builtin {
                    kind,
                    frame_idx,
                    total_frames,
                    phase_a,
                    phase_b,
                } => {
                    let rem = total_frames.saturating_sub(*frame_idx);
                    let n = rem.min(CHUNK_FRAMES);
                    if n == 0 {
                        self.music = MusicSource::None;
                    } else {
                        max_frames = max_frames.max(n);
                        for i in 0..n {
                            let cur = *frame_idx + i;
                            let (f1, f2, env) = builtin_freqs_and_env(*kind, cur, *total_frames, rate);
                            let step1 = (((f1 as u64) << 32) / (rate as u64)) as u32;
                            let step2 = (((f2 as u64) << 32) / (rate as u64)) as u32;
                            *phase_a = phase_a.wrapping_add(step1);
                            *phase_b = phase_b.wrapping_add(step2);
                            let s1 = sample_sine_q32(*phase_a) as i32;
                            let s2 = sample_sine_q32(*phase_b) as i32;
                            let mono = (((s1 * 3 + s2 * 2) / 5) * env) >> 8;
                            let l = (mono + ((s1 - s2) >> 4)).clamp(-32767, 32767) as i16;
                            let r = (mono - ((s1 - s2) >> 4)).clamp(-32767, 32767) as i16;
                            self.pcm_scratch[i * 2] = l;
                            self.pcm_scratch[i * 2 + 1] = r;
                        }
                        *frame_idx += n;
                        if *frame_idx >= *total_frames {
                            self.music = MusicSource::None;
                        }
                    }
                }
                MusicSource::WavFile {
                    path,
                    info,
                    file_pos,
                    file_end,
                    ratio_q16,
                    frac_q16,
                    cur_l,
                    cur_r,
                    frames_emitted,
                    bytes_per_frame,
                    disk_buf,
                    disk_pos,
                    disk_len,
                } => {
                    let mut out_f = 0usize;
                    let same_rate = info.sample_rate == rate;
                    let mut eof = false;

                    while out_f < CHUNK_FRAMES {
                        if *disk_pos + *bytes_per_frame > *disk_len {
                            if *file_pos >= *file_end {
                                eof = true;
                                break;
                            }
                            let to_read = ((*file_end - *file_pos) as usize).min(disk_buf.len());
                            let aligned = (to_read / *bytes_per_frame) * *bytes_per_frame;
                            if aligned == 0 {
                                eof = true;
                                break;
                            }
                            let read_res = vfs::with_vfs(|s| {
                                s.read_file_chunk(path, *file_pos, &mut disk_buf[..aligned])
                            });
                            match read_res {
                                Ok(n) if n >= *bytes_per_frame => {
                                    let usable = (n / *bytes_per_frame) * *bytes_per_frame;
                                    *disk_pos = 0;
                                    *disk_len = usable;
                                    *file_pos += usable as u32;
                                }
                                Ok(_) => {
                                    eof = true;
                                    break;
                                }
                                Err(err) => {
                                    self.last_error = Some(format!("WAV read: {}", err.message()));
                                    eof = true;
                                    break;
                                }
                            }
                        }

                        let slice = &disk_buf[*disk_pos..*disk_pos + *bytes_per_frame];
                        let (l, r) = decode_frame(slice, info.channels, info.bits_per_sample);

                        if same_rate {
                            *disk_pos += *bytes_per_frame;
                            self.pcm_scratch[out_f * 2] = l;
                            self.pcm_scratch[out_f * 2 + 1] = r;
                            out_f += 1;
                            *frames_emitted += 1;
                        } else {
                            while *frac_q16 < 0x10000 && out_f < CHUNK_FRAMES {
                                let interp = (*frac_q16 >> 8) as i32;
                                let ol = *cur_l as i32 + ((((l as i32) - (*cur_l as i32)) * interp) >> 8);
                                let or = *cur_r as i32 + ((((r as i32) - (*cur_r as i32)) * interp) >> 8);
                                self.pcm_scratch[out_f * 2] = ol.clamp(-32767, 32767) as i16;
                                self.pcm_scratch[out_f * 2 + 1] = or.clamp(-32767, 32767) as i16;
                                out_f += 1;
                                *frames_emitted += 1;
                                *frac_q16 += *ratio_q16;
                            }
                            if *frac_q16 >= 0x10000 {
                                *frac_q16 -= 0x10000;
                                *cur_l = l;
                                *cur_r = r;
                                *disk_pos += *bytes_per_frame;
                            }
                        }
                    }

                    max_frames = max_frames.max(out_f);
                    if eof {
                        self.music = MusicSource::None;
                    }
                }
            }
        }

        // 2. Mix active non-blocking UI chime/tone voices
        for voice in &mut self.voices {
            if !voice.active {
                continue;
            }
            let rem = voice.total_frames.saturating_sub(voice.frame_idx);
            let n = rem.min(CHUNK_FRAMES);
            if n == 0 {
                voice.active = false;
                continue;
            }
            max_frames = max_frames.max(n);
            for i in 0..n {
                let cur = voice.frame_idx + i;
                let raw = sample_sine_q32(voice.phase) as i32;
                voice.phase = voice.phase.wrapping_add(voice.phase_step);

                let env = if cur < voice.fade_frames {
                    (cur as i32 * 256) / (voice.fade_frames as i32)
                } else if cur + voice.fade_frames > voice.total_frames {
                    let rem_f = voice.total_frames - cur;
                    (rem_f as i32 * 256) / (voice.fade_frames as i32)
                } else {
                    256
                };

                let sample = ((raw * env * voice.gain_q8) >> 16) as i32;
                let nl = (self.pcm_scratch[i * 2] as i32 + sample).clamp(-32767, 32767) as i16;
                let nr = (self.pcm_scratch[i * 2 + 1] as i32 + sample).clamp(-32767, 32767) as i16;
                self.pcm_scratch[i * 2] = nl;
                self.pcm_scratch[i * 2 + 1] = nr;
            }
            voice.frame_idx += n;
            if voice.frame_idx >= voice.total_frames {
                voice.active = false;
            }
        }

        // 3. Update live VU peaks & 32-bar waveform preview
        if max_frames > 0 {
            let mut peak_l = 0i32;
            let mut peak_r = 0i32;
            let stride = (max_frames / 32).max(1);
            for b in 0..32 {
                let fi = (b * stride).min(max_frames - 1);
                let l = self.pcm_scratch[fi * 2] as i32;
                let r = self.pcm_scratch[fi * 2 + 1] as i32;
                peak_l = peak_l.max(l.abs());
                peak_r = peak_r.max(r.abs());
                self.waveform[b] = ((l + r) / 520).clamp(-100, 100) as i8;
            }
            self.vu_left = ((peak_l * 100) / 18000).min(100) as u8;
            self.vu_right = ((peak_r * 100) / 18000).min(100) as u8;
        }

        max_frames
    }

    fn snapshot(&self) -> MixerSnapshot {
        let hw = ac97::is_ready();
        let (sr, ch, bps, pos_ms, dur_ms, playing) = match &self.music {
            MusicSource::None => (
                self.hardware_rate,
                2,
                16,
                0,
                0,
                self.voices.iter().any(|v| v.active),
            ),
            MusicSource::Builtin {
                kind,
                frame_idx,
                total_frames,
                ..
            } => {
                let dur = kind.duration_ms();
                let pos = if *total_frames > 0 {
                    ((*frame_idx as u64 * dur as u64) / *total_frames as u64) as u32
                } else {
                    0
                };
                (self.hardware_rate, 2, 16, pos, dur, !self.paused)
            }
            MusicSource::WavFile {
                info,
                frames_emitted,
                ..
            } => {
                let pos = ((*frames_emitted * 1000) / self.hardware_rate.max(1) as u64) as u32;
                (
                    info.sample_rate,
                    info.channels,
                    info.bits_per_sample,
                    pos.min(info.duration_ms),
                    info.duration_ms,
                    !self.paused,
                )
            }
        };

        MixerSnapshot {
            hardware_ready: hw,
            playing,
            paused: self.paused && !matches!(self.music, MusicSource::None),
            title: if self.music_title.is_empty() {
                String::from("Idle")
            } else {
                self.music_title.clone()
            },
            path: self.music_path.clone(),
            sample_rate: sr,
            channels: ch,
            bits_per_sample: bps,
            position_ms: pos_ms,
            duration_ms: dur_ms,
            volume_percent: self.volume_percent,
            vu_left: self.vu_left,
            vu_right: self.vu_right,
            waveform: self.waveform,
            last_error: self.last_error.clone(),
        }
    }
}

static MIXER: PreemptMutex<BackgroundMixer> = PreemptMutex::new(BackgroundMixer::new());

pub fn play_tone_async(freq_hz: u32, duration_ms: u32) {
    if !ac97::is_ready() {
        return;
    }
    if let Some(mut mx) = MIXER.try_lock() {
        mx.add_tone(freq_hz, duration_ms);
    }
    tick();
}

pub fn play_builtin_async(track: BuiltinTrack) {
    let mut mx = MIXER.lock();
    if ac97::is_ready() {
        let mut dev = AC97_DRIVER.lock();
        mx.hardware_rate = dev.prepare_nonblocking(MIX_RATE);
        mx.volume_percent = dev.stats().volume_percent;
    } else {
        mx.last_error = Some(String::from("AC'97 controller not present (-device AC97)"));
    }
    mx.start_builtin(track);
    drop(mx);
    tick();
}

pub fn play_wav_async(path: &str) -> Result<WavInfo, String> {
    if !ac97::is_ready() {
        let msg = String::from("Intel AC'97 is not active (boot QEMU with -device AC97)");
        MIXER.lock().last_error = Some(msg.clone());
        return Err(msg);
    }
    let mut mx = MIXER.lock();
    let mut dev = AC97_DRIVER.lock();
    mx.hardware_rate = dev.prepare_nonblocking(MIX_RATE);
    mx.volume_percent = dev.stats().volume_percent;
    drop(dev);
    let res = mx.start_wav(path);
    drop(mx);
    if res.is_ok() {
        tick();
    }
    res
}

pub fn pause_toggle() {
    let mut mx = MIXER.lock();
    if !matches!(mx.music, MusicSource::None) {
        mx.paused = !mx.paused;
        if mx.paused {
            mx.vu_left = 0;
            mx.vu_right = 0;
            mx.waveform = [0; 32];
            if let Some(mut dev) = AC97_DRIVER.try_lock() {
                dev.stop();
            }
        }
    }
}

pub fn stop() {
    let mut mx = MIXER.lock();
    mx.stop_music();
}

pub fn set_volume(percent: u8) {
    let pct = percent.min(100);
    if let Some(mut mx) = MIXER.try_lock() {
        mx.volume_percent = pct;
    }
    if ac97::is_ready() {
        AC97_DRIVER.lock().set_volume(pct);
    }
}

pub fn is_active() -> bool {
    if let Some(mx) = MIXER.try_lock() {
        mx.has_active_audio()
    } else {
        false
    }
}

pub fn snapshot() -> MixerSnapshot {
    let mx = MIXER.lock();
    mx.snapshot()
}

/// Non-blocking tick driven by desktop/shell idle loop; feeds up to 6 BDL buffers (~140 ms) per call.
pub fn tick() -> bool {
    if !ac97::is_ready() {
        return false;
    }
    let Some(mut mx) = MIXER.try_lock() else {
        return false;
    };
    if !mx.has_active_audio() {
        if let Some(mut dev) = AC97_DRIVER.try_lock() {
            if dev.poll_halted() {
                mx.vu_left = 0;
                mx.vu_right = 0;
                mx.waveform = [0; 32];
            }
        }
        return false;
    }

    let Some(mut dev) = AC97_DRIVER.try_lock() else {
        return true;
    };
    mx.hardware_rate = dev.prepare_nonblocking(MIX_RATE);
    let free = dev.free_bdl_slots().min(6);
    drop(dev);

    for _ in 0..free {
        if !mx.has_active_audio() {
            break;
        }
        let frames = mx.render_chunk_into_scratch();
        if frames == 0 {
            break;
        }
        let samples = &mx.pcm_scratch[..frames * 2];
        let mut dev = AC97_DRIVER.lock();
        if !matches!(dev.try_queue_one_chunk(samples), Ok(true)) {
            break;
        }
    }
    mx.has_active_audio()
}

fn builtin_freqs_and_env(kind: BuiltinTrack, frame: usize, total: usize, rate: u32) -> (u32, u32, i32) {
    let ms = ((frame as u64 * 1000) / rate.max(1) as u64) as usize;
    let fade_out = ((total.saturating_sub(frame)) * 256 / (rate as usize / 8).max(1)).min(256) as i32;
    let fade_in = ((frame * 256) / (rate as usize / 40).max(1)).min(256) as i32;
    let env = fade_in.min(fade_out).clamp(0, 256);

    match kind {
        BuiltinTrack::Reference440 => (440, 440, env),
        BuiltinTrack::StartupChime => {
            let notes = [(523, 659), (659, 784), (784, 1046), (1046, 1318)];
            let idx = (ms / 450).min(notes.len() - 1);
            let local_ms = ms % 450;
            let pluck = if local_ms < 20 {
                (local_ms as i32 * 256) / 20
            } else {
                (256 - ((local_ms as i32 - 20) * 160) / 430).clamp(64, 256)
            };
            (notes[idx].0, notes[idx].1, (env * pluck) >> 8)
        }
        BuiltinTrack::AmbientArp => {
            let chord = [261, 329, 392, 493, 523, 493, 392, 329];
            let step = (ms / 250) % chord.len();
            let bass = chord[step] / 2;
            let lead = chord[(step + 2) % chord.len()];
            let beat_ms = ms % 250;
            let decay = (256 - (beat_ms as i32 * 180) / 250).clamp(72, 256);
            (lead, bass, (env * decay) >> 8)
        }
    }
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
