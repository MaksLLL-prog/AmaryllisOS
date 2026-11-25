
use alloc::vec::Vec;

use core::ptr::{
    read_volatile,
    write_bytes,
    write_volatile,
};

use core::sync::atomic::{
    AtomicU64,
    Ordering,
};

use x86_64::VirtAddr;
use x86_64::structures::paging::page_table::PageTableEntry;
use x86_64::structures::paging::PageTableFlags;

use super::vmm::{
    active_level_4_table_address,
    is_shared_kernel_entry,
    PhysicalMemoryMapping,
    KERNEL_PML4_START,
    PAGE_SIZE,
    PAGE_TABLE_ENTRIES,
};

use super::PhysicalFrameAllocator;

static NEXT_ADDRESS_SPACE_ID: AtomicU64 =
    AtomicU64::new(1);

// ============================================================
// ERRORS
// ============================================================

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddressSpaceError {
    KernelAddressSpace,
    ActiveAddressSpace,
    InvalidRootFrame,
    InvalidTableAddress,
    FrameNotOwned,
    DeallocationFailed,
    DuplicateTableFrame,
    CycleDetected,
    TrackingCapacityExceeded,
    TrackingAllocationFailed,
}

// ============================================================
// ADDRESS SPACE
// ============================================================

#[derive(Debug)]
pub struct AddressSpace {
    id: u64,
    pml4_physical_address: u64,
    kernel_pml4_physical_address: u64,
}

impl AddressSpace {
    // --------------------------------------------------------
    // CREATE ADDRESS SPACE
    // --------------------------------------------------------

    pub fn new(
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
    ) -> Option<Self> {
        let kernel_pml4 =
            active_level_4_table_address();

        Self::with_kernel(
            mapping,
            allocator,
            kernel_pml4,
        )
    }

    // --------------------------------------------------------
    // CREATE ADDRESS SPACE WITH SPECIFIC KERNEL PML4
    // --------------------------------------------------------

    pub fn with_kernel(
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
        kernel_pml4: u64,
    ) -> Option<Self> {
        if kernel_pml4 == 0
            || kernel_pml4 % PAGE_SIZE != 0
        {
            return None;
        }

        let kernel_virtual =
            mapping.translate_frame(kernel_pml4)?;

        let new_pml4 =
            allocator.allocate_frame()?;

        let new_virtual =
            match mapping.translate_frame(new_pml4) {
                Some(address) => address,
                None => {
                    allocator.deallocate_frame(new_pml4);
                    return None;
                }
            };

        let new_pointer =
            new_virtual.as_u64() as *mut PageTableEntry;

        let kernel_pointer =
            kernel_virtual.as_u64() as *const PageTableEntry;

        unsafe {
            write_bytes(
                new_pointer as *mut u8,
                0,
                PAGE_SIZE as usize,
            );

            for index in 0..PAGE_TABLE_ENTRIES {
                if !is_shared_kernel_entry(index) {
                    continue;
                }

                let entry =
                    read_volatile(kernel_pointer.add(index));

                write_volatile(
                    new_pointer.add(index),
                    entry,
                );
            }
        }

        let id = NEXT_ADDRESS_SPACE_ID.fetch_add(
            1,
            Ordering::Relaxed,
        );

        Some(Self {
            id,
            pml4_physical_address: new_pml4,
            kernel_pml4_physical_address: kernel_pml4,
        })
    }

    // --------------------------------------------------------
    // INFORMATION
    // --------------------------------------------------------

    pub fn id(&self) -> u64 {
        self.id
    }

    pub fn pml4_physical_address(&self) -> u64 {
        self.pml4_physical_address
    }

    pub fn kernel_pml4_physical_address(&self) -> u64 {
        self.kernel_pml4_physical_address
    }

    pub fn is_kernel_address_space(&self) -> bool {
        self.pml4_physical_address
            == self.kernel_pml4_physical_address
    }

    pub fn is_destroyed(&self) -> bool {
        self.pml4_physical_address == 0
    }

    // --------------------------------------------------------
    // VERIFY SHARED KERNEL MAPPINGS
    // --------------------------------------------------------

