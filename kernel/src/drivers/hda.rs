use core::ptr::{copy_nonoverlapping, read_volatile, write_volatile};
use core::sync::atomic::{fence, AtomicBool, AtomicU64, Ordering};

use crate::freebsd_kpi::{
    bus_alloc_resource_any, bus_dma_tag_create, bus_dmamap_sync, bus_dmamem_alloc,
    bus_setup_intr, bus_space_barrier, bus_space_read_1, bus_space_read_2, bus_space_read_4,
    bus_space_write_1, bus_space_write_2, bus_space_write_4, bus_space_write_8,
    device_add_pci_child, device_probe_and_attach, pci_enable_busmaster, with_device_mut,
    BusDmaMap, BusDmaTag, BusSpaceHandle, DeviceId, DeviceT, DriverT, Mtx,
    BUS_DMASYNC_PREWRITE, BUS_DMA_NOWAIT, BUS_DMA_ZERO, BUS_PROBE_SPECIFIC, ENXIO, SYS_RES_IRQ,
    SYS_RES_MEMORY,
};
use crate::hardware::PciHdaDescriptor;
use crate::interrupts::timer;
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::pci;

/// FreeBSD `snd_hda` (Intel High Definition Audio Controller) FreeBSD-KPI driver.
const BDL_ENTRIES: usize = 32;
const PCM_BYTES_PER_BUF: usize = 4096; // 1024 stereo S16LE frames
const CORB_ENTRIES: usize = 256;
const RIRB_ENTRIES: usize = 256;

// HDA Controller Registers
const HDA_GCAP: usize = 0x00;
const HDA_VMIN: usize = 0x02;
const HDA_VMAJ: usize = 0x03;
const HDA_GCTL: usize = 0x08;
const HDA_STATESTS: usize = 0x0E;
const HDA_INTCTL: usize = 0x20;
const HDA_INTSTS: usize = 0x24;
const HDA_WALCLK: usize = 0x30;
const HDA_CORBLBASE: usize = 0x40;
const HDA_CORBWP: usize = 0x48;
const HDA_CORBRP: usize = 0x4A;
const HDA_CORBCTL: usize = 0x4C;
const HDA_CORBSIZE: usize = 0x4E;
const HDA_RIRBLBASE: usize = 0x50;
const HDA_RIRBWP: usize = 0x58;
const HDA_RINTCNT: usize = 0x5A;
const HDA_RIRBCTL: usize = 0x5C;
const HDA_RIRBSTS: usize = 0x5D;
const HDA_RIRBSIZE: usize = 0x5E;

// Output Stream 0 descriptor base = 0x80 + (iss * 0x20)
const HDA_SD_CTL: usize = 0x00; // 3-byte / u32
const HDA_SD_STS: usize = 0x03; // u8
const HDA_SD_LPIB: usize = 0x04;
const HDA_SD_CBL: usize = 0x08;
const HDA_SD_LVI: usize = 0x0C;
const HDA_SD_FMT: usize = 0x12;
const HDA_SD_BDPL: usize = 0x18;
const HDA_SD_BDPU: usize = 0x1C;

const HDA_GCTL_CRST: u32 = 1 << 0;
const HDA_SD_CTL_SRST: u32 = 1 << 0;
const HDA_SD_CTL_RUN: u32 = 1 << 1;
const HDA_SD_CTL_IOCE: u32 = 1 << 2;
const HDA_SD_CTL_STRM1: u32 = 1 << 20;

#[repr(C, align(16))]
#[derive(Clone, Copy)]
struct HdaBdlEntry {
    addr: u64,
    len: u32,
    ioc: u32,
}

#[derive(Clone, Copy)]
pub struct HdaStats {
    pub initialized: bool,
    pub mmio_phys: u64,
    pub mmio_virt: u64,
    pub irq_line: u8,
    pub irq_gsi: Option<u32>,
    pub irq_vector: Option<u8>,
    pub version_major: u8,
    pub version_minor: u8,
    pub codec_mask: u16,
    pub codec_addr: u8,
    pub dac_nid: u16,
    pub pin_nid: u16,
    pub sample_rate: u32,
    pub volume_percent: u8,
    pub playing: bool,
    pub buffers_played: u64,
}

