use core::arch::asm;
use core::ptr::write_volatile;
use core::sync::atomic::{fence, AtomicU16, AtomicU64, Ordering};

use crate::task::preempt::PreemptMutex;

use crate::drivers::registry::{AudioDevice, DeviceClass, DriverStatus, KernelDriver};
use crate::hardware::PciAudioDescriptor;
use crate::interrupts::timer;
use crate::memory::PhysicalFrameAllocator;
use crate::memory::vmm::{PAGE_SIZE, PhysicalMemoryMapping};

const BDL_ENTRIES: usize = 32;
const SAMPLES_PER_FRAME: usize = 2048; // 4096 bytes / 2 bytes per 16-bit sample
const PCM_BYTES_PER_BUF: usize = 4096;

// NAMBAR Mixer Registers (16-bit I/O)
const NAM_RESET: u16 = 0x00;
const NAM_MASTER_VOL: u16 = 0x02;
const NAM_PCM_VOL: u16 = 0x18;
const NAM_POWERDOWN: u16 = 0x26;
const NAM_EXT_AUDIO_ID: u16 = 0x28;
const NAM_EXT_AUDIO_CTRL: u16 = 0x2A;
const NAM_PCM_FRONT_DAC_RATE: u16 = 0x2C;

// NABMBAR Bus Master Registers
const NABM_PO_BDBAR: u16 = 0x10; // u32
const NABM_PO_CIV: u16 = 0x14; // u8
const NABM_PO_LVI: u16 = 0x15; // u8
const NABM_PO_SR: u16 = 0x16; // u16
const NABM_PO_PICB: u16 = 0x18; // u16
const NABM_PO_CR: u16 = 0x1B; // u8
const NABM_GLOB_CNT: u16 = 0x2C; // u32
const NABM_GLOB_STA: u16 = 0x30; // u32

// PO_CR bits
const PO_CR_RPBM: u8 = 1 << 0; // Run/Pause Bus Master
const PO_CR_RR: u8 = 1 << 1; // Reset Registers
const PO_CR_LVBIE: u8 = 1 << 2; // Last Valid Buffer Interrupt Enable
const PO_CR_IOCE: u8 = 1 << 4; // Interrupt on Completion Enable

// BDL flags
const BDL_FLAG_IOC: u16 = 1 << 15;

// PO_SR bits
const PO_SR_DCH: u16 = 1 << 0; // DMA Controller Halted
const PO_SR_W1C_MASK: u16 = 0x001C;

#[repr(C, align(8))]
#[derive(Clone, Copy)]
struct BufferDescriptor {
    addr: u32,
    samples: u16,
    flags: u16,
}

#[derive(Clone, Copy)]
pub struct Ac97Stats {
    pub initialized: bool,
    pub nambar: u16,
    pub nabmbar: u16,
    pub irq_line: u8,
    pub vra_supported: bool,
    pub sample_rate: u32,
    pub volume_percent: u8,
    pub playing: bool,
    pub buffers_played: u64,
}

pub struct Ac97Device {
    initialized: bool,
    status: DriverStatus,
    nambar: u16,
    nabmbar: u16,
    irq_line: u8,
    vra_supported: bool,
    sample_rate: u32,
    volume_percent: u8,
    bdl_phys: u64,
    bdl_virt: u64,
    buf_phys: [u64; BDL_ENTRIES],
    buf_virt: [u64; BDL_ENTRIES],
    next_fill_idx: usize,
    active_end_idx: usize,
    playing: bool,
    buffers_played: u64,
}

#[inline]
unsafe fn inb(port: u16) -> u8 {
    let val: u8;
    unsafe {
        asm!("in al, dx", in("dx") port, out("al") val, options(nomem, nostack, preserves_flags));
    }
    val
}

#[inline]
unsafe fn outb(port: u16, val: u8) {
    unsafe {
        asm!("out dx, al", in("dx") port, in("al") val, options(nomem, nostack, preserves_flags));
    }
}

#[inline]
unsafe fn inw(port: u16) -> u16 {
    let val: u16;
    unsafe {
        asm!("in ax, dx", in("dx") port, out("ax") val, options(nomem, nostack, preserves_flags));
    }
    val
}