    pub fn shares_kernel_mappings(
        &self,
        mapping: &PhysicalMemoryMapping,
    ) -> bool {
        if self.is_destroyed() {
            return false;
        }

        let new_virtual =
            match mapping.translate_frame(
                self.pml4_physical_address,
            ) {
                Some(address) => address,
                None => return false,
            };

        let kernel_virtual =
            match mapping.translate_frame(
                self.kernel_pml4_physical_address,
            ) {
                Some(address) => address,
                None => return false,
            };

        let new_table =
            new_virtual.as_u64() as *const PageTableEntry;

        let kernel_table =
            kernel_virtual.as_u64() as *const PageTableEntry;

        unsafe {
            for index in 0..PAGE_TABLE_ENTRIES {
                if !is_shared_kernel_entry(index) {
                    continue;
                }

                let new_entry =
                    read_volatile(new_table.add(index));

                let kernel_entry =
                    read_volatile(kernel_table.add(index));

                if new_entry.addr() != kernel_entry.addr()
                    || new_entry.flags() != kernel_entry.flags()
                {
                    return false;
                }
            }
        }

        true
    }

    // --------------------------------------------------------
    // VERIFY USER HALF IS EMPTY
    // --------------------------------------------------------

    pub fn user_half_is_empty(
        &self,
        mapping: &PhysicalMemoryMapping,
    ) -> bool {
        if self.is_destroyed() {
            return false;
        }

        let virtual_address =
            match mapping.translate_frame(
                self.pml4_physical_address,
            ) {
                Some(address) => address,
                None => return false,
            };

        let table =
            virtual_address.as_u64() as *const PageTableEntry;

        unsafe {
            for index in 0..KERNEL_PML4_START {
                if is_shared_kernel_entry(index) {
                    continue;
                }

                if !read_volatile(table.add(index)).is_unused() {
                    return false;
                }
            }
        }

        true
    }

    // --------------------------------------------------------
    // COLLECT AND VALIDATE PRIVATE PAGE TABLES
    // --------------------------------------------------------

    fn collect_private_tables(
        &self,
        mapping: &PhysicalMemoryMapping,
        allocator: &PhysicalFrameAllocator<'_>,
        visitor: &mut dyn FnMut(u64) -> Result<(), AddressSpaceError>,
    ) -> Result<(), AddressSpaceError> {
        let root_physical =
            self.pml4_physical_address;

        if self.is_destroyed()
            || root_physical == 0
            || root_physical % PAGE_SIZE != 0
            || !allocator.owns_frame(root_physical)
        {
            return Err(AddressSpaceError::InvalidRootFrame);
        }

        let root_virtual =
            mapping
                .translate_frame(root_physical)
                .ok_or(AddressSpaceError::InvalidTableAddress)?;

        let mut path = [0u64; 4];
        path[0] = root_physical;

        visitor(root_physical)?;

        let root =
            root_virtual.as_u64() as *const PageTableEntry;

        unsafe {
            for index in 0..PAGE_TABLE_ENTRIES {
                if is_shared_kernel_entry(index) {
                    continue;
                }

                let entry =
                    read_volatile(root.add(index));

                let flags = entry.flags();

                if !flags.contains(PageTableFlags::PRESENT) {
                    continue;
                }

                if flags.contains(PageTableFlags::HUGE_PAGE) {
                    return Err(AddressSpaceError::InvalidTableAddress);
                }

                collect_table_tree(
                    mapping,
                    allocator,
                    entry.addr().as_u64(),
                    3,
                    &mut path,
                    1,
                    visitor,
                )?;
            }
        }

        Ok(())
    }

    // --------------------------------------------------------
    // DESTROY ADDRESS SPACE
    // --------------------------------------------------------

