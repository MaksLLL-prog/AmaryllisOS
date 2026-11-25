
use core::arch::asm;
use core::ptr::{read_volatile, write_bytes, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};

use x86_64::{PhysAddr, VirtAddr};
use x86_64::registers::control::Cr3;
use x86_64::structures::paging::PageTableFlags;
use x86_64::structures::paging::page_table::PageTableEntry;

use super::PhysicalFrameAllocator;

pub const PAGE_SIZE: u64 = 4096;
pub const COW_PAGE_FLAG: PageTableFlags = PageTableFlags::BIT_9;

pub const PAGE_TABLE_ENTRIES: usize = 512;

const ADDRESS_MASK_4K: u64 = 0x000F_FFFF_FFFF_F000;
const ADDRESS_MASK_2M: u64 = 0x000F_FFFF_FFE0_0000;
const ADDRESS_MASK_1G: u64 = 0x000F_FFFF_C000_0000;

const TABLE_FLAGS: PageTableFlags =
    PageTableFlags::PRESENT.union(PageTableFlags::WRITABLE);

// Upper half always belongs to the shared kernel space.
pub const KERNEL_PML4_START: usize = 256;

// 512-bit atomic mask of PML4 indices shared across all address spaces.
// Index 0 (0x0000_0000_0000_0000..0x0000_007F_FFFF_FFFF) is strictly reserved
// for private Ring 3 user space; indices 256..512 are always shared; indices 1..256
// are populated dynamically from the bootloader / kernel active PML4.
static SHARED_KERNEL_PML4_MASK: [AtomicU64; 8] = [
    AtomicU64::new((1u64 << 2) | (1u64 << 3)),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(0),
    AtomicU64::new(u64::MAX),
    AtomicU64::new(u64::MAX),
    AtomicU64::new(u64::MAX),
    AtomicU64::new(u64::MAX),
];

pub fn is_shared_kernel_entry(index: usize) -> bool {
    if index >= PAGE_TABLE_ENTRIES {
        return false;
    }
    if index == 0 {
        return false;
    }
    if index >= KERNEL_PML4_START {
        return true;
    }
    let word = SHARED_KERNEL_PML4_MASK[index / 64].load(Ordering::Acquire);
    (word & (1u64 << (index % 64))) != 0
}

pub fn mark_shared_kernel_entry(index: usize) {
    if index == 0 || index >= PAGE_TABLE_ENTRIES {
        return;
    }
    SHARED_KERNEL_PML4_MASK[index / 64].fetch_or(1u64 << (index % 64), Ordering::AcqRel);
}

/// Synchronizes all active kernel PML4 entries in `1..256` (e.g. bootloader
/// `Mapping::Dynamic` regions for kernel image, stack, HHDM, framebuffer, and
/// kernel driver MMIO bars) so every newly created user `AddressSpace` shares them.
pub fn sync_shared_kernel_entries(mapping: &PhysicalMemoryMapping) {
    let Some(pml4_virt) = mapping.active_level_4_table() else {
        return;
    };
    let table_ptr = pml4_virt.as_u64() as *const PageTableEntry;
    unsafe {
        for index in 1..KERNEL_PML4_START {
            let entry = read_volatile(table_ptr.add(index));
            if entry.flags().contains(PageTableFlags::PRESENT) {
                mark_shared_kernel_entry(index);
            }
        }
    }
}


#[derive(Clone, Copy)]
struct TableChange {
    parent_table: u64,
    index: usize,
    frame: u64,
    created: bool,
    old_flags: PageTableFlags,
}

#[derive(Debug, Clone, Copy)]
pub struct PhysicalMemoryMapping {
    offset: VirtAddr,
}

#[derive(Debug, Clone, Copy, Default)]
pub struct Pml4Stats {
    pub total_entries: usize,
    pub occupied_entries: usize,
    pub present_entries: usize,
    pub writable_entries: usize,
    pub user_accessible_entries: usize,
}

