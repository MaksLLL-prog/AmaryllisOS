use core::ptr::{copy_nonoverlapping, read_volatile, write_bytes, write_volatile};
use core::sync::atomic::{fence, AtomicU64, AtomicU8, Ordering};

use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::task::preempt::PreemptMutex;

use crate::hardware::PciAhciDescriptor;
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::storage::block::{BlockDevice, BlockDeviceError, DEFAULT_SECTOR_SIZE};

const AHCI_MMIO_VIRT_BASE: u64 = 0x0000_5200_0000_0000;
const AHCI_MMIO_MAP_BYTES: u64 = 64 * 1024;

// HBA Generic Host Control Registers
const HBA_CAP: usize = 0x00;
const HBA_GHC: usize = 0x04;
const HBA_IS: usize = 0x08;
const HBA_PI: usize = 0x0C;
const HBA_VS: usize = 0x10;
const HBA_CAP2: usize = 0x24;

const GHC_IE: u32 = 1 << 1;
const GHC_AE: u32 = 1 << 31;

// Per-Port Registers (port_base = 0x100 + port * 0x80)
const PX_CLB: usize = 0x00;
const PX_CLBU: usize = 0x04;
const PX_FB: usize = 0x08;
const PX_FBU: usize = 0x0C;
const PX_IS: usize = 0x10;
const PX_IE: usize = 0x14;
const PX_CMD: usize = 0x18;
const PX_TFD: usize = 0x20;
const PX_SIG: usize = 0x24;
const PX_SSTS: usize = 0x28;
const PX_SERR: usize = 0x30;
const PX_CI: usize = 0x38;

const PX_CMD_ST: u32 = 1 << 0;
const PX_CMD_FRE: u32 = 1 << 4;
const PX_CMD_FR: u32 = 1 << 14;
const PX_CMD_CR: u32 = 1 << 15;

const PX_TFD_DRQ: u32 = 1 << 3;
const PX_TFD_BSY: u32 = 1 << 7;

const SATA_SIG_ATA: u32 = 0x0000_0101;

const FIS_TYPE_REG_H2D: u8 = 0x27;
const ATA_CMD_READ_DMA_EXT: u8 = 0x25;
const ATA_CMD_WRITE_DMA_EXT: u8 = 0x35;
const ATA_CMD_FLUSH_CACHE_EXT: u8 = 0xEA;
const ATA_CMD_IDENTIFY: u8 = 0xEC;

const AHCI_MAX_DMA_FRAMES: usize = 16;

