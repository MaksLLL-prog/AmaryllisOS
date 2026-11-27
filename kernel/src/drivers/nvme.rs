use core::ptr::{copy_nonoverlapping, read_volatile, write_volatile};
use core::sync::atomic::{AtomicBool, AtomicU16, AtomicU64, Ordering};

use crate::freebsd_kpi::{
    bus_alloc_resource_any, bus_dma_tag_create, bus_dmamap_sync, bus_dmamem_alloc,
    bus_setup_intr, bus_space_barrier, bus_space_read_4, bus_space_read_8, bus_space_write_4,
    bus_space_write_8, device_add_pci_child, device_probe_and_attach, pci_enable_busmaster,
    with_device_mut, BusDmaMap, BusDmaTag, BusSpaceHandle, DeviceId, DeviceT, DriverT, KpiError,
    Mtx, BUS_DMASYNC_POSTREAD, BUS_DMASYNC_PREWRITE, BUS_DMA_NOWAIT, BUS_DMA_ZERO,
    BUS_PROBE_SPECIFIC, ENXIO, SYS_RES_IRQ, SYS_RES_MEMORY,
};
use crate::hardware::PciNvmeDescriptor;
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::pci;
use crate::storage::block::{BlockDevice, BlockDeviceError, DEFAULT_SECTOR_SIZE};

const ADMIN_Q_DEPTH: usize = 32;
const IO_Q_DEPTH: usize = 32;
const NVME_MAX_DMA_PAGES: usize = 8; // 32 KiB = 64 x 512B sectors per batch

// NVMe Controller BAR0 Register Offsets
const NVME_REG_CAP: usize = 0x00;
const NVME_REG_VS: usize = 0x08;
const NVME_REG_INTMS: usize = 0x0C;
const NVME_REG_INTMC: usize = 0x10;
const NVME_REG_CC: usize = 0x14;
const NVME_REG_CSTS: usize = 0x1C;
const NVME_REG_AQA: usize = 0x24;
const NVME_REG_ASQ: usize = 0x28;
const NVME_REG_ACQ: usize = 0x30;
const NVME_DOORBELL_BASE: usize = 0x1000;

const CC_EN: u32 = 1 << 0;
const CC_IOSQES_64B: u32 = 6 << 16;
const CC_IOCQES_16B: u32 = 4 << 20;
const CSTS_RDY: u32 = 1 << 0;
const CSTS_CFS: u32 = 1 << 1;

// Admin Opcodes
const NVME_ADMIN_CREATE_SQ: u8 = 0x01;
const NVME_ADMIN_CREATE_CQ: u8 = 0x05;
const NVME_ADMIN_IDENTIFY: u8 = 0x06;

// NVM Command Opcodes
const NVME_CMD_FLUSH: u8 = 0x00;
const NVME_CMD_WRITE: u8 = 0x01;
const NVME_CMD_READ: u8 = 0x02;

#[repr(C)]
#[derive(Clone, Copy)]
struct NvmeSqe {
    cdw0: u32,
    nsid: u32,
    cdw2: u32,
    cdw3: u32,
    mptr: u64,
    prp1: u64,
    prp2: u64,
    cdw10: u32,
    cdw11: u32,
    cdw12: u32,
    cdw13: u32,
    cdw14: u32,
    cdw15: u32,
}

impl NvmeSqe {
    const ZERO: Self = Self {
        cdw0: 0,
        nsid: 0,
        cdw2: 0,
        cdw3: 0,
        mptr: 0,
        prp1: 0,
        prp2: 0,
        cdw10: 0,
        cdw11: 0,
        cdw12: 0,
        cdw13: 0,
        cdw14: 0,
        cdw15: 0,
    };