#[derive(Debug, Clone, Copy)]
pub struct PageTranslation {
    pub physical_address: u64,
    pub page_size: u64,
    pub flags: PageTableFlags,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapError {
    InvalidVirtualAddress,
    InvalidPhysicalAddress,
    UnalignedAddress,
    InvalidFlags,
    AlreadyMapped,
    NotMapped,
    HugePageConflict,
    AllocationFailed,
    InvalidTableAddress,
}

impl PhysicalMemoryMapping {
    // --------------------------------------------------------
    // PHYSICAL MEMORY MAPPING
    // --------------------------------------------------------

    pub fn new(offset: u64) -> Option<Self> {
        let offset = VirtAddr::try_new(offset).ok()?;
        Some(Self { offset })
    }

    pub fn offset(&self) -> u64 {
        self.offset.as_u64()
    }

    pub fn translate(&self, physical_address: u64) -> Option<VirtAddr> {
        let virtual_address = self
            .offset
            .as_u64()
            .checked_add(physical_address)?;

        VirtAddr::try_new(virtual_address).ok()
    }

    pub fn translate_frame(&self, physical_address: u64) -> Option<VirtAddr> {
        if physical_address % PAGE_SIZE != 0 {
            return None;
        }

        self.translate(physical_address)
    }

    pub fn active_level_4_table(&self) -> Option<VirtAddr> {
        self.translate_frame(active_level_4_table_address())
    }

    // --------------------------------------------------------
    // PAGE TABLE ENTRY ACCESS
    // --------------------------------------------------------

    fn table_entry_pointer(
        &self,
        table_physical_address: u64,
        index: usize,
    ) -> Option<*mut PageTableEntry> {
        if index >= PAGE_TABLE_ENTRIES
            || table_physical_address % PAGE_SIZE != 0
        {
            return None;
        }

        let table_virtual_address =
            self.translate_frame(table_physical_address)?;

        let entries =
            table_virtual_address.as_u64() as *mut PageTableEntry;

        Some(unsafe { entries.add(index) })
    }

    fn read_table_entry(
        &self,
        table_physical_address: u64,
        index: usize,
    ) -> Option<PageTableEntry> {
        let pointer =
            self.table_entry_pointer(table_physical_address, index)?;

        unsafe {
            Some(read_volatile(pointer))
        }
    }

    fn write_table_entry(
        &self,
        table_physical_address: u64,
        index: usize,
        entry: PageTableEntry,
    ) -> Option<()> {
        let pointer =
            self.table_entry_pointer(table_physical_address, index)?;

        unsafe {
            write_volatile(pointer, entry);
        }

        Some(())
    }

    fn validate_pml4(&self, pml4: u64) -> Result<(), MapError> {
        if pml4 % PAGE_SIZE != 0 {
            return Err(MapError::UnalignedAddress);
        }

        if self.translate_frame(pml4).is_none() {
            return Err(MapError::InvalidTableAddress);
        }

        Ok(())
    }

    // --------------------------------------------------------
    // PAGE TABLE INSPECTION
    // --------------------------------------------------------

    fn table_is_empty(
        &self,
        table_physical_address: u64,
    ) -> Option<bool> {
        let table_virtual_address =
            self.translate_frame(table_physical_address)?;

        let table_pointer =
            table_virtual_address.as_u64() as *const PageTableEntry;

        unsafe {
            for index in 0..PAGE_TABLE_ENTRIES {
                let entry =
                    read_volatile(table_pointer.add(index));

                if entry.flags().contains(PageTableFlags::PRESENT) {
                    return Some(false);
                }
            }
        }

        Some(true)
    }

    // --------------------------------------------------------
    // PAGE TABLE ALLOCATION
    // --------------------------------------------------------

    fn allocate_table(
        &self,
        allocator: &mut PhysicalFrameAllocator<'_>,
    ) -> Result<u64, MapError> {
        let physical_address = allocator
            .allocate_frame()
            .ok_or(MapError::AllocationFailed)?;

        let virtual_address =
            match self.translate_frame(physical_address) {
                Some(address) => address,
                None => {
                    allocator.deallocate_frame(physical_address);
                    return Err(MapError::InvalidTableAddress);
                }
            };

        unsafe {
            write_bytes(
                virtual_address.as_u64() as *mut u8,
                0,
                PAGE_SIZE as usize,
            );
        }

        Ok(physical_address)
    }

