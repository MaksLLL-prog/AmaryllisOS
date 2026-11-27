use alloc::string::String;
use alloc::vec::Vec;
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

use crate::arch::{acpi, apic};
use crate::interrupts::timer;
use crate::memory::{self, vmm::PhysicalMemoryMapping, PhysicalFrameAllocator};
use crate::userland::sched;
use crate::userland::syscall::{copy_from_user_bytes_in, copy_to_user_bytes_in};

// Top-level FreeBSD MIB identifiers (CTL_*)
pub const CTL_SYSCTL: i32 = 0;
pub const CTL_KERN: i32 = 1;
pub const CTL_VM: i32 = 2;
pub const CTL_VFS: i32 = 3;
pub const CTL_NET: i32 = 4;
pub const CTL_DEBUG: i32 = 5;
pub const CTL_HW: i32 = 6;
pub const CTL_MACHDEP: i32 = 7;

// CTL_SYSCTL magic leaf for sysctlnametomib()
pub const CTL_SYSCTL_NAME2OID: i32 = 3;

// CTL_KERN leaves
pub const KERN_OSTYPE: i32 = 1;
pub const KERN_OSRELEASE: i32 = 2;
pub const KERN_OSREV: i32 = 3;
pub const KERN_VERSION: i32 = 4;
pub const KERN_MAXPROC: i32 = 6;
pub const KERN_HOSTNAME: i32 = 10;
pub const KERN_HOSTID: i32 = 11;
pub const KERN_OSRELDATE: i32 = 24;
pub const KERN_NPROCS: i32 = 99;

// CTL_VM leaves
pub const VM_STATS: i32 = 10;
pub const VM_STATS_FREE_COUNT: i32 = 1;
pub const VM_STATS_TOTAL_PAGES: i32 = 2;

// CTL_HW leaves
pub const HW_MACHINE: i32 = 1;
pub const HW_MODEL: i32 = 2;
pub const HW_NCPU: i32 = 3;
pub const HW_BYTEORDER: i32 = 4;
pub const HW_PHYSMEM: i32 = 5;
pub const HW_USERMEM: i32 = 6;
pub const HW_PAGESIZE: i32 = 7;
pub const HW_REALMEM: i32 = 19;

// CTL_MACHDEP leaves
pub const MACHDEP_TSC_FREQ: i32 = 1;
pub const MACHDEP_UPTIME_MS: i32 = 2;

pub const FREEBSD_OSRELDATE: u32 = 1401000;

struct SysctlDynamicState {
    hostname: [u8; 64],
    hostname_len: usize,
    hostid: i32,
}

impl SysctlDynamicState {
    const fn new() -> Self {
        let mut hostname = [0u8; 64];
        let default = b"amaryllis";
        let mut i = 0;
        while i < default.len() {
            hostname[i] = default[i];
            i += 1;
        }
        Self {
            hostname,
            hostname_len: default.len(),
            hostid: 0x414D_4152u32 as i32,
        }
    }
}

static SYSCTL_STATE: Mutex<SysctlDynamicState> = Mutex::new(SysctlDynamicState::new());

fn name_to_oid(name: &str) -> Option<&'static [i32]> {
    match name.trim_end_matches('\0').trim() {
        "kern.ostype" => Some(&[CTL_KERN, KERN_OSTYPE]),
        "kern.osrelease" => Some(&[CTL_KERN, KERN_OSRELEASE]),
        "kern.osrevision" => Some(&[CTL_KERN, KERN_OSREV]),
        "kern.version" => Some(&[CTL_KERN, KERN_VERSION]),
        "kern.maxproc" => Some(&[CTL_KERN, KERN_MAXPROC]),
        "kern.hostname" => Some(&[CTL_KERN, KERN_HOSTNAME]),
        "kern.hostid" => Some(&[CTL_KERN, KERN_HOSTID]),
        "kern.osreldate" => Some(&[CTL_KERN, KERN_OSRELDATE]),
        "kern.nprocs" => Some(&[CTL_KERN, KERN_NPROCS]),
        "vm.stats.free_count" | "vm.stats.vm.v_free_count" => {
            Some(&[CTL_VM, VM_STATS, VM_STATS_FREE_COUNT])
        }
        "vm.stats.total_pages" | "vm.stats.vm.v_page_count" => {
            Some(&[CTL_VM, VM_STATS, VM_STATS_TOTAL_PAGES])
        }
        "hw.machine" => Some(&[CTL_HW, HW_MACHINE]),
        "hw.model" => Some(&[CTL_HW, HW_MODEL]),
        "hw.ncpu" => Some(&[CTL_HW, HW_NCPU]),
        "hw.byteorder" => Some(&[CTL_HW, HW_BYTEORDER]),
        "hw.physmem" => Some(&[CTL_HW, HW_PHYSMEM]),
        "hw.usermem" => Some(&[CTL_HW, HW_USERMEM]),
        "hw.pagesize" => Some(&[CTL_HW, HW_PAGESIZE]),
        "hw.realmem" => Some(&[CTL_HW, HW_REALMEM]),
        "machdep.tsc_freq" => Some(&[CTL_MACHDEP, MACHDEP_TSC_FREQ]),
        "machdep.uptime_ms" => Some(&[CTL_MACHDEP, MACHDEP_UPTIME_MS]),
        _ => None,
    }
}

