with open("kernel/src/userland/sched.rs", "r") as f:
    sh = f.read()

sh = sh.replace(
    "    PipeRead {\n        pipe_id: usize,\n        buf_ptr: u64,\n        cap: usize,\n        deadline_ms: u64,\n    },",
    "    PipeRead {\n        pipe_id: usize,\n        buf_ptr: u64,\n        cap: usize,\n        deadline_ms: u64,\n    },\n    WaitPid {\n        target_pid: isize,\n        status_ptr: u64,\n    },",
    1,
)
sh = sh.replace(
    "    Exited(i64),\n    Faulted(UserFaultInfo),",
    "    Exited(i64),\n    Zombie(i64),\n    Faulted(UserFaultInfo),",
    1,
)
sh = sh.replace(
    "struct UserProcessSlot {\n    pid: usize,",
    "struct UserProcessSlot {\n    pid: usize,\n    parent_pid: usize,",
    1,
)
sh = sh.replace(
    "            sched.slots[free_slot] = Some(UserProcessSlot {\n                pid,",
    "            sched.slots[free_slot] = Some(UserProcessSlot {\n                pid,\n                parent_pid: 0,",
    1,
)

cow_impl = """
fn resolve_cow_page_for_slot(
    slot_idx: usize,
    page_vaddr: u64,
    vma: VmRegion,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> bool {
    if !vma.writable || vma.kind == VmRegionKind::SharedMem || vma.kind == VmRegionKind::StackGuard {
        return false;
    }
    let pml4 = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        sched.slots[slot_idx].as_ref().map(|p| p.pml4)
    });
    let Some(pml4) = pml4 else { return false; };
    let Ok(virt) = VirtAddr::try_new(page_vaddr) else { return false; };
    let Some(trans) = mapping.translate_virtual_address_in(pml4, virt) else { return false; };
    if trans.page_size != PAGE_SIZE || !trans.flags.contains(crate::memory::vmm::COW_PAGE_FLAG) {
        return false;
    }

    let old_phys = trans.physical_address & !(PAGE_SIZE - 1);
    let refs = crate::memory::frame_ref_count(old_phys);

    if refs <= 1 {
        let mut new_flags = trans.flags;
        new_flags.remove(crate::memory::vmm::COW_PAGE_FLAG);
        new_flags.insert(PageTableFlags::WRITABLE);
        if !mapping.update_page_flags_in(pml4, virt, new_flags) {
            return false;
        }
    } else {
        let Some(new_frame) = allocator.allocate_frame() else {
            return false;
        };
        let (Some(src_hhdm), Some(dst_hhdm)) = (
            mapping.translate_frame(old_phys),
            mapping.translate_frame(new_frame),
        ) else {
            allocator.deallocate_frame(new_frame);
            return false;
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                src_hhdm.as_u64() as *const u8,
                dst_hhdm.as_u64() as *mut u8,
                PAGE_SIZE as usize,
            );
        }
        let mut new_flags = trans.flags;
        new_flags.remove(crate::memory::vmm::COW_PAGE_FLAG);
        new_flags.insert(PageTableFlags::WRITABLE);
        if mapping.map_page_in(allocator, pml4, virt, new_frame, new_flags).is_err() {
            allocator.deallocate_frame(new_frame);
            return false;
        }
        allocator.deallocate_frame(old_phys);
    }

    unsafe {
        asm!("invlpg [{}]", in(reg) page_vaddr, options(nostack, preserves_flags));
    }
    crate::arch::smp::tlb_shootdown_page(page_vaddr);

    without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        if let Some(ref mut proc) = sched.slots[slot_idx] {
            proc.lazy_faults = proc.lazy_faults.saturating_add(1);
        }
    });
    true
}

pub fn sys_fork_current(
    parent_ctx: UserCpuContext,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    let snapshot = without_interrupts(|| {
        let sched = USER_SCHED.lock();
        let idx = sched.current_slot?;
        let p = sched.slots[idx].as_ref()?;
        let _ = sched.slots.iter().position(|s| s.is_none())?;
        Some((
            p.pid, p.name.clone(), p.pml4, p.fpu_state, p.weight,
            p.entry_point, p.pages_mapped, p.bytes_loaded, p.brk_base,
            p.brk_current, p.mmap_next, p.shm_attach_next, p.shm_attachments.clone(),
            p.dynamic_bytes, p.vmas.clone(), p.fds.clone(),
        ))
    });

    let Some((
        parent_pid, name, parent_pml4, fpu_state, weight,
        entry_point, pages_mapped, bytes_loaded, brk_base,
        brk_current, mmap_next, shm_attach_next, shm_attachments,
        dynamic_bytes, vmas, fds,
    )) = snapshot else {
        return -11;
    };

    let Some(child_space) = crate::memory::address_space::AddressSpace::new(mapping, allocator) else {
        return -12;
    };
    let child_pml4 = child_space.pml4_physical_address();
    core::mem::forget(child_space);

    if mapping.clone_user_pages_cow(allocator, parent_pml4, child_pml4, &vmas).is_err() {
        crate::memory::address_space::AddressSpace::destroy_forked_user_space(child_pml4, &vmas, mapping, allocator);
        return -12;
    }

    for entry in fds.iter().flatten() {
        match entry {
            FdEntry::PipeRead { pipe_id } => super::ipc::retain_pipe_end(*pipe_id, false),
            FdEntry::PipeWrite { pipe_id } => super::ipc::retain_pipe_end(*pipe_id, true),
            _ => {}
        }
    }
    for att in &shm_attachments {
        let _ = super::ipc::retain_shm_frames(att.shm_id);
    }

    crate::arch::smp::tlb_shootdown_page(u64::MAX);

    let mut child_ctx = parent_ctx;
    child_ctx.rax = 0;

    let child_pid_res = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let Some(slot_idx) = sched.slots.iter().position(|s| s.is_none()) else { return None; };
        let child_pid = sched.next_pid;
        sched.next_pid += 1;
        let start_vr = sched.min_vruntime();
        sched.slots[slot_idx] = Some(UserProcessSlot {
            pid: child_pid, parent_pid, name, pml4: child_pml4, ctx: child_ctx,
            fpu_state, state: UserTaskState::Ready, cpu_ms: 0, vruntime_ms: start_vr,
            weight, preempt_count: 0, lazy_faults: 0, syscalls_served: 0,
            entry_point, pages_mapped, bytes_loaded, brk_base, brk_current,
            mmap_next, shm_attach_next, shm_attachments, owned_shm_ids: Vec::new(),
            dynamic_bytes, vmas, image: None, fds, stdout_buf: String::new(),
            has_gui_window: false,
        });
        Some(child_pid as i64)
    });

    match child_pid_res {
        Some(cpid) => {
            klog::log_fmt(KLogLevel::Info, "fork", format_args!("PID={} forked child PID={} (CoW pml4=0x{:x})", parent_pid, cpid, child_pml4));
            cpid
        }
        None => {
            close_fd_list(fds);
            cleanup_process_ipc_and_windows(0, child_pml4, false, shm_attachments, Vec::new(), mapping, allocator);
            crate::memory::address_space::AddressSpace::destroy_forked_user_space(child_pml4, &vmas, mapping, allocator);
            -11
        }
    }
}

pub fn sys_waitpid_current(
    target_pid: isize,
    status_ptr: u64,
    options: usize,
    parent_ctx: UserCpuContext,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    const WNOHANG: usize = 1;
    enum WaitCheck {
        NoChildren,
        Reaped(usize, i64, u64, Option<LoadedElfImage>, [Option<FdEntry>; MAX_PROC_FDS], bool, Vec<ShmAttachment>, Vec<u64>, Vec<VmRegion>),
        StillRunning,
    }

    let check = without_interrupts(|| {
        let mut sched = USER_SCHED.lock();
        let Some(cur_idx) = sched.current_slot else { return WaitCheck::NoChildren; };
        let Some(parent_pid) = sched.slots[cur_idx].as_ref().map(|p| p.pid) else { return WaitCheck::NoChildren; };

        let mut has_matching_child = false;
        let mut zombie_slot: Option<usize> = None;

        for (i, slot) in sched.slots.iter().enumerate() {
            let Some(child) = slot.as_ref() else { continue; };
            if child.parent_pid != parent_pid { continue; }
            if target_pid > 0 && child.pid != (target_pid as usize) { continue; }
            has_matching_child = true;
            if matches!(child.state, UserTaskState::Zombie(_)) {
                zombie_slot = Some(i);
                break;
            }
        }

        if let Some(zidx) = zombie_slot {
            let zproc = sched.slots[zidx].take().unwrap();
            let code = match zproc.state { UserTaskState::Zombie(c) => c, _ => 0 };
            return WaitCheck::Reaped(zproc.pid, code, zproc.pml4, zproc.image, zproc.fds, zproc.has_gui_window, zproc.shm_attachments, zproc.owned_shm_ids, zproc.vmas);
        }

        if !has_matching_child { WaitCheck::NoChildren } else { WaitCheck::StillRunning }
    });

    match check {
        WaitCheck::NoChildren => -10,
        WaitCheck::Reaped(cpid, code, pml4, image, fds, had_gui, shms, owned_shms, vmas) => {
            close_fd_list(fds);
            cleanup_process_ipc_and_windows(cpid, pml4, had_gui, shms, owned_shms, mapping, allocator);
            if let Some(mut img) = image {
                img.cleanup(mapping, allocator);
            } else {
                crate::memory::address_space::AddressSpace::destroy_forked_user_space(pml4, &vmas, mapping, allocator);
            }
            if status_ptr != 0 {
                let wstatus: i32 = ((code as i32) & 0xFF) << 8;
                if syscall::copy_to_user_bytes_in(mapping, Some(allocator), syscall::active_user_pml4_raw(), status_ptr, &wstatus.to_le_bytes()).is_err() {
                    return -14;
                }
            }
            cpid as i64
        }
        WaitCheck::StillRunning => {
            if (options & WNOHANG) != 0 { return 0; }
            unsafe {
                suspend_current_from_syscall(parent_ctx, UserTaskState::Waiting(WaitReason::WaitPid { target_pid, status_ptr }));
            }
        }
    }
}
"""