    fn ensure_next_table(
        &self,
        allocator: &mut PhysicalFrameAllocator<'_>,
        parent_table: u64,
        index: usize,
        user_accessible: bool,
    ) -> Result<(u64, TableChange), MapError> {
        let mut entry = self
            .read_table_entry(parent_table, index)
            .ok_or(MapError::InvalidTableAddress)?;

        let flags = entry.flags();

        if flags.contains(PageTableFlags::PRESENT) {
            if flags.contains(PageTableFlags::HUGE_PAGE) {
                return Err(MapError::HugePageConflict);
            }

            let table_address = entry.addr().as_u64();

            let mut new_flags = flags | PageTableFlags::WRITABLE;

            if user_accessible {
                new_flags |= PageTableFlags::USER_ACCESSIBLE;
            }

            if new_flags != flags {
                entry.set_flags(new_flags);

                self.write_table_entry(parent_table, index, entry)
                    .ok_or(MapError::InvalidTableAddress)?;
            }

            return Ok((
                table_address,
                TableChange {
                    parent_table,
                    index,
                    frame: table_address,
                    created: false,
                    old_flags: flags,
                },
            ));
        }

        let new_table = self.allocate_table(allocator)?;

        let physical_address =
            match PhysAddr::try_new(new_table) {
                Ok(address) => address,
                Err(_) => {
                    allocator.deallocate_frame(new_table);
                    return Err(MapError::InvalidPhysicalAddress);
                }
            };

        let mut new_flags = TABLE_FLAGS;

        if user_accessible {
            new_flags |= PageTableFlags::USER_ACCESSIBLE;
        }

        entry.set_addr(physical_address, new_flags);

        if self
            .write_table_entry(parent_table, index, entry)
            .is_none()
        {
            allocator.deallocate_frame(new_table);
            return Err(MapError::InvalidTableAddress);
        }

        Ok((
            new_table,
            TableChange {
                parent_table,
                index,
                frame: new_table,
                created: true,
                old_flags: PageTableFlags::empty(),
            },
        ))
    }

    // --------------------------------------------------------
    // TLB HELPERS
    // --------------------------------------------------------

    fn flush_if_active(
        &self,
        pml4: u64,
        virtual_address: VirtAddr,
    ) {
        if pml4 == active_level_4_table_address() {
            flush_tlb(virtual_address);
            crate::arch::smp::tlb_shootdown_page(virtual_address.as_u64());
        } else if is_shared_kernel_entry(usize::from(virtual_address.p4_index())) {
            flush_tlb(virtual_address);
            crate::arch::smp::tlb_shootdown_page(virtual_address.as_u64());
        }
    }

    // --------------------------------------------------------
    // ROLLBACK
    // --------------------------------------------------------

    fn rollback_created_tables(
        &self,
        allocator: &mut PhysicalFrameAllocator<'_>,
        changes: &[TableChange; 3],
        count: usize,
        pml4: u64,
        virtual_address: VirtAddr,
    ) {
        let mut unlinked = [false; 3];

        // Restore entries in reverse order.
        for position in (0..count).rev() {
            let change = changes[position];

            let mut entry = match self.read_table_entry(
                change.parent_table,
                change.index,
            ) {
                Some(entry) => entry,
                None => continue,
            };

            if change.created {
                entry.set_unused();
            } else {
                entry.set_flags(change.old_flags);
            }

            if self
                .write_table_entry(
                    change.parent_table,
                    change.index,
                    entry,
                )
                .is_some()
            {
                unlinked[position] = change.created;
            }
        }

        self.flush_if_active(pml4, virtual_address);

        // Release only newly allocated tables that were unlinked.
        for position in (0..count).rev() {
            if unlinked[position] {
                let _ = allocator.deallocate_frame(
                    changes[position].frame,
                );
            }
        }
    }

    // --------------------------------------------------------
    // MAP PAGE: ACTIVE ADDRESS SPACE
    // --------------------------------------------------------