fn cstr_bytes(s: &str) -> Vec<u8> {
    let mut v = Vec::with_capacity(s.len() + 1);
    v.extend_from_slice(s.as_bytes());
    v.push(0);
    v
}

fn read_mib_value(mib: &[i32]) -> Result<(Vec<u8>, bool), i64> {
    match mib {
        [CTL_KERN, KERN_OSTYPE] => Ok((cstr_bytes("FreeBSD"), false)),
        [CTL_KERN, KERN_OSRELEASE] => Ok((cstr_bytes("14.1-RELEASE"), false)),
        [CTL_KERN, KERN_OSREV] => Ok(((FREEBSD_OSRELDATE as i32).to_le_bytes().to_vec(), false)),
        [CTL_KERN, KERN_VERSION] => Ok((
            cstr_bytes("FreeBSD 14.1-RELEASE (AmaryllisOS ANU Hybrid Kernel x86_64)"),
            false,
        )),
        [CTL_KERN, KERN_MAXPROC] => Ok(((sched::MAX_USER_PROCS as i32).to_le_bytes().to_vec(), false)),
        [CTL_KERN, KERN_HOSTNAME] => {
            let bytes = without_interrupts(|| {
                let st = SYSCTL_STATE.lock();
                let mut v = Vec::with_capacity(st.hostname_len + 1);
                v.extend_from_slice(&st.hostname[..st.hostname_len]);
                v.push(0);
                v
            });
            Ok((bytes, true))
        }
        [CTL_KERN, KERN_HOSTID] => {
            let id = without_interrupts(|| SYSCTL_STATE.lock().hostid);
            Ok((id.to_le_bytes().to_vec(), true))
        }
        [CTL_KERN, KERN_OSRELDATE] => Ok(((FREEBSD_OSRELDATE as i32).to_le_bytes().to_vec(), false)),
        [CTL_KERN, KERN_NPROCS] => Ok(((sched::active_count() as i32).to_le_bytes().to_vec(), false)),
        [CTL_VM, VM_STATS, VM_STATS_FREE_COUNT] => {
            let (_total, free) = memory::pmm_stats_hint();
            Ok(((free as u64).to_le_bytes().to_vec(), false))
        }
        [CTL_VM, VM_STATS, VM_STATS_TOTAL_PAGES] => {
            let (total, _free) = memory::pmm_stats_hint();
            Ok(((total as u64).to_le_bytes().to_vec(), false))
        }
        [CTL_HW, HW_MACHINE] => Ok((cstr_bytes("amd64"), false)),
        [CTL_HW, HW_MODEL] => {
            let snap = apic::snapshot();
            let b = snap.brand_str();
            let text = if b.is_empty() { "x86_64 Generic CPU" } else { b };
            Ok((cstr_bytes(text), false))
        }
        [CTL_HW, HW_NCPU] => {
            let ncpu = acpi::snapshot().cpu_count.max(1) as i32;
            Ok((ncpu.to_le_bytes().to_vec(), false))
        }
        [CTL_HW, HW_BYTEORDER] => Ok((1234i32.to_le_bytes().to_vec(), false)),
        [CTL_HW, HW_PHYSMEM] | [CTL_HW, HW_REALMEM] => {
            let (total, _free) = memory::pmm_stats_hint();
            let bytes = (total as u64).saturating_mul(memory::FRAME_SIZE);
            Ok((bytes.to_le_bytes().to_vec(), false))
        }
        [CTL_HW, HW_USERMEM] => {
            let (_total, free) = memory::pmm_stats_hint();
            let bytes = (free as u64).saturating_mul(memory::FRAME_SIZE);
            Ok((bytes.to_le_bytes().to_vec(), false))
        }
        [CTL_HW, HW_PAGESIZE] => Ok((4096i32.to_le_bytes().to_vec(), false)),
        [CTL_MACHDEP, MACHDEP_TSC_FREQ] => Ok((2_000_000_000u64.to_le_bytes().to_vec(), false)),
        [CTL_MACHDEP, MACHDEP_UPTIME_MS] => Ok((timer::uptime_ms().to_le_bytes().to_vec(), false)),
        _ => Err(-2), // ENOENT
    }
}