#[inline]
unsafe fn outw(port: u16, val: u16) {
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") val, options(nomem, nostack, preserves_flags));
    }
}

#[inline]
unsafe fn inl(port: u16) -> u32 {
    let val: u32;
    unsafe {
        asm!("in eax, dx", in("dx") port, out("eax") val, options(nomem, nostack, preserves_flags));
    }
    val
}

#[inline]
unsafe fn outl(port: u16, val: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") val, options(nomem, nostack, preserves_flags));
    }
}

impl Ac97Device {
    pub const fn empty() -> Self {
        Self {
            initialized: false,
            status: DriverStatus::NotPresent,
            nambar: 0,
            nabmbar: 0,
            irq_line: 0,
            vra_supported: false,
            sample_rate: 48000,
            volume_percent: 85,
            bdl_phys: 0,
            bdl_virt: 0,
            buf_phys: [0; BDL_ENTRIES],
            buf_virt: [0; BDL_ENTRIES],
            next_fill_idx: 0,
            active_end_idx: 0,
            playing: false,
            buffers_played: 0,
        }
    }

    pub fn init_hardware(
        &mut self,
        desc: &PciAudioDescriptor,
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
    ) -> Result<(), &'static str> {
        if !desc.valid || desc.nambar_io == 0 || desc.nabmbar_io == 0 {
            self.status = DriverStatus::NotPresent;
            return Err("Intel AC'97 device not present");
        }

        self.nambar = desc.nambar_io;
        self.nabmbar = desc.nabmbar_io;
        self.irq_line = desc.irq_line;

        // Cold reset and clear interrupt bits in Global Control.
        unsafe {
            outl(self.nabmbar + NABM_GLOB_CNT, 0x0000_0002);
        }
        timer::sleep_ms(10);

        // Wait for Primary Codec Ready (bit 8 in Global Status).
        for _ in 0..100_000 {
            let sta = unsafe { inl(self.nabmbar + NABM_GLOB_STA) };
            if (sta & (1 << 8)) != 0 {
                break;
            }
            core::hint::spin_loop();
        }

        // Reset mixer registers & wake up codec.
        unsafe {
            outw(self.nambar + NAM_RESET, 0xFFFF);
            outw(self.nambar + NAM_POWERDOWN, 0x0000);
        }
        timer::sleep_ms(5);

        // Check Variable Rate Audio (VRA).
        let ext_id = unsafe { inw(self.nambar + NAM_EXT_AUDIO_ID) };
        self.vra_supported = (ext_id & 0x0001) != 0;
        if self.vra_supported {
            let ext_ctrl = unsafe { inw(self.nambar + NAM_EXT_AUDIO_CTRL) };
            unsafe {
                outw(self.nambar + NAM_EXT_AUDIO_CTRL, ext_ctrl | 0x0001);
            }
        }

        self.set_volume(85);
        let _ = self.set_sample_rate(44100);

        // Allocate Buffer Descriptor List (BDL) frame.
        let bdl_phys = allocator
            .allocate_frame()
            .ok_or("Failed to allocate AC'97 BDL frame")?;
        if bdl_phys > u32::MAX as u64 {
            return Err("AC'97 BDL physical address exceeds 32-bit DMA limit");
        }
        let bdl_virt = mapping
            .translate_frame(bdl_phys)
            .ok_or("Failed to translate AC'97 BDL frame")?
            .as_u64();
        unsafe {
            core::ptr::write_bytes(bdl_virt as *mut u8, 0, PAGE_SIZE as usize);
        }
        self.bdl_phys = bdl_phys;
        self.bdl_virt = bdl_virt;