    pub fn map_page(
        &self,
        allocator: &mut PhysicalFrameAllocator<'_>,
        virtual_address: VirtAddr,
        physical_address: u64,
        flags: PageTableFlags,
    ) -> Result<(), MapError> {
        self.map_page_in(
            allocator,
            active_level_4_table_address(),
            virtual_address,
            physical_address,
            flags,
        )
    }

    // --------------------------------------------------------
    // MAP PAGE: ARBITRARY ADDRESS SPACE
    // --------------------------------------------------------

    pub fn map_page_in(
        &self,
        allocator: &mut PhysicalFrameAllocator<'_>,
        pml4: u64,
        virtual_address: VirtAddr,
        physical_address: u64,
        flags: PageTableFlags,
    ) -> Result<(), MapError> {
        self.validate_pml4(pml4)?;

        if virtual_address.as_u64() % PAGE_SIZE != 0
            || physical_address % PAGE_SIZE != 0
        {
            return Err(MapError::UnalignedAddress);
        }

        if !flags.contains(PageTableFlags::PRESENT)
            || flags.contains(PageTableFlags::HUGE_PAGE)
        {
            return Err(MapError::InvalidFlags);
        }

        let physical_address =
            PhysAddr::try_new(physical_address)
                .map_err(|_| MapError::InvalidPhysicalAddress)?;

        let p4_index = usize::from(virtual_address.p4_index());
        let p3_index = usize::from(virtual_address.p3_index());
        let p2_index = usize::from(virtual_address.p2_index());
        let p1_index = usize::from(virtual_address.p1_index());

        let user_accessible =
            flags.contains(PageTableFlags::USER_ACCESSIBLE);

        // Never expose shared kernel mappings to user mode.
        if user_accessible && is_shared_kernel_entry(p4_index) {
            return Err(MapError::InvalidFlags);
        }

        let mut changes = [
            TableChange {
                parent_table: 0,
                index: 0,
                frame: 0,
                created: false,
                old_flags: PageTableFlags::empty(),
            };
            3
        ];

        let mut change_count = 0usize;

        // PML4 -> PDPT
        let (pdpt, change) = self.ensure_next_table(
            allocator,
            pml4,
            p4_index,
            user_accessible,
        )?;

        changes[change_count] = change;
        change_count += 1;

        // PDPT -> PD
        let (pd, change) = match self.ensure_next_table(
            allocator,
            pdpt,
            p3_index,
            user_accessible,
        ) {
            Ok(result) => result,
            Err(error) => {
                self.rollback_created_tables(
                    allocator,
                    &changes,
                    change_count,
                    pml4,
                    virtual_address,
                );

                return Err(error);
            }
        };

        changes[change_count] = change;
        change_count += 1;

        // PD -> PT
        let (pt, change) = match self.ensure_next_table(
            allocator,
            pd,
            p2_index,
            user_accessible,
        ) {
            Ok(result) => result,
            Err(error) => {
                self.rollback_created_tables(
                    allocator,
                    &changes,
                    change_count,
                    pml4,
                    virtual_address,
                );

                return Err(error);
            }
        };

        changes[change_count] = change;
        change_count += 1;

        // PTE
        let mut entry = match self.read_table_entry(pt, p1_index) {
            Some(entry) => entry,
            None => {
                self.rollback_created_tables(
                    allocator,
                    &changes,
                    change_count,
                    pml4,
                    virtual_address,
                );

                return Err(MapError::InvalidTableAddress);
            }
        };

        if entry.flags().contains(PageTableFlags::PRESENT) {
            self.rollback_created_tables(
                allocator,
                &changes,
                change_count,
                pml4,
                virtual_address,
            );

            return Err(MapError::AlreadyMapped);
        }

        entry.set_addr(physical_address, flags);

        if self
            .write_table_entry(pt, p1_index, entry)
            .is_none()
        {
            self.rollback_created_tables(
                allocator,
                &changes,
                change_count,
                pml4,
                virtual_address,
            );

            return Err(MapError::InvalidTableAddress);
        }

        self.flush_if_active(pml4, virtual_address);

        Ok(())
    }

    // --------------------------------------------------------
    // UNMAP PAGE: ACTIVE ADDRESS SPACE
    // --------------------------------------------------------

