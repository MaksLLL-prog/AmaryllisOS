use alloc::vec;
use core::ptr::{copy_nonoverlapping, read_volatile};
use core::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use crate::freebsd_kpi::{
    bus_alloc_resource_any, bus_dma_tag_create, bus_dmamap_sync, bus_dmamem_alloc,
    bus_setup_intr, bus_space_barrier, bus_space_read_1, bus_space_read_2, bus_space_read_4,
    bus_space_write_1, bus_space_write_2, bus_space_write_4, device_add_pci_child,
    device_probe_and_attach, pci_enable_busmaster, with_device_mut, BusDmaMap, BusDmaTag,
    BusSpaceHandle, DeviceId, DeviceT, DriverT, Mtx, BUS_DMASYNC_POSTREAD,
    BUS_DMASYNC_PREWRITE, BUS_DMA_NOWAIT, BUS_DMA_ZERO, BUS_PROBE_SPECIFIC, ENXIO, SYS_RES_MEMORY,
};
use crate::hardware::PciSdhciDescriptor;
use crate::interrupts::timer;
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::pci;
use crate::storage::block::{BlockDevice, BlockDeviceError, DEFAULT_SECTOR_SIZE};

/// FreeBSD `sdhci_pci` + `mmc` + `mmcsd` FreeBSD-KPI SD/MMC Card Reader driver.

const SDHCI_DMA_ADDRESS: usize = 0x00;
const SDHCI_BLOCK_SIZE: usize = 0x04;
const SDHCI_BLOCK_COUNT: usize = 0x06;
const SDHCI_ARGUMENT: usize = 0x08;
const SDHCI_TRANSFER_MODE: usize = 0x0C;
const SDHCI_COMMAND: usize = 0x0E;
const SDHCI_RESPONSE0: usize = 0x10;
const SDHCI_BUFFER: usize = 0x20;
const SDHCI_PRESENT_STATE: usize = 0x24;
const SDHCI_HOST_CONTROL: usize = 0x28;
const SDHCI_POWER_CONTROL: usize = 0x29;
const SDHCI_CLOCK_CONTROL: usize = 0x2C;
const SDHCI_TIMEOUT_CONTROL: usize = 0x2E;
const SDHCI_SOFTWARE_RESET: usize = 0x2F;
const SDHCI_INT_STATUS: usize = 0x30;
const SDHCI_INT_ENABLE: usize = 0x34;
const SDHCI_SIGNAL_ENABLE: usize = 0x38;
const SDHCI_CAPABILITIES: usize = 0x40;
const SDHCI_HOST_VERSION: usize = 0xFE;

const PRESENT_CARD_INSERTED: u32 = 1 << 16;
const PRESENT_CMD_INHIBIT: u32 = 1 << 0;
const PRESENT_DAT_INHIBIT: u32 = 1 << 1;

const INT_CMD_COMPLETE: u32 = 1 << 0;
const INT_XFER_COMPLETE: u32 = 1 << 1;
const INT_DMA_INT: u32 = 1 << 3;
const INT_SPACE_AVAIL: u32 = 1 << 4;
const INT_DATA_AVAIL: u32 = 1 << 5;
const INT_ERR_MASK: u32 = 0xFFFF_0000;

#[derive(Clone, Copy)]
pub struct SdhciSnapshot {
    pub present: bool,
    pub initialized: bool,
    pub card_present: bool,
    pub card_ready: bool,
    pub sdhc_v2: bool,
    pub host_version: u16,
    pub bar0_phys: u64,
    pub bar0_virt: u64,
    pub irq_line: u8,
    pub irq_gsi: Option<u32>,
    pub irq_vector: Option<u8>,
    pub rca: u16,
    pub sector_count: u64,
    pub reads: u64,
    pub writes: u64,
    pub max_dma_sectors: usize,
}

impl SdhciSnapshot {
    pub const EMPTY: Self = Self {
        present: false,
        initialized: false,
        card_present: false,
        card_ready: false,
        sdhc_v2: false,
        host_version: 0,
        bar0_phys: 0,
        bar0_virt: 0,
        irq_line: 0,
        irq_gsi: None,
        irq_vector: None,
        rca: 0,
        sector_count: 0,
        reads: 0,
        writes: 0,
        max_dma_sectors: 8,
    };
}

struct SdhciSoftc {
    snap: SdhciSnapshot,
    mmio: BusSpaceHandle,
    tag: BusDmaTag,
    dma_map: BusDmaMap,
    has_sdma: bool,
}

