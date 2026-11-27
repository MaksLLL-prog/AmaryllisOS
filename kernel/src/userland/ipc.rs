use alloc::collections::{BTreeMap, VecDeque};
use alloc::string::String;
use alloc::vec::Vec;
use core::ptr::{copy_nonoverlapping, write_bytes};
use spin::Mutex;
use x86_64::instructions::interrupts::without_interrupts;

use crate::memory::vmm::{PhysicalMemoryMapping, PAGE_SIZE};
use crate::memory::PhysicalFrameAllocator;
use crate::task::preempt::PreemptMutex;

pub const MAX_PIPES: usize = 64;
pub const PIPE_BUF_SIZE: usize = 16384;

pub const MAX_SHM_REGIONS: usize = 64;
pub const MAX_SHM_FRAMES: usize = 2048; // 8 MiB max per SHM region (fits 1280x800x4 RGBA frames)

pub const MAX_MACH_PORTS: usize = 256;
pub const MAX_MACH_PORT_QUEUE: usize = 32;
pub const MAX_MACH_INLINE_BYTES: usize = 4096;
pub const MAX_MACH_OOL_FRAMES: usize = 1024; // up to 4 MiB per OOL page transfer

pub type MachPortName = u32;
pub const MACH_PORT_NULL: MachPortName = 0;
pub const MACH_PORT_DEAD: MachPortName = u32::MAX;

pub const MACH_SEND_MSG: u32 = 1 << 0;
pub const MACH_RCV_MSG: u32 = 1 << 1;
pub const MACH_RCV_TIMEOUT: u32 = 1 << 8;

pub const MACH_MSGH_BITS_COMPLEX: u32 = 1 << 31;
pub const MACH_MSGH_BITS_REMOTE_MASK: u32 = 0xFF;
pub const MACH_MSGH_BITS_LOCAL_MASK: u32 = 0xFF00;