    pub fn unmap_page(
        &self,
        virtual_address: VirtAddr,
    ) -> Result<u64, MapError> {
        self.unmap_page_in(
            active_level_4_table_address(),
            virtual_address,
        )
    }

    // --------------------------------------------------------
    // UNMAP PAGE: ACTIVE ADDRESS SPACE WITH CLEANUP
    // --------------------------------------------------------

    pub fn unmap_page_with_allocator(
        &self,
        allocator: &mut PhysicalFrameAllocator<'_>,
        virtual_address: VirtAddr,
    ) -> Result<u64, MapError> {
        self.unmap_page_in_with_allocator(
            allocator,
            active_level_4_table_address(),
            virtual_address,
        )
    }

    // --------------------------------------------------------
    // UNMAP PAGE: ARBITRARY ADDRESS SPACE
    // --------------------------------------------------------

    pub fn unmap_page_in(
        &self,
        pml4: u64,
        virtual_address: VirtAddr,
    ) -> Result<u64, MapError> {
        self.validate_pml4(pml4)?;

        if virtual_address.as_u64() % PAGE_SIZE != 0 {
            return Err(MapError::UnalignedAddress);
        }

        let p4_index = usize::from(virtual_address.p4_index());
        let p3_index = usize::from(virtual_address.p3_index());
        let p2_index = usize::from(virtual_address.p2_index());
        let p1_index = usize::from(virtual_address.p1_index());

        // Shared kernel page tables are visible from every address
        // space. Do not modify them through an inactive address space.
        if is_shared_kernel_entry(p4_index)
            && pml4 != active_level_4_table_address()
        {
            return Err(MapError::InvalidFlags);
        }

        let pml4_entry = self
            .read_table_entry(pml4, p4_index)
            .ok_or(MapError::InvalidTableAddress)?;

        if !pml4_entry.flags().contains(PageTableFlags::PRESENT) {
            return Err(MapError::NotMapped);
        }

        if pml4_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            return Err(MapError::HugePageConflict);
        }

        let pdpt = pml4_entry.addr().as_u64();

        let pdpt_entry = self
            .read_table_entry(pdpt, p3_index)
            .ok_or(MapError::InvalidTableAddress)?;

        if !pdpt_entry.flags().contains(PageTableFlags::PRESENT) {
            return Err(MapError::NotMapped);
        }

        if pdpt_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            return Err(MapError::HugePageConflict);
        }

        let pd = pdpt_entry.addr().as_u64();

        let pd_entry = self
            .read_table_entry(pd, p2_index)
            .ok_or(MapError::InvalidTableAddress)?;

        if !pd_entry.flags().contains(PageTableFlags::PRESENT) {
            return Err(MapError::NotMapped);
        }

