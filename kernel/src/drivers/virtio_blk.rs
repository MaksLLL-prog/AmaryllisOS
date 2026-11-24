use alloc::vec::Vec;
use core::arch::asm;
use core::ptr::{copy_nonoverlapping, write_bytes};
use core::sync::atomic::{fence, AtomicU16, AtomicU64, Ordering};

use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::task::preempt::PreemptMutex;

use crate::hardware::PciVirtioBlkDescriptor;
use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::storage::block::{BlockDevice, BlockDeviceError, DEFAULT_SECTOR_SIZE};

const VIRTIO_BLK_MMIO_VIRT_BASE: u64 = 0x0000_5280_0000_0000;

// Legacy VirtIO-PCI I/O Port Offsets (BAR0)
const REG_DEV_FEATURES: u16 = 0x00;
const REG_DRV_FEATURES: u16 = 0x04;
const REG_QUEUE_PFN: u16 = 0x08;
const REG_QUEUE_SIZE: u16 = 0x0C;
const REG_QUEUE_SELECT: u16 = 0x0E;
const REG_QUEUE_NOTIFY: u16 = 0x10;
const REG_DEV_STATUS: u16 = 0x12;
const REG_ISR_STATUS: u16 = 0x13;
const REG_BLK_CAP_LO: u16 = 0x14;
const REG_BLK_CAP_HI: u16 = 0x18;

const STATUS_ACKNOWLEDGE: u8 = 1;
const STATUS_DRIVER: u8 = 2;
const STATUS_DRIVER_OK: u8 = 4;

const VRING_DESC_F_NEXT: u16 = 1;
const VRING_DESC_F_WRITE: u16 = 2;

const VIRTIO_BLK_T_IN: u32 = 0;
const VIRTIO_BLK_T_OUT: u32 = 1;
const VIRTIO_BLK_T_FLUSH: u32 = 4;

const VIRTIO_BLK_F_FLUSH: u32 = 1 << 9;

#[repr(C)]
#[derive(Clone, Copy)]
struct VringDesc {
    addr: u64,
    len: u32,
    flags: u16,
    next: u16,
}

#[repr(C)]
#[derive(Clone, Copy)]
struct VirtioBlkReqHdr {
    req_type: u32,
    reserved: u32,
    sector: u64,
}

#[derive(Clone, Copy)]
pub struct VirtioBlkSnapshot {
    pub present: bool,
    pub initialized: bool,
    pub is_modern: bool,
    pub io_base: u16,
    pub mmio_phys: u64,
    pub queue_size: u16,
    pub sector_count: u64,
    pub features: u32,
    pub reads: u64,
    pub writes: u64,
    pub max_dma_sectors: usize,
    pub dma_batches: u64,
}

impl VirtioBlkSnapshot {
    pub const EMPTY: Self = Self {
        present: false,
        initialized: false,
        is_modern: false,
        io_base: 0,
        mmio_phys: 0,
        queue_size: 0,
        sector_count: 0,
        features: 0,
        reads: 0,
        writes: 0,
        max_dma_sectors: 1,
        dma_batches: 0,
    };
}

struct VirtioBlkState {
    snap: VirtioBlkSnapshot,
    ring_virt: u64,
    used_off: usize,
    avail_off: usize,
    last_used_idx: u16,
    cmd_phys: u64,
    cmd_virt: u64,
    data_phys: u64,
    data_virt: u64,
    data_bytes: usize,
    negotiated_features: u32,
}

impl VirtioBlkState {
    const fn empty() -> Self {
        Self {
            snap: VirtioBlkSnapshot::EMPTY,
            ring_virt: 0,
            used_off: 0,
            avail_off: 0,
            last_used_idx: 0,
            cmd_phys: 0,
            cmd_virt: 0,
            data_phys: 0,
            data_virt: 0,
            data_bytes: 0,
            negotiated_features: 0,
        }
    }
}