    /// Frees only the private page-table hierarchy.
    ///
    /// Page-table frames:
    /// - PML4
    /// - PDPT
    /// - PD
    /// - PT
    ///
    /// Physical frames mapped by leaf entries are not freed.
    /// Shared kernel mappings are not traversed or freed.
    /// MMIO and framebuffer mappings are treated as leaf mappings.
    ///
    /// # Safety
    ///
    /// The caller must guarantee:
    ///
    /// - This address space is not active on any CPU.
    /// - No other address space shares its private page tables.
    /// - No concurrent code modifies its page tables.
    /// - Private page tables were allocated by this allocator.
    pub unsafe fn destroy(
        &mut self,
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
    ) -> Result<(), AddressSpaceError> {
        if self.is_destroyed() {
            return Err(AddressSpaceError::InvalidRootFrame);
        }

        if self.is_kernel_address_space() {
            return Err(AddressSpaceError::KernelAddressSpace);
        }

        if active_level_4_table_address()
            == self.pml4_physical_address
        {
            return Err(AddressSpaceError::ActiveAddressSpace);
        }

        if allocator.is_bootstrap() {
            self.destroy_bootstrap(mapping, allocator)?;
        } else {
            self.destroy_dynamic(mapping, allocator)?;
        }

        self.pml4_physical_address = 0;

        Ok(())
    }

    // --------------------------------------------------------
    // BOOTSTRAP DESTRUCTION
    // --------------------------------------------------------

    fn destroy_bootstrap(
        &mut self,
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
    ) -> Result<(), AddressSpaceError> {
        let root_physical =
            self.pml4_physical_address;

        // Bootstrap does not allocate a temporary Vec.
        // Every discovered frame is checked by a fresh traversal.
        let mut visitor = |frame: u64| {
            let occurrences =
                count_private_table_occurrences(
                    mapping,
                    allocator,
                    root_physical,
                    frame,
                )?;

            if occurrences > 1 {
                return Err(AddressSpaceError::DuplicateTableFrame);
            }

            Ok(())
        };

        self.collect_private_tables(
            mapping,
            allocator,
            &mut visitor,
        )?;

        // Both validation stages have succeeded.
        // The tree is now released in post-order.
        let mut path = [0u64; 4];
        path[0] = root_physical;

        unsafe {
            free_private_table_tree(
                mapping,
                allocator,
                root_physical,
                4,
                &mut path,
                1,
            )?;
        }

        Ok(())
    }

    // --------------------------------------------------------
    // DYNAMIC DESTRUCTION
    // --------------------------------------------------------

    fn destroy_dynamic(
        &mut self,
        mapping: &PhysicalMemoryMapping,
        allocator: &mut PhysicalFrameAllocator<'_>,
    ) -> Result<(), AddressSpaceError> {
        let allocation_limit =
            allocator.allocated_frames();

        let mut frames = Vec::<u64>::new();

        frames
            .try_reserve_exact(allocation_limit)
            .map_err(|_| {
                AddressSpaceError::TrackingAllocationFailed
            })?;

        {
            let mut visitor = |frame: u64| {
                // A valid private tree cannot contain more table
                // occurrences than allocated physical frames.
                if frames.len() >= allocation_limit {
                    return Err(AddressSpaceError::DuplicateTableFrame);
                }

                frames.push(frame);

                Ok(())
            };

            self.collect_private_tables(
                mapping,
                allocator,
                &mut visitor,
            )?;
        }

        // Sort once, then detect duplicate physical frames.
        frames.sort_unstable();

        if frames.windows(2).any(|pair| pair[0] == pair[1]) {
            return Err(AddressSpaceError::DuplicateTableFrame);
        }

        // The complete hierarchy has passed validation.
        // No further table traversal is necessary.
        for frame in frames.into_iter().rev() {
            if !allocator.deallocate_frame(frame) {
                return Err(AddressSpaceError::DeallocationFailed);
            }
        }

        Ok(())
    }
}

// ============================================================
// TABLE FRAME VALIDATION
// ============================================================

