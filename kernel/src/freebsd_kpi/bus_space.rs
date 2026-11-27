use core::arch::asm;
use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::memory::vmm::{mark_shared_kernel_entry, PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;

use super::KpiError;

/// Separate non-overlapping MMIO virtual range for drivers using FreeBSD `bus_space`.
pub const KPI_MMIO_VIRT_BASE: u64 = 0x0000_5500_0000_0000;
const KPI_MMIO_VIRT_LIMIT: u64 = 0x0000_5520_0000_0000;

static NEXT_MMIO_VIRT: AtomicU64 = AtomicU64::new(KPI_MMIO_VIRT_BASE);

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum BusSpaceTag {
    Memory,
    Io,
}

#[derive(Clone, Copy, Debug)]
pub struct BusSpaceHandle {
    pub tag: BusSpaceTag,
    pub phys_or_port: u64,
    pub virt_base: u64,
    pub size: u64,
}

impl BusSpaceHandle {
    pub const EMPTY: Self = Self {
        tag: BusSpaceTag::Memory,
        phys_or_port: 0,
        virt_base: 0,
        size: 0,
    };

    #[inline]
    pub fn from_io_port(port_base: u16, size: u64) -> Self {
        Self {
            tag: BusSpaceTag::Io,
            phys_or_port: port_base as u64,
            virt_base: port_base as u64,
            size,
        }
    }
}

/// Maps a physical MMIO region into a fresh kernel virtual range with uncacheable flags.
pub fn map_mmio(
    phys_addr: u64,
    size: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<BusSpaceHandle, KpiError> {
    if phys_addr == 0 {
        return Err(KpiError::InvalidArg);
    }
    let page_off = phys_addr & (PAGE_SIZE - 1);
    let phys_aligned = phys_addr & !(PAGE_SIZE - 1);
    let total_bytes = size.max(PAGE_SIZE) + page_off;
    let pages = ((total_bytes + PAGE_SIZE - 1) / PAGE_SIZE).max(1);
    let aligned_bytes = pages * PAGE_SIZE;

    let virt_aligned = NEXT_MMIO_VIRT.fetch_add(aligned_bytes + PAGE_SIZE, Ordering::AcqRel);
    if virt_aligned + aligned_bytes >= KPI_MMIO_VIRT_LIMIT {
        return Err(KpiError::NoMem);
    }

    let pml4_idx = ((virt_aligned >> 39) & 0x1FF) as usize;
    mark_shared_kernel_entry(pml4_idx);

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::NO_CACHE
        | PageTableFlags::WRITE_THROUGH;

    for i in 0..pages {
        let virt = VirtAddr::new(virt_aligned + i * PAGE_SIZE);
        let phys = phys_aligned + i * PAGE_SIZE;
        if mapping.translate_virtual_address(virt).is_none() {
            mapping
                .map_page(allocator, virt, phys, flags)
                .map_err(|_| KpiError::Io)?;
        }
    }

    Ok(BusSpaceHandle {
        tag: BusSpaceTag::Memory,
        phys_or_port: phys_addr,
        virt_base: virt_aligned + page_off,
        size,
    })
}

#[inline]
pub fn bus_space_read_1(handle: &BusSpaceHandle, offset: usize) -> u8 {
    match handle.tag {
        BusSpaceTag::Memory => unsafe { read_volatile((handle.virt_base + offset as u64) as *const u8) },
        BusSpaceTag::Io => {
            let port = (handle.phys_or_port + offset as u64) as u16;
            let val: u8;
            unsafe {
                asm!("in al, dx", out("al") val, in("dx") port, options(nomem, nostack, preserves_flags));
            }
            val
        }
    }
}

#[inline]
pub fn bus_space_read_2(handle: &BusSpaceHandle, offset: usize) -> u16 {
    match handle.tag {
        BusSpaceTag::Memory => unsafe { read_volatile((handle.virt_base + offset as u64) as *const u16) },
        BusSpaceTag::Io => {
            let port = (handle.phys_or_port + offset as u64) as u16;
            let val: u16;
            unsafe {
                asm!("in ax, dx", out("ax") val, in("dx") port, options(nomem, nostack, preserves_flags));
            }
            val
        }
    }
}

#[inline]
pub fn bus_space_read_4(handle: &BusSpaceHandle, offset: usize) -> u32 {
    match handle.tag {
        BusSpaceTag::Memory => unsafe { read_volatile((handle.virt_base + offset as u64) as *const u32) },
        BusSpaceTag::Io => {
            let port = (handle.phys_or_port + offset as u64) as u16;
            let val: u32;
            unsafe {
                asm!("in eax, dx", out("eax") val, in("dx") port, options(nomem, nostack, preserves_flags));
            }
            val
        }
    }
}

#[inline]
pub fn bus_space_read_8(handle: &BusSpaceHandle, offset: usize) -> u64 {
    match handle.tag {
        BusSpaceTag::Memory => unsafe { read_volatile((handle.virt_base + offset as u64) as *const u64) },
        BusSpaceTag::Io => {
            let lo = bus_space_read_4(handle, offset) as u64;
            let hi = bus_space_read_4(handle, offset + 4) as u64;
            lo | (hi << 32)
        }
    }
}

#[inline]
pub fn bus_space_write_1(handle: &BusSpaceHandle, offset: usize, value: u8) {
    match handle.tag {
        BusSpaceTag::Memory => unsafe {
            write_volatile((handle.virt_base + offset as u64) as *mut u8, value);
        },
        BusSpaceTag::Io => {
            let port = (handle.phys_or_port + offset as u64) as u16;
            unsafe {
                asm!("out dx, al", in("dx") port, in("al") value, options(nomem, nostack, preserves_flags));
            }
        }
    }
}

#[inline]
pub fn bus_space_write_2(handle: &BusSpaceHandle, offset: usize, value: u16) {
    match handle.tag {
        BusSpaceTag::Memory => unsafe {
            write_volatile((handle.virt_base + offset as u64) as *mut u16, value);
        },
        BusSpaceTag::Io => {
            let port = (handle.phys_or_port + offset as u64) as u16;
            unsafe {
                asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
            }
        }
    }
}

#[inline]
pub fn bus_space_write_4(handle: &BusSpaceHandle, offset: usize, value: u32) {
    match handle.tag {
        BusSpaceTag::Memory => unsafe {
            write_volatile((handle.virt_base + offset as u64) as *mut u32, value);
        },
        BusSpaceTag::Io => {
            let port = (handle.phys_or_port + offset as u64) as u16;
            unsafe {
                asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
            }
        }
    }
}

#[inline]
pub fn bus_space_write_8(handle: &BusSpaceHandle, offset: usize, value: u64) {
    match handle.tag {
        BusSpaceTag::Memory => unsafe {
            write_volatile((handle.virt_base + offset as u64) as *mut u64, value);
        },
        BusSpaceTag::Io => {
            bus_space_write_4(handle, offset, value as u32);
            bus_space_write_4(handle, offset + 4, (value >> 32) as u32);
        }
    }
}

#[inline]
pub fn bus_space_barrier() {
    core::sync::atomic::fence(Ordering::SeqCst);
}