impl SdhciSoftc {
    const EMPTY: Self = Self {
        snap: SdhciSnapshot::EMPTY,
        mmio: BusSpaceHandle::EMPTY,
        tag: BusDmaTag::EMPTY,
        dma_map: BusDmaMap::empty(),
        has_sdma: false,
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

    fn reset_host(&self, mask: u8) -> bool {
        self.w8(SDHCI_SOFTWARE_RESET, mask);
        for _ in 0..20_000 {
            if (self.r8(SDHCI_SOFTWARE_RESET) & mask) == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn setup_power_and_clock(&self, high_speed: bool) {
        // 3.3V VDD1 power (0x0E | 0x01)
        self.w8(SDHCI_POWER_CONTROL, 0x0F);
        timer::sleep_ms(2);
        // Set clock divider: slow (~400kHz) for init or faster (div 2) for data
        let div: u16 = if high_speed { 0x02 } else { 0x40 };
        self.w16(SDHCI_CLOCK_CONTROL, 0);
        self.w16(SDHCI_CLOCK_CONTROL, (div << 8) | 0x01);
        for _ in 0..10_000 {
            if (self.r16(SDHCI_CLOCK_CONTROL) & 0x02) != 0 {
                break;
            }
            core::hint::spin_loop();
        }
        self.w16(SDHCI_CLOCK_CONTROL, (div << 8) | 0x07); // SD Clock Enable
        self.w8(SDHCI_TIMEOUT_CONTROL, 0x0E);
    }

    fn wait_cmd_ready(&self, need_dat: bool) -> bool {
        let mask = PRESENT_CMD_INHIBIT | if need_dat { PRESENT_DAT_INHIBIT } else { 0 };
        for _ in 0..50_000 {
            if (self.r32(SDHCI_PRESENT_STATE) & mask) == 0 {
                return true;
            }
            core::hint::spin_loop();
        }
        false
    }

    fn send_cmd(
        &self,
        cmd_idx: u8,
        arg: u32,
        resp_type: u16, // 0=none, 1=R136, 2=R48, 3=R48busy
        has_data: bool,
    ) -> Result<[u32; 4], BlockDeviceError> {
        if !self.wait_cmd_ready(has_data) {
            return Err(BlockDeviceError::Timeout);
        }
        self.w32(SDHCI_INT_STATUS, 0xFFFF_FFFF);
        self.w32(SDHCI_ARGUMENT, arg);

        let mut flags = (resp_type & 0x03)
            | if resp_type == 2 || resp_type == 3 { (1 << 3) | (1 << 4) } else { 0 }
            | if resp_type == 1 { 1 << 3 } else { 0 };
        if has_data {
            flags |= 1 << 5;
        }
        let cmd_reg = (((cmd_idx as u16) & 0x3F) << 8) | flags;
        self.w16(SDHCI_COMMAND, cmd_reg);
        bus_space_barrier();

        for _ in 0..100_000 {
            let st = self.r32(SDHCI_INT_STATUS);
            if (st & INT_ERR_MASK) != 0 {
                self.w32(SDHCI_INT_STATUS, st);
                let _ = self.reset_host(0x06);
                return Err(BlockDeviceError::IoError);
            }
            if (st & INT_CMD_COMPLETE) != 0 {
                self.w32(SDHCI_INT_STATUS, INT_CMD_COMPLETE);
                return Ok([
                    self.r32(SDHCI_RESPONSE0),
                    self.r32(SDHCI_RESPONSE0 + 4),
                    self.r32(SDHCI_RESPONSE0 + 8),
                    self.r32(SDHCI_RESPONSE0 + 12),
                ]);
            }
            core::hint::spin_loop();
        }
        Err(BlockDeviceError::Timeout)
    }

    fn probe_card(&mut self) {
        let present = (self.r32(SDHCI_PRESENT_STATE) & PRESENT_CARD_INSERTED) != 0;
        self.snap.card_present = present;
        if !present {
            return;
        }

        // CMD0: GO_IDLE_STATE
        let _ = self.send_cmd(0, 0, 0, false);
        // CMD8: SEND_IF_COND (3.3V check pattern 0xAA)
        let cmd8_ok = self.send_cmd(8, 0x01AA, 2, false).is_ok();
        let hcs = if cmd8_ok { 1u32 << 30 } else { 0 };

        // ACMD41: SD_SEND_OP_COND
        let mut ocr = 0u32;
        for _ in 0..64 {
            if self.send_cmd(55, 0, 2, false).is_err() {
                break;
            }
            if let Ok(r) = self.send_cmd(41, 0x00FF_8000 | hcs, 2, false) {
                if (r[0] & (1 << 31)) != 0 {
                    ocr = r[0];
                    break;
                }
            }
            timer::sleep_ms(5);
        }
        if ocr == 0 {
            return;
        }
        self.snap.sdhc_v2 = (ocr & (1 << 30)) != 0;

        // CMD2 (ALL_SEND_CID), CMD3 (SEND_RELATIVE_ADDR)
        if self.send_cmd(2, 0, 1, false).is_err() {
            return;
        }
        let Ok(r3) = self.send_cmd(3, 0, 2, false) else {
            return;
        };
        let rca = ((r3[0] >> 16) & 0xFFFF) as u16;
        self.snap.rca = rca;

        // CMD9 (SEND_CSD) -> capacity
        if let Ok(csd) = self.send_cmd(9, (rca as u32) << 16, 1, false) {
            // Note: SDHCI strips 8-bit CRC in 120-bit R2 response -> shift by 8
            let csd_ver = (csd[3] >> 22) & 0x03;
            if csd_ver == 1 {
                let c_size = (((csd[1] >> 8) & 0x3F_FFFF) as u64) + 1;
                self.snap.sector_count = c_size * 1024;
            } else {
                let read_bl_len = ((csd[2] >> 8) & 0x0F) as u32;
                let c_size = (((csd[2] & 0x03) << 10) | ((csd[1] >> 22) & 0x3FF)) as u64;
                let c_mult = (((csd[1] >> 7) & 0x07) + 2) as u32;
                let bytes = (c_size + 1)
                    .saturating_mul(1u64 << c_mult)
                    .saturating_mul(1u64 << read_bl_len.min(12));
                self.snap.sector_count = bytes / 512;
            }
        }
        if self.snap.sector_count == 0 {
            self.snap.sector_count = 32_768; // safe fallback if CSD decode returned 0
        }

        // CMD7 (SELECT_CARD) & CMD16 (SET_BLOCKLEN=512)
        if self.send_cmd(7, (rca as u32) << 16, 3, false).is_ok() {
            let _ = self.send_cmd(16, 512, 2, false);
            self.setup_power_and_clock(true);
            self.snap.card_ready = true;
        }
    }

    fn rw_sectors(
        &mut self,
        write: bool,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        if !self.snap.initialized || !self.snap.card_ready {
            return Err(BlockDeviceError::DeviceNotReady);
        }
        let needed = sector_count * 512;
        if sector_count == 0 || sector_count > self.snap.max_dma_sectors || buffer.len() < needed {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        if start_lba + sector_count as u64 > self.snap.sector_count {
            return Err(BlockDeviceError::SectorOutOfRange);
        }

        let addr = if self.snap.sdhc_v2 {
            start_lba as u32
        } else {
            (start_lba * 512) as u32
        };

        let use_sdma = self.has_sdma && self.dma_map.phys_addr() != 0;
        if use_sdma {
            if write {
                unsafe {
                    copy_nonoverlapping(buffer.as_ptr(), self.dma_map.virt_addr() as *mut u8, needed);
                }
                bus_dmamap_sync(BUS_DMASYNC_PREWRITE);
            }
            self.w32(SDHCI_DMA_ADDRESS, self.dma_map.phys_addr() as u32);
        }

        self.w16(SDHCI_BLOCK_SIZE, 512 | (7 << 12)); // 512B + 512KiB SDMA boundary
        self.w16(SDHCI_BLOCK_COUNT, sector_count as u16);

        let mut xfer_mode: u16 = 0;
        if use_sdma {
            xfer_mode |= 1 << 0; // DMA Enable
        }
        if sector_count > 1 {
            xfer_mode |= (1 << 1) | (1 << 5) | (1 << 2); // Block Count + Multi-block + Auto CMD12
        }
        if !write {
            xfer_mode |= 1 << 4; // Read direction
        }
        self.w16(SDHCI_TRANSFER_MODE, xfer_mode);

        let cmd_idx = match (write, sector_count > 1) {
            (false, false) => 17,
            (false, true) => 18,
            (true, false) => 24,
            (true, true) => 25,
        };
        self.send_cmd(cmd_idx, addr, 2, true)?;

        if use_sdma {
            for _ in 0..200_000 {
                let st = self.r32(SDHCI_INT_STATUS);
                if (st & INT_ERR_MASK) != 0 {
                    self.w32(SDHCI_INT_STATUS, st);
                    let _ = self.reset_host(0x06);
                    return Err(BlockDeviceError::IoError);
                }
                if (st & INT_DMA_INT) != 0 {
                    self.w32(SDHCI_INT_STATUS, INT_DMA_INT);
                    let next_dma = self.r32(SDHCI_DMA_ADDRESS);
                    self.w32(SDHCI_DMA_ADDRESS, next_dma);
                }
                if (st & INT_XFER_COMPLETE) != 0 {
                    self.w32(SDHCI_INT_STATUS, INT_XFER_COMPLETE);
                    break;
                }
                core::hint::spin_loop();
            }
            if !write {
                bus_dmamap_sync(BUS_DMASYNC_POSTREAD);
                unsafe {
                    copy_nonoverlapping(self.dma_map.virt_addr() as *const u8, buffer.as_mut_ptr(), needed);
                }
            }
        } else {
            // PIO fallback (4 bytes at a time from Buffer Data Port)
            for s in 0..sector_count {
                let flag = if write { INT_SPACE_AVAIL } else { INT_DATA_AVAIL };
                let mut ready = false;
                for _ in 0..100_000 {
                    let st = self.r32(SDHCI_INT_STATUS);
                    if (st & INT_ERR_MASK) != 0 {
                        self.w32(SDHCI_INT_STATUS, st);
                        return Err(BlockDeviceError::IoError);
                    }
                    if (st & flag) != 0 {
                        self.w32(SDHCI_INT_STATUS, flag);
                        ready = true;
                        break;
                    }
                    core::hint::spin_loop();
                }
                if !ready {
                    return Err(BlockDeviceError::Timeout);
                }
                let base_off = s * 512;
                for w in 0..128 {
                    let off = base_off + w * 4;
                    if write {
                        let val = u32::from_le_bytes([
                            buffer[off],
                            buffer[off + 1],
                            buffer[off + 2],
                            buffer[off + 3],
                        ]);
                        self.w32(SDHCI_BUFFER, val);
                    } else {
                        let val = self.r32(SDHCI_BUFFER).to_le_bytes();
                        buffer[off..off + 4].copy_from_slice(&val);
                    }
                }
            }
            for _ in 0..50_000 {
                let st = self.r32(SDHCI_INT_STATUS);
                if (st & INT_XFER_COMPLETE) != 0 {
                    self.w32(SDHCI_INT_STATUS, INT_XFER_COMPLETE);
                    break;
                }
                core::hint::spin_loop();
            }
        }

        if write {
            self.snap.writes += sector_count as u64;
        } else {
            self.snap.reads += sector_count as u64;
        }
        Ok(())
    }
}

static SDHCI_SOFTC: Mtx<SdhciSoftc> = Mtx::new("sdhci0", SdhciSoftc::EMPTY);
static SDHCI_READY_FAST: AtomicBool = AtomicBool::new(false);
static SDHCI_MMIO_VIRT_FAST: AtomicU64 = AtomicU64::new(0);
static SDHCI_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

fn sdhci_irq_handler(_arg: usize) -> bool {
    if !SDHCI_READY_FAST.load(Ordering::Acquire) {
        return false;
    }
    let mmio = SDHCI_MMIO_VIRT_FAST.load(Ordering::Acquire);
    if mmio == 0 {
        return false;
    }
    let st = unsafe { read_volatile((mmio + SDHCI_INT_STATUS as u64) as *const u32) };
    if (st & 0x0000_00FF) != 0 || (st & INT_ERR_MASK) != 0 {
        SDHCI_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::drivers::iokit::signal_workloop(crate::drivers::iokit::WorkLoopKind::Storage);
        true
    } else {
        false
    }
}

pub fn irq_top_half() -> bool {
    sdhci_irq_handler(0)
}

pub fn irq_count() -> u64 {
    SDHCI_IRQ_COUNT.load(Ordering::Relaxed)
}

fn sdhci_probe(dev: &DeviceT) -> i32 {
    if dev.class_code == 0x08 && dev.subclass == 0x05 {
        BUS_PROBE_SPECIFIC
    } else {
        ENXIO
    }
}

fn sdhci_attach(
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

    let mut sc = SDHCI_SOFTC.lock();
    sc.snap.present = true;
    sc.mmio = handle;
    sc.snap.bar0_phys = bar0.phys_start;
    sc.snap.bar0_virt = handle.virt_base;
    sc.snap.irq_line = irq_res;

    if !sc.reset_host(0x01) {
        return 5;
    }

    sc.snap.host_version = sc.r16(SDHCI_HOST_VERSION);
    let caps = sc.r32(SDHCI_CAPABILITIES);
    sc.has_sdma = (caps & (1 << 22)) != 0;
    sc.w8(SDHCI_HOST_CONTROL, 0x00); // 1-bit + SDMA
    sc.setup_power_and_clock(false);
    sc.w32(SDHCI_INT_ENABLE, 0x00FF_003F);
    sc.w32(SDHCI_SIGNAL_ENABLE, 0);

    sc.tag = bus_dma_tag_create("sdhci_dma", PAGE_SIZE, 0xFFFF_FFFF, u64::MAX, PAGE_SIZE as usize, 1, PAGE_SIZE as usize);
    if let Ok(dmap) = bus_dmamem_alloc(&sc.tag, PAGE_SIZE as usize, BUS_DMA_NOWAIT | BUS_DMA_ZERO, map, allocator) {
        sc.dma_map = dmap;
        sc.snap.max_dma_sectors = 8;
    }

    sc.snap.initialized = true;
    sc.probe_card();

    let intr = bus_setup_intr(irq_res, "sdhci0", sdhci_irq_handler, dev_id).ok();
    sc.snap.irq_gsi = intr.map(|r| r.0);
    sc.snap.irq_vector = intr.map(|r| r.1);

    SDHCI_MMIO_VIRT_FAST.store(handle.virt_base, Ordering::Release);
    SDHCI_READY_FAST.store(true, Ordering::Release);

    let snap = sc.snap;
    drop(sc);

    with_device_mut(dev_id, |d| {
        d.bar0_phys = bar0.phys_start;
        d.bar0_virt = handle.virt_base;
        d.irq_line = irq_res;
        d.irq_gsi = intr.map(|r| r.0);
        d.irq_vector = intr.map(|r| r.1);
        d.set_desc(&alloc::format!(
            "SDHCI v0x{:02x} card={} sect={}",
            snap.host_version & 0xFF, snap.card_ready, snap.sector_count
        ));
    });
    0
}

fn sdhci_detach(_dev_id: DeviceId) -> i32 {
    SDHCI_READY_FAST.store(false, Ordering::Release);
    let mut sc = SDHCI_SOFTC.lock();
    if sc.snap.initialized {
        let _ = sc.reset_host(0x01);
        sc.snap.initialized = false;
    }
    0
}

pub static SDHCI_DRIVER: DriverT = DriverT {
    name: "sdhci_pci",
    probe: sdhci_probe,
    attach: sdhci_attach,
    detach: sdhci_detach,
};

pub fn init(
    desc: &PciSdhciDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<SdhciSnapshot, &'static str> {
    if !desc.valid || desc.bar0_phys == 0 {
        return Err("No SDHCI PCI controller");
    }
    let Some(pci_dev) = pci::find_pci_device(desc.bus, desc.device, desc.function) else {
        return Err("SDHCI PCI device missing from inventory");
    };
    let dev_id = device_add_pci_child(&pci_dev, "sdhci_pci");
    device_probe_and_attach(dev_id, &SDHCI_DRIVER, mapping, allocator)
        .map_err(|e| e.as_str())?;
    Ok(snapshot())
}

pub fn is_ready() -> bool {
    SDHCI_READY_FAST.load(Ordering::Acquire)
}

pub fn snapshot() -> SdhciSnapshot {
    SDHCI_SOFTC.lock().snap
}

pub fn read_sector_dma(lba: u64, buffer: &mut [u8; 512]) -> Result<(), BlockDeviceError> {
    SDHCI_SOFTC.lock().rw_sectors(false, lba, 1, buffer)
}

pub struct SdhciBlockDevice;

impl BlockDevice for SdhciBlockDevice {
    fn sector_size(&self) -> u32 {
        DEFAULT_SECTOR_SIZE
    }

    fn sector_count(&self) -> u64 {
        SDHCI_SOFTC.lock().snap.sector_count
    }

    fn max_transfer_sectors(&self) -> usize {
        SDHCI_SOFTC.lock().snap.max_dma_sectors.max(1)
    }

    fn read_sector(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
        SDHCI_SOFTC.lock().rw_sectors(false, lba, 1, buffer)
    }

    fn write_sector(&mut self, lba: u64, buffer: &[u8]) -> Result<(), BlockDeviceError> {
        if buffer.len() < 512 {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let mut tmp = [0u8; 512];
        tmp.copy_from_slice(&buffer[..512]);
        SDHCI_SOFTC.lock().rw_sectors(true, lba, 1, &mut tmp)
    }

    fn read_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        SDHCI_SOFTC.lock().rw_sectors(false, start_lba, sector_count, buffer)
    }

    fn write_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &[u8],
    ) -> Result<(), BlockDeviceError> {
        let needed = sector_count * 512;
        if buffer.len() < needed {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        let mut scratch = vec![0u8; needed];
        scratch.copy_from_slice(&buffer[..needed]);
        SDHCI_SOFTC.lock().rw_sectors(true, start_lba, sector_count, &mut scratch)
    }

    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        Ok(())
    }
}