    #[inline]
    fn with_opcode(opcode: u8, cid: u16, nsid: u32) -> Self {
        let mut s = Self::ZERO;
        s.cdw0 = (opcode as u32) | ((cid as u32) << 16);
        s.nsid = nsid;
        s
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
struct NvmeCqe {
    result: u32,
    rsvd: u32,
    sq_head: u16,
    sq_id: u16,
    cid: u16,
    status: u16,
}

#[derive(Clone, Copy)]
pub struct NvmeSnapshot {
    pub present: bool,
    pub initialized: bool,
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    pub vendor_id: u16,
    pub device_id: u16,
    pub bar0_phys: u64,
    pub bar0_virt: u64,
    pub version: u32,
    pub cap: u64,
    pub dstrd_bytes: usize,
    pub max_queue_entries: u16,
    pub irq_line: u8,
    pub irq_gsi: Option<u32>,
    pub irq_vector: Option<u8>,
    pub nsid: u32,
    pub lba_size: u32,
    pub total_lbas: u64,
    pub sector_count_512: u64,
    pub model: [u8; 40],
    pub model_len: usize,
    pub serial: [u8; 20],
    pub serial_len: usize,
    pub reads: u64,
    pub writes: u64,
    pub dma_batches: u64,
    pub max_dma_sectors: usize,
}

impl NvmeSnapshot {
    pub const EMPTY: Self = Self {
        present: false,
        initialized: false,
        bus: 0,
        device: 0,
        function: 0,
        vendor_id: 0,
        device_id: 0,
        bar0_phys: 0,
        bar0_virt: 0,
        version: 0,
        cap: 0,
        dstrd_bytes: 4,
        max_queue_entries: 0,
        irq_line: 0,
        irq_gsi: None,
        irq_vector: None,
        nsid: 1,
        lba_size: 512,
        total_lbas: 0,
        sector_count_512: 0,
        model: [0; 40],
        model_len: 0,
        serial: [0; 20],
        serial_len: 0,
        reads: 0,
        writes: 0,
        dma_batches: 0,
        max_dma_sectors: NVME_MAX_DMA_PAGES * 8,
    };

    pub fn model_str(&self) -> &str {
        core::str::from_utf8(&self.model[..self.model_len]).unwrap_or("NVMe Namespace")
    }

    pub fn serial_str(&self) -> &str {
        core::str::from_utf8(&self.serial[..self.serial_len]).unwrap_or("")
    }
}

struct NvmeQueueState {
    sq_map: BusDmaMap,
    cq_map: BusDmaMap,
    depth: usize,
    sq_tail: usize,
    cq_head: usize,
    cq_phase: u16,
    next_cid: u16,
    qid: usize,
}

impl NvmeQueueState {
    const fn empty(qid: usize, depth: usize) -> Self {
        Self {
            sq_map: BusDmaMap::empty(),
            cq_map: BusDmaMap::empty(),
            depth,
            sq_tail: 0,
            cq_head: 0,
            cq_phase: 1,
            next_cid: 1,
            qid,
        }
    }
}

struct NvmeSoftc {
    snap: NvmeSnapshot,
    newbus_id: Option<DeviceId>,
    bar0: BusSpaceHandle,
    dma_tag_q: BusDmaTag,
    dma_tag_io: BusDmaTag,
    admin_q: NvmeQueueState,
    io_q: NvmeQueueState,
    ident_map: BusDmaMap,
    prp_list_map: BusDmaMap,
    io_data_map: BusDmaMap,
}

impl NvmeSoftc {
    const fn empty() -> Self {
        Self {
            snap: NvmeSnapshot::EMPTY,
            newbus_id: None,
            bar0: BusSpaceHandle::EMPTY,
            dma_tag_q: BusDmaTag::EMPTY,
            dma_tag_io: BusDmaTag::EMPTY,
            admin_q: NvmeQueueState::empty(0, ADMIN_Q_DEPTH),
            io_q: NvmeQueueState::empty(1, IO_Q_DEPTH),
            ident_map: BusDmaMap::empty(),
            prp_list_map: BusDmaMap::empty(),
            io_data_map: BusDmaMap::empty(),
        }
    }

    fn submit_sync(
        bar0: &BusSpaceHandle,
        dstrd_bytes: usize,
        q: &mut NvmeQueueState,
        mut sqe: NvmeSqe,
    ) -> Result<NvmeCqe, KpiError> {
        let sq_virt = q.sq_map.virt_addr();
        let cq_virt = q.cq_map.virt_addr();
        if sq_virt == 0 || cq_virt == 0 {
            return Err(KpiError::NoDev);
        }

        let cid = q.next_cid;
        q.next_cid = q.next_cid.wrapping_add(1).max(1);
        sqe.cdw0 = (sqe.cdw0 & 0x0000_FFFF) | ((cid as u32) << 16);

        let sq_slot = q.sq_tail;
        let sq_ptr = (sq_virt + (sq_slot * core::mem::size_of::<NvmeSqe>()) as u64) as *mut NvmeSqe;
        unsafe {
            write_volatile(sq_ptr, sqe);
        }
        bus_dmamap_sync(BUS_DMASYNC_PREWRITE);

        q.sq_tail = (q.sq_tail + 1) % q.depth;
        let sq_db = NVME_DOORBELL_BASE + (2 * q.qid) * dstrd_bytes;
        bus_space_write_4(bar0, sq_db, q.sq_tail as u32);

        if q.qid == 1 {
            NVME_IO_ACTIVE.store(true, Ordering::Release);
        }

        let cq_ptr =
            (cq_virt + (q.cq_head * core::mem::size_of::<NvmeCqe>()) as u64) as *const NvmeCqe;

        let mut completed: Option<NvmeCqe> = None;
        for _ in 0..8_000_000 {
            let cqe = unsafe { read_volatile(cq_ptr) };
            if (cqe.status & 1) == q.cq_phase {
                completed = Some(cqe);
                break;
            }
            core::hint::spin_loop();
        }

        if q.qid == 1 {
            NVME_IO_ACTIVE.store(false, Ordering::Release);
        }

        let Some(cqe) = completed else {
            return Err(KpiError::TimedOut);
        };

        bus_dmamap_sync(BUS_DMASYNC_POSTREAD);

        q.cq_head += 1;
        if q.cq_head >= q.depth {
            q.cq_head = 0;
            q.cq_phase ^= 1;
        }
        if q.qid == 1 {
            NVME_IOCQ_HEAD_FAST.store(q.cq_head as u16, Ordering::Release);
            NVME_IOCQ_PHASE_FAST.store(q.cq_phase, Ordering::Release);
        }

        let cq_db = NVME_DOORBELL_BASE + (2 * q.qid + 1) * dstrd_bytes;
        bus_space_write_4(bar0, cq_db, q.cq_head as u32);

        let status_field = cqe.status >> 1;
        if status_field != 0 || cqe.cid != cid {
            return Err(KpiError::Io);
        }
        Ok(cqe)
    }

    fn wait_ready(&self, want_ready: bool) -> Result<(), KpiError> {
        for _ in 0..5_000_000 {
            let csts = bus_space_read_4(&self.bar0, NVME_REG_CSTS);
            if (csts & CSTS_CFS) != 0 {
                return Err(KpiError::Io);
            }
            let rdy = (csts & CSTS_RDY) != 0;
            if rdy == want_ready {
                return Ok(());
            }
            core::hint::spin_loop();
        }
        Err(KpiError::TimedOut)
    }

    fn identify(&mut self) -> Result<(), KpiError> {
        let ident_phys = self.ident_map.phys_addr();
        let ident_virt = self.ident_map.virt_addr();

        // 1. Identify Controller (CNS = 1)
        let mut sqe = NvmeSqe::with_opcode(NVME_ADMIN_IDENTIFY, 0, 0);
        sqe.prp1 = ident_phys;
        sqe.cdw10 = 1;
        Self::submit_sync(
            &self.bar0,
            self.snap.dstrd_bytes,
            &mut self.admin_q,
            sqe,
        )?;

        let id_bytes = unsafe { core::slice::from_raw_parts(ident_virt as *const u8, 4096) };
        let serial_slice = trim_ascii(&id_bytes[4..24]);
        let model_slice = trim_ascii(&id_bytes[24..64]);
        let sn_len = serial_slice.len().min(20);
        let mn_len = model_slice.len().min(40);
        self.snap.serial[..sn_len].copy_from_slice(&serial_slice[..sn_len]);
        self.snap.serial_len = sn_len;
        self.snap.model[..mn_len].copy_from_slice(&model_slice[..mn_len]);
        self.snap.model_len = mn_len;

        // 2. Identify Namespace 1 (CNS = 0, NSID = 1)
        let mut ns_sqe = NvmeSqe::with_opcode(NVME_ADMIN_IDENTIFY, 0, 1);
        ns_sqe.prp1 = ident_phys;
        ns_sqe.cdw10 = 0;
        Self::submit_sync(
            &self.bar0,
            self.snap.dstrd_bytes,
            &mut self.admin_q,
            ns_sqe,
        )?;

        let ns_bytes = unsafe { core::slice::from_raw_parts(ident_virt as *const u8, 4096) };
        let nsze = u64::from_le_bytes([
            ns_bytes[0], ns_bytes[1], ns_bytes[2], ns_bytes[3], ns_bytes[4], ns_bytes[5],
            ns_bytes[6], ns_bytes[7],
        ]);
        let flbas = (ns_bytes[26] & 0x0F) as usize;
        let lbaf_off = 128 + flbas * 4;
        let lbaf = u32::from_le_bytes([
            ns_bytes[lbaf_off],
            ns_bytes[lbaf_off + 1],
            ns_bytes[lbaf_off + 2],
            ns_bytes[lbaf_off + 3],
        ]);
        let lbads = ((lbaf >> 16) & 0xFF) as u32;
        let lba_size = if lbads >= 9 && lbads <= 16 {
            1u32 << lbads
        } else {
            512
        };

        self.snap.nsid = 1;
        self.snap.lba_size = lba_size;
        self.snap.total_lbas = nsze;
        self.snap.sector_count_512 = nsze.saturating_mul((lba_size / 512) as u64);
        Ok(())
    }

    fn create_io_queues(&mut self) -> Result<(), KpiError> {
        let iocq_phys = self.io_q.cq_map.phys_addr();
        let iosq_phys = self.io_q.sq_map.phys_addr();

        // Create I/O Completion Queue #1 (PC=1, IEN=1)
        let mut cq_sqe = NvmeSqe::with_opcode(NVME_ADMIN_CREATE_CQ, 0, 0);
        cq_sqe.prp1 = iocq_phys;
        cq_sqe.cdw10 = (((IO_Q_DEPTH as u32 - 1) & 0xFFFF) << 16) | 1;
        cq_sqe.cdw11 = (1 << 1) | 1; // IEN | PC
        Self::submit_sync(
            &self.bar0,
            self.snap.dstrd_bytes,
            &mut self.admin_q,
            cq_sqe,
        )?;

        // Create I/O Submission Queue #1 mapped to CQID=1 (PC=1)
        let mut sq_sqe = NvmeSqe::with_opcode(NVME_ADMIN_CREATE_SQ, 0, 0);
        sq_sqe.prp1 = iosq_phys;
        sq_sqe.cdw10 = (((IO_Q_DEPTH as u32 - 1) & 0xFFFF) << 16) | 1;
        sq_sqe.cdw11 = (1 << 16) | 1; // CQID=1 | PC
        Self::submit_sync(
            &self.bar0,
            self.snap.dstrd_bytes,
            &mut self.admin_q,
            sq_sqe,
        )?;

        Ok(())
    }

    fn setup_prp_for_pages(&self, pages_needed: usize) -> (u64, u64) {
        let prp1 = self.io_data_map.phys_addrs[0];
        if pages_needed <= 1 {
            (prp1, 0)
        } else if pages_needed == 2 {
            (prp1, self.io_data_map.phys_addrs[1])
        } else {
            let list_virt = self.prp_list_map.virt_addr() as *mut u64;
            for i in 1..pages_needed {
                unsafe {
                    write_volatile(list_virt.add(i - 1), self.io_data_map.phys_addrs[i]);
                }
            }
            bus_dmamap_sync(BUS_DMASYNC_PREWRITE);
            (prp1, self.prp_list_map.phys_addr())
        }
    }

    fn rw_native_lbas(
        &mut self,
        is_write: bool,
        start_lba: u64,
        lba_count: usize,
    ) -> Result<(), KpiError> {
        let lba_size = self.snap.lba_size as usize;
        let total_bytes = lba_count * lba_size;
        let pages_needed = ((total_bytes + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize).max(1);
        if pages_needed > self.io_data_map.page_count() {
            return Err(KpiError::InvalidArg);
        }

        let (prp1, prp2) = self.setup_prp_for_pages(pages_needed);
        let opcode = if is_write {
            NVME_CMD_WRITE
        } else {
            NVME_CMD_READ
        };
        let mut sqe = NvmeSqe::with_opcode(opcode, 0, self.snap.nsid);
        sqe.prp1 = prp1;
        sqe.prp2 = prp2;
        sqe.cdw10 = start_lba as u32;
        sqe.cdw11 = (start_lba >> 32) as u32;
        sqe.cdw12 = (lba_count - 1) as u32; // 0-based NLB

        Self::submit_sync(&self.bar0, self.snap.dstrd_bytes, &mut self.io_q, sqe)?;
        Ok(())
    }

    fn copy_out_io_buf(&self, dst: &mut [u8], byte_offset: usize, len: usize) {
        let mut rem = len;
        let mut src_pos = byte_offset;
        let mut dst_pos = 0usize;
        let pg = PAGE_SIZE as usize;
        while rem > 0 {
            let page_idx = src_pos / pg;
            let page_off = src_pos % pg;
            let chunk = rem.min(pg - page_off);
            let src_ptr = (self.io_data_map.virt_addrs[page_idx] + page_off as u64) as *const u8;
            unsafe {
                copy_nonoverlapping(src_ptr, dst.as_mut_ptr().add(dst_pos), chunk);
            }
            rem -= chunk;
            src_pos += chunk;
            dst_pos += chunk;
        }
    }

    fn copy_into_io_buf(&self, src: &[u8], byte_offset: usize, len: usize) {
        let mut rem = len;
        let mut dst_pos = byte_offset;
        let mut src_pos = 0usize;
        let pg = PAGE_SIZE as usize;
        while rem > 0 {
            let page_idx = dst_pos / pg;
            let page_off = dst_pos % pg;
            let chunk = rem.min(pg - page_off);
            let dst_ptr = (self.io_data_map.virt_addrs[page_idx] + page_off as u64) as *mut u8;
            unsafe {
                copy_nonoverlapping(src.as_ptr().add(src_pos), dst_ptr, chunk);
            }
            rem -= chunk;
            dst_pos += chunk;
            src_pos += chunk;
        }
    }

    fn rw_sectors_512(
        &mut self,
        is_write: bool,
        start_sector: u64,
        sector_count: usize,
        buf: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        if !self.snap.initialized || self.snap.sector_count_512 == 0 {
            return Err(BlockDeviceError::DeviceNotReady);
        }
        if sector_count == 0 {
            return Ok(());
        }
        if start_sector.saturating_add(sector_count as u64) > self.snap.sector_count_512 {
            return Err(BlockDeviceError::SectorOutOfRange);
        }
        let needed_bytes = sector_count * (DEFAULT_SECTOR_SIZE as usize);
        if buf.len() < needed_bytes {
            return Err(BlockDeviceError::InvalidBufferSize);
        }

        let lba_size = self.snap.lba_size.max(512) as usize;
        if lba_size == 512 {
            let max_chunk_sectors = self.io_data_map.page_count() * 8;
            let mut done = 0usize;
            while done < sector_count {
                let batch = (sector_count - done).min(max_chunk_sectors);
                let byte_len = batch * 512;
                let buf_off = done * 512;
                if is_write {
                    self.copy_into_io_buf(&buf[buf_off..buf_off + byte_len], 0, byte_len);
                }
                self.rw_native_lbas(is_write, start_sector + done as u64, batch)
                    .map_err(|_| BlockDeviceError::IoError)?;
                if !is_write {
                    self.copy_out_io_buf(&mut buf[buf_off..buf_off + byte_len], 0, byte_len);
                }
                if is_write {
                    self.snap.writes += batch as u64;
                } else {
                    self.snap.reads += batch as u64;
                }
                self.snap.dma_batches += 1;
                done += batch;
            }
            return Ok(());
        }

        // Support larger native LBA formats (e.g. 4096 B NVMe namespaces) transparently.
        let sectors_per_lba = (lba_size / 512).max(1) as u64;
        for i in 0..sector_count {
            let sec = start_sector + i as u64;
            let native_lba = sec / sectors_per_lba;
            let sub_off = ((sec % sectors_per_lba) as usize) * 512;
            self.rw_native_lbas(false, native_lba, 1)
                .map_err(|_| BlockDeviceError::IoError)?;
            if is_write {
                self.copy_into_io_buf(&buf[i * 512..(i + 1) * 512], sub_off, 512);
                self.rw_native_lbas(true, native_lba, 1)
                    .map_err(|_| BlockDeviceError::IoError)?;
                self.snap.writes += 1;
            } else {
                self.copy_out_io_buf(&mut buf[i * 512..(i + 1) * 512], sub_off, 512);
                self.snap.reads += 1;
            }
            self.snap.dma_batches += 1;
        }
        Ok(())
    }

    fn flush_cmd(&mut self) -> Result<(), BlockDeviceError> {
        if !self.snap.initialized {
            return Err(BlockDeviceError::DeviceNotReady);
        }
        let sqe = NvmeSqe::with_opcode(NVME_CMD_FLUSH, 0, self.snap.nsid);
        Self::submit_sync(&self.bar0, self.snap.dstrd_bytes, &mut self.io_q, sqe)
            .map_err(|_| BlockDeviceError::IoError)?;
        Ok(())
    }
}

static NVME: Mtx<NvmeSoftc> = Mtx::new("nvme0", NvmeSoftc::empty());
static NVME_IOCQ_VIRT_FAST: AtomicU64 = AtomicU64::new(0);
static NVME_IOCQ_HEAD_FAST: AtomicU16 = AtomicU16::new(0);
static NVME_IOCQ_PHASE_FAST: AtomicU16 = AtomicU16::new(1);
static NVME_IO_ACTIVE: AtomicBool = AtomicBool::new(false);
static NVME_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

fn trim_ascii(bytes: &[u8]) -> &[u8] {
    let mut end = bytes.len();
    while end > 0 && (bytes[end - 1] == 0 || bytes[end - 1] == b' ') {
        end -= 1;
    }
    &bytes[..end]
}

fn nvme_irq_handler(_arg: usize) -> bool {
    let cq_virt = NVME_IOCQ_VIRT_FAST.load(Ordering::Acquire);
    if cq_virt == 0 {
        return false;
    }
    let head = NVME_IOCQ_HEAD_FAST.load(Ordering::Acquire) as usize;
    let phase = NVME_IOCQ_PHASE_FAST.load(Ordering::Acquire);
    let ptr = (cq_virt + (head * core::mem::size_of::<NvmeCqe>()) as u64) as *const NvmeCqe;
    let cqe = unsafe { read_volatile(ptr) };
    if (cqe.status & 1) == phase || NVME_IO_ACTIVE.load(Ordering::Acquire) {
        NVME_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::drivers::iokit::signal_workloop(crate::drivers::iokit::WorkLoopKind::Storage);
        true
    } else {
        false
    }
}

pub fn irq_count() -> u64 {
    NVME_IRQ_COUNT.load(Ordering::Relaxed)
}

fn nvme_probe(dev: &DeviceT) -> i32 {
    if dev.class_code == 0x01 && dev.subclass == 0x08 && dev.progif == 0x02 {
        BUS_PROBE_SPECIFIC
    } else {
        ENXIO
    }
}

fn nvme_attach(
    dev_id: DeviceId,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i32 {
    let Some(map) = mapping else {
        return 12; // ENOMEM
    };
    let Some(dev_copy) = crate::freebsd_kpi::with_device(dev_id, |d| *d) else {
        return ENXIO;
    };

    let irq_line = pci_enable_busmaster(&dev_copy, false, true);
    let Ok(bar0_res) = bus_alloc_resource_any(&dev_copy, SYS_RES_MEMORY, 0, Some(map), allocator)
    else {
        return 5; // EIO
    };
    let Some(bar0) = bar0_res.bus_handle else {
        return 5;
    };
    let irq_res = bus_alloc_resource_any(&dev_copy, SYS_RES_IRQ, 0, Some(map), allocator)
        .ok()
        .and_then(|r| r.irq_line)
        .unwrap_or(irq_line);

    let dma_tag_q = bus_dma_tag_create(
        "nvme_q_tag",
        PAGE_SIZE,
        u64::MAX,
        u64::MAX,
        PAGE_SIZE as usize,
        1,
        PAGE_SIZE as usize,
    );
    let dma_tag_io = bus_dma_tag_create(
        "nvme_io_tag",
        PAGE_SIZE,
        u64::MAX,
        u64::MAX,
        NVME_MAX_DMA_PAGES * PAGE_SIZE as usize,
        NVME_MAX_DMA_PAGES,
        PAGE_SIZE as usize,
    );

    let alloc_flags = BUS_DMA_NOWAIT | BUS_DMA_ZERO;
    let Ok(asq_map) = bus_dmamem_alloc(&dma_tag_q, PAGE_SIZE as usize, alloc_flags, map, allocator)
    else {
        return 12;
    };
    let Ok(acq_map) = bus_dmamem_alloc(&dma_tag_q, PAGE_SIZE as usize, alloc_flags, map, allocator)
    else {
        return 12;
    };
    let Ok(iosq_map) = bus_dmamem_alloc(&dma_tag_q, PAGE_SIZE as usize, alloc_flags, map, allocator)
    else {
        return 12;
    };
    let Ok(iocq_map) = bus_dmamem_alloc(&dma_tag_q, PAGE_SIZE as usize, alloc_flags, map, allocator)
    else {
        return 12;
    };
    let Ok(ident_map) =
        bus_dmamem_alloc(&dma_tag_q, PAGE_SIZE as usize, alloc_flags, map, allocator)
    else {
        return 12;
    };
    let Ok(prp_list_map) =
        bus_dmamem_alloc(&dma_tag_q, PAGE_SIZE as usize, alloc_flags, map, allocator)
    else {
        return 12;
    };
    let Ok(io_data_map) = bus_dmamem_alloc(
        &dma_tag_io,
        NVME_MAX_DMA_PAGES * PAGE_SIZE as usize,
        alloc_flags,
        map,
        allocator,
    ) else {
        return 12;
    };

    let cap = bus_space_read_8(&bar0, NVME_REG_CAP);
    let vs = bus_space_read_4(&bar0, NVME_REG_VS);
    let dstrd = ((cap >> 32) & 0xF) as usize;
    let dstrd_bytes = 4usize << dstrd.min(8);
    let mqes = ((cap & 0xFFFF) as u16).saturating_add(1);

    let mut sc = NVME.lock();
    sc.snap = NvmeSnapshot::EMPTY;
    sc.snap.present = true;
    sc.snap.bus = dev_copy.bus;
    sc.snap.device = dev_copy.device;
    sc.snap.function = dev_copy.function;
    sc.snap.vendor_id = dev_copy.vendor_id;
    sc.snap.device_id = dev_copy.device_id;
    sc.snap.bar0_phys = bar0.phys_or_port;
    sc.snap.bar0_virt = bar0.virt_base;
    sc.snap.cap = cap;
    sc.snap.version = vs;
    sc.snap.dstrd_bytes = dstrd_bytes;
    sc.snap.max_queue_entries = mqes;
    sc.snap.irq_line = irq_res;
    sc.newbus_id = Some(dev_id);
    sc.bar0 = bar0;
    sc.dma_tag_q = dma_tag_q;
    sc.dma_tag_io = dma_tag_io;
    sc.admin_q = NvmeQueueState::empty(0, ADMIN_Q_DEPTH);
    sc.admin_q.sq_map = asq_map;
    sc.admin_q.cq_map = acq_map;
    sc.io_q = NvmeQueueState::empty(1, IO_Q_DEPTH);
    sc.io_q.sq_map = iosq_map;
    sc.io_q.cq_map = iocq_map;
    sc.ident_map = ident_map;
    sc.prp_list_map = prp_list_map;
    sc.io_data_map = io_data_map;
    sc.snap.max_dma_sectors = sc.io_data_map.page_count() * 8;

    // 1. Reset controller (CC.EN = 0 -> wait CSTS.RDY == 0)
    let cur_cc = bus_space_read_4(&sc.bar0, NVME_REG_CC);
    if (cur_cc & CC_EN) != 0 {
        bus_space_write_4(&sc.bar0, NVME_REG_CC, cur_cc & !CC_EN);
    }
    if sc.wait_ready(false).is_err() {
        return 5;
    }

    // 2. Configure Admin Queue Attributes & Base Addresses
    let aqa = (((ADMIN_Q_DEPTH as u32 - 1) & 0x0FFF) << 16) | ((ADMIN_Q_DEPTH as u32 - 1) & 0x0FFF);
    bus_space_write_4(&sc.bar0, NVME_REG_AQA, aqa);
    bus_space_write_8(&sc.bar0, NVME_REG_ASQ, sc.admin_q.sq_map.phys_addr());
    bus_space_write_8(&sc.bar0, NVME_REG_ACQ, sc.admin_q.cq_map.phys_addr());
    bus_space_write_4(&sc.bar0, NVME_REG_INTMS, 0);
    bus_space_write_4(&sc.bar0, NVME_REG_INTMC, 0xFFFF_FFFF);
    bus_space_barrier();

    // 3. Enable Controller (CSS=0, MPS=0 (4K), AMS=0, IOSQES=6, IOCQES=4, EN=1)
    let cc = CC_EN | CC_IOSQES_64B | CC_IOCQES_16B;
    bus_space_write_4(&sc.bar0, NVME_REG_CC, cc);
    if sc.wait_ready(true).is_err() {
        return 5;
    }

    NVME_IOCQ_VIRT_FAST.store(sc.io_q.cq_map.virt_addr(), Ordering::Release);
    NVME_IOCQ_HEAD_FAST.store(0, Ordering::Release);
    NVME_IOCQ_PHASE_FAST.store(1, Ordering::Release);

    // 4. Admin Identify + I/O Queue Creation
    if sc.identify().is_err() || sc.create_io_queues().is_err() {
        return 5;
    }

    // 5. Attach interrupt handler via FreeBSD KPI `bus_setup_intr`
    let intr_route = bus_setup_intr(irq_res, "nvme0", nvme_irq_handler, dev_id).ok();
    if let Some((gsi, vec)) = intr_route {
        sc.snap.irq_gsi = Some(gsi);
        sc.snap.irq_vector = Some(vec);
    }

    sc.snap.initialized = true;

    let bar0_v = sc.snap.bar0_virt;
    let bar0_p = sc.snap.bar0_phys;
    let model = alloc::string::String::from(sc.snap.model_str());
    let secs = sc.snap.sector_count_512;
    drop(sc);

    with_device_mut(dev_id, |d| {
        d.bar0_phys = bar0_p;
        d.bar0_virt = bar0_v;
        d.irq_line = irq_res;
        d.irq_gsi = intr_route.map(|r| r.0);
        d.irq_vector = intr_route.map(|r| r.1);
        d.set_desc(&alloc::format!("{} ({} sect)", model, secs));
    });

    0
}

fn nvme_detach(_dev_id: DeviceId) -> i32 {
    let mut sc = NVME.lock();
    if sc.snap.initialized {
        let cc = bus_space_read_4(&sc.bar0, NVME_REG_CC);
        bus_space_write_4(&sc.bar0, NVME_REG_CC, cc & !CC_EN);
        sc.snap.initialized = false;
    }
    0
}

pub static NVME_DRIVER: DriverT = DriverT {
    name: "nvme",
    probe: nvme_probe,
    attach: nvme_attach,
    detach: nvme_detach,
};

pub fn init(
    desc: &PciNvmeDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<NvmeSnapshot, &'static str> {
    if !desc.valid || desc.bar0_phys == 0 {
        return Err("No NVMe PCI controller");
    }
    let Some(pci_dev) = pci::find_pci_device(desc.bus, desc.device, desc.function) else {
        return Err("NVMe PCI device missing from inventory");
    };

    let dev_id = device_add_pci_child(&pci_dev, "nvme");
    device_probe_and_attach(dev_id, &NVME_DRIVER, mapping, allocator)
        .map_err(|e| e.as_str())?;

    Ok(snapshot())
}

pub fn snapshot() -> NvmeSnapshot {
    NVME.lock().snap
}

pub fn read_sector_dma(lba: u64, buffer: &mut [u8; 512]) -> Result<(), BlockDeviceError> {
    NVME.lock().rw_sectors_512(false, lba, 1, buffer)
}

pub struct NvmeBlockDevice;

impl BlockDevice for NvmeBlockDevice {
    fn sector_size(&self) -> u32 {
        DEFAULT_SECTOR_SIZE
    }

    fn sector_count(&self) -> u64 {
        NVME.lock().snap.sector_count_512
    }

    fn max_transfer_sectors(&self) -> usize {
        NVME.lock().snap.max_dma_sectors.max(1)
    }

    fn read_sector(&mut self, lba: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
        NVME.lock().rw_sectors_512(false, lba, 1, buffer)
    }

    fn write_sector(&mut self, lba: u64, buffer: &[u8]) -> Result<(), BlockDeviceError> {
        let mut copy_buf = [0u8; 512];
        if buffer.len() < 512 {
            return Err(BlockDeviceError::InvalidBufferSize);
        }
        copy_buf.copy_from_slice(&buffer[..512]);
        NVME.lock().rw_sectors_512(true, lba, 1, &mut copy_buf)
    }

    fn read_sectors(
        &mut self,
        start_lba: u64,
        sector_count: usize,
        buffer: &mut [u8],
    ) -> Result<(), BlockDeviceError> {
        NVME.lock()
            .rw_sectors_512(false, start_lba, sector_count, buffer)
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
        let mut scratch = alloc::vec::Vec::with_capacity(needed);
        scratch.extend_from_slice(&buffer[..needed]);
        NVME.lock()
            .rw_sectors_512(true, start_lba, sector_count, &mut scratch)
    }

    fn flush(&mut self) -> Result<(), BlockDeviceError> {
        NVME.lock().flush_cmd()
    }
}
