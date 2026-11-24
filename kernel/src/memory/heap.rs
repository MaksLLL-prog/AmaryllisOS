
use core::alloc::{GlobalAlloc, Layout};
use core::cmp::max;
use core::mem::{align_of, size_of};
use core::ptr::{null_mut, NonNull};

// ============================================================
// HEAP STRUCTURES
// ============================================================

#[repr(C)]
struct FreeBlock {
    size: usize,
    next: *mut FreeBlock,
}

#[repr(C)]
struct AllocationHeader {
    block_start: *mut u8,
    block_size: usize,
}

pub struct HeapAllocator {
    head: *mut FreeBlock,
    start: usize,
    end: usize,
    initialized: bool,
}

// The allocator is accessed through a spin::Mutex.
// Its internal pointers are protected by that lock.
unsafe impl Send for HeapAllocator {}

// ============================================================
// GLOBAL HEAP ALLOCATOR
// ============================================================

pub struct GlobalHeapAllocator {
    inner: spin::Mutex<HeapAllocator>,
}

impl GlobalHeapAllocator {
    pub const fn new() -> Self {
        Self {
            inner: spin::Mutex::new(HeapAllocator::new()),
        }
    }

    // --------------------------------------------------------
    // INITIALIZATION
    // --------------------------------------------------------

    /// Initialize the global heap.
    ///
    /// # Safety
    ///
    /// The memory region must be mapped, writable,
    /// exclusively owned by this allocator and valid
    /// for its entire lifetime.
    pub unsafe fn initialize(
        &self,
        memory_start: *mut u8,
        memory_size: usize,
    ) -> bool {
        let _pg = crate::task::preempt::PreemptGuard::no_resched();
        unsafe {
            self.inner
                .lock()
                .initialize(memory_start, memory_size)
        }
    }

    // --------------------------------------------------------
    // PUBLIC ALLOCATION API
    // --------------------------------------------------------

    pub fn allocate(
        &self,
        layout: Layout,
    ) -> Option<NonNull<u8>> {
        let _pg = crate::task::preempt::PreemptGuard::no_resched();
        self.inner.lock().allocate(layout)
    }

    /// Release a previously allocated block.
    ///
    /// # Safety
    ///
    /// The pointer must have been returned by this allocator,
    /// must not have been released before, and must not be used
    /// after this call.
    pub unsafe fn deallocate(
        &self,
        pointer: NonNull<u8>,
    ) -> bool {
        let _pg = crate::task::preempt::PreemptGuard::no_resched();
        unsafe {
            self.inner.lock().deallocate(pointer)
        }
    }

    // --------------------------------------------------------
    // STATISTICS API
    // --------------------------------------------------------

    pub fn is_initialized(&self) -> bool {
        let _pg = crate::task::preempt::PreemptGuard::no_resched();
        self.inner.lock().is_initialized()
    }

    pub fn total_size(&self) -> usize {
        let _pg = crate::task::preempt::PreemptGuard::no_resched();
        self.inner.lock().total_size()
    }

    pub fn free_bytes(&self) -> usize {
        let _pg = crate::task::preempt::PreemptGuard::no_resched();
        self.inner.lock().free_bytes()
    }

    pub fn free_blocks(&self) -> usize {
        let _pg = crate::task::preempt::PreemptGuard::no_resched();
        self.inner.lock().free_blocks()
    }

    pub fn largest_free_block(&self) -> usize {
        let _pg = crate::task::preempt::PreemptGuard::no_resched();
        self.inner.lock().largest_free_block()
    }

    pub fn check_integrity(&self) -> bool {
        let _pg = crate::task::preempt::PreemptGuard::no_resched();
        self.inner.lock().check_integrity()
    }
}

// ============================================================
// RUST GLOBAL ALLOCATOR
// ============================================================

unsafe impl GlobalAlloc for GlobalHeapAllocator {
    unsafe fn alloc(
        &self,
        layout: Layout,
    ) -> *mut u8 {
        match self.allocate(layout) {
            Some(pointer) => pointer.as_ptr(),
            None => null_mut(),
        }
    }

    unsafe fn dealloc(
        &self,
        pointer: *mut u8,
        _layout: Layout,
    ) {
        if let Some(pointer) = NonNull::new(pointer) {
            let _ = unsafe {
                self.deallocate(pointer)
            };
        }
    }
}