pub const MACH_MSG_TYPE_MOVE_RECEIVE: u32 = 16;
pub const MACH_MSG_TYPE_MOVE_SEND: u32 = 17;
pub const MACH_MSG_TYPE_MOVE_SEND_ONCE: u32 = 18;
pub const MACH_MSG_TYPE_COPY_SEND: u32 = 19;
pub const MACH_MSG_TYPE_MAKE_SEND: u32 = 20;
pub const MACH_MSG_TYPE_MAKE_SEND_ONCE: u32 = 21;

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MachMsgHeader {
    pub bits: u32,
    pub size: u32,
    pub remote_port: u32,
    pub local_port: u32,
    pub id: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
pub struct MachMsgOolDesc {
    pub addr: u64,
    pub size: u64,
    pub deallocate: bool,
    pub reserved: [u8; 7],
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum PortRightKind {
    Receive,
    Send,
    SendOnce,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PortRightEntry {
    pub name: MachPortName,
    pub port_id: u64,
    pub right: PortRightKind,
    pub also_send: bool,
}

pub struct MachOolPayload {
    pub frames: Vec<u64>,
    pub size_bytes: u64,
}

pub struct MachKernelMsg {
    pub bits: u32,
    pub id: u32,
    pub reply_port_id: u64,
    pub reply_right: Option<PortRightKind>,
    pub inline_data: Vec<u8>,
    pub ool: Option<MachOolPayload>,
}

struct MachPort {
    id: u64,
    queue: VecDeque<MachKernelMsg>,
    send_rights: usize,
    send_once_rights: usize,
    receive_rights: usize,
    dead: bool,
}

struct MachPortTable {
    next_id: u64,
    ports: BTreeMap<u64, MachPort>,
}

impl MachPortTable {
    const fn new() -> Self {
        Self {
            next_id: 1,
            ports: BTreeMap::new(),
        }
    }
}

static MACH_PORTS: PreemptMutex<MachPortTable> = PreemptMutex::new(MachPortTable::new());
static BOOTSTRAP_SERVICES: PreemptMutex<BTreeMap<String, u64>> =
    PreemptMutex::new(BTreeMap::new());

#[derive(Clone)]
struct PipeSlot {
    in_use: bool,
    buf: [u8; PIPE_BUF_SIZE],
    head: usize,
    tail: usize,
    len: usize,
    readers: usize,
    writers: usize,
}

impl PipeSlot {
    const EMPTY: Self = Self {
        in_use: false,
        buf: [0; PIPE_BUF_SIZE],
        head: 0,
        tail: 0,
        len: 0,
        readers: 0,
        writers: 0,
    };
}

struct PipeTable {
    slots: [PipeSlot; MAX_PIPES],
}

impl PipeTable {
    const fn new() -> Self {
        Self {
            slots: [const { PipeSlot::EMPTY }; MAX_PIPES],
        }
    }
}

static PIPES: Mutex<PipeTable> = Mutex::new(PipeTable::new());

struct ShmRegion {
    id: u64,
    name: String,
    size_bytes: usize,
    frames: Vec<u64>,
    ref_count: usize,
}

struct ShmTable {
    next_id: u64,
    regions: Vec<ShmRegion>,
}

impl ShmTable {
    const fn new() -> Self {
        Self {
            next_id: 1,
            regions: Vec::new(),
        }
    }
}

static SHMS: Mutex<ShmTable> = Mutex::new(ShmTable::new());

#[derive(Clone)]
pub struct ShmInfoItem {
    pub id: u64,
    pub name: String,
    pub size_bytes: usize,
    pub pages: usize,
    pub ref_count: usize,
}

#[derive(Clone)]
pub struct IpcSnapshot {
    pub active_pipes: usize,
    pub buffered_pipe_bytes: usize,
    pub shm_regions: Vec<ShmInfoItem>,
    pub total_shm_bytes: usize,
    pub mach_ports: usize,
    pub mach_queued_msgs: usize,
    pub bootstrap_services: usize,
}

pub fn create_pipe() -> Result<usize, i64> {
    without_interrupts(|| {
        let mut table = PIPES.lock();
        let Some(idx) = table.slots.iter().position(|s| !s.in_use) else {
            return Err(-24); // EMFILE
        };
        let slot = &mut table.slots[idx];
        slot.in_use = true;
        slot.head = 0;
        slot.tail = 0;
        slot.len = 0;
        slot.readers = 1;
        slot.writers = 1;
        Ok(idx)
    })
}

pub fn retain_pipe_end(pipe_id: usize, is_writer: bool) {
    without_interrupts(|| {
        let mut table = PIPES.lock();
        if let Some(slot) = table.slots.get_mut(pipe_id) {
            if slot.in_use {
                if is_writer {
                    slot.writers = slot.writers.saturating_add(1);
                } else {
                    slot.readers = slot.readers.saturating_add(1);
                }
            }
        }
    });
}

pub fn close_pipe_end(pipe_id: usize, is_writer: bool) {
    without_interrupts(|| {
        let mut table = PIPES.lock();
        if let Some(slot) = table.slots.get_mut(pipe_id) {
            if !slot.in_use {
                return;
            }
            if is_writer {
                slot.writers = slot.writers.saturating_sub(1);
            } else {
                slot.readers = slot.readers.saturating_sub(1);
            }
            if slot.readers == 0 && slot.writers == 0 {
                slot.in_use = false;
                slot.head = 0;
                slot.tail = 0;
                slot.len = 0;
            }
        }
    });
}

pub fn read_pipe(pipe_id: usize, out: &mut [u8]) -> Result<usize, i64> {
    if out.is_empty() {
        return Ok(0);
    }
    without_interrupts(|| {
        let mut table = PIPES.lock();
        let Some(slot) = table.slots.get_mut(pipe_id) else {
            return Err(-9); // EBADF
        };
        if !slot.in_use {
            return Err(-9);
        }
        if slot.len == 0 {
            if slot.writers == 0 {
                return Ok(0); // EOF
            }
            return Err(-11); // EAGAIN
        }
        let n = out.len().min(slot.len);
        for dst in out.iter_mut().take(n) {
            *dst = slot.buf[slot.tail];
            slot.tail = (slot.tail + 1) % PIPE_BUF_SIZE;
        }
        slot.len -= n;
        Ok(n)
    })
}

pub fn write_pipe(pipe_id: usize, data: &[u8]) -> Result<usize, i64> {
    if data.is_empty() {
        return Ok(0);
    }
    without_interrupts(|| {
        let mut table = PIPES.lock();
        let Some(slot) = table.slots.get_mut(pipe_id) else {
            return Err(-9);
        };
        if !slot.in_use {
            return Err(-9);
        }
        if slot.readers == 0 {
            return Err(-32); // EPIPE
        }
        let free = PIPE_BUF_SIZE.saturating_sub(slot.len);
        if free == 0 {
            return Err(-11); // EAGAIN
        }
        let n = data.len().min(free);
        for &b in data.iter().take(n) {
            slot.buf[slot.head] = b;
            slot.head = (slot.head + 1) % PIPE_BUF_SIZE;
        }
        slot.len += n;
        Ok(n)
    })
}

pub fn pipe_status(pipe_id: usize) -> Option<(usize, usize, usize, usize)> {
    without_interrupts(|| {
        let table = PIPES.lock();
        let slot = table.slots.get(pipe_id)?;
        if !slot.in_use {
            return None;
        }
        let readable = slot.len;
        let writable = PIPE_BUF_SIZE.saturating_sub(slot.len);
        Some((readable, writable, slot.readers, slot.writers))
    })
}

pub fn create_or_open_shm(
    name: &str,
    requested_size: usize,
    mapping: &PhysicalMemoryMapping,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<(u64, usize), i64> {
    let trimmed = name.trim();
    let short_name = if trimmed.len() > 48 {
        &trimmed[..48]
    } else {
        trimmed
    };

    if !short_name.is_empty() {
        let existing = without_interrupts(|| {
            let mut shms = SHMS.lock();
            if let Some(reg) = shms.regions.iter_mut().find(|r| r.name == short_name) {
                reg.ref_count = reg.ref_count.saturating_add(1);
                Some((reg.id, reg.size_bytes))
            } else {
                None
            }
        });
        if let Some(hit) = existing {
            return Ok(hit);
        }
    }

    if requested_size == 0 {
        return Err(-22); // EINVAL
    }
    let page_count = (requested_size + PAGE_SIZE as usize - 1) / PAGE_SIZE as usize;
    if page_count == 0 || page_count > MAX_SHM_FRAMES {
        return Err(-12); // ENOMEM
    }

    let can_create = without_interrupts(|| SHMS.lock().regions.len() < MAX_SHM_REGIONS);
    if !can_create {
        return Err(-12);
    }

    let mut frames = Vec::with_capacity(page_count);
    for _ in 0..page_count {
        let Some(frame) = allocator.allocate_frame() else {
            for f in frames {
                let _ = allocator.deallocate_frame(f);
            }
            return Err(-12);
        };
        let Some(hhdm) = mapping.translate_frame(frame) else {
            let _ = allocator.deallocate_frame(frame);
            for f in frames {
                let _ = allocator.deallocate_frame(f);
            }
            return Err(-14);
        };
        unsafe {
            write_bytes(hhdm.as_u64() as *mut u8, 0, PAGE_SIZE as usize);
        }
        frames.push(frame);
    }

    let aligned_size = page_count * PAGE_SIZE as usize;
    let id = without_interrupts(|| {
        let mut shms = SHMS.lock();
        let id = shms.next_id;
        shms.next_id = shms.next_id.saturating_add(1);
        shms.regions.push(ShmRegion {
            id,
            name: String::from(short_name),
            size_bytes: aligned_size,
            frames,
            ref_count: 1,
        });
        id
    });

    Ok((id, aligned_size))
}

pub fn retain_shm_frames(shm_id: u64) -> Option<Vec<u64>> {
    without_interrupts(|| {
        let mut shms = SHMS.lock();
        let reg = shms.regions.iter_mut().find(|r| r.id == shm_id)?;
        reg.ref_count = reg.ref_count.saturating_add(1);
        Some(reg.frames.clone())
    })
}

pub fn shm_size_bytes(shm_id: u64) -> Option<usize> {
    without_interrupts(|| {
        let shms = SHMS.lock();
        shms.regions
            .iter()
            .find(|r| r.id == shm_id)
            .map(|r| r.size_bytes)
    })
}

pub fn release_shm(shm_id: u64, allocator: &mut PhysicalFrameAllocator<'_>) {
    let freed_frames = without_interrupts(|| {
        let mut shms = SHMS.lock();
        if let Some(pos) = shms.regions.iter().position(|r| r.id == shm_id) {
            shms.regions[pos].ref_count = shms.regions[pos].ref_count.saturating_sub(1);
            if shms.regions[pos].ref_count == 0 {
                let reg = shms.regions.swap_remove(pos);
                return Some(reg.frames);
            }
        }
        None
    });

    if let Some(frames) = freed_frames {
        for f in frames {
            let _ = allocator.deallocate_frame(f);
        }
    }
}

/// Zero-copy read of an SHM region row directly from physical frames mapped in HHDM
/// into `dst_row` (in `0xAARRGGBB` / `u32` format with opaque alpha set).
pub fn read_shm_pixel_row(
    shm_id: u64,
    mapping: &PhysicalMemoryMapping,
    byte_offset: usize,
    dst_pixels: &mut [u32],
) -> bool {
    let needed_bytes = dst_pixels.len() * 4;
    if needed_bytes == 0 {
        return true;
    }

    without_interrupts(|| {
        let shms = SHMS.lock();
        let Some(reg) = shms.regions.iter().find(|r| r.id == shm_id) else {
            return false;
        };
        if byte_offset.saturating_add(needed_bytes) > reg.size_bytes {
            return false;
        }

        let mut copied_bytes = 0usize;
        let dst_bytes = unsafe {
            core::slice::from_raw_parts_mut(dst_pixels.as_mut_ptr() as *mut u8, needed_bytes)
        };
        while copied_bytes < needed_bytes {
            let cur = byte_offset + copied_bytes;
            let page_idx = cur / (PAGE_SIZE as usize);
            let page_off = cur % (PAGE_SIZE as usize);
            let step = (needed_bytes - copied_bytes).min(PAGE_SIZE as usize - page_off);
            let Some(&phys) = reg.frames.get(page_idx) else {
                return false;
            };
            let Some(hhdm) = mapping.translate_frame(phys) else {
                return false;
            };
            unsafe {
                copy_nonoverlapping(
                    (hhdm.as_u64() + page_off as u64) as *const u8,
                    dst_bytes.as_mut_ptr().add(copied_bytes),
                    step,
                );
            }
            copied_bytes += step;
        }
        for px in dst_pixels.iter_mut() {
            *px |= 0xFF00_0000;
        }
        true
    })
}

/// Single-lock SHM rectangle blit from HHDM physical frames directly into a compositor buffer.
pub fn blit_shm_rect(
    shm_id: u64,
    mapping: &PhysicalMemoryMapping,
    src_stride: usize,
    off_x: usize,
    off_y: usize,
    row_w: usize,
    vis_h: usize,
    dst_buf: &mut [u32],
    dst_stride: usize,
    dst_x: usize,
    dst_y: usize,
) -> bool {
    if row_w == 0 || vis_h == 0 || src_stride == 0 || dst_stride == 0 {
        return true;
    }
    let Some(needed_row_bytes) = row_w.checked_mul(4) else {
        return false;
    };

    without_interrupts(|| {
        let shms = SHMS.lock();
        let Some(reg) = shms.regions.iter().find(|r| r.id == shm_id) else {
            return false;
        };

        for row in 0..vis_h {
            let src_idx = match (off_y + row).checked_mul(src_stride).and_then(|v| v.checked_add(off_x)) {
                Some(v) => v,
                None => return false,
            };
            let byte_offset = match src_idx.checked_mul(4) {
                Some(v) => v,
                None => return false,
            };
            if byte_offset.saturating_add(needed_row_bytes) > reg.size_bytes {
                return false;
            }

            let dst_start = (dst_y + row) * dst_stride + dst_x;
            if dst_start + row_w > dst_buf.len() {
                return false;
            }
            let dst_pixels = &mut dst_buf[dst_start..dst_start + row_w];
            let dst_bytes = unsafe {
                core::slice::from_raw_parts_mut(dst_pixels.as_mut_ptr() as *mut u8, needed_row_bytes)
            };

            let mut copied_bytes = 0usize;
            while copied_bytes < needed_row_bytes {
                let cur = byte_offset + copied_bytes;
                let page_idx = cur / (PAGE_SIZE as usize);
                let page_off = cur % (PAGE_SIZE as usize);
                let step = (needed_row_bytes - copied_bytes).min(PAGE_SIZE as usize - page_off);
                let Some(&phys) = reg.frames.get(page_idx) else {
                    return false;
                };
                let Some(hhdm) = mapping.translate_frame(phys) else {
                    return false;
                };
                unsafe {
                    copy_nonoverlapping(
                        (hhdm.as_u64() + page_off as u64) as *const u8,
                        dst_bytes.as_mut_ptr().add(copied_bytes),
                        step,
                    );
                }
                copied_bytes += step;
            }
            for px in dst_pixels.iter_mut() {
                *px |= 0xFF00_0000;
            }
        }
        true
    })
}

pub fn snapshot_ipc() -> IpcSnapshot {
    without_interrupts(|| {
        let pipes = PIPES.lock();
        let mut active_pipes = 0usize;
        let mut buffered_pipe_bytes = 0usize;
        for s in &pipes.slots {
            if s.in_use {
                active_pipes += 1;
                buffered_pipe_bytes += s.len;
            }
        }
        drop(pipes);

        let shms = SHMS.lock();
        let mut shm_regions = Vec::with_capacity(shms.regions.len());
        let mut total_shm_bytes = 0usize;
        for reg in &shms.regions {
            total_shm_bytes += reg.size_bytes;
            shm_regions.push(ShmInfoItem {
                id: reg.id,
                name: reg.name.clone(),
                size_bytes: reg.size_bytes,
                pages: reg.frames.len(),
                ref_count: reg.ref_count,
            });
        }
        drop(shms);

        let mach = MACH_PORTS.lock();
        let mach_ports = mach.ports.len();
        let mach_queued_msgs = mach.ports.values().map(|p| p.queue.len()).sum();
        drop(mach);

        let bootstrap_services = BOOTSTRAP_SERVICES.lock().len();

        IpcSnapshot {
            active_pipes,
            buffered_pipe_bytes,
            shm_regions,
            total_shm_bytes,
            mach_ports,
            mach_queued_msgs,
            bootstrap_services,
        }
    })
}

pub fn mach_port_allocate() -> Result<u64, i64> {
    without_interrupts(|| {
        let mut table = MACH_PORTS.lock();
        if table.ports.len() >= MAX_MACH_PORTS {
            return Err(-12); // ENOMEM
        }
        let id = table.next_id.max(1);
        table.next_id = id.saturating_add(1);
        table.ports.insert(
            id,
            MachPort {
                id,
                queue: VecDeque::new(),
                send_rights: 1,
                send_once_rights: 0,
                receive_rights: 1,
                dead: false,
            },
        );
        Ok(id)
    })
}

pub fn mach_retain_right(port_id: u64, right: PortRightKind, also_send: bool) -> bool {
    without_interrupts(|| {
        let mut table = MACH_PORTS.lock();
        let Some(port) = table.ports.get_mut(&port_id) else {
            return false;
        };
        match right {
            PortRightKind::Receive => {
                if port.dead {
                    return false;
                }
                port.receive_rights = port.receive_rights.saturating_add(1);
                if also_send {
                    port.send_rights = port.send_rights.saturating_add(1);
                }
            }
            PortRightKind::Send => {
                port.send_rights = port.send_rights.saturating_add(1);
            }
            PortRightKind::SendOnce => {
                port.send_once_rights = port.send_once_rights.saturating_add(1);
            }
        }
        true
    })
}

pub fn mach_port_is_dead(port_id: u64) -> bool {
    without_interrupts(|| {
        let table = MACH_PORTS.lock();
        table.ports.get(&port_id).map(|p| p.dead).unwrap_or(true)
    })
}

pub fn mach_port_has_queued_msg(port_id: u64) -> bool {
    without_interrupts(|| {
        let table = MACH_PORTS.lock();
        table
            .ports
            .get(&port_id)
            .map(|p| !p.queue.is_empty())
            .unwrap_or(false)
    })
}

pub fn mach_send_msg(
    port_id: u64,
    consume_send_once: bool,
    msg: MachKernelMsg,
) -> Result<(), (i64, MachKernelMsg)> {
    without_interrupts(|| {
        let mut table = MACH_PORTS.lock();
        let Some(port) = table.ports.get_mut(&port_id) else {
            return Err((-32, msg)); // EPIPE (dead/invalid port)
        };
        if consume_send_once {
            port.send_once_rights = port.send_once_rights.saturating_sub(1);
        }
        if port.dead {
            return Err((-32, msg)); // EPIPE / MACH_SEND_INVALID_DEST
        }
        if port.queue.len() >= MAX_MACH_PORT_QUEUE {
            return Err((-11, msg)); // EAGAIN
        }
        port.queue.push_back(msg);
        Ok(())
    })
}

pub fn mach_try_recv_msg(port_id: u64) -> Result<MachKernelMsg, i64> {
    without_interrupts(|| {
        let mut table = MACH_PORTS.lock();
        let Some(port) = table.ports.get_mut(&port_id) else {
            return Err(-32); // EPIPE
        };
        if let Some(msg) = port.queue.pop_front() {
            return Ok(msg);
        }
        if port.dead {
            return Err(-32);
        }
        Err(-11) // EAGAIN
    })
}

pub fn mach_requeue_front_msg(port_id: u64, msg: MachKernelMsg) -> Result<(), MachKernelMsg> {
    without_interrupts(|| {
        let mut table = MACH_PORTS.lock();
        let Some(port) = table.ports.get_mut(&port_id) else {
            return Err(msg);
        };
        if port.dead || port.queue.len() >= MAX_MACH_PORT_QUEUE {
            return Err(msg);
        }
        port.queue.push_front(msg);
        Ok(())
    })
}

fn release_right_inner(
    port_id: u64,
    right: PortRightKind,
    also_send: bool,
    drained_out: &mut Vec<MachKernelMsg>,
) -> bool {
    let became_dead = without_interrupts(|| {
        let mut table = MACH_PORTS.lock();
        let Some(port) = table.ports.get_mut(&port_id) else {
            return false;
        };
        match right {
            PortRightKind::Receive => {
                port.receive_rights = port.receive_rights.saturating_sub(1);
                if also_send {
                    port.send_rights = port.send_rights.saturating_sub(1);
                }
            }
            PortRightKind::Send => {
                port.send_rights = port.send_rights.saturating_sub(1);
            }
            PortRightKind::SendOnce => {
                port.send_once_rights = port.send_once_rights.saturating_sub(1);
            }
        }

        let mut died = false;
        if port.receive_rights == 0 && !port.dead {
            port.dead = true;
            died = true;
            while let Some(m) = port.queue.pop_front() {
                drained_out.push(m);
            }
        }

        if port.receive_rights == 0 && port.send_rights == 0 && port.send_once_rights == 0 {
            table.ports.remove(&port_id);
        }
        died
    });

    if became_dead {
        let mut evicted_send_rights = 0usize;
        without_interrupts(|| {
            let mut bs = BOOTSTRAP_SERVICES.lock();
            bs.retain(|_, &mut pid| {
                if pid == port_id {
                    evicted_send_rights += 1;
                    false
                } else {
                    true
                }
            });
        });
        for _ in 0..evicted_send_rights {
            let _ = release_right_inner(port_id, PortRightKind::Send, false, drained_out);
        }
    }

    became_dead
}

pub fn mach_release_port_id_right(
    port_id: u64,
    right: PortRightKind,
    also_send: bool,
    allocator: &mut PhysicalFrameAllocator<'_>,
) {
    let mut pending_msgs: Vec<MachKernelMsg> = Vec::new();
    let _ = release_right_inner(port_id, right, also_send, &mut pending_msgs);

    while let Some(msg) = pending_msgs.pop() {
        if let Some(ool) = msg.ool {
            for frame in ool.frames {
                let _ = allocator.deallocate_frame(frame);
            }
        }
        if msg.reply_port_id != 0 {
            if let Some(rr) = msg.reply_right {
                let _ = release_right_inner(msg.reply_port_id, rr, false, &mut pending_msgs);
            }
        }
    }
}

pub fn mach_drop_msg_resources(
    msg: MachKernelMsg,
    allocator: &mut PhysicalFrameAllocator<'_>,
) {
    if let Some(ool) = msg.ool {
        for frame in ool.frames {
            let _ = allocator.deallocate_frame(frame);
        }
    }
    if msg.reply_port_id != 0 {
        if let Some(rr) = msg.reply_right {
            mach_release_port_id_right(msg.reply_port_id, rr, false, allocator);
        }
    }
}

pub fn bootstrap_register_port(
    name: &str,
    port_id: u64,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<(), i64> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err(-22); // EINVAL
    }

    let ok = without_interrupts(|| {
        let mut table = MACH_PORTS.lock();
        let Some(port) = table.ports.get_mut(&port_id) else {
            return Err(-32);
        };
        if port.dead {
            return Err(-32);
        }
        port.send_rights = port.send_rights.saturating_add(1);
        Ok(())
    });
    ok?;

    let prev = without_interrupts(|| {
        let mut bs = BOOTSTRAP_SERVICES.lock();
        bs.insert(String::from(trimmed), port_id)
    });
    if let Some(old_port_id) = prev {
        mach_release_port_id_right(old_port_id, PortRightKind::Send, false, allocator);
    }
    Ok(())
}

pub fn bootstrap_lookup_port(name: &str) -> Result<u64, i64> {
    let trimmed = name.trim();
    if trimmed.is_empty() || trimmed.len() > 64 {
        return Err(-22);
    }
    let port_id = without_interrupts(|| BOOTSTRAP_SERVICES.lock().get(trimmed).copied())
        .ok_or(-2)?; // ENOENT

    let retained = without_interrupts(|| {
        let mut table = MACH_PORTS.lock();
        let Some(port) = table.ports.get_mut(&port_id) else {
            return Err(-32);
        };
        if port.dead {
            return Err(-32);
        }
        port.send_rights = port.send_rights.saturating_add(1);
        Ok(port.id)
    });

    if retained.is_err() {
        let _ = without_interrupts(|| BOOTSTRAP_SERVICES.lock().remove(trimmed));
    }
    retained
}