#[repr(C)]
#[derive(Clone, Copy)]
struct HbaCmdHeader {
    dw0: u32, // bits 0..4 CFL (FIS len in dwords), bit 6 Write, bit 7 Prefetchable
    prdtl: u16,
    prdbc: u32,
    ctba: u32,
    ctbau: u32,
    rsv: [u32; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
struct HbaPrdtEntry {
    dba: u32,
    dbau: u32,
    rsv: u32,
    dbc: u32, // byte_count - 1 (bits 0..21)
}

#[repr(C)]
#[derive(Clone, Copy)]
struct FisRegH2D {
    fis_type: u8,
    pmport_c: u8, // bit 7 = C (command)
    command: u8,
    featurel: u8,
    lba0: u8,
    lba1: u8,
    lba2: u8,
    device: u8,
    lba3: u8,
    lba4: u8,
    lba5: u8,
    featureh: u8,
    countl: u8,
    counth: u8,
    icc: u8,
    control: u8,
    rsv: [u8; 4],
}

#[repr(C, align(128))]
struct HbaCmdTable {
    cfis: [u8; 64],
    acmd: [u8; 16],
    rsv: [u8; 48],
    prdt: [HbaPrdtEntry; AHCI_MAX_DMA_FRAMES],
}

#[derive(Clone, Copy)]
pub struct AhciSnapshot {
    pub initialized: bool,
    pub present: bool,
    pub abar_phys: u64,
    pub abar_virt: u64,
    pub version: u32,
    pub cap: u32,
    pub cap2: u32,
    pub ports_implemented: u32,
    pub active_port: Option<u8>,
    pub port_slots: u8,
    pub sector_count: u64,
    pub model: [u8; 40],
    pub model_len: usize,
    pub reads: u64,
    pub writes: u64,
    pub max_dma_sectors: usize,
    pub dma_batches: u64,
}

impl AhciSnapshot {
    pub const EMPTY: Self = Self {
        initialized: false,
        present: false,
        abar_phys: 0,
        abar_virt: 0,
        version: 0,
        cap: 0,
        cap2: 0,
        ports_implemented: 0,
        active_port: None,
        port_slots: 0,
        sector_count: 0,
        model: [0; 40],
        model_len: 0,
        reads: 0,
        writes: 0,
        max_dma_sectors: 1,
        dma_batches: 0,
    };

    pub fn model_str(&self) -> &str {
        core::str::from_utf8(&self.model[..self.model_len]).unwrap_or("AHCI SATA Disk")
    }
}

struct AhciController {
    snap: AhciSnapshot,
    clb_virt: u64,
    cmd_table_phys: u64,
    cmd_table_virt: u64,
    dma_frames_phys: [u64; AHCI_MAX_DMA_FRAMES],
    dma_frames_virt: [u64; AHCI_MAX_DMA_FRAMES],
    dma_frame_count: usize,
}

impl AhciController {
    const fn empty() -> Self {
        Self {
            snap: AhciSnapshot::EMPTY,
            clb_virt: 0,
            cmd_table_phys: 0,
            cmd_table_virt: 0,
            dma_frames_phys: [0; AHCI_MAX_DMA_FRAMES],
            dma_frames_virt: [0; AHCI_MAX_DMA_FRAMES],
            dma_frame_count: 0,
        }
    }

    #[inline]
    unsafe fn hba_read(&self, off: usize) -> u32 {
        read_volatile((self.snap.abar_virt + off as u64) as *const u32)
    }

    #[inline]
    unsafe fn hba_write(&self, off: usize, val: u32) {
        write_volatile((self.snap.abar_virt + off as u64) as *mut u32, val);
    }

    #[inline]
    unsafe fn port_read(&self, port: u8, off: usize) -> u32 {
        self.hba_read(0x100 + (port as usize) * 0x80 + off)
    }

    #[inline]
    unsafe fn port_write(&self, port: u8, off: usize, val: u32) {
        self.hba_write(0x100 + (port as usize) * 0x80 + off, val);
    }

    fn stop_port(&self, port: u8) {
        unsafe {
            let mut cmd = self.port_read(port, PX_CMD);
            cmd &= !(PX_CMD_ST | PX_CMD_FRE);
            self.port_write(port, PX_CMD, cmd);
            for _ in 0..500_000 {
                let cur = self.port_read(port, PX_CMD);
                if (cur & (PX_CMD_CR | PX_CMD_FR)) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
        }
    }

    fn start_port(&self, port: u8) {
        unsafe {
            for _ in 0..500_000 {
                if (self.port_read(port, PX_CMD) & PX_CMD_CR) == 0 {
                    break;
                }
                core::hint::spin_loop();
            }
            let mut cmd = self.port_read(port, PX_CMD);
            cmd |= PX_CMD_FRE;
            self.port_write(port, PX_CMD, cmd);
            cmd |= PX_CMD_ST;
            self.port_write(port, PX_CMD, cmd);
        }
    }

    fn issue_dma_command(
        &mut self,
        command: u8,
        lba: u64,
        sector_count: usize,
        byte_count: usize,
        is_write: bool,
    ) -> Result<(), BlockDeviceError> {
        let Some(port) = self.snap.active_port else {
            return Err(BlockDeviceError::DeviceNotReady);
        };
        if !self.snap.initialized {
            return Err(BlockDeviceError::DeviceNotReady);
        }

        let max_bytes = self.dma_frame_count * PAGE_SIZE as usize;
        if byte_count > max_bytes {
            return Err(BlockDeviceError::InvalidBufferSize);
        }

        let prdt_count = if byte_count == 0 {
            0usize
        } else {
            (byte_count + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize
        };

        unsafe {
            // Wait until BSY/DRQ clear
            let mut spin = 0u32;
            while (self.port_read(port, PX_TFD) & (PX_TFD_BSY | PX_TFD_DRQ)) != 0 {
                spin += 1;
                if spin > 1_500_000 {
                    return Err(BlockDeviceError::Timeout);
                }
                core::hint::spin_loop();
            }

            self.port_write(port, PX_IS, u32::MAX);

            let cmd_hdr = self.clb_virt as *mut HbaCmdHeader;
            let cfl_dwords = (core::mem::size_of::<FisRegH2D>() / 4) as u32;
            let mut dw0 = cfl_dwords;
            if is_write {
                dw0 |= 1 << 6;
            }
            write_volatile(
                cmd_hdr,
                HbaCmdHeader {
                    dw0,
                    prdtl: prdt_count as u16,
                    prdbc: 0,
                    ctba: self.cmd_table_phys as u32,
                    ctbau: (self.cmd_table_phys >> 32) as u32,
                    rsv: [0; 4],
                },
            );

            let tbl = self.cmd_table_virt as *mut HbaCmdTable;
            write_bytes(tbl as *mut u8, 0, core::mem::size_of::<HbaCmdTable>());

            let mut rem = byte_count;
            for i in 0..prdt_count {
                let chunk_bytes = rem.min(PAGE_SIZE as usize);
                let frame_phys = self.dma_frames_phys[i];
                (*tbl).prdt[i] = HbaPrdtEntry {
                    dba: frame_phys as u32,
                    dbau: (frame_phys >> 32) as u32,
                    rsv: 0,
                    dbc: (chunk_bytes - 1) as u32,
                };
                rem -= chunk_bytes;
            }

            let fis = (*tbl).cfis.as_mut_ptr() as *mut FisRegH2D;
            *fis = FisRegH2D {
                fis_type: FIS_TYPE_REG_H2D,
                pmport_c: 0x80,
                command,
                featurel: 0,
                lba0: (lba & 0xFF) as u8,
                lba1: ((lba >> 8) & 0xFF) as u8,
                lba2: ((lba >> 16) & 0xFF) as u8,
                device: if command == ATA_CMD_IDENTIFY { 0 } else { 1 << 6 },
                lba3: ((lba >> 24) & 0xFF) as u8,
                lba4: ((lba >> 32) & 0xFF) as u8,
                lba5: ((lba >> 40) & 0xFF) as u8,
                featureh: 0,
                countl: (sector_count & 0xFF) as u8,
                counth: ((sector_count >> 8) & 0xFF) as u8,
                icc: 0,
                control: 0,
                rsv: [0; 4],
            };

            fence(Ordering::SeqCst);
            self.port_write(port, PX_CI, 1);

            let mut poll = 0u32;
            loop {
                let ci = self.port_read(port, PX_CI);
                if (ci & 1) == 0 {
                    break;
                }
                let is = self.port_read(port, PX_IS);
                if (is & (1 << 30)) != 0 {
                    self.port_write(port, PX_IS, u32::MAX);
                    return Err(BlockDeviceError::IoError);
                }
                poll += 1;
                if poll > 4_000_000 {
                    return Err(BlockDeviceError::Timeout);
                }
                core::hint::spin_loop();
            }

            if (self.port_read(port, PX_IS) & (1 << 30)) != 0 {
                self.port_write(port, PX_IS, u32::MAX);
                return Err(BlockDeviceError::IoError);
            }
            fence(Ordering::SeqCst);
        }

        Ok(())
    }

    fn copy_from_dma(&self, dst: &mut [u8]) {
        let mut off = 0usize;
        let mut i = 0usize;
        while off < dst.len() && i < self.dma_frame_count {
            let chunk = (dst.len() - off).min(PAGE_SIZE as usize);
            unsafe {
                copy_nonoverlapping(
                    self.dma_frames_virt[i] as *const u8,
                    dst.as_mut_ptr().add(off),
                    chunk,
                );
            }
            off += chunk;
            i += 1;
        }
    }

    fn copy_to_dma(&self, src: &[u8]) {
        let mut off = 0usize;
        let mut i = 0usize;
        while off < src.len() && i < self.dma_frame_count {
            let chunk = (src.len() - off).min(PAGE_SIZE as usize);
            unsafe {
                copy_nonoverlapping(
                    src.as_ptr().add(off),
                    self.dma_frames_virt[i] as *mut u8,
                    chunk,
                );
            }
            off += chunk;
            i += 1;
        }
    }

    fn identify(&mut self) -> bool {
        if self
            .issue_dma_command(ATA_CMD_IDENTIFY, 0, 1, 512, false)
            .is_err()
        {
            return false;
        }
        let words = unsafe {
            core::slice::from_raw_parts(self.dma_frames_virt[0] as *const u16, 256)
        };
        let lba48 = (words[100] as u64)
            | ((words[101] as u64) << 16)
            | ((words[102] as u64) << 32)
            | ((words[103] as u64) << 48);
        let lba28 = (words[60] as u64) | ((words[61] as u64) << 16);
        self.snap.sector_count = if lba48 != 0 { lba48 } else { lba28 };

        let mut model_bytes = [0u8; 40];
        for i in 0..20 {
            let w = words[27 + i];
            model_bytes[i * 2] = (w >> 8) as u8;
            model_bytes[i * 2 + 1] = (w & 0xFF) as u8;
        }
        let trimmed = core::str::from_utf8(&model_bytes)
            .unwrap_or("AHCI SATA")
            .trim();
        let bytes = trimmed.as_bytes();
        let len = bytes.len().min(40);
        self.snap.model[..len].copy_from_slice(&bytes[..len]);
        self.snap.model_len = len;
        true
    }
}

static AHCI: PreemptMutex<AhciController> = PreemptMutex::new(AhciController::empty());
static AHCI_ABAR_VIRT_FAST: AtomicU64 = AtomicU64::new(0);
static AHCI_ACTIVE_PORT_FAST: AtomicU8 = AtomicU8::new(0xFF);
static AHCI_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

/// Lock-free Top-Half interrupt handler for AHCI SATA HBA.
pub fn irq_top_half() -> bool {
    let abar = AHCI_ABAR_VIRT_FAST.load(Ordering::Acquire);
    let port = AHCI_ACTIVE_PORT_FAST.load(Ordering::Acquire);
    if abar == 0 || port == 0xFF {
        return false;
    }
    unsafe {
        let hba_is = read_volatile((abar + HBA_IS as u64) as *const u32);
        let port_off = 0x100 + (port as usize) * 0x80;
        let px_is = read_volatile((abar + (port_off + PX_IS) as u64) as *const u32);
        if px_is != 0 {
            write_volatile((abar + (port_off + PX_IS) as u64) as *mut u32, px_is);
        }
        if hba_is != 0 {
            write_volatile((abar + HBA_IS as u64) as *mut u32, hba_is);
        }
        if px_is != 0 || hba_is != 0 {
            AHCI_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
            crate::drivers::iokit::signal_workloop(crate::drivers::iokit::WorkLoopKind::Storage);
            true
        } else {
            false
        }
    }
}

pub fn irq_count() -> u64 {
    AHCI_IRQ_COUNT.load(Ordering::Relaxed)
}

pub fn init(
    desc: &PciAhciDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<AhciSnapshot, &'static str> {
    if !desc.valid || desc.abar_phys == 0 {
        return Err("No AHCI PCI controller");
    }
    let Some(mapping) = mapping else {
        return Err("PhysicalMemoryMapping unavailable");
    };

    let map_len = desc.abar_size.max(0x2000).min(AHCI_MMIO_MAP_BYTES);
    let pages = ((map_len + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
    let phys_base = desc.abar_phys & !(PAGE_SIZE - 1);
    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH;

    for i in 0..pages {
        let virt = VirtAddr::new(AHCI_MMIO_VIRT_BASE + (i as u64) * PAGE_SIZE);
        let phys = phys_base + (i as u64) * PAGE_SIZE;
        if mapping.translate_virtual_address(virt).is_none() {
            mapping
                .map_page(allocator, virt, phys, flags)
                .map_err(|_| "AHCI MMIO map_page failed")?;
        }
    }

    let abar_virt = AHCI_MMIO_VIRT_BASE + (desc.abar_phys & (PAGE_SIZE - 1));
    let clb_frame = allocator.allocate_frame().ok_or("No frame for AHCI CLB/FB")?;
    let cmd_frame = allocator.allocate_frame().ok_or("No frame for AHCI CmdTable")?;

    let clb_virt = mapping
        .translate_frame(clb_frame)
        .ok_or("HHDM fail clb")?
        .as_u64();
    let cmd_virt = mapping
        .translate_frame(cmd_frame)
        .ok_or("HHDM fail cmd")?
        .as_u64();

    unsafe {
        write_bytes(clb_virt as *mut u8, 0, PAGE_SIZE as usize);
        write_bytes(cmd_virt as *mut u8, 0, PAGE_SIZE as usize);
    }

    let mut dma_frames_phys = [0u64; AHCI_MAX_DMA_FRAMES];
    let mut dma_frames_virt = [0u64; AHCI_MAX_DMA_FRAMES];
    let mut dma_frame_count = 0usize;
    for i in 0..AHCI_MAX_DMA_FRAMES {
        let Some(frame) = allocator.allocate_frame() else {
            break;
        };
        let Some(virt) = mapping.translate_frame(frame) else {
            let _ = allocator.deallocate_frame(frame);
            break;
        };
        let v = virt.as_u64();
        unsafe {
            write_bytes(v as *mut u8, 0, PAGE_SIZE as usize);
        }
        dma_frames_phys[i] = frame;
        dma_frames_virt[i] = v;
        dma_frame_count += 1;
    }
    if dma_frame_count == 0 {
        return Err("No frame for AHCI DMA buf");
    }

    let mut ctrl = AHCI.lock();
    ctrl.snap = AhciSnapshot::EMPTY;
    ctrl.snap.present = true;
    ctrl.snap.abar_phys = desc.abar_phys;
    ctrl.snap.abar_virt = abar_virt;
    ctrl.clb_virt = clb_virt;
    ctrl.cmd_table_phys = cmd_frame;
    ctrl.cmd_table_virt = cmd_virt;
    ctrl.dma_frames_phys = dma_frames_phys;
    ctrl.dma_frames_virt = dma_frames_virt;
    ctrl.dma_frame_count = dma_frame_count;
    ctrl.snap.max_dma_sectors = dma_frame_count * (PAGE_SIZE as usize / DEFAULT_SECTOR_SIZE as usize);

    unsafe {
        let ghc = ctrl.hba_read(HBA_GHC);
        ctrl.hba_write(HBA_GHC, ghc | GHC_AE);

        ctrl.snap.cap = ctrl.hba_read(HBA_CAP);
        ctrl.snap.cap2 = ctrl.hba_read(HBA_CAP2);
        ctrl.snap.version = ctrl.hba_read(HBA_VS);
        ctrl.snap.ports_implemented = ctrl.hba_read(HBA_PI);
        ctrl.snap.port_slots = (((ctrl.snap.cap >> 8) & 0x1F) + 1) as u8;
    }

    let mut found_port = None;
    for p in 0..32u8 {
        if (ctrl.snap.ports_implemented & (1u32 << p)) == 0 {
            continue;
        }
        let ssts = unsafe { ctrl.port_read(p, PX_SSTS) };
        let det = ssts & 0xF;
        let ipm = (ssts >> 8) & 0xF;
        let sig = unsafe { ctrl.port_read(p, PX_SIG) };
        if det == 3 && ipm == 1 && sig == SATA_SIG_ATA {
            found_port = Some(p);
            break;
        }
    }

    if let Some(port) = found_port {
        ctrl.snap.active_port = Some(port);
        ctrl.stop_port(port);

        let fb_phys = clb_frame + 2048;
        unsafe {
            ctrl.port_write(port, PX_CLB, clb_frame as u32);
            ctrl.port_write(port, PX_CLBU, (clb_frame >> 32) as u32);
            ctrl.port_write(port, PX_FB, fb_phys as u32);
            ctrl.port_write(port, PX_FBU, (fb_phys >> 32) as u32);
            ctrl.port_write(port, PX_SERR, u32::MAX);
            ctrl.port_write(port, PX_IS, u32::MAX);
            ctrl.port_write(port, PX_IE, 0x0000_0021); // DHRS | DPS
            let ghc = ctrl.hba_read(HBA_GHC);
            ctrl.hba_write(HBA_IS, u32::MAX);
            ctrl.hba_write(HBA_GHC, ghc | GHC_AE | GHC_IE);
        }
        ctrl.start_port(port);
        ctrl.snap.initialized = true;
        AHCI_ABAR_VIRT_FAST.store(abar_virt, Ordering::Release);
        AHCI_ACTIVE_PORT_FAST.store(port, Ordering::Release);
        let _ = ctrl.identify();
    } else {
        ctrl.snap.initialized = true;
    }

    Ok(ctrl.snap)
}

pub fn snapshot() -> AhciSnapshot {
    AHCI.lock().snap
}

pub fn read_sectors_dma(
    start_lba: u64,
    sector_count: usize,
    buffer: &mut [u8],
) -> Result<(), BlockDeviceError> {
    if sector_count == 0 {
        return if buffer.is_empty() {
            Ok(())
        } else {
            Err(BlockDeviceError::InvalidBufferSize)
        };
    }
    let expected = sector_count
        .checked_mul(DEFAULT_SECTOR_SIZE as usize)
        .ok_or(BlockDeviceError::InvalidBufferSize)?;
    if buffer.len() != expected {
        return Err(BlockDeviceError::InvalidBufferSize);
    }

    let mut ctrl = AHCI.lock();
    if ctrl.snap.active_port.is_none() || !ctrl.snap.initialized {
        return Err(BlockDeviceError::DeviceNotReady);
    }
    let end_lba = start_lba
        .checked_add(sector_count as u64)
        .ok_or(BlockDeviceError::SectorOutOfRange)?;
    if ctrl.snap.sector_count > 0 && end_lba > ctrl.snap.sector_count {
        return Err(BlockDeviceError::SectorOutOfRange);
    }

    let max_chunk = ctrl.snap.max_dma_sectors.max(1);
    let mut done = 0usize;
    while done < sector_count {
        let chunk = (sector_count - done).min(max_chunk);
        let bytes = chunk * DEFAULT_SECTOR_SIZE as usize;
        ctrl.issue_dma_command(
            ATA_CMD_READ_DMA_EXT,
            start_lba + done as u64,
            chunk,
            bytes,
            false,
        )?;
        let byte_off = done * DEFAULT_SECTOR_SIZE as usize;
        ctrl.copy_from_dma(&mut buffer[byte_off..byte_off + bytes]);
        ctrl.snap.reads = ctrl.snap.reads.saturating_add(chunk as u64);
        ctrl.snap.dma_batches = ctrl.snap.dma_batches.saturating_add(1);
        done += chunk;
    }
    Ok(())
}

pub fn write_sectors_dma(
    start_lba: u64,
    sector_count: usize,
    buffer: &[u8],
) -> Result<(), BlockDeviceError> {
    if sector_count == 0 {
        return if buffer.is_empty() {
            Ok(())
        } else {
            Err(BlockDeviceError::InvalidBufferSize)
        };
    }
    let expected = sector_count
        .checked_mul(DEFAULT_SECTOR_SIZE as usize)
        .ok_or(BlockDeviceError::InvalidBufferSize)?;
    if buffer.len() != expected {
        return Err(BlockDeviceError::InvalidBufferSize);
    }

    let mut ctrl = AHCI.lock();
    if ctrl.snap.active_port.is_none() || !ctrl.snap.initialized {
        return Err(BlockDeviceError::DeviceNotReady);
    }
    let end_lba = start_lba
        .checked_add(sector_count as u64)
        .ok_or(BlockDeviceError::SectorOutOfRange)?;
    if ctrl.snap.sector_count > 0 && end_lba > ctrl.snap.sector_count {
        return Err(BlockDeviceError::SectorOutOfRange);
    }

    let max_chunk = ctrl.snap.max_dma_sectors.max(1);
    let mut done = 0usize;
    while done < sector_count {
        let chunk = (sector_count - done).min(max_chunk);
        let bytes = chunk * DEFAULT_SECTOR_SIZE as usize;
        let byte_off = done * DEFAULT_SECTOR_SIZE as usize;
        ctrl.copy_to_dma(&buffer[byte_off..byte_off + bytes]);
        ctrl.issue_dma_command(
            ATA_CMD_WRITE_DMA_EXT,
            start_lba + done as u64,
            chunk,
            bytes,
            true,
        )?;
        ctrl.snap.writes = ctrl.snap.writes.saturating_add(chunk as u64);
        ctrl.snap.dma_batches = ctrl.snap.dma_batches.saturating_add(1);
        done += chunk;
    }
    Ok(())
}

pub fn flush_dma() -> Result<(), BlockDeviceError> {
    let mut ctrl = AHCI.lock();
    if ctrl.snap.active_port.is_none() || !ctrl.snap.initialized {
        return Err(BlockDeviceError::DeviceNotReady);
    }
    ctrl.issue_dma_command(ATA_CMD_FLUSH_CACHE_EXT, 0, 0, 0, false)
}

pub fn read_sector_dma(lba: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
    read_sectors_dma(lba, 1, buffer)
}

pub fn write_sector_dma(lba: u64, buffer: &[u8]) -> Result<(), BlockDeviceError> {
    write_sectors_dma(lba, 1, buffer)
}

pub struct AhciBlockDevice;

impl BlockDevice for AhciBlockDevice {
    fn sector_size(&self) -> u32 {
        DEFAULT_SECTOR_SIZE
    }

    fn sector_count(&self) -> u64 {
        AHCI.lock().snap.sector_count
    }

    fn max_transfer_sectors(&self) -> usize {
        AHCI.lock().snap.max_dma_sectors.max(1)
    }

    fn read_sector(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
        read_sector_dma(lba, buffer)
    }

    fn write_sector(&mut self, lba: u64, buffer: &[u8]) -> Result<(), BlockDeviceError> {
        write_sector_dma(lba, buffer)
    }

    fn read_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        read_sectors_dma(start_lba, sector_count, buffer)
    }

    fn write_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &[u8],
    ) -> Result<(), BlockDeviceError> {
        write_sectors_dma(start_lba, sector_count, buffer)
    }

    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        flush_dma()
    }
}