impl HdaStats {
    pub const EMPTY: Self = Self {
        initialized: false,
        mmio_phys: 0,
        mmio_virt: 0,
        irq_line: 0,
        irq_gsi: None,
        irq_vector: None,
        version_major: 0,
        version_minor: 0,
        codec_mask: 0,
        codec_addr: 0,
        dac_nid: 0,
        pin_nid: 0,
        sample_rate: 48_000,
        volume_percent: 80,
        playing: false,
        buffers_played: 0,
    };
}

struct HdaSoftc {
    snap: HdaStats,
    mmio: BusSpaceHandle,
    tag: BusDmaTag,
    corb_map: BusDmaMap,
    rirb_map: BusDmaMap,
    bdl_map: BusDmaMap,
    buf_maps: [Option<BusDmaMap>; BDL_ENTRIES],
    osd_base: usize,
    corb_wp: u16,
    rirb_rp: u16,
    next_fill_idx: usize,
    active_end_idx: usize,
    amp_max_gain: u8,
}

impl HdaSoftc {
    const EMPTY: Self = Self {
        snap: HdaStats::EMPTY,
        mmio: BusSpaceHandle::EMPTY,
        tag: BusDmaTag::EMPTY,
        corb_map: BusDmaMap::empty(),
        rirb_map: BusDmaMap::empty(),
        bdl_map: BusDmaMap::empty(),
        buf_maps: [const { None }; BDL_ENTRIES],
        osd_base: 0x80,
        corb_wp: 0,
        rirb_rp: 0,
        next_fill_idx: 0,
        active_end_idx: 0,
        amp_max_gain: 0x4A,
    };

    #[inline]
    fn r8(&self, off: usize) -> u8 {
        bus_space_read_1(&self.mmio, off)
    }
    #[inline]
    fn w8(&self, off: usize, v: u8) {
        bus_space_write_1(&self.mmio, off, v);
    }
    #[inline]
    fn r16(&self, off: usize) -> u16 {
        bus_space_read_2(&self.mmio, off)
    }
    #[inline]
    fn w16(&self, off: usize, v: u16) {
        bus_space_write_2(&self.mmio, off, v);
    }
    #[inline]
    fn r32(&self, off: usize) -> u32 {
        bus_space_read_4(&self.mmio, off)
    }
    #[inline]
    fn w32(&self, off: usize, v: u32) {
        bus_space_write_4(&self.mmio, off, v);
    }