fn validate_table_frame(
    mapping: &PhysicalMemoryMapping,
    allocator: &PhysicalFrameAllocator<'_>,
    physical_address: u64,
    is_root: bool,
) -> Result<VirtAddr, AddressSpaceError> {
    if physical_address == 0
        || physical_address % PAGE_SIZE != 0
    {
        return Err(if is_root {
            AddressSpaceError::InvalidRootFrame
        } else {
            AddressSpaceError::InvalidTableAddress
        });
    }

    if !allocator.owns_frame(physical_address) {
        return Err(if is_root {
            AddressSpaceError::InvalidRootFrame
        } else {
            AddressSpaceError::FrameNotOwned
        });
    }

    mapping
        .translate_frame(physical_address)
        .ok_or(AddressSpaceError::InvalidTableAddress)
}

// ============================================================
// CYCLE DETECTION
// ============================================================

fn check_path(
    path: &[u64; 4],
    depth: usize,
    next_frame: u64,
) -> Result<(), AddressSpaceError> {
    if path[..depth].contains(&next_frame) {
        return Err(AddressSpaceError::CycleDetected);
    }

    if depth >= path.len() {
        return Err(AddressSpaceError::InvalidTableAddress);
    }

    Ok(())
}

// ============================================================
// COUNT PRIVATE TABLE OCCURRENCES
// ============================================================

fn count_private_table_occurrences(
    mapping: &PhysicalMemoryMapping,
    allocator: &PhysicalFrameAllocator<'_>,
    root_physical: u64,
    target: u64,
) -> Result<usize, AddressSpaceError> {
    let root_virtual =
        validate_table_frame(
            mapping,
            allocator,
            root_physical,
            true,
        )?;

    let mut count =
        usize::from(root_physical == target);

    let mut path = [0u64; 4];
    path[0] = root_physical;

    count_table_occurrences_recursive(
        mapping,
        allocator,
        root_virtual,
        4,
        true,
        target,
        &mut count,
        &mut path,
        1,
    )?;

    Ok(count)
}


fn count_table_occurrences_recursive(
    mapping: &PhysicalMemoryMapping,
    allocator: &PhysicalFrameAllocator<'_>,
    virtual_address: VirtAddr,
    level: u8,
    is_root: bool,
    target: u64,
    count: &mut usize,
    path: &mut [u64; 4],
    depth: usize,
) -> Result<(), AddressSpaceError> {
    if level <= 1 || *count > 1 {
        return Ok(());
    }

    let table =
        virtual_address.as_u64() as *const PageTableEntry;

    unsafe {
        for index in 0..PAGE_TABLE_ENTRIES {
            if is_root && is_shared_kernel_entry(index) {
                continue;
            }

            let entry =
                read_volatile(table.add(index));

            let flags = entry.flags();

            if !flags.contains(PageTableFlags::PRESENT) {
                continue;
            }

            if flags.contains(PageTableFlags::HUGE_PAGE) {
                if level == 4 {
                    return Err(
                        AddressSpaceError::InvalidTableAddress
                    );
                }

                if level == 3 || level == 2 {
                    continue;
                }
            }

            let child_physical =
                entry.addr().as_u64();

            check_path(
                path,
                depth,
                child_physical,
            )?;

            let child_virtual =
                validate_table_frame(
                    mapping,
                    allocator,
                    child_physical,
                    false,
                )?;

            if child_physical == target {
                *count += 1;

                if *count > 1 {
                    return Ok(());
                }
            }

            path[depth] = child_physical;

            count_table_occurrences_recursive(
                mapping,
                allocator,
                child_virtual,
                level - 1,
                false,
                target,
                count,
                path,
                depth + 1,
            )?;

            path[depth] = 0;

            if *count > 1 {
                return Ok(());
            }
        }
    }

    Ok(())
}


// ============================================================
// COLLECT PAGE TABLE TREE
// ============================================================