fn write_mib_value(mib: &[i32], new_bytes: &[u8]) -> Result<(), i64> {
    match mib {
        [CTL_KERN, KERN_HOSTNAME] => {
            let trimmed_len = new_bytes
                .iter()
                .position(|&b| b == 0)
                .unwrap_or(new_bytes.len())
                .min(63);
            if trimmed_len == 0 {
                return Err(-22);
            }
            without_interrupts(|| {
                let mut st = SYSCTL_STATE.lock();
                st.hostname.fill(0);
                st.hostname[..trimmed_len].copy_from_slice(&new_bytes[..trimmed_len]);
                st.hostname_len = trimmed_len;
            });
            Ok(())
        }
        [CTL_KERN, KERN_HOSTID] => {
            if new_bytes.len() < 4 {
                return Err(-22);
            }
            let val = i32::from_le_bytes(new_bytes[0..4].try_into().unwrap());
            without_interrupts(|| {
                SYSCTL_STATE.lock().hostid = val;
            });
            Ok(())
        }
        _ => Err(-1), // EPERM / read-only MIB node
    }
}

pub fn handle_sysctl(
    pml4: u64,
    name_ptr: u64,
    namelen: u32,
    oldp: u64,
    oldlenp: u64,
    newp: u64,
    newlen: usize,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> i64 {
    if namelen == 0 || namelen > 16 || name_ptr == 0 {
        return -22; // EINVAL
    }

    let name_bytes_len = (namelen as usize) * 4;
    let mut name_raw = alloc::vec![0u8; name_bytes_len];
    if copy_from_user_bytes_in(mapping, Some(allocator), pml4, name_ptr, &mut name_raw).is_err() {
        return -14; // EFAULT
    }

    let mut mib = Vec::with_capacity(namelen as usize);
    for i in 0..(namelen as usize) {
        let v = i32::from_le_bytes(name_raw[i * 4..i * 4 + 4].try_into().unwrap());
        mib.push(v);
    }

    // Special sysctlnametomib lookup node: [CTL_SYSCTL, CTL_SYSCTL_NAME2OID]
    if mib.as_slice() == [CTL_SYSCTL, CTL_SYSCTL_NAME2OID] {
        if newp == 0 || newlen == 0 || newlen > 128 {
            return -22;
        }
        let mut str_buf = alloc::vec![0u8; newlen];
        if copy_from_user_bytes_in(mapping, Some(allocator), pml4, newp, &mut str_buf).is_err() {
            return -14;
        }
        let Ok(name_str) = core::str::from_utf8(&str_buf) else {
            return -22;
        };
        let Some(oid) = name_to_oid(name_str) else {
            return -2; // ENOENT
        };
        let needed_bytes = oid.len() * 4;
        if oldlenp != 0 {
            let mut old_len_raw = [0u8; 8];
            if copy_from_user_bytes_in(mapping, Some(allocator), pml4, oldlenp, &mut old_len_raw)
                .is_err()
            {
                return -14;
            }
            let cap = u64::from_le_bytes(old_len_raw) as usize;
            if copy_to_user_bytes_in(
                mapping,
                Some(allocator),
                pml4,
                oldlenp,
                &(needed_bytes as u64).to_le_bytes(),
            )
            .is_err()
            {
                return -14;
            }
            if oldp != 0 {
                if cap < needed_bytes {
                    return -12; // ENOMEM
                }
                let mut oid_bytes = Vec::with_capacity(needed_bytes);
                for &part in oid {
                    oid_bytes.extend_from_slice(&part.to_le_bytes());
                }
                if copy_to_user_bytes_in(mapping, Some(allocator), pml4, oldp, &oid_bytes).is_err() {
                    return -14;
                }
            }
        }
        return 0;
    }

    let (val_bytes, writable) = match read_mib_value(&mib) {
        Ok(v) => v,
        Err(e) => return e,
    };

    if oldp != 0 && oldlenp == 0 {
        return -22;
    }

    let mut ret_err: Option<i64> = None;
    if oldlenp != 0 {
        let mut old_len_raw = [0u8; 8];
        if copy_from_user_bytes_in(mapping, Some(allocator), pml4, oldlenp, &mut old_len_raw).is_err()
        {
            return -14;
        }
        let user_cap = u64::from_le_bytes(old_len_raw) as usize;
        let full_len = val_bytes.len();
        if copy_to_user_bytes_in(
            mapping,
            Some(allocator),
            pml4,
            oldlenp,
            &(full_len as u64).to_le_bytes(),
        )
        .is_err()
        {
            return -14;
        }
        if oldp != 0 {
            let copy_len = user_cap.min(full_len);
            if copy_len > 0
                && copy_to_user_bytes_in(
                    mapping,
                    Some(allocator),
                    pml4,
                    oldp,
                    &val_bytes[..copy_len],
                )
                .is_err()
            {
                return -14;
            }
            if user_cap < full_len {
                ret_err = Some(-12); // ENOMEM (short read still copies prefix)
            }
        }
    }

    if let Some(e) = ret_err {
        return e;
    }

    if newp != 0 {
        if !writable {
            return -1; // EPERM
        }
        if newlen == 0 || newlen > 128 {
            return -22;
        }
        let mut new_buf = alloc::vec![0u8; newlen];
        if copy_from_user_bytes_in(mapping, Some(allocator), pml4, newp, &mut new_buf).is_err() {
            return -14;
        }
        if let Err(e) = write_mib_value(&mib, &new_buf) {
            return e;
        }
    }

    let _ = String::new();
    0
}