        if pd_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            return Err(MapError::HugePageConflict);
        }

        let pt = pd_entry.addr().as_u64();

        let mut pte = self
            .read_table_entry(pt, p1_index)
            .ok_or(MapError::InvalidTableAddress)?;

        if !pte.flags().contains(PageTableFlags::PRESENT) {
            return Err(MapError::NotMapped);
        }

        let physical_address =
            pte.addr().as_u64() & ADDRESS_MASK_4K;

        pte.set_unused();

        self.write_table_entry(pt, p1_index, pte)
            .ok_or(MapError::InvalidTableAddress)?;

        self.flush_if_active(pml4, virtual_address);

        // The caller owns the data frame.
        Ok(physical_address)
    }

    // --------------------------------------------------------
    // UNMAP PAGE: ARBITRARY ADDRESS SPACE WITH CLEANUP
    // --------------------------------------------------------

    pub fn unmap_page_in_with_allocator(
        &self,
        allocator: &mut PhysicalFrameAllocator<'_>,
        pml4: u64,
        virtual_address: VirtAddr,
    ) -> Result<u64, MapError> {
        self.validate_pml4(pml4)?;

        let p4_index = usize::from(virtual_address.p4_index());
        let p3_index = usize::from(virtual_address.p3_index());
        let p2_index = usize::from(virtual_address.p2_index());

        // Shared kernel page tables must never be pruned here.
        // Their lower-level tables may be referenced by other
        // address spaces.
        let shared_kernel_entry =
            is_shared_kernel_entry(p4_index);

        let physical_address =
            self.unmap_page_in(pml4, virtual_address)?;

        if shared_kernel_entry {
            return Ok(physical_address);
        }

        // Resolve the table hierarchy again after removing the PTE.
        let pml4_entry = self
            .read_table_entry(pml4, p4_index)
            .ok_or(MapError::InvalidTableAddress)?;

        if !pml4_entry.flags().contains(PageTableFlags::PRESENT) {
            return Ok(physical_address);
        }

        let pdpt = pml4_entry.addr().as_u64();

        let pdpt_entry = self
            .read_table_entry(pdpt, p3_index)
            .ok_or(MapError::InvalidTableAddress)?;

        if !pdpt_entry.flags().contains(PageTableFlags::PRESENT)
            || pdpt_entry.flags().contains(PageTableFlags::HUGE_PAGE)
        {
            return Ok(physical_address);
        }

        let pd = pdpt_entry.addr().as_u64();

        let pd_entry = self
            .read_table_entry(pd, p2_index)
            .ok_or(MapError::InvalidTableAddress)?;

        if !pd_entry.flags().contains(PageTableFlags::PRESENT)
            || pd_entry.flags().contains(PageTableFlags::HUGE_PAGE)
        {
            return Ok(physical_address);
        }

        let pt = pd_entry.addr().as_u64();

        // ----------------------------------------------------
        // 1. Remove an empty PT
        // ----------------------------------------------------

        if self.table_is_empty(pt) != Some(true)
            || !allocator.owns_frame(pt)
        {
            return Ok(physical_address);
        }

        let mut parent_entry = pd_entry;
        parent_entry.set_unused();

        if self
            .write_table_entry(pd, p2_index, parent_entry)
            .is_none()
        {
            return Ok(physical_address);
        }

        if !allocator.deallocate_frame(pt) {
            // The frame was unlinked, but the allocator refused
            // to release it. Do not continue pruning its parents.
            return Ok(physical_address);
        }

        self.flush_if_active(pml4, virtual_address);

        // ----------------------------------------------------
        // 2. Remove an empty PD
        // ----------------------------------------------------

        if self.table_is_empty(pd) != Some(true)
            || !allocator.owns_frame(pd)
        {
            return Ok(physical_address);
        }

        let mut parent_entry = pdpt_entry;
        parent_entry.set_unused();

        if self
            .write_table_entry(pdpt, p3_index, parent_entry)
            .is_none()
        {
            return Ok(physical_address);
        }

        if !allocator.deallocate_frame(pd) {
            return Ok(physical_address);
        }

        self.flush_if_active(pml4, virtual_address);

        // ----------------------------------------------------
        // 3. Remove an empty PDPT
        // ----------------------------------------------------

        if self.table_is_empty(pdpt) != Some(true)
            || !allocator.owns_frame(pdpt)
        {
            return Ok(physical_address);
        }

        let mut parent_entry = pml4_entry;
        parent_entry.set_unused();

        if self
            .write_table_entry(pml4, p4_index, parent_entry)
            .is_none()
        {
            return Ok(physical_address);
        }

        if !allocator.deallocate_frame(pdpt) {
            return Ok(physical_address);
        }

        self.flush_if_active(pml4, virtual_address);

        // The PML4 root itself is never released here.
        Ok(physical_address)
    }

    // --------------------------------------------------------
    // PML4 STATISTICS
    // --------------------------------------------------------

    pub fn inspect_active_level_4_table(
        &self,
    ) -> Option<Pml4Stats> {
        self.inspect_level_4_table(active_level_4_table_address())
    }

    pub fn inspect_level_4_table(
        &self,
        pml4: u64,
    ) -> Option<Pml4Stats> {
        self.validate_pml4(pml4).ok()?;

        let virtual_address = self.translate_frame(pml4)?;

        let table_pointer =
            virtual_address.as_u64() as *const PageTableEntry;

        let mut stats = Pml4Stats {
            total_entries: PAGE_TABLE_ENTRIES,
            ..Pml4Stats::default()
        };

        unsafe {
            for index in 0..PAGE_TABLE_ENTRIES {
                let entry = read_volatile(table_pointer.add(index));

                if entry.is_unused() {
                    continue;
                }

                stats.occupied_entries += 1;

                let flags = entry.flags();

                if flags.contains(PageTableFlags::PRESENT) {
                    stats.present_entries += 1;
                }

                if flags.contains(PageTableFlags::WRITABLE) {
                    stats.writable_entries += 1;
                }

                if flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                    stats.user_accessible_entries += 1;
                }
            }
        }

        Some(stats)
    }

    // --------------------------------------------------------
    // VIRTUAL ADDRESS TRANSLATION: ACTIVE SPACE
    // --------------------------------------------------------


    /// Updates the PTE flags of an existing 4 KiB page inside `pml4` in place.
    pub fn update_page_flags_in(
        &self,
        pml4: u64,
        virtual_address: VirtAddr,
        new_flags: PageTableFlags,
    ) -> bool {
        if self.validate_pml4(pml4).is_err() || virtual_address.as_u64() % PAGE_SIZE != 0 {
            return false;
        }
        let p4_index = usize::from(virtual_address.p4_index());
        let p3_index = usize::from(virtual_address.p3_index());
        let p2_index = usize::from(virtual_address.p2_index());
        let p1_index = usize::from(virtual_address.p1_index());

        let Some(pml4_entry) = self.read_table_entry(pml4, p4_index) else { return false; };
        if !pml4_entry.flags().contains(PageTableFlags::PRESENT) { return false; }

        let pdpt = pml4_entry.addr().as_u64();
        let Some(pdpt_entry) = self.read_table_entry(pdpt, p3_index) else { return false; };
        if !pdpt_entry.flags().contains(PageTableFlags::PRESENT) || pdpt_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            return false;
        }

        let pd = pdpt_entry.addr().as_u64();
        let Some(pd_entry) = self.read_table_entry(pd, p2_index) else { return false; };
        if !pd_entry.flags().contains(PageTableFlags::PRESENT) || pd_entry.flags().contains(PageTableFlags::HUGE_PAGE) {
            return false;
        }

        let pt = pd_entry.addr().as_u64();
        let Some(mut pt_entry) = self.read_table_entry(pt, p1_index) else { return false; };
        if !pt_entry.flags().contains(PageTableFlags::PRESENT) {
            return false;
        }

        let phys = pt_entry.addr();
        pt_entry.set_addr(phys, new_flags);
        self.write_table_entry(pt, p1_index, pt_entry).is_some()
    }

    /// Clones all committed 4 KiB user pages from `parent_pml4` into `child_pml4` using Copy-on-Write.
    pub fn clone_user_pages_cow(
        &self,
        allocator: &mut PhysicalFrameAllocator<'_>,
        parent_pml4: u64,
        child_pml4: u64,
        regions: &[crate::userland::sched::VmRegion],
    ) -> Result<usize, MapError> {
        let mut shared_pages = 0usize;
        for region in regions {
            if region.kind == crate::userland::sched::VmRegionKind::StackGuard {
                continue;
            }
            let is_shared_mem = region.kind == crate::userland::sched::VmRegionKind::SharedMem;
            let mut page = region.start & !(PAGE_SIZE - 1);
            let end = (region.end.saturating_add(PAGE_SIZE - 1)) & !(PAGE_SIZE - 1);

            while page < end {
                let Ok(virt) = VirtAddr::try_new(page) else {
                    break;
                };
                if let Some(trans) = self.translate_virtual_address_in(parent_pml4, virt) {
                    if trans.page_size == PAGE_SIZE && trans.flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                        let phys = trans.physical_address & !(PAGE_SIZE - 1);
                        let mut child_flags = trans.flags;

                        if !is_shared_mem && trans.flags.contains(PageTableFlags::WRITABLE) {
                            let mut cow_flags = trans.flags;
                            cow_flags.remove(PageTableFlags::WRITABLE);
                            cow_flags.insert(COW_PAGE_FLAG);
                            if !self.update_page_flags_in(parent_pml4, virt, cow_flags) {
                                return Err(MapError::InvalidFlags);
                            }
                            child_flags = cow_flags;
                        }

                        self.map_page_in(allocator, child_pml4, virt, phys, child_flags)?;
                        crate::memory::inc_frame_ref(phys);
                        shared_pages += 1;
                    }
                }
                page = page.saturating_add(PAGE_SIZE);
            }
        }
        Ok(shared_pages)
    }

    pub fn translate_virtual_address(
        &self,
        virtual_address: VirtAddr,
    ) -> Option<PageTranslation> {
        self.translate_virtual_address_in(
            active_level_4_table_address(),
            virtual_address,
        )
    }

    // --------------------------------------------------------
    // VIRTUAL ADDRESS TRANSLATION: ARBITRARY SPACE
    // --------------------------------------------------------

    pub fn translate_virtual_address_in(
        &self,
        pml4: u64,
        virtual_address: VirtAddr,
    ) -> Option<PageTranslation> {
        self.validate_pml4(pml4).ok()?;

        let p4_index = usize::from(virtual_address.p4_index());
        let p3_index = usize::from(virtual_address.p3_index());
        let p2_index = usize::from(virtual_address.p2_index());
        let p1_index = usize::from(virtual_address.p1_index());

        // Level 4: PML4.
        let pml4_entry =
            self.read_table_entry(pml4, p4_index)?;

        let pml4_flags = pml4_entry.flags();

        if !pml4_flags.contains(PageTableFlags::PRESENT) {
            return None;
        }

        let pdpt_address = pml4_entry.addr().as_u64();

        // Level 3: PDPT.
        let pdpt_entry =
            self.read_table_entry(pdpt_address, p3_index)?;

        let pdpt_flags = pdpt_entry.flags();

        if !pdpt_flags.contains(PageTableFlags::PRESENT) {
            return None;
        }

        // 1 GiB page.
        if pdpt_flags.contains(PageTableFlags::HUGE_PAGE) {
            let physical_base =
                pdpt_entry.addr().as_u64() & ADDRESS_MASK_1G;

            let page_offset =
                virtual_address.as_u64() & ((1u64 << 30) - 1);

            return Some(PageTranslation {
                physical_address: physical_base | page_offset,
                page_size: 1u64 << 30,
                flags: pdpt_flags,
            });
        }

        let pd_address = pdpt_entry.addr().as_u64();

        // Level 2: Page Directory.
        let pd_entry =
            self.read_table_entry(pd_address, p2_index)?;

        let pd_flags = pd_entry.flags();

        if !pd_flags.contains(PageTableFlags::PRESENT) {
            return None;
        }

        // 2 MiB page.
        if pd_flags.contains(PageTableFlags::HUGE_PAGE) {
            let physical_base =
                pd_entry.addr().as_u64() & ADDRESS_MASK_2M;

            let page_offset =
                virtual_address.as_u64() & ((1u64 << 21) - 1);

            return Some(PageTranslation {
                physical_address: physical_base | page_offset,
                page_size: 1u64 << 21,
                flags: pd_flags,
            });
        }

        let pt_address = pd_entry.addr().as_u64();

        // Level 1: Page Table.
        let pt_entry =
            self.read_table_entry(pt_address, p1_index)?;

        let pt_flags = pt_entry.flags();

        if !pt_flags.contains(PageTableFlags::PRESENT) {
            return None;
        }

        let physical_base =
            pt_entry.addr().as_u64() & ADDRESS_MASK_4K;

        let page_offset =
            virtual_address.as_u64() & (PAGE_SIZE - 1);

        Some(PageTranslation {
            physical_address: physical_base | page_offset,
            page_size: PAGE_SIZE,
            flags: pt_flags,
        })
    }
}

// ============================================================
// TLB
// ============================================================

fn flush_tlb(address: VirtAddr) {
    unsafe {
        asm!(
            "invlpg [{}]",
            in(reg) address.as_u64(),
            options(nostack, preserves_flags),
        );
    }
}

// ============================================================
// ACTIVE PML4
// ============================================================

pub fn active_level_4_table_address() -> u64 {
    let (frame, _) = Cr3::read();
    frame.start_address().as_u64()
}