fn collect_table_tree(
    mapping: &PhysicalMemoryMapping,
    allocator: &PhysicalFrameAllocator<'_>,
    table_physical: u64,
    level: u8,
    path: &mut [u64; 4],
    depth: usize,
    visitor: &mut dyn FnMut(u64) -> Result<(), AddressSpaceError>,
) -> Result<(), AddressSpaceError> {
    check_path(
        path,
        depth,
        table_physical,
    )?;

    let virtual_address =
        validate_table_frame(
            mapping,
            allocator,
            table_physical,
            false,
        )?;

    path[depth] = table_physical;

    visitor(table_physical)?;

    if level <= 1 {
        path[depth] = 0;
        return Ok(());
    }

    let table =
        virtual_address.as_u64() as *const PageTableEntry;

    unsafe {
        for index in 0..PAGE_TABLE_ENTRIES {
            let entry =
                read_volatile(table.add(index));

            let flags = entry.flags();

            if !flags.contains(PageTableFlags::PRESENT) {
                continue;
            }

            // 1 GiB and 2 MiB mappings are leaves.
            if (level == 3 || level == 2)
                && flags.contains(PageTableFlags::HUGE_PAGE)
            {
                continue;
            }

            collect_table_tree(
                mapping,
                allocator,
                entry.addr().as_u64(),
                level - 1,
                path,
                depth + 1,
                visitor,
            )?;
        }
    }

    path[depth] = 0;

    Ok(())
}

// ============================================================
// FREE VALIDATED PRIVATE PAGE TABLE TREE
// ============================================================

unsafe fn free_private_table_tree(
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
    table_physical: u64,
    level: u8,
    path: &mut [u64; 4],
    depth: usize,
) -> Result<(), AddressSpaceError> {
    check_path(
        path,
        depth.saturating_sub(1),
        table_physical,
    )?;

    let virtual_address =
        validate_table_frame(
            mapping,
            &*allocator,
            table_physical,
            depth == 1,
        )?;

    path[depth - 1] = table_physical;

    if level > 1 {
        let table =
            virtual_address.as_u64() as *mut PageTableEntry;

        for index in 0..PAGE_TABLE_ENTRIES {
            if level == 4 && is_shared_kernel_entry(index) {
                continue;
            }

            let entry = unsafe {
                read_volatile(table.add(index))
            };

            let flags = entry.flags();

            if !flags.contains(PageTableFlags::PRESENT) {
                continue;
            }

            if (level == 3 || level == 2)
                && flags.contains(PageTableFlags::HUGE_PAGE)
            {
                continue;
            }

            let child_physical =
                entry.addr().as_u64();

            check_path(
                path,
                depth,
                child_physical,
            )?;

            unsafe {
                free_private_table_tree(
                    mapping,
                    allocator,
                    child_physical,
                    level - 1,
                    path,
                    depth + 1,
                )?;
            }

            let mut empty_entry = entry;
            empty_entry.set_unused();

            unsafe {
                write_volatile(
                    table.add(index),
                    empty_entry,
                );
            }
        }
    }

    if !allocator.deallocate_frame(table_physical) {
        return Err(AddressSpaceError::DeallocationFailed);
    }

    path[depth - 1] = 0;

    Ok(())
}

impl AddressSpace {
    pub fn destroy_forked_user_space(pml4: u64, regions: &[crate::userland::sched::VmRegion], mapping: &PhysicalMemoryMapping, allocator: &mut PhysicalFrameAllocator<'_>) {
        if pml4 == 0 || pml4 % PAGE_SIZE != 0 { return; }
        for r in regions {
            if r.kind == crate::userland::sched::VmRegionKind::StackGuard { continue; }
            let mut p = r.start & !(PAGE_SIZE - 1);
            let e = (r.end.saturating_add(PAGE_SIZE - 1)) & !(PAGE_SIZE - 1);
            while p < e {
                if let Ok(v) = VirtAddr::try_new(p) {
                    if let Some(t) = mapping.translate_virtual_address_in(pml4, v) {
                        if t.page_size == PAGE_SIZE && t.flags.contains(PageTableFlags::USER_ACCESSIBLE) {
                            let ph = t.physical_address & !(PAGE_SIZE - 1);
                            let _ = mapping.unmap_page_in(pml4, v);
                            allocator.deallocate_frame(ph);
                        }
                    }
                }
                p = p.saturating_add(PAGE_SIZE);
            }
        }
        let mut tmp = Self { id: 0, pml4_physical_address: pml4, kernel_pml4_physical_address: active_level_4_table_address() };
        unsafe { let _ = tmp.destroy(mapping, allocator); }
    }
}