#[global_allocator]
pub static GLOBAL_ALLOCATOR: GlobalHeapAllocator =
    GlobalHeapAllocator::new();

// ============================================================
// HEAP ALLOCATOR
// ============================================================

impl HeapAllocator {
    pub const fn new() -> Self {
        Self {
            head: null_mut(),
            start: 0,
            end: 0,
            initialized: false,
        }
    }

    // --------------------------------------------------------
    // INITIALIZATION
    // --------------------------------------------------------

    /// Initialize the heap over an already mapped memory region.
    ///
    /// # Safety
    ///
    /// The memory region must:
    /// - Be mapped and writable.
    /// - Be exclusively owned by this allocator.
    /// - Remain valid for the allocator's lifetime.
    pub unsafe fn initialize(
        &mut self,
        memory_start: *mut u8,
        memory_size: usize,
    ) -> bool {
        if self.initialized || memory_start.is_null() {
            return false;
        }

        let raw_start = memory_start as usize;

        let start = match align_up(
            raw_start,
            align_of::<FreeBlock>(),
        ) {
            Some(value) => value,
            None => return false,
        };

        let raw_end = match raw_start.checked_add(memory_size) {
            Some(value) => value,
            None => return false,
        };

        let end = raw_end & !(align_of::<FreeBlock>() - 1);

        if end <= start {
            return false;
        }

        let available_size = end - start;

        if available_size < size_of::<FreeBlock>() {
            return false;
        }

        let block = start as *mut FreeBlock;

        unsafe {
            block.write(FreeBlock {
                size: available_size,
                next: null_mut(),
            });
        }

        self.head = block;
        self.start = start;
        self.end = end;
        self.initialized = true;

        true
    }

    // --------------------------------------------------------
    // ALLOCATION
    // --------------------------------------------------------

    pub fn allocate(
        &mut self,
        layout: Layout,
    ) -> Option<NonNull<u8>> {
        if !self.initialized || layout.size() == 0 {
            return None;
        }

        let header_size = size_of::<AllocationHeader>();

        let required_alignment = max(
            layout.align(),
            align_of::<AllocationHeader>(),
        );

        let mut previous: *mut FreeBlock = null_mut();
        let mut current = self.head;

        unsafe {
            while !current.is_null() {
                let block_start = current as usize;
                let block_size = (*current).size;
                let next = (*current).next;

                let minimum_user_address =
                    block_start.checked_add(header_size)?;

                let user_address = align_up(
                    minimum_user_address,
                    required_alignment,
                )?;

                let allocation_end =
                    user_address.checked_add(layout.size())?;

                let raw_consumed =
                    allocation_end.checked_sub(block_start)?;

                let mut consumed = align_up(
                    raw_consumed,
                    align_of::<FreeBlock>(),
                )?;

                if consumed > block_size {
                    previous = current;
                    current = next;
                    continue;
                }

                let mut remaining = block_size - consumed;

                if remaining < size_of::<FreeBlock>() {
                    consumed = block_size;
                    remaining = 0;
                }

                if remaining > 0 {
                    let remainder_address =
                        block_start + consumed;

                    let remainder =
                        remainder_address as *mut FreeBlock;

                    remainder.write(FreeBlock {
                        size: remaining,
                        next,
                    });

                    if previous.is_null() {
                        self.head = remainder;
                    } else {
                        (*previous).next = remainder;
                    }
                } else {
                    if previous.is_null() {
                        self.head = next;
                    } else {
                        (*previous).next = next;
                    }
                }

                let header_address =
                    user_address - header_size;

                let header =
                    header_address as *mut AllocationHeader;

                header.write(AllocationHeader {
                    block_start: block_start as *mut u8,
                    block_size: consumed,
                });

                return NonNull::new(
                    user_address as *mut u8,
                );
            }
        }

        None
    }

    // --------------------------------------------------------
    // DEALLOCATION
    // --------------------------------------------------------