        // Allocate 32 PCM buffer frames (4096B each).
        for i in 0..BDL_ENTRIES {
            let frame_phys = allocator
                .allocate_frame()
                .ok_or("Failed to allocate AC'97 PCM buffer frame")?;
            if frame_phys > u32::MAX as u64 {
                return Err("AC'97 PCM frame exceeds 32-bit DMA limit");
            }
            let frame_virt = mapping
                .translate_frame(frame_phys)
                .ok_or("Failed to translate AC'97 PCM buffer frame")?
                .as_u64();
            unsafe {
                core::ptr::write_bytes(frame_virt as *mut u8, 0, PAGE_SIZE as usize);
            }
            self.buf_phys[i] = frame_phys;
            self.buf_virt[i] = frame_virt;
        }

        self.reset_pcm_out();
        self.initialized = true;
        self.status = DriverStatus::Active;
        AC97_NABMBAR_FAST.store(self.nabmbar, Ordering::Release);
        Ok(())
    }

    fn reset_pcm_out(&mut self) {
        unsafe {
            outb(self.nabmbar + NABM_PO_CR, PO_CR_RR);
            for _ in 0..50_000 {
                if (inb(self.nabmbar + NABM_PO_CR) & PO_CR_RR) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            outw(self.nabmbar + NABM_PO_SR, PO_SR_W1C_MASK);
            outl(self.nabmbar + NABM_PO_BDBAR, self.bdl_phys as u32);
        }
        self.next_fill_idx = 0;
        self.active_end_idx = 0;
        self.playing = false;
    }

    pub fn begin_playback(&mut self, requested_rate: u32) -> u32 {
        if !self.initialized {
            return 48000;
        }
        let actual = self.set_sample_rate(requested_rate);
        self.reset_pcm_out();
        actual
    }

    pub fn prepare_nonblocking(&mut self, requested_rate: u32) -> u32 {
        if !self.initialized {
            return 44100;
        }
        if !self.playing || self.sample_rate != requested_rate {
            return self.begin_playback(requested_rate);
        }
        self.sample_rate
    }

    pub fn poll_halted(&mut self) -> bool {
        if !self.initialized || !self.playing {
            self.playing = false;
            return true;
        }
        let sr = unsafe { inw(self.nabmbar + NABM_PO_SR) };
        if (sr & PO_SR_DCH) != 0 {
            self.playing = false;
            return true;
        }
        false
    }

    pub fn free_bdl_slots(&mut self) -> usize {
        if !self.initialized {
            return 0;
        }
        if self.poll_halted() {
            return BDL_ENTRIES - 1;
        }
        let civ = unsafe { inb(self.nabmbar + NABM_PO_CIV) } as usize % BDL_ENTRIES;
        let used = (self.next_fill_idx + BDL_ENTRIES - civ) % BDL_ENTRIES;
        // Leave 1 guard descriptor between fill pointer and CIV
        (BDL_ENTRIES - 2).saturating_sub(used)
    }

    /// Non-blocking submission of up to `SAMPLES_PER_FRAME` (2048 i16 = 1024 stereo frames).
    /// Returns Ok(true) if queued, Ok(false) if hardware DMA ring is currently full.
    pub fn try_queue_one_chunk(&mut self, samples: &[i16]) -> Result<bool, &'static str> {
        if !self.initialized {
            return Err("AC'97 driver is not initialized");
        }
        if samples.is_empty() {
            return Ok(true);
        }
        if self.free_bdl_slots() == 0 {
            return Ok(false);
        }

        let chunk_len = samples.len().min(SAMPLES_PER_FRAME);
        let idx = self.next_fill_idx;

        let dst = self.buf_virt[idx] as *mut i16;
        unsafe {
            core::ptr::copy_nonoverlapping(samples.as_ptr(), dst, chunk_len);
            if chunk_len < SAMPLES_PER_FRAME {
                core::ptr::write_bytes(
                    dst.add(chunk_len) as *mut u8,
                    0,
                    (SAMPLES_PER_FRAME - chunk_len) * 2,
                );
            }
        }

        let bdl_ptr = self.bdl_virt as *mut BufferDescriptor;
        let flags = if (idx & 3) == 0 { BDL_FLAG_IOC } else { 0 };
        unsafe {
            write_volatile(
                bdl_ptr.add(idx),
                BufferDescriptor {
                    addr: self.buf_phys[idx] as u32,
                    samples: chunk_len as u16,
                    flags,
                },
            );
        }
        fence(Ordering::SeqCst);

        self.active_end_idx = idx;
        self.next_fill_idx = (idx + 1) % BDL_ENTRIES;
        self.buffers_played += 1;

        unsafe {
            outb(self.nabmbar + NABM_PO_LVI, idx as u8);
            if !self.playing {
                outw(self.nabmbar + NABM_PO_SR, PO_SR_W1C_MASK);
                outb(self.nabmbar + NABM_PO_CR, PO_CR_RPBM | PO_CR_IOCE | PO_CR_LVBIE);
                self.playing = true;
            }
        }

        Ok(true)
    }

    /// Submits one PCM buffer of 16-bit interleaved stereo samples (up to 2048 i16 samples = 4096 bytes).
    /// Blocks briefly via timer sleep if all 32 DMA BDL slots are currently queued.
    pub fn queue_pcm_s16le(&mut self, samples: &[i16]) -> Result<(), &'static str> {
        if !self.initialized {
            return Err("AC'97 driver is not initialized");
        }
        if samples.is_empty() {
            return Ok(());
        }

        let mut offset = 0usize;
        while offset < samples.len() {
            let chunk_len = (samples.len() - offset).min(SAMPLES_PER_FRAME);
            let idx = self.next_fill_idx;

            // If DMA engine is running and queue is full (next slot == CIV), wait for hardware to advance.
            if self.playing {
                for _ in 0..500 {
                    let sr = unsafe { inw(self.nabmbar + NABM_PO_SR) };
                    let civ = unsafe { inb(self.nabmbar + NABM_PO_CIV) } as usize;
                    if (sr & PO_SR_DCH) != 0 {
                        // Hardware finished existing buffers and halted; restart cleanly from idx.
                        self.playing = false;
                        break;
                    }
                    let next_after = (idx + 1) % BDL_ENTRIES;
                    if idx != civ && next_after != civ {
                        break;
                    }
                    timer::sleep_ms(5);
                }
            }

            // Copy samples into DMA frame.
            let dst = self.buf_virt[idx] as *mut i16;
            unsafe {
                core::ptr::copy_nonoverlapping(samples[offset..].as_ptr(), dst, chunk_len);
                if chunk_len < SAMPLES_PER_FRAME {
                    core::ptr::write_bytes(
                        dst.add(chunk_len) as *mut u8,
                        0,
                        (SAMPLES_PER_FRAME - chunk_len) * 2,
                    );
                }
            }

            let bdl_ptr = self.bdl_virt as *mut BufferDescriptor;
            unsafe {
                write_volatile(
                    bdl_ptr.add(idx),
                    BufferDescriptor {
                        addr: self.buf_phys[idx] as u32,
                        samples: chunk_len as u16,
                        flags: 0,
                    },
                );
            }
            fence(Ordering::SeqCst);

            self.active_end_idx = idx;
            self.next_fill_idx = (idx + 1) % BDL_ENTRIES;
            self.buffers_played += 1;

            unsafe {
                outb(self.nabmbar + NABM_PO_LVI, idx as u8);
                if !self.playing {
                    outw(self.nabmbar + NABM_PO_SR, PO_SR_W1C_MASK);
                    outb(self.nabmbar + NABM_PO_CR, PO_CR_RPBM);
                    self.playing = true;
                }
            }

            offset += chunk_len;
        }

        Ok(())
    }

    pub fn finish_playback(&mut self) {
        if !self.initialized || !self.playing {
            return;
        }

        // Wait until DMA halts (DCH bit set) or CIV == LVI with PICB == 0.
        for _ in 0..800 {
            let sr = unsafe { inw(self.nabmbar + NABM_PO_SR) };
            let civ = unsafe { inb(self.nabmbar + NABM_PO_CIV) } as usize;
            let picb = unsafe { inw(self.nabmbar + NABM_PO_PICB) };
            if (sr & PO_SR_DCH) != 0 || (civ == self.active_end_idx && picb == 0) {
                break;
            }
            timer::sleep_ms(10);
        }

        self.stop();
    }

    pub fn stats(&self) -> Ac97Stats {
        let _ = PCM_BYTES_PER_BUF;
        Ac97Stats {
            initialized: self.initialized,
            nambar: self.nambar,
            nabmbar: self.nabmbar,
            irq_line: self.irq_line,
            vra_supported: self.vra_supported,
            sample_rate: self.sample_rate,
            volume_percent: self.volume_percent,
            playing: self.playing,
            buffers_played: self.buffers_played,
        }
    }
}