static VIRTIO_BLK: PreemptMutex<VirtioBlkState> = PreemptMutex::new(VirtioBlkState::empty());
static VIRTIO_IO_BASE_FAST: AtomicU16 = AtomicU16::new(0);
static VIRTIO_IRQ_COUNT: AtomicU64 = AtomicU64::new(0);

/// Lock-free Top-Half IRQ acknowledgment for Legacy/Transitional VirtIO-Blk PCI.
pub fn irq_top_half() -> bool {
    let io = VIRTIO_IO_BASE_FAST.load(Ordering::Acquire);
    if io == 0 {
        return false;
    }
    let isr = unsafe { inb(io + REG_ISR_STATUS) };
    if (isr & 1) != 0 {
        VIRTIO_IRQ_COUNT.fetch_add(1, Ordering::Relaxed);
        crate::drivers::iokit::signal_workloop(crate::drivers::iokit::WorkLoopKind::Storage);
        true
    } else {
        false
    }
}

pub fn irq_count() -> u64 {
    VIRTIO_IRQ_COUNT.load(Ordering::Relaxed)
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

fn alloc_contiguous_frames(
    allocator: &mut PhysicalFrameAllocator<'_>,
    needed: usize,
) -> Option<u64> {
    if needed == 0 {
        return None;
    }
    let mut pool: Vec<u64> = Vec::new();
    for _ in 0..128 {
        let Some(f) = allocator.allocate_frame() else {
            break;
        };
        pool.push(f);
        if pool.len() >= needed {
            pool.sort_unstable();
            for win in pool.windows(needed) {
                let base = win[0];
                let contiguous = (1..needed).all(|i| win[i] == base + (i as u64) * PAGE_SIZE);
                if contiguous {
                    for &p in &pool {
                        if p < base || p >= base + (needed as u64) * PAGE_SIZE {
                            let _ = allocator.deallocate_frame(p);
                        }
                    }
                    return Some(base);
                }
            }
        }
    }
    for p in pool {
        let _ = allocator.deallocate_frame(p);
    }
    None
}

pub fn init(
    desc: &PciVirtioBlkDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<VirtioBlkSnapshot, &'static str> {
    if !desc.valid {
        return Err("No VirtIO-Blk PCI device");
    }
    let Some(mapping) = mapping else {
        return Err("PhysicalMemoryMapping unavailable");
    };

    if desc.mmio_base != 0 {
        let map_len = desc.mmio_size.max(0x4000).min(64 * 1024);
        let pages = ((map_len + PAGE_SIZE - 1) / PAGE_SIZE) as usize;
        let phys_base = desc.mmio_base & !(PAGE_SIZE - 1);
        let flags = PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::NO_CACHE
            | PageTableFlags::WRITE_THROUGH;
        for i in 0..pages {
            let virt = VirtAddr::new(VIRTIO_BLK_MMIO_VIRT_BASE + (i as u64) * PAGE_SIZE);
            let phys = phys_base + (i as u64) * PAGE_SIZE;
            if mapping.translate_virtual_address(virt).is_none() {
                let _ = mapping.map_page(allocator, virt, phys, flags);
            }
        }
    }

    let mut st = VIRTIO_BLK.lock();
    st.snap = VirtioBlkSnapshot::EMPTY;
    st.snap.present = true;
    st.snap.is_modern = desc.is_modern && desc.io_base == 0;
    st.snap.io_base = desc.io_base;
    st.snap.mmio_phys = desc.mmio_base;

    if desc.io_base == 0 {
        // Modern-only VirtIO without Legacy I/O BAR0: detected cleanly in registry, I/O queue requires legacy/transitional mode
        return Err("VirtIO-Blk Modern-only BAR0 absent (enable transitional mode)");
    }

    let io = desc.io_base;
    unsafe {
        // 1. Reset device
        outb(io + REG_DEV_STATUS, 0);
        // 2. Acknowledge + Driver
        outb(io + REG_DEV_STATUS, STATUS_ACKNOWLEDGE | STATUS_DRIVER);
        // 3. Negotiate flush feature when offered
        let dev_features = inl(io + REG_DEV_FEATURES);
        st.snap.features = dev_features;
        let drv_features = dev_features & VIRTIO_BLK_F_FLUSH;
        outl(io + REG_DRV_FEATURES, drv_features);
        st.negotiated_features = drv_features;

        // 4. Select queue 0
        outw(io + REG_QUEUE_SELECT, 0);
        let qsz = inw(io + REG_QUEUE_SIZE);
        if qsz < 4 || qsz > 512 {
            outb(io + REG_DEV_STATUS, 0);
            return Err("VirtIO-Blk invalid queue size");
        }
        st.snap.queue_size = qsz;

        let desc_bytes = (qsz as usize) * 16;
        let avail_bytes = 6 + (qsz as usize) * 2;
        let used_off = (desc_bytes + avail_bytes + (PAGE_SIZE as usize - 1)) & !(PAGE_SIZE as usize - 1);
        let used_bytes = 6 + (qsz as usize) * 8;
        let total_bytes = used_off + used_bytes;
        let ring_pages = (total_bytes + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;

        let Some(ring_phys) = alloc_contiguous_frames(allocator, ring_pages) else {
            return Err("Contiguous DMA frames exhausted for Virtqueue");
        };
        let Some(ring_virt) = mapping.translate_frame(ring_phys) else {
            return Err("HHDM translate fail ring");
        };
        write_bytes(ring_virt.as_u64() as *mut u8, 0, ring_pages * PAGE_SIZE as usize);

        let cmd_frame = allocator
            .allocate_frame()
            .ok_or("No frame for VirtIO-Blk cmd buffer")?;
        let cmd_virt = mapping
            .translate_frame(cmd_frame)
            .ok_or("HHDM fail cmd")?
            .as_u64();
        write_bytes(cmd_virt as *mut u8, 0, PAGE_SIZE as usize);

        // Allocate up to 64 KiB (16 contiguous 4 KiB pages = 128 sectors) for multi-block DMA.
        let mut data_phys = 0u64;
        let mut data_pages = 0usize;
        for candidate_pages in [16usize, 8, 4, 2, 1] {
            if let Some(p) = alloc_contiguous_frames(allocator, candidate_pages) {
                data_phys = p;
                data_pages = candidate_pages;
                break;
            }
        }
        let data_virt;
        let data_bytes;
        if data_pages > 0 {
            data_virt = mapping
                .translate_frame(data_phys)
                .ok_or("HHDM fail data DMA")?
                .as_u64();
            data_bytes = data_pages * PAGE_SIZE as usize;
            write_bytes(data_virt as *mut u8, 0, data_bytes);
        } else {
            // Fallback to cmd_frame tail if allocator cannot provide another frame
            data_phys = cmd_frame + 128;
            data_virt = cmd_virt + 128;
            data_bytes = 512;
        }

        outl(io + REG_QUEUE_PFN, (ring_phys / PAGE_SIZE) as u32);

        let cap_lo = inl(io + REG_BLK_CAP_LO) as u64;
        let cap_hi = inl(io + REG_BLK_CAP_HI) as u64;
        st.snap.sector_count = cap_lo | (cap_hi << 32);

        outb(
            io + REG_DEV_STATUS,
            STATUS_ACKNOWLEDGE | STATUS_DRIVER | STATUS_DRIVER_OK,
        );
        let _ = inb(io + REG_ISR_STATUS);

        st.ring_virt = ring_virt.as_u64();
        st.avail_off = desc_bytes;
        st.used_off = used_off;
        st.last_used_idx = 0;
        st.cmd_phys = cmd_frame;
        st.cmd_virt = cmd_virt;
        st.data_phys = data_phys;
        st.data_virt = data_virt;
        st.data_bytes = data_bytes;
        st.snap.max_dma_sectors = (data_bytes / DEFAULT_SECTOR_SIZE as usize).max(1);
        st.snap.initialized = true;
        VIRTIO_IO_BASE_FAST.store(io, Ordering::Release);
    }

    Ok(st.snap)
}

unsafe fn submit_and_wait(st: &mut VirtioBlkState) -> Result<(), BlockDeviceError> {
    unsafe {
        let avail_idx_ptr = (st.ring_virt + st.avail_off as u64 + 2) as *mut u16;
        let cur_avail = avail_idx_ptr.read_volatile();
        let slot = (cur_avail % st.snap.queue_size) as u64;
        let ring_entry = (st.ring_virt + st.avail_off as u64 + 4 + slot * 2) as *mut u16;
        ring_entry.write_volatile(0);

        fence(Ordering::SeqCst);
        avail_idx_ptr.write_volatile(cur_avail.wrapping_add(1));
        fence(Ordering::SeqCst);

        outw(st.snap.io_base + REG_QUEUE_NOTIFY, 0);

        let used_idx_ptr = (st.ring_virt + st.used_off as u64 + 2) as *const u16;
        let target = st.last_used_idx.wrapping_add(1);
        let mut spins = 0u32;
        while used_idx_ptr.read_volatile() != target {
            spins += 1;
            if spins > 4_000_000 {
                return Err(BlockDeviceError::Timeout);
            }
            core::hint::spin_loop();
        }
        let _ = inb(st.snap.io_base + REG_ISR_STATUS);
        st.last_used_idx = target;

        fence(Ordering::SeqCst);
        let status_virt = (st.cmd_virt + 64) as *const u8;
        let status = status_virt.read_volatile();
        if status != 0 {
            return Err(BlockDeviceError::IoError);
        }
    }
    Ok(())
}

fn do_io_sectors_chunk(
    st: &mut VirtioBlkState,
    lba: u64,
    sector_count: usize,
    buf_ptr: *mut u8,
    is_write: bool,
) -> Result<(), BlockDeviceError> {
    let total_bytes = sector_count * DEFAULT_SECTOR_SIZE as usize;
    if total_bytes == 0 || total_bytes > st.data_bytes {
        return Err(BlockDeviceError::InvalidBufferSize);
    }

    let hdr_ptr = st.cmd_virt as *mut VirtioBlkReqHdr;
    let status_virt = (st.cmd_virt + 64) as *mut u8;
    let status_phys = st.cmd_phys + 64;

    unsafe {
        *hdr_ptr = VirtioBlkReqHdr {
            req_type: if is_write { VIRTIO_BLK_T_OUT } else { VIRTIO_BLK_T_IN },
            reserved: 0,
            sector: lba,
        };
        if is_write {
            copy_nonoverlapping(buf_ptr as *const u8, st.data_virt as *mut u8, total_bytes);
        }
        *status_virt = 0xFF;

        let descs = st.ring_virt as *mut VringDesc;
        *descs.add(0) = VringDesc {
            addr: st.cmd_phys,
            len: core::mem::size_of::<VirtioBlkReqHdr>() as u32,
            flags: VRING_DESC_F_NEXT,
            next: 1,
        };
        *descs.add(1) = VringDesc {
            addr: st.data_phys,
            len: total_bytes as u32,
            flags: VRING_DESC_F_NEXT | if is_write { 0 } else { VRING_DESC_F_WRITE },
            next: 2,
        };
        *descs.add(2) = VringDesc {
            addr: status_phys,
            len: 1,
            flags: VRING_DESC_F_WRITE,
            next: 0,
        };

        submit_and_wait(st)?;

        if !is_write {
            copy_nonoverlapping(st.data_virt as *const u8, buf_ptr, total_bytes);
            st.snap.reads = st.snap.reads.saturating_add(sector_count as u64);
        } else {
            st.snap.writes = st.snap.writes.saturating_add(sector_count as u64);
        }
        st.snap.dma_batches = st.snap.dma_batches.saturating_add(1);
    }

    Ok(())
}

pub fn snapshot() -> VirtioBlkSnapshot {
    VIRTIO_BLK.lock().snap
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

    let mut st = VIRTIO_BLK.lock();
    if !st.snap.initialized || st.snap.io_base == 0 {
        return Err(BlockDeviceError::DeviceNotReady);
    }
    let end_lba = start_lba
        .checked_add(sector_count as u64)
        .ok_or(BlockDeviceError::SectorOutOfRange)?;
    if st.snap.sector_count > 0 && end_lba > st.snap.sector_count {
        return Err(BlockDeviceError::SectorOutOfRange);
    }

    let max_chunk = st.snap.max_dma_sectors.max(1);
    let mut done = 0usize;
    while done < sector_count {
        let chunk = (sector_count - done).min(max_chunk);
        let byte_off = done * DEFAULT_SECTOR_SIZE as usize;
        let ptr = unsafe { buffer.as_mut_ptr().add(byte_off) };
        do_io_sectors_chunk(&mut st, start_lba + done as u64, chunk, ptr, false)?;
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

    let mut st = VIRTIO_BLK.lock();
    if !st.snap.initialized || st.snap.io_base == 0 {
        return Err(BlockDeviceError::DeviceNotReady);
    }
    let end_lba = start_lba
        .checked_add(sector_count as u64)
        .ok_or(BlockDeviceError::SectorOutOfRange)?;
    if st.snap.sector_count > 0 && end_lba > st.snap.sector_count {
        return Err(BlockDeviceError::SectorOutOfRange);
    }

    let max_chunk = st.snap.max_dma_sectors.max(1);
    let mut done = 0usize;
    while done < sector_count {
        let chunk = (sector_count - done).min(max_chunk);
        let byte_off = done * DEFAULT_SECTOR_SIZE as usize;
        let ptr = unsafe { buffer.as_ptr().add(byte_off) as *mut u8 };
        do_io_sectors_chunk(&mut st, start_lba + done as u64, chunk, ptr, true)?;
        done += chunk;
    }
    Ok(())
}

pub fn flush_dma() -> Result<(), BlockDeviceError> {
    let mut st = VIRTIO_BLK.lock();
    if !st.snap.initialized || st.snap.io_base == 0 {
        return Err(BlockDeviceError::DeviceNotReady);
    }
    if (st.negotiated_features & VIRTIO_BLK_F_FLUSH) == 0 {
        return Ok(());
    }

    let hdr_ptr = st.cmd_virt as *mut VirtioBlkReqHdr;
    let status_virt = (st.cmd_virt + 64) as *mut u8;
    let status_phys = st.cmd_phys + 64;

    unsafe {
        *hdr_ptr = VirtioBlkReqHdr {
            req_type: VIRTIO_BLK_T_FLUSH,
            reserved: 0,
            sector: 0,
        };
        *status_virt = 0xFF;

        let descs = st.ring_virt as *mut VringDesc;
        *descs.add(0) = VringDesc {
            addr: st.cmd_phys,
            len: core::mem::size_of::<VirtioBlkReqHdr>() as u32,
            flags: VRING_DESC_F_NEXT,
            next: 1,
        };
        *descs.add(1) = VringDesc {
            addr: status_phys,
            len: 1,
            flags: VRING_DESC_F_WRITE,
            next: 0,
        };

        submit_and_wait(&mut st)?;
    }

    Ok(())
}

pub fn read_sector_dma(lba: u64, buffer: &mut [u8]) -> Result<(), BlockDeviceError> {
    read_sectors_dma(lba, 1, buffer)
}

pub fn write_sector_dma(lba: u64, buffer: &[u8]) -> Result<(), BlockDeviceError> {
    write_sectors_dma(lba, 1, buffer)
}

pub struct VirtioBlockDevice;

impl BlockDevice for VirtioBlockDevice {
    fn sector_size(&self) -> u32 {
        DEFAULT_SECTOR_SIZE
    }

    fn sector_count(&self) -> u64 {
        VIRTIO_BLK.lock().snap.sector_count
    }

    fn max_transfer_sectors(&self) -> usize {
        VIRTIO_BLK.lock().snap.max_dma_sectors.max(1)
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