    /// Release a previously allocated block.
    ///
    /// # Safety
    ///
    /// The pointer must have been returned by this allocator,
    /// must not have been released before, and must not be used
    /// after this call.
    pub unsafe fn deallocate(
        &mut self,
        pointer: NonNull<u8>,
    ) -> bool {
        if !self.initialized {
            return false;
        }

        let user_address = pointer.as_ptr() as usize;

        let header_size = size_of::<AllocationHeader>();

        let header_address =
            match user_address.checked_sub(header_size) {
                Some(address) => address,
                None => return false,
            };

        let header =
            unsafe {
                (header_address as *const AllocationHeader).read()
            };

        let block_start = header.block_start as usize;
        let block_size = header.block_size;

        let block_end = match block_start.checked_add(block_size) {
            Some(value) => value,
            None => return false,
        };

        if block_start < self.start
            || block_end > self.end
            || block_size < size_of::<FreeBlock>()
            || block_start % align_of::<FreeBlock>() != 0
        {
            return false;
        }

        let mut previous: *mut FreeBlock = null_mut();
        let mut current = self.head;

        unsafe {
            while !current.is_null()
                && (current as usize) < block_start
            {
                previous = current;
                current = (*current).next;
            }

            if !previous.is_null() {
                let previous_end =
                    match (previous as usize).checked_add((*previous).size) {
                        Some(end) => end,
                        None => return false,
                    };

                if previous_end > block_start {
                    return false;
                }
            }

            if !current.is_null() {
                if block_end > current as usize {
                    return false;
                }
            }

            let released = block_start as *mut FreeBlock;

            released.write(FreeBlock {
                size: block_size,
                next: current,
            });

            let mut merged = released;

            if !previous.is_null() {
                let previous_end =
                    (previous as usize) + (*previous).size;

                if previous_end == block_start {
                    (*previous).size += block_size;
                    (*previous).next = current;

                    merged = previous;
                } else {
                    (*previous).next = released;
                }
            } else {
                self.head = released;
            }

            let merged_end =
                (merged as usize) + (*merged).size;

            let next = (*merged).next;

            if !next.is_null()
                && merged_end == next as usize
            {
                (*merged).size += (*next).size;
                (*merged).next = (*next).next;
            }
        }

        true
    }

    // --------------------------------------------------------
    // STATISTICS
    // --------------------------------------------------------

    pub fn is_initialized(&self) -> bool {
        self.initialized
    }

    pub fn total_size(&self) -> usize {
        if !self.initialized {
            return 0;
        }

        self.end - self.start
    }

    pub fn free_bytes(&self) -> usize {
        if !self.initialized {
            return 0;
        }

        let mut total = 0usize;
        let mut current = self.head;

        unsafe {
            while !current.is_null() {
                total = total.saturating_add((*current).size);
                current = (*current).next;
            }
        }

        total
    }

    pub fn free_blocks(&self) -> usize {
        if !self.initialized {
            return 0;
        }

        let mut count = 0usize;
        let mut current = self.head;

        unsafe {
            while !current.is_null() {
                count += 1;
                current = (*current).next;
            }
        }

        count
    }

    pub fn largest_free_block(&self) -> usize {
        if !self.initialized {
            return 0;
        }

        let mut largest = 0usize;
        let mut current = self.head;

        unsafe {
            while !current.is_null() {
                largest = max(largest, (*current).size);
                current = (*current).next;
            }
        }

        largest
    }

    // --------------------------------------------------------
    // INTEGRITY CHECK
    // --------------------------------------------------------

    pub fn check_integrity(&self) -> bool {
        if !self.initialized {
            return self.head.is_null();
        }

        if self.start >= self.end {
            return false;
        }

        let mut current = self.head;
        let mut previous_end = 0usize;
        let mut has_previous = false;

        unsafe {
            while !current.is_null() {
                let address = current as usize;
                let size = (*current).size;

                if address < self.start || address >= self.end {
                    return false;
                }

                if address % align_of::<FreeBlock>() != 0 {
                    return false;
                }

                if size < size_of::<FreeBlock>() {
                    return false;
                }

                let end = match address.checked_add(size) {
                    Some(value) => value,
                    None => return false,
                };

                if end > self.end {
                    return false;
                }

                if has_previous && address <= previous_end {
                    return false;
                }

                previous_end = end;
                has_previous = true;

                current = (*current).next;
            }
        }

        true
    }
}

// ============================================================
// ALIGNMENT
// ============================================================

fn align_up(
    address: usize,
    alignment: usize,
) -> Option<usize> {
    if alignment == 0 || !alignment.is_power_of_two() {
        return None;
    }

    address
        .checked_add(alignment - 1)
        .map(|value| value & !(alignment - 1))
}