impl KernelDriver for Ac97Device {
    fn driver_name(&self) -> &'static str {
        "Intel AC'97 Audio (ICH)"
    }

    fn device_class(&self) -> DeviceClass {
        DeviceClass::Audio
    }

    fn status(&self) -> DriverStatus {
        self.status
    }
}

impl AudioDevice for Ac97Device {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    fn set_sample_rate(&mut self, rate_hz: u32) -> u32 {
        let clamped = rate_hz.clamp(8000, 48000);
        if self.vra_supported && self.nambar != 0 {
            unsafe {
                outw(self.nambar + NAM_PCM_FRONT_DAC_RATE, clamped as u16);
            }
            let readback = unsafe { inw(self.nambar + NAM_PCM_FRONT_DAC_RATE) } as u32;
            self.sample_rate = if readback >= 8000 && readback <= 48000 {
                readback
            } else {
                clamped
            };
        } else {
            self.sample_rate = 48000;
        }
        self.sample_rate
    }

    fn set_volume(&mut self, percent: u8) {
        let pct = percent.min(100);
        self.volume_percent = pct;
        if self.nambar == 0 {
            return;
        }
        if pct == 0 {
            unsafe {
                outw(self.nambar + NAM_MASTER_VOL, 0x8000);
                outw(self.nambar + NAM_PCM_VOL, 0x8000);
            }
            return;
        }
        // AC'97 attenuation: 0 = max volume (0 dB), 31 = -46.5 dB.
        let atten = ((100 - pct as u16) * 31) / 100;
        let reg_val = (atten << 8) | atten;
        unsafe {
            outw(self.nambar + NAM_MASTER_VOL, reg_val);
            outw(self.nambar + NAM_PCM_VOL, 0x0808);
        }
    }

