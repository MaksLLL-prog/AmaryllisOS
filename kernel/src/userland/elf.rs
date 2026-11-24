use alloc::vec::Vec;
use core::ptr::{copy_nonoverlapping, write_bytes};

use x86_64::structures::paging::PageTableFlags;
use x86_64::VirtAddr;

use crate::memory::vmm::{is_shared_kernel_entry, PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::storage::vfs::{self, VfsError, VfsNodeKind};

pub const ELF_MAGIC: [u8; 4] = [0x7F, b'E', b'L', b'F'];
pub const ELFCLASS64: u8 = 2;
pub const ELFDATA2LSB: u8 = 1;
pub const ET_EXEC: u16 = 2;
pub const EM_X86_64: u16 = 0x3E;

pub const PT_NULL: u32 = 0;
pub const PT_LOAD: u32 = 1;

pub const PF_X: u32 = 1;
pub const PF_W: u32 = 2;
pub const PF_R: u32 = 4;

// Canonical lower user virtual space limit (just below 0x0000_8000_0000_0000).
pub const USER_ADDR_LIMIT: u64 = 0x0000_7FFF_FFFF_F000;

// Default Ring 3 stack layout (PML4 index 0):
// 64 KiB mapped eagerly at launch + 192 KiB lazy on-demand growth window + 4 KiB unmapped guard page.
pub const USER_STACK_TOP: u64 = 0x0000_007F_FFFF_0000;
pub const USER_STACK_INITIAL_PAGES: usize = 16;
pub const USER_STACK_MAX_PAGES: usize = 64;
pub const USER_STACK_PAGES: usize = USER_STACK_INITIAL_PAGES;
pub const USER_STACK_MAX_BOTTOM: u64 =
    USER_STACK_TOP - (USER_STACK_MAX_PAGES as u64) * PAGE_SIZE;
pub const USER_STACK_GUARD_ADDR: u64 = USER_STACK_MAX_BOTTOM - PAGE_SIZE;

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Elf64Header {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[repr(C, packed)]
#[derive(Clone, Copy)]
pub struct Elf64ProgramHeader {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

#[derive(Clone, Copy)]
pub struct ElfSegmentSummary {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}

pub struct ElfInspection {
    pub file_size: u32,
    pub entry_point: u64,
    pub e_type: u16,
    pub e_machine: u16,
    pub ph_num: u16,
    pub segments: Vec<ElfSegmentSummary>,
}

pub struct LoadedElfImage {
    pub entry_point: u64,
    pub user_rsp: u64,
    pub brk_base: u64,
    pub initial_argc: u64,
    pub initial_argv: u64,
    pub mapped_pages: Vec<(u64, u64)>,
    pub load_regions: Vec<(u64, u64, bool, bool)>,
    pub load_segment_count: usize,
    pub total_bytes_loaded: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ElfError {
    Vfs(VfsError),
    IsDirectory,
    TooSmall,
    InvalidMagic,
    UnsupportedClass,
    UnsupportedEndian,
    UnsupportedType,
    UnsupportedMachine,
    InvalidProgramHeaders,
    InvalidEntryPoint,
    InvalidSegmentRange,
    SegmentOverlapsKernel,
    OutOfMemory,
    MappingFailed,
}

impl ElfError {
    pub const fn message(self) -> &'static str {
        match self {
            Self::Vfs(err) => err.message(),
            Self::IsDirectory => "Target path is a directory, not an ELF executable",
            Self::TooSmall => "File is too small to contain an ELF64 header",
            Self::InvalidMagic => "Invalid ELF magic bytes (expected 0x7F ELF)",
            Self::UnsupportedClass => "Unsupported ELF class (only 64-bit ELF64 supported)",
            Self::UnsupportedEndian => "Unsupported ELF endianness (expected little-endian)",
            Self::UnsupportedType => "Unsupported ELF object type (expected ET_EXEC executable)",
            Self::UnsupportedMachine => "Unsupported ELF architecture (expected x86_64)",
            Self::InvalidProgramHeaders => "Invalid or out-of-bounds ELF program header table",
            Self::InvalidEntryPoint => "ELF entry point is null or outside Ring 3 user space",
            Self::InvalidSegmentRange => "ELF PT_LOAD segment range is invalid or exceeds file size",
            Self::SegmentOverlapsKernel => "ELF segment virtual address collides with kernel mappings",
            Self::OutOfMemory => "Out of physical frames or kernel heap while loading ELF",
            Self::MappingFailed => "Failed to map ELF page into process address space",
        }
    }
}

impl From<VfsError> for ElfError {
    fn from(value: VfsError) -> Self {
        Self::Vfs(value)
    }
}

pub fn is_valid_user_vaddr(addr: u64) -> bool {
    if addr == 0 || addr >= USER_ADDR_LIMIT {
        return false;
    }

    let p4 = ((addr >> 39) & 0x1FF) as usize;
    !is_shared_kernel_entry(p4)
}

fn check_user_range(start: u64, size: u64) -> Result<(), ElfError> {
    if size == 0 {
        return Ok(());
    }

    let end = start
        .checked_add(size)
        .ok_or(ElfError::InvalidSegmentRange)?;

    if start == 0 || end > USER_ADDR_LIMIT {
        return Err(ElfError::InvalidSegmentRange);
    }

    let first_page = start & !(PAGE_SIZE - 1);
    let last_page = (end - 1) & !(PAGE_SIZE - 1);

    let mut page = first_page;
    while page <= last_page {
        let p4 = ((page >> 39) & 0x1FF) as usize;
        if is_shared_kernel_entry(p4) {
            return Err(ElfError::SegmentOverlapsKernel);
        }
        page = page.saturating_add(PAGE_SIZE);
    }

    Ok(())
}

fn read_exact_vfs(path: &str, offset: u32, out: &mut [u8]) -> Result<(), ElfError> {
    let mut done = 0usize;
    while done < out.len() {
        let read = vfs::with_vfs(|vfs_state| {
            vfs_state.read_file_chunk(path, offset + done as u32, &mut out[done..])
        })?;
        if read == 0 {
            return Err(ElfError::InvalidProgramHeaders);
        }
        done += read;
    }
    Ok(())
}

fn parse_header_and_phdrs(path: &str) -> Result<(u32, Elf64Header, Vec<Elf64ProgramHeader>), ElfError> {
    let stat = vfs::with_vfs(|vfs_state| vfs_state.stat(path))?;
    if stat.kind == VfsNodeKind::Directory {
        return Err(ElfError::IsDirectory);
    }

    let eh_size = core::mem::size_of::<Elf64Header>();
    if (stat.size as usize) < eh_size {
        return Err(ElfError::TooSmall);
    }

    let mut hdr_bytes = [0u8; core::mem::size_of::<Elf64Header>()];
    read_exact_vfs(path, 0, &mut hdr_bytes)?;

    let header: Elf64Header = unsafe {
        core::ptr::read_unaligned(hdr_bytes.as_ptr() as *const Elf64Header)
    };

    if header.e_ident[0..4] != ELF_MAGIC {
        return Err(ElfError::InvalidMagic);
    }
    if header.e_ident[4] != ELFCLASS64 {
        return Err(ElfError::UnsupportedClass);
    }
    if header.e_ident[5] != ELFDATA2LSB {
        return Err(ElfError::UnsupportedEndian);
    }
    if header.e_type != ET_EXEC {
        return Err(ElfError::UnsupportedType);
    }
    if header.e_machine != EM_X86_64 {
        return Err(ElfError::UnsupportedMachine);
    }

    if !is_valid_user_vaddr(header.e_entry) {
        return Err(ElfError::InvalidEntryPoint);
    }

    let ph_num = header.e_phnum;
    let ph_entsize = header.e_phentsize;
    let min_ph_size = core::mem::size_of::<Elf64ProgramHeader>() as u16;

    if ph_num == 0 || ph_num > 64 || ph_entsize < min_ph_size {
        return Err(ElfError::InvalidProgramHeaders);
    }

    let ph_table_bytes = (ph_num as u64)
        .checked_mul(ph_entsize as u64)
        .ok_or(ElfError::InvalidProgramHeaders)?;
    let ph_end = header
        .e_phoff
        .checked_add(ph_table_bytes)
        .ok_or(ElfError::InvalidProgramHeaders)?;

    if ph_end > stat.size as u64 {
        return Err(ElfError::InvalidProgramHeaders);
    }

    let mut phdrs = Vec::with_capacity(ph_num as usize);
    let mut ph_buf = [0u8; core::mem::size_of::<Elf64ProgramHeader>()];

    for idx in 0..ph_num {
        let off = header.e_phoff + (idx as u64) * (ph_entsize as u64);
        read_exact_vfs(path, off as u32, &mut ph_buf)?;
        let phdr = unsafe {
            core::ptr::read_unaligned(ph_buf.as_ptr() as *const Elf64ProgramHeader)
        };
        phdrs.push(phdr);
    }

    Ok((stat.size, header, phdrs))
}

pub fn inspect_elf(path: &str) -> Result<ElfInspection, ElfError> {
    let (file_size, header, phdrs) = parse_header_and_phdrs(path)?;

    let segments = phdrs
        .into_iter()
        .map(|ph| ElfSegmentSummary {
            p_type: ph.p_type,
            p_flags: ph.p_flags,
            p_offset: ph.p_offset,
            p_vaddr: ph.p_vaddr,
            p_filesz: ph.p_filesz,
            p_memsz: ph.p_memsz,
            p_align: ph.p_align,
        })
        .collect();

    Ok(ElfInspection {
        file_size,
        entry_point: header.e_entry,
        e_type: header.e_type,
        e_machine: header.e_machine,
        ph_num: header.e_phnum,
        segments,
    })
}

pub fn load_elf_into_address_space(
    path: &str,
    pml4: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
    nxe_enabled: bool,
) -> Result<LoadedElfImage, ElfError> {
    load_elf_into_address_space_with_args(path, &[path], pml4, mapping, allocator, nxe_enabled)
}

pub fn load_elf_into_address_space_with_args(
    path: &str,
    args: &[&str],
    pml4: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
    nxe_enabled: bool,
) -> Result<LoadedElfImage, ElfError> {
    let (file_size, header, phdrs) = parse_header_and_phdrs(path)?;

    let mut image = LoadedElfImage {
        entry_point: header.e_entry,
        user_rsp: 0,
        brk_base: 0x0200_0000,
        initial_argc: 0,
        initial_argv: 0,
        mapped_pages: Vec::new(),
        load_regions: Vec::new(),
        load_segment_count: 0,
        total_bytes_loaded: 0,
    };

    let result = load_segments_and_stack(
        path,
        args,
        file_size,
        &phdrs,
        pml4,
        mapping,
        allocator,
        nxe_enabled,
        &mut image,
    );

    if let Err(err) = result {
        cleanup_loaded_image(&mut image, pml4, mapping, allocator);
        return Err(err);
    }

    Ok(image)
}

fn load_segments_and_stack(
    path: &str,
    args: &[&str],
    file_size: u32,
    phdrs: &[Elf64ProgramHeader],
    pml4: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
    nxe_enabled: bool,
    image: &mut LoadedElfImage,
) -> Result<(), ElfError> {
    let mut chunk_buf = [0u8; 512];
    let mut max_seg_end: u64 = 0x0200_0000;

    for ph in phdrs {
        if ph.p_type != PT_LOAD {
            continue;
        }

        if ph.p_filesz > ph.p_memsz {
            return Err(ElfError::InvalidSegmentRange);
        }

        let file_end = ph
            .p_offset
            .checked_add(ph.p_filesz)
            .ok_or(ElfError::InvalidSegmentRange)?;
        if file_end > file_size as u64 {
            return Err(ElfError::InvalidSegmentRange);
        }

        if ph.p_memsz == 0 {
            continue;
        }

        check_user_range(ph.p_vaddr, ph.p_memsz)?;
        let seg_end_aligned = (ph.p_vaddr.saturating_add(ph.p_memsz) + PAGE_SIZE - 1) & !(PAGE_SIZE - 1);
        if seg_end_aligned < USER_STACK_TOP - 0x0100_0000 && seg_end_aligned > max_seg_end {
            max_seg_end = seg_end_aligned;
        }
        image.load_segment_count += 1;

        let mut flags = PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE;
        // Map segment writable during initial setup? We write via HHDM directly,
        // so the user page-table entry strictly respects PF_W and PF_X permissions!
        if (ph.p_flags & PF_W) != 0 {
            flags |= PageTableFlags::WRITABLE;
        }
        if nxe_enabled && (ph.p_flags & PF_X) == 0 {
            flags |= PageTableFlags::NO_EXECUTE;
        }

        let seg_start = ph.p_vaddr;
        let seg_end = seg_start + ph.p_memsz;
        let first_page = seg_start & !(PAGE_SIZE - 1);
        let last_page = (seg_end - 1) & !(PAGE_SIZE - 1);
        image.load_regions.push((
            first_page,
            last_page + PAGE_SIZE,
            (ph.p_flags & PF_W) != 0,
            (ph.p_flags & PF_X) != 0,
        ));

        let mut page_vaddr = first_page;
        while page_vaddr <= last_page {
            let page_virt = VirtAddr::try_new(page_vaddr)
                .map_err(|_| ElfError::InvalidSegmentRange)?;

            let frame_phys = if let Some(&(_, existing_phys)) = image
                .mapped_pages
                .iter()
                .find(|&&(v, _)| v == page_vaddr)
            {
                // Boundary page shared between two PT_LOAD segments:
                // upgrade permissions if needed so neither segment is blocked.
                if let Some(trans) = mapping.translate_virtual_address_in(pml4, page_virt) {
                    let mut combined = trans.flags | (flags & PageTableFlags::WRITABLE);
                    if !flags.contains(PageTableFlags::NO_EXECUTE) {
                        combined.remove(PageTableFlags::NO_EXECUTE);
                    }
                    if combined != trans.flags {
                        let _ = mapping.unmap_page_in(pml4, page_virt);
                        mapping
                            .map_page_in(allocator, pml4, page_virt, existing_phys, combined)
                            .map_err(|_| ElfError::MappingFailed)?;
                    }
                }
                existing_phys
            } else {
                let frame = allocator.allocate_frame().ok_or(ElfError::OutOfMemory)?;
                let hhdm_virt = mapping
                    .translate_frame(frame)
                    .ok_or_else(|| {
                        allocator.deallocate_frame(frame);
                        ElfError::MappingFailed
                    })?;

                unsafe {
                    write_bytes(hhdm_virt.as_u64() as *mut u8, 0, PAGE_SIZE as usize);
                }

                if mapping
                    .map_page_in(allocator, pml4, page_virt, frame, flags)
                    .is_err()
                {
                    allocator.deallocate_frame(frame);
                    return Err(ElfError::MappingFailed);
                }

                image.mapped_pages.push((page_vaddr, frame));
                frame
            };

            // Determine overlap between this 4 KiB page and the initialized file portion [seg_start .. seg_start + p_filesz).
            let page_end = page_vaddr + PAGE_SIZE;
            let file_vaddr_end = seg_start + ph.p_filesz;

            let copy_start = seg_start.max(page_vaddr);
            let copy_end = file_vaddr_end.min(page_end);

            if copy_start < copy_end {
                let total_to_copy = (copy_end - copy_start) as usize;
                let file_base_offset = ph.p_offset + (copy_start - seg_start);
                let intra_page_offset = (copy_start - page_vaddr) as usize;

                let frame_hhdm = mapping
                    .translate_frame(frame_phys)
                    .ok_or(ElfError::MappingFailed)?;

                let mut copied = 0usize;
                while copied < total_to_copy {
                    let step = (total_to_copy - copied).min(chunk_buf.len());
                    read_exact_vfs(
                        path,
                        (file_base_offset + copied as u64) as u32,
                        &mut chunk_buf[..step],
                    )?;

                    unsafe {
                        let dst = (frame_hhdm.as_u64() as *mut u8)
                            .add(intra_page_offset + copied);
                        copy_nonoverlapping(chunk_buf.as_ptr(), dst, step);
                    }

                    copied += step;
                }

                image.total_bytes_loaded += total_to_copy as u64;
            }

            page_vaddr += PAGE_SIZE;
        }
    }

    if image.load_segment_count == 0 {
        return Err(ElfError::InvalidProgramHeaders);
    }

    // Allocate & map Ring 3 User Stack (USER_STACK_PAGES * 4 KiB ending at USER_STACK_TOP)
    let mut stack_flags =
        PageTableFlags::PRESENT | PageTableFlags::USER_ACCESSIBLE | PageTableFlags::WRITABLE;
    if nxe_enabled {
        stack_flags |= PageTableFlags::NO_EXECUTE;
    }

    let stack_bottom = USER_STACK_TOP - (USER_STACK_PAGES as u64) * PAGE_SIZE;
    for page_idx in 0..USER_STACK_PAGES {
        let page_vaddr = stack_bottom + (page_idx as u64) * PAGE_SIZE;
        let page_virt = VirtAddr::new(page_vaddr);

        let frame = allocator.allocate_frame().ok_or(ElfError::OutOfMemory)?;
        let hhdm_virt = mapping
            .translate_frame(frame)
            .ok_or_else(|| {
                allocator.deallocate_frame(frame);
                ElfError::MappingFailed
            })?;

        unsafe {
            write_bytes(hhdm_virt.as_u64() as *mut u8, 0, PAGE_SIZE as usize);
        }

        if mapping
            .map_page_in(allocator, pml4, page_virt, frame, stack_flags)
            .is_err()
        {
            allocator.deallocate_frame(frame);
            return Err(ElfError::MappingFailed);
        }

        image.mapped_pages.push((page_vaddr, frame));
    }

    image.brk_base = max_seg_end;

    // Populate SysV x86_64 initial stack with argc + argv strings/pointers (capped at 16 args / 2 KiB)
    let mut sp = USER_STACK_TOP - 16;
    let capped_args = args.iter().take(16);
    let mut arg_ptrs: Vec<u64> = Vec::new();
    let mut str_budget = 2048usize;

    for raw_arg in capped_args {
        let bytes = raw_arg.as_bytes();
        let take = bytes.len().min(250);
        if take + 1 > str_budget {
            break;
        }
        str_budget -= take + 1;
        sp -= (take + 1) as u64;
        write_stack_bytes(pml4, mapping, sp, &bytes[..take])?;
        write_stack_bytes(pml4, mapping, sp + take as u64, &[0])?;
        arg_ptrs.push(sp);
    }

    if arg_ptrs.is_empty() {
        let fallback = path.as_bytes();
        let take = fallback.len().min(200);
        sp -= (take + 1) as u64;
        write_stack_bytes(pml4, mapping, sp, &fallback[..take])?;
        write_stack_bytes(pml4, mapping, sp + take as u64, &[0])?;
        arg_ptrs.push(sp);
    }

    // Align down to 16 bytes, then push [argc, argv[0].., NULL, envp=NULL]
    sp &= !0xF;
    let pointer_qwords = 1 + arg_ptrs.len() + 1 + 1;
    let total_table_bytes = (pointer_qwords * 8) as u64;
    sp -= total_table_bytes;
    sp &= !0xF;

    let argc = arg_ptrs.len() as u64;
    let argv_ptr = sp + 8;

    write_stack_bytes(pml4, mapping, sp, &argc.to_le_bytes())?;
    for (idx, ptr_val) in arg_ptrs.iter().enumerate() {
        write_stack_bytes(pml4, mapping, argv_ptr + (idx as u64) * 8, &ptr_val.to_le_bytes())?;
    }
    write_stack_bytes(
        pml4,
        mapping,
        argv_ptr + (arg_ptrs.len() as u64) * 8,
        &0u64.to_le_bytes(),
    )?;
    write_stack_bytes(
        pml4,
        mapping,
        argv_ptr + ((arg_ptrs.len() + 1) as u64) * 8,
        &0u64.to_le_bytes(),
    )?;

    image.user_rsp = sp;
    image.initial_argc = argc;
    image.initial_argv = argv_ptr;
    Ok(())
}

fn write_stack_bytes(
    pml4: u64,
    mapping: &PhysicalMemoryMapping,
    vaddr: u64,
    data: &[u8],
) -> Result<(), ElfError> {
    let mut written = 0usize;
    while written < data.len() {
        let cur = vaddr + written as u64;
        let page_off = (cur & (PAGE_SIZE - 1)) as usize;
        let step = (data.len() - written).min(PAGE_SIZE as usize - page_off);
        let virt = VirtAddr::new(cur);
        let trans = mapping
            .translate_virtual_address_in(pml4, virt)
            .ok_or(ElfError::MappingFailed)?;
        let dst = mapping.translate(trans.physical_address).ok_or(ElfError::MappingFailed)?;
        unsafe {
            copy_nonoverlapping(data.as_ptr().add(written), dst.as_u64() as *mut u8, step);
        }
        written += step;
    }
    Ok(())
}

pub fn cleanup_loaded_image(
    image: &mut LoadedElfImage,
    pml4: u64,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) {
    while let Some((vaddr, frame)) = image.mapped_pages.pop() {
        if let Ok(virt) = VirtAddr::try_new(vaddr) {
            let _ = mapping.unmap_page_in(pml4, virt);
        }
        let _ = allocator.deallocate_frame(frame);
    }
}