    fn reset_controller(&self) -> bool {
        let gctl = self.r32(HDA_GCTL);
        self.w32(HDA_GCTL, gctl & !HDA_GCTL_CRST);
        for _ in 0..50_000 {
            if (self.r32(HDA_GCTL) & HDA_GCTL_CRST) == 0 {
                break;
            }
            core::hint::spin_loop();
        }
        timer::sleep_ms(2);
        self.w32(HDA_GCTL, HDA_GCTL_CRST);
        for _ in 0..50_000 {
            if (self.r32(HDA_GCTL) & HDA_GCTL_CRST) != 0 {
                timer::sleep_ms(2);
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn init_corb_rirb(&mut self) {
        // Stop CORB/RIRB DMA engines before programming
        self.w8(HDA_CORBCTL, 0);
        self.w8(HDA_RIRBCTL, 0);
        for _ in 0..10_000 {
            if (self.r8(HDA_CORBCTL) & 2) == 0 && (self.r8(HDA_RIRBCTL) & 2) == 0 {
                break;
            }
        }

        // CORB: 256 entries (size encoding 0x02)
        self.w8(HDA_CORBSIZE, 0x02);
        bus_space_write_8(&self.mmio, HDA_CORBLBASE, self.corb_map.phys_addr());
        self.w16(HDA_CORBRP, 1 << 15);
        for _ in 0..5_000 {
            if (self.r16(HDA_CORBRP) & (1 << 15)) != 0 {
                break;
            }
        }
        self.w16(HDA_CORBRP, 0);
        for _ in 0..5_000 {
            if (self.r16(HDA_CORBRP) & (1 << 15)) == 0 {
                break;
            }
        }
        self.corb_wp = 0;
        self.w16(HDA_CORBWP, 0);

        // RIRB: 256 entries
        self.w8(HDA_RIRBSIZE, 0x02);
        bus_space_write_8(&self.mmio, HDA_RIRBLBASE, self.rirb_map.phys_addr());
        self.w16(HDA_RIRBWP, 1 << 15);
        self.rirb_rp = 0;
        self.w16(HDA_RINTCNT, 1);

        // Start engines
        self.w8(HDA_CORBCTL, 0x02);
        self.w8(HDA_RIRBCTL, 0x03);
        bus_space_barrier();
    }

    fn send_verb(&mut self, cad: u8, nid: u16, verb: u32) -> Option<u32> {
        let cmd = (((cad as u32) & 0x0F) << 28)
            | (((nid as u32) & 0xFF) << 20)
            | (verb & 0xFFFFF);
        self.corb_wp = (self.corb_wp + 1) % (CORB_ENTRIES as u16);
        let corb_ptr = self.corb_map.virt_addr() as *mut u32;
        unsafe { write_volatile(corb_ptr.add(self.corb_wp as usize), cmd) };
        fence(Ordering::SeqCst);
        self.w16(HDA_CORBWP, self.corb_wp);

        for _ in 0..50_000 {
            let wp = self.r16(HDA_RIRBWP) & 0xFF;
            if wp != self.rirb_rp {
                self.rirb_rp = (self.rirb_rp + 1) % (RIRB_ENTRIES as u16);
                let rirb_ptr = self.rirb_map.virt_addr() as *const u64;
                let resp = unsafe { read_volatile(rirb_ptr.add(self.rirb_rp as usize)) };
                return Some((resp & 0xFFFF_FFFF) as u32);
            }
            core::hint::spin_loop();
        }
        None
    }

    fn get_param(&mut self, cad: u8, nid: u16, param: u8) -> u32 {
        self.send_verb(cad, nid, 0xF0000 | (param as u32)).unwrap_or(0)
    }

    fn configure_codec(&mut self) {
        let statests = self.r16(HDA_STATESTS);
        self.snap.codec_mask = statests;
        let cad = (0..15u8).find(|&i| (statests & (1 << i)) != 0).unwrap_or(0);
        self.snap.codec_addr = cad;

        // Root node (NID 0): subordinate node count (param 0x04)
        let sub = self.get_param(cad, 0, 0x04);
        let start_fg = ((sub >> 16) & 0xFF) as u16;
        let num_fg = (sub & 0xFF) as u16;

        let mut afg_nid = 1u16;
        for nid in start_fg..start_fg.saturating_add(num_fg.max(1)) {
            let ftype = self.get_param(cad, nid, 0x05) & 0xFF;
            if ftype == 0x01 {
                afg_nid = nid;
                break;
            }
        }
        // Power state D0 on AFG
        let _ = self.send_verb(cad, afg_nid, 0x70500);

        let wsub = self.get_param(cad, afg_nid, 0x04);
        let wstart = ((wsub >> 16) & 0xFF) as u16;
        let wcount = (wsub & 0xFF) as u16;

        let mut dac_nid = 0u16;
        let mut pin_nid = 0u16;
        for nid in wstart..wstart.saturating_add(wcount) {
            let wcap = self.get_param(cad, nid, 0x09);
            let wtype = (wcap >> 20) & 0x0F;
            match wtype {
                0x0 if dac_nid == 0 => dac_nid = nid,
                0x4 => {
                    let pincap = self.get_param(cad, nid, 0x0C);
                    let cfg = self.send_verb(cad, nid, 0xF1C00).unwrap_or(0);
                    let dev = (cfg >> 20) & 0x0F;
                    // Output-capable Speaker (0x1) or Line Out (0x0) or Headphone (0x2)
                    if (pincap & (1 << 4)) != 0 && (pin_nid == 0 || matches!(dev, 0 | 1 | 2)) {
                        pin_nid = nid;
                    }
                }
                _ => {}
            }
        }
        if dac_nid == 0 {
            dac_nid = 2;
        }
        if pin_nid == 0 {
            pin_nid = 4;
        }
        self.snap.dac_nid = dac_nid;
        self.snap.pin_nid = pin_nid;

        // Read output amplifier step max from DAC (param 0x12 bits [14:8])
        let amp_cap = self.get_param(cad, dac_nid, 0x12);
        let max_steps = ((amp_cap >> 8) & 0x7F) as u8;
        if max_steps > 0 {
            self.amp_max_gain = max_steps;
        }

        // Power D0 DAC & Pin, bind DAC to Stream 1 Ch 0 (verb 0x706: 0x10), EAPD enable, Pin OUT enable
        let _ = self.send_verb(cad, dac_nid, 0x70500);
        let _ = self.send_verb(cad, dac_nid, 0x70610);
        let _ = self.send_verb(cad, pin_nid, 0x70500);
        let _ = self.send_verb(cad, pin_nid, 0x70740); // PinCtl OUT_EN
        let _ = self.send_verb(cad, pin_nid, 0x70C02); // EAPD
        // Unmute pin output amp (verb 0x3B000 = SetAmpGainMute output L+R gain=0)
        let _ = self.send_verb(cad, pin_nid, 0x3B000);
        self.apply_dac_volume(self.snap.volume_percent);
    }

    fn apply_dac_volume(&mut self, percent: u8) {
        let pct = percent.min(100);
        self.snap.volume_percent = pct;
        if !self.snap.initialized || self.snap.dac_nid == 0 {
            return;
        }
        let cad = self.snap.codec_addr;
        let dac = self.snap.dac_nid;
        let val = if pct == 0 {
            0xB080u32 // Output L+R mute
        } else {
            let gain = (((pct as u32) * (self.amp_max_gain as u32)) / 100).min(0x7F);
            0xB000 | gain
        };
        let _ = self.send_verb(cad, dac, 0x30000 | val);
    }

    fn set_format_rate(&mut self, rate_hz: u32) -> u32 {
        // HDA format word: 48kHz=0x0011 (16-bit 2ch), 44.1kHz=0x4011 (44.1 base, 16-bit 2ch)
        let (fmt, actual) = if rate_hz <= 44_100 {
            (0x4011u16, 44_100u32)
        } else {
            (0x0011u16, 48_000u32)
        };
        self.snap.sample_rate = actual;
        if self.snap.initialized {
            let _ = self.send_verb(self.snap.codec_addr, self.snap.dac_nid, 0x20000 | fmt as u32);
            self.w16(self.osd_base + HDA_SD_FMT, fmt);
        }
        actual
    }

    fn reset_output_stream(&mut self) {
        let base = self.osd_base;
        self.w32(base + HDA_SD_CTL, HDA_SD_CTL_SRST);
        for _ in 0..10_000 {
            if (self.r32(base + HDA_SD_CTL) & HDA_SD_CTL_SRST) != 0 {
                break;
            }
        }
        self.w32(base + HDA_SD_CTL, 0);
        for _ in 0..10_000 {
            if (self.r32(base + HDA_SD_CTL) & HDA_SD_CTL_SRST) == 0 {
                break;
            }
        }
        self.w8(base + HDA_SD_STS, 0x1C);
    }

    fn begin_playback(&mut self, rate_hz: u32) -> u32 {
        self.stop();
        self.reset_output_stream();
        let actual = self.set_format_rate(rate_hz);
        self.next_fill_idx = 0;
        self.active_end_idx = 0;
        self.snap.playing = false;

        let base = self.osd_base;
        self.w32(base + HDA_SD_BDPL, self.bdl_map.phys_addr() as u32);
        self.w32(base + HDA_SD_BDPU, (self.bdl_map.phys_addr() >> 32) as u32);
        self.w32(base + HDA_SD_CBL, (BDL_ENTRIES * PCM_BYTES_PER_BUF) as u32);
        self.w16(base + HDA_SD_LVI, 0);
        actual
    }

    fn prepare_nonblocking(&mut self, rate_hz: u32) -> u32 {
        if self.snap.playing && self.snap.sample_rate == rate_hz {
            return self.snap.sample_rate;
        }
        self.begin_playback(rate_hz)
    }

    fn current_hw_buf_idx(&self) -> usize {
        if !self.snap.initialized {
            return 0;
        }
        let lpib = self.r32(self.osd_base + HDA_SD_LPIB) as usize;
        (lpib / PCM_BYTES_PER_BUF) % BDL_ENTRIES
    }

    fn free_bdl_slots(&self) -> usize {
        if !self.snap.playing {
            return BDL_ENTRIES - 1;
        }
        let cur = self.current_hw_buf_idx();
        let in_flight = (self.next_fill_idx + BDL_ENTRIES - cur) % BDL_ENTRIES;
        (BDL_ENTRIES - 2).saturating_sub(in_flight)
    }

    fn try_queue_one_chunk(&mut self, pcm_s16: &[i16]) -> Result<bool, &'static str> {
        if !self.snap.initialized {
            return Err("Intel HDA not initialized");
        }
        if pcm_s16.is_empty() {
            return Ok(true);
        }
        if self.free_bdl_slots() == 0 {
            return Ok(false);
        }
        let idx = self.next_fill_idx % BDL_ENTRIES;
        let Some(ref bmap) = self.buf_maps[idx] else {
            return Err("HDA BDL buffer missing");
        };
        let samples = pcm_s16.len().min(PCM_BYTES_PER_BUF / 2);
        let bytes = samples * 2;
        unsafe {
            let dst = bmap.virt_addr() as *mut u8;
            copy_nonoverlapping(pcm_s16.as_ptr() as *const u8, dst, bytes);
            if bytes < PCM_BYTES_PER_BUF {
                core::ptr::write_bytes(dst.add(bytes), 0, PCM_BYTES_PER_BUF - bytes);
            }
        }
        bus_dmamap_sync(BUS_DMASYNC_PREWRITE);

        let bdl_ptr = self.bdl_map.virt_addr() as *mut HdaBdlEntry;
        unsafe {
            write_volatile(
                bdl_ptr.add(idx),
                HdaBdlEntry {
                    addr: bmap.phys_addr(),
                    len: PCM_BYTES_PER_BUF as u32,
                    ioc: 1,
                },
            );
        }
        fence(Ordering::SeqCst);

        self.active_end_idx = idx;
        self.next_fill_idx = (idx + 1) % BDL_ENTRIES;
        self.w16(self.osd_base + HDA_SD_LVI, idx as u16);

        if !self.snap.playing {
            self.w32(
                self.osd_base + HDA_SD_CTL,
                HDA_SD_CTL_STRM1 | HDA_SD_CTL_IOCE | HDA_SD_CTL_RUN,
            );
            self.snap.playing = true;
        }
        self.snap.buffers_played += 1;
        Ok(true)
    }

    fn queue_pcm_s16le(&mut self, pcm_s16: &[i16]) -> Result<(), &'static str> {
        let mut offset = 0usize;
        while offset < pcm_s16.len() {
            let chunk = (pcm_s16.len() - offset).min(PCM_BYTES_PER_BUF / 2);
            for _ in 0..400 {
                if self.try_queue_one_chunk(&pcm_s16[offset..offset + chunk])? {
                    break;
                }
                timer::sleep_ms(5);
            }
            offset += chunk;
        }
        Ok(())
    }

    fn poll_halted(&mut self) -> bool {
        if !self.snap.initialized || !self.snap.playing {
            return true;
        }
        let cur = self.current_hw_buf_idx();
        let ctl = self.r32(self.osd_base + HDA_SD_CTL);
        if (ctl & HDA_SD_CTL_RUN) == 0 || cur == self.active_end_idx {
            let lpib = self.r32(self.osd_base + HDA_SD_LPIB) as usize;
            let end_byte = (self.active_end_idx + 1) * PCM_BYTES_PER_BUF;
            if (ctl & HDA_SD_CTL_RUN) == 0 || lpib + 128 >= end_byte {
                self.stop();
                return true;
            }
        }
        false
    }

    fn finish_playback(&mut self) {
        if !self.snap.initialized || !self.snap.playing {
            return;
        }
        for _ in 0..800 {
            if self.poll_halted() {
                break;
            }
            timer::sleep_ms(10);
        }
        self.stop();
    }

    fn stop(&mut self) {
        if self.snap.initialized {
            self.w32(self.osd_base + HDA_SD_CTL, 0);
            self.w8(self.osd_base + HDA_SD_STS, 0x1C);
        }
        self.snap.playing = false;
    }
}

static HDA_SOFTC: Mtx<HdaSoftc> = Mtx::new("hdac0", HdaSoftc::EMPTY);
static HDA_READY_FAST: AtomicBool = AtomicBool::new(false);
static HDA_MMIO_VIRT_FAST: AtomicU64 = AtomicU64::new(0);
static HDA_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

fn hda_irq_handler(_arg: usize) -> bool {
    if !HDA_READY_FAST.load(Ordering::Acquire) {
        return false;
    }
    let mmio = HDA_MMIO_VIRT_FAST.load(Ordering::Acquire);
    if mmio == 0 {
        return false;
    }
    let intsts = unsafe { read_volatile((mmio + HDA_INTSTS as u64) as *const u32) };
    let rirbsts = unsafe { read_volatile((mmio + HDA_RIRBSTS as u64) as *const u8) };
    if intsts == 0 && (rirbsts & 0x05) == 0 {
        return false;
    }
    if (rirbsts & 0x05) != 0 {
        unsafe { write_volatile((mmio + HDA_RIRBSTS as u64) as *mut u8, rirbsts & 0x05) };
    }
    // Clear stream interrupt status bits if any output stream fired
    let gcap = unsafe { read_volatile((mmio + HDA_GCAP as u64) as *const u16) };
    let iss = ((gcap >> 8) & 0x0F) as usize;
    let osd = 0x80 + iss * 0x20;
    let sdsts = unsafe { read_volatile((mmio + (osd + HDA_SD_STS) as u64) as *const u8) };
    if (sdsts & 0x1C) != 0 {
        unsafe { write_volatile((mmio + (osd + HDA_SD_STS) as u64) as *mut u8, sdsts & 0x1C) };
    }
    HDA_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
    crate::drivers::iokit::signal_workloop(crate::drivers::iokit::WorkLoopKind::Audio);
    true
}

pub fn irq_top_half() -> bool {
    hda_irq_handler(0)
}

pub fn irq_count() -> u64 {
    HDA_IRQ_COUNT.load(Ordering::Relaxed)
}

fn hda_probe(dev: &DeviceT) -> i32 {
    if dev.class_code == 0x04 && dev.subclass == 0x03 {
        BUS_PROBE_SPECIFIC
    } else {
        ENXIO
    }
}

fn hda_attach(
    dev_id: DeviceId,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i32 {
    let Some(dev_copy) = crate::freebsd_kpi::with_device(dev_id, |d| *d) else { return 6; };
    let dev = &dev_copy;
    let Some(map) = mapping else { return 12; };
    let irq_res = pci_enable_busmaster(dev, false, true);
    let Ok(bar0) = bus_alloc_resource_any(dev, SYS_RES_MEMORY, 0, Some(map), allocator) else {
        return 6;
    };
    let Some(handle) = bar0.bus_handle else { return 6; };

    let mut sc = HDA_SOFTC.lock();
    sc.mmio = handle;
    sc.snap.mmio_phys = bar0.phys_start;
    sc.snap.mmio_virt = handle.virt_base;
    sc.snap.irq_line = irq_res;

    let gcap = sc.r16(HDA_GCAP);
    let iss = ((gcap >> 8) & 0x0F) as usize;
    sc.osd_base = 0x80 + iss * 0x20;
    sc.snap.version_minor = sc.r8(HDA_VMIN);
    sc.snap.version_major = sc.r8(HDA_VMAJ);

    if !sc.reset_controller() {
        return 5;
    }

    sc.tag = bus_dma_tag_create("hda_dma", 128, 0xFFFF_FFFF, u64::MAX, 128 * 1024, 32, PAGE_SIZE as usize);
    let Ok(corb) = bus_dmamem_alloc(&sc.tag, PAGE_SIZE as usize, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) else {
        return 12;
    };
    let Ok(rirb) = bus_dmamem_alloc(&sc.tag, PAGE_SIZE as usize, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) else {
        return 12;
    };
    let Ok(bdl) = bus_dmamem_alloc(&sc.tag, PAGE_SIZE as usize, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) else {
        return 12;
    };
    for i in 0..BDL_ENTRIES {
        let Ok(b) = bus_dmamem_alloc(&sc.tag, PAGE_SIZE as usize, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) else {
            return 12;
        };
        sc.buf_maps[i] = Some(b);
    }
    sc.corb_map = corb;
    sc.rirb_map = rirb;
    sc.bdl_map = bdl;

    let _ = sc.r32(HDA_WALCLK);
    sc.init_corb_rirb();
    sc.configure_codec();
    sc.w32(HDA_INTCTL, (1 << 31) | (1 << 30) | (1 << iss));

    sc.snap.initialized = true;
    sc.set_format_rate(48_000);

    let intr = bus_setup_intr(irq_res, "hdac0", hda_irq_handler, dev_id).ok();
    sc.snap.irq_gsi = intr.map(|r| r.0);
    sc.snap.irq_vector = intr.map(|r| r.1);

    HDA_MMIO_VIRT_FAST.store(handle.virt_base, Ordering::Release);
    HDA_READY_FAST.store(true, Ordering::Release);

    let snap = sc.snap;
    drop(sc);

    with_device_mut(dev_id, |d| {
        d.bar0_phys = bar0.phys_start;
        d.bar0_virt = handle.virt_base;
        d.irq_line = irq_res;
        d.irq_gsi = intr.map(|r| r.0);
        d.irq_vector = intr.map(|r| r.1);
        d.set_desc(&alloc::format!(
            "HDA v{}.{} codec={} dac={} pin={}",
            snap.version_major, snap.version_minor, snap.codec_addr, snap.dac_nid, snap.pin_nid
        ));
    });
    0
}

fn hda_detach(_dev_id: DeviceId) -> i32 {
    HDA_READY_FAST.store(false, Ordering::Release);
    HDA_SOFTC.lock().stop();
    0
}

pub static HDA_DRIVER: DriverT = DriverT {
    name: "snd_hda",
    probe: hda_probe,
    attach: hda_attach,
    detach: hda_detach,
};

pub fn init(
    desc: &PciHdaDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<HdaStats, &'static str> {
    if !desc.valid || desc.mmio_base == 0 {
        return Err("No Intel HDA PCI controller");
    }
    let Some(pci_dev) = pci::find_pci_device(desc.bus, desc.device, desc.function) else {
        return Err("Intel HDA PCI device missing from inventory");
    };
    let dev_id = device_add_pci_child(&pci_dev, "hdac");
    device_probe_and_attach(dev_id, &HDA_DRIVER, mapping, allocator)
        .map_err(|e| e.as_str())?;
    Ok(stats())
}

pub fn is_ready() -> bool {
    HDA_READY_FAST.load(Ordering::Acquire)
}

pub fn stats() -> HdaStats {
    HDA_SOFTC.lock().snap
}

pub fn begin_playback(rate_hz: u32) -> u32 {
    HDA_SOFTC.lock().begin_playback(rate_hz)
}

pub fn queue_pcm_s16le(pcm_s16: &[i16]) -> Result<(), &'static str> {
    HDA_SOFTC.lock().queue_pcm_s16le(pcm_s16)
}

pub fn finish_playback() {
    HDA_SOFTC.lock().finish_playback();
}

pub fn set_volume(percent: u8) {
    HDA_SOFTC.lock().apply_dac_volume(percent);
}

pub fn stop() {
    HDA_SOFTC.lock().stop();
}

pub fn try_stop() -> bool {
    if let Some(mut sc) = HDA_SOFTC.try_lock() {
        sc.stop();
        true
    } else {
        false
    }
}

pub fn try_poll_halted() -> Option<bool> {
    HDA_SOFTC.try_lock().map(|mut sc| sc.poll_halted())
}

pub fn try_prepare_nonblocking(rate_hz: u32) -> Option<(u32, usize, u8)> {
    HDA_SOFTC.try_lock().map(|mut sc| {
        let rate = sc.prepare_nonblocking(rate_hz);
        let free = sc.free_bdl_slots();
        let vol = sc.snap.volume_percent;
        (rate, free, vol)
    })
}

pub fn prepare_nonblocking_with_vol(rate_hz: u32) -> (u32, u8) {
    let mut sc = HDA_SOFTC.lock();
    let rate = sc.prepare_nonblocking(rate_hz);
    (rate, sc.snap.volume_percent)
}

pub fn try_queue_one_chunk(pcm_s16: &[i16]) -> Result<bool, &'static str> {
    HDA_SOFTC.lock().try_queue_one_chunk(pcm_s16)
}