    fn stop(&mut self) {
        if self.nabmbar != 0 {
            unsafe {
                outb(self.nabmbar + NABM_PO_CR, 0);
                outw(self.nabmbar + NABM_PO_SR, PO_SR_W1C_MASK);
            }
        }
        self.playing = false;
    }
}

pub static AC97_DRIVER: PreemptMutex<Ac97Device> = PreemptMutex::new(Ac97Device::empty());
static AC97_NABMBAR_FAST: AtomicU16 = AtomicU16::new(0);
static AC97_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

/// Lock-free Top-Half interrupt handler for Intel AC'97 PCM Out DMA completion.
pub fn irq_top_half() -> bool {
    let nabm = AC97_NABMBAR_FAST.load(Ordering::Acquire);
    if nabm == 0 {
        return false;
    }
    let sr = unsafe { inw(nabm + NABM_PO_SR) };
    let ack = sr & PO_SR_W1C_MASK;
    if ack != 0 {
        unsafe {
            outw(nabm + NABM_PO_SR, ack);
        }
        AC97_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::drivers::iokit::signal_workloop(crate::drivers::iokit::WorkLoopKind::Audio);
        true
    } else {
        false
    }
}

pub fn irq_count() -> u64 {
    AC97_IRQ_COUNT.load(Ordering::Relaxed)
}

pub fn init(
    desc: &PciAudioDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<u32, &'static str> {
    let mapping = mapping.ok_or("PhysicalMemoryMapping unavailable for AC'97")?;
    let mut audio = AC97_DRIVER.lock();
    audio.init_hardware(desc, mapping, allocator)?;
    Ok(audio.sample_rate())
}

pub fn is_ready() -> bool {
    AC97_DRIVER.lock().initialized
}

pub fn stats() -> Ac97Stats {
    AC97_DRIVER.lock().stats()
}