Используйте код с осторожностью.
sh = sh.replace("pub fn try_resolve_user_page_fault(", cow_impl + "\npub fn try_resolve_user_page_fault(", 1)
sh = sh.replace(
"    // Bit 0 == 1 means page IS already present (protection violation, not demand-page).\n    if (error_code & 0x1) != 0 {\n        return FaultResolveOutcome::NotHandled;\n    }",
"    if (error_code & 0x1) != 0 {\n        if (error_code & 0x2) != 0 && vma.writable {\n            let page_vaddr = cr2 & !(PAGE_SIZE - 1);\n            if resolve_cow_page_for_slot(slot_idx, page_vaddr, vma, mapping, allocator) {\n                return FaultResolveOutcome::Resolved;\n            }\n        }\n        return FaultResolveOutcome::NotHandled;\n    }",
1,
)
sh = sh.replace(
"            if require_writable && !trans.flags.contains(PageTableFlags::WRITABLE) {\n                return Err(-14);\n            }",
"            if require_writable && !trans.flags.contains(PageTableFlags::WRITABLE) {\n                if trans.flags.contains(crate::memory::vmm::COW_PAGE_FLAG) {\n                    if let Some(alloc_ref) = allocator.as_deref_mut() {\n                        let cow_info = without_interrupts(|| {\n                            let sched = USER_SCHED.lock();\n                            let idx = sched.find_slot_by_pml4(user_pml4)?;\n                            let proc = sched.slots[idx].as_ref()?;\n                            let vma = proc.find_vma(page)?;\n                            Some((idx, vma))\n                        });\n                        if let Some((idx, vma)) = cow_info {\n                            if resolve_cow_page_for_slot(idx, page, vma, mapping, alloc_ref) {\n                                page = page.saturating_add(PAGE_SIZE);\n                                continue;\n                            }\n                        }\n                    }\n                }\n                return Err(-14);\n            }",
1,
)
sh = sh.replace(
"                proc.state = UserTaskState::Exited(exit_code);",
"                let has_live_parent = proc.parent_pid != 0 && sched.slots.iter().flatten().any(|p| p.pid == proc.parent_pid && !matches!(p.state, UserTaskState::Exited() | UserTaskState::Zombie() | UserTaskState::Faulted(_)));\n                proc.state = if has_live_parent { UserTaskState::Zombie(exit_code) } else { UserTaskState::Exited(exit_code) };",
1,
)
sh = sh.replace(
"            WaitReason::PipeRead {\n                pipe_id,\n                buf_ptr,\n                cap,\n                deadline_ms,\n            } => {",
"""            WaitReason::WaitPid { target_pid, status_ptr } => {
enum PollWaitRes {
Ready(usize, i64, u64, Option, [Option; MAX_PROC_FDS], bool, Vec, Vec, Vec),
NoChild,
Pending,
}
let res = without_interrupts(|| {
let mut sched = USER_SCHED.lock();
let mut found_child = false;
let mut zslot = None;
for (i, s) in sched.slots.iter().enumerate() {
let Some(c) = s.as_ref() else { continue; };
if c.parent_pid != pid { continue; }
if target_pid > 0 && c.pid != (target_pid as usize) { continue; }
found_child = true;
if matches!(c.state, UserTaskState::Zombie(_)) { zslot = Some(i); break; }
}
if let Some(zi) = zslot {
let z = sched.slots[zi].take().unwrap();
let code = match z.state { UserTaskState::Zombie(c) => c, _ => 0 };
PollWaitRes::Ready(z.pid, code, z.pml4, z.image, z.fds, z.has_gui_window, z.shm_attachments, z.owned_shm_ids, z.vmas)
} else if !found_child { PollWaitRes::NoChild } else { PollWaitRes::Pending }
});
match res {
PollWaitRes::NoChild => wake_ret = Some(-10),
PollWaitRes::Ready(cpid, code, cpml4, cimg, cfds, chad_gui, cshms, cowned, cvmas) => {
close_fd_list(cfds);
cleanup_process_ipc_and_windows(cpid, cpml4, chad_gui, cshms, cowned, mapping, allocator);
if let Some(mut img) = cimg { img.cleanup(mapping, allocator); } else { crate::memory::address_space::AddressSpace::destroy_forked_user_space(cpml4, &cvmas, mapping, allocator); }
if status_ptr != 0 {
let wstatus: i32 = ((code as i32) & 0xFF) << 8;
if syscall::copy_to_user_bytes_in(mapping, Some(allocator), pml4, status_ptr, &wstatus.to_le_bytes()).is_err() { wake_ret = Some(-14); } else { wake_ret = Some(cpid as i64); }
} else { wake_ret = Some(cpid as i64); }
}
PollWaitRes::Pending => {}
}
}
WaitReason::PipeRead {
pipe_id,
buf_ptr,
cap,
deadline_ms,
} => {""",
1,
)
with open("kernel/src/userland/sched.rs", "w") as f:
f.write(sh)
print("sched.rs успешно обновлён!")
