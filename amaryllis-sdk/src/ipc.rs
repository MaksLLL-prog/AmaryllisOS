//! Inter-Process Communication (POSIX Pipes & Zero-Copy Shared Memory) for AmaryllisOS Ring 3.

use alloc::vec;
use alloc::vec::Vec;

use crate::syscall::{
    syscall0, syscall1, syscall2, syscall3, syscall6, SYS_BOOTSTRAP_LOOKUP,
    SYS_BOOTSTRAP_REGISTER, SYS_CLOSE, SYS_MACH_MSG, SYS_MACH_PORT_ALLOCATE,
    SYS_MACH_PORT_DEALLOCATE, SYS_MUNMAP, SYS_PIPE, SYS_READ, SYS_SHM_CREATE, SYS_SHM_MAP,
    SYS_SHM_UNMAP, SYS_WRITE,
};

pub struct PipeReader {
    fd: i32,
}

pub struct PipeWriter {
    fd: i32,
}

impl PipeReader {
    #[inline]
    pub const fn raw_fd(&self) -> i32 {
        self.fd
    }

    pub fn read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        let res = unsafe {
            syscall3(
                SYS_READ,
                self.fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
            )
        };
        if res >= 0 {
            Ok(res as usize)
        } else {
            Err(res)
        }
    }
}

impl PipeWriter {
    #[inline]
    pub const fn raw_fd(&self) -> i32 {
        self.fd
    }

    pub fn write(&self, data: &[u8]) -> Result<usize, i64> {
        let res = unsafe {
            syscall3(
                SYS_WRITE,
                self.fd as u64,
                data.as_ptr() as u64,
                data.len() as u64,
            )
        };
        if res >= 0 {
            Ok(res as usize)
        } else {
            Err(res)
        }
    }
}

impl Drop for PipeReader {
    fn drop(&mut self) {
        if self.fd >= 0 {
            let _ = unsafe { syscall1(SYS_CLOSE, self.fd as u64) };
        }
    }
}

impl Drop for PipeWriter {
    fn drop(&mut self) {
        if self.fd >= 0 {
            let _ = unsafe { syscall1(SYS_CLOSE, self.fd as u64) };
        }
    }
}

/// Creates a kernel-buffered POSIX pipe `(PipeReader, PipeWriter)`.
pub fn pipe() -> Result<(PipeReader, PipeWriter), i64> {
    let mut fds = [-1i32; 2];
    let res = unsafe { syscall1(SYS_PIPE, fds.as_mut_ptr() as u64) };
    if res < 0 {
        return Err(res);
    }
    Ok((PipeReader { fd: fds[0] }, PipeWriter { fd: fds[1] }))
}

/// A mapped Shared Memory (`SHM`) region backed by shared physical frames in the kernel.
pub struct SharedMem {
    id: u64,
    vaddr: u64,
    size_bytes: usize,
    writable: bool,
}

impl SharedMem {
    /// Creates or opens a named shared memory region and maps it into the caller's address space.
    pub fn create_or_open(name: &str, size_bytes: usize, writable: bool) -> Result<Self, i64> {
        let id_res = unsafe {
            syscall3(
                SYS_SHM_CREATE,
                name.as_ptr() as u64,
                name.len() as u64,
                size_bytes as u64,
            )
        };
        if id_res < 0 {
            return Err(id_res);
        }
        let id = id_res as u64;
        let vaddr_res = unsafe { syscall2(SYS_SHM_MAP, id, if writable { 1 } else { 0 }) };
        if vaddr_res < 0 {
            return Err(vaddr_res);
        }
        let aligned = ((size_bytes.max(1) + 4095) / 4096) * 4096;
        Ok(Self {
            id,
            vaddr: vaddr_res as u64,
            size_bytes: aligned,
            writable,
        })
    }

    /// Maps an existing SHM object by numeric ID.
    pub fn map_id(id: u64, size_bytes: usize, writable: bool) -> Result<Self, i64> {
        let vaddr_res = unsafe { syscall2(SYS_SHM_MAP, id, if writable { 1 } else { 0 }) };
        if vaddr_res < 0 {
            return Err(vaddr_res);
        }
        let aligned = ((size_bytes.max(1) + 4095) / 4096) * 4096;
        Ok(Self {
            id,
            vaddr: vaddr_res as u64,
            size_bytes: aligned,
            writable,
        })
    }

    #[inline]
    pub const fn id(&self) -> u64 {
        self.id
    }

    #[inline]
    pub const fn size(&self) -> usize {
        self.size_bytes
    }

    pub fn as_bytes(&self) -> &[u8] {
        unsafe { core::slice::from_raw_parts(self.vaddr as *const u8, self.size_bytes) }
    }

    pub fn as_bytes_mut(&mut self) -> Option<&mut [u8]> {
        if !self.writable {
            return None;
        }
        Some(unsafe { core::slice::from_raw_parts_mut(self.vaddr as *mut u8, self.size_bytes) })
    }
}

impl Drop for SharedMem {
    fn drop(&mut self) {
        if self.vaddr != 0 {
            let _ = unsafe { syscall1(SYS_SHM_UNMAP, self.vaddr) };
            self.vaddr = 0;
        }
    }
}

// ============================================================================
// Mach-inspired IPC Ports, Zero-Copy OOL Transfer, and Bootstrap Registry
// ============================================================================

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

pub const MAX_MACH_INLINE_BYTES: usize = 4096;

#[inline]
pub const fn mach_msgh_bits(remote: u32, local: u32, complex: bool) -> u32 {
    (remote & MACH_MSGH_BITS_REMOTE_MASK)
        | ((local & 0xFF) << 8)
        | if complex { MACH_MSGH_BITS_COMPLEX } else { 0 }
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct MachMsgHeader {
    pub bits: u32,
    pub size: u32,
    pub remote_port: MachPortName,
    pub local_port: MachPortName,
    pub id: u32,
    pub reserved: u32,
}

#[repr(C)]
#[derive(Clone, Copy, Default, Debug)]
pub struct MachMsgOolDesc {
    pub addr: u64,
    pub size: u64,
    pub deallocate: bool,
    pub reserved: [u8; 7],
}

/// Descriptor for passing a zero-copy page-aligned memory region out-of-line (`OOL`).
#[derive(Clone, Copy)]
pub struct MachOolBuf<'a> {
    pub data: &'a [u8],
    pub deallocate: bool,
}

/// Received Mach IPC message with RAII cleanup for any transferred reply port or OOL mapping.
pub struct MachReceivedMsg {
    pub header: MachMsgHeader,
    pub inline: Vec<u8>,
    reply_port: Option<MachPort>,
    ool_addr: u64,
    ool_size: u64,
}

impl MachReceivedMsg {
    /// Returns the received inline payload bytes.
    #[inline]
    pub fn inline_bytes(&self) -> &[u8] {
        &self.inline
    }

    /// Borrows the reply port transferred in `header.local_port`, if any.
    #[inline]
    pub fn reply_port(&self) -> Option<&MachPort> {
        self.reply_port.as_ref()
    }

    /// Takes ownership of the reply port transferred in `header.local_port`.
    #[inline]
    pub fn take_reply_port(&mut self) -> Option<MachPort> {
        self.reply_port.take()
    }

    /// Borrows the zero-copy Out-of-Line (`OOL`) mapped buffer, if present.
    #[inline]
    pub fn ool_bytes(&self) -> Option<&[u8]> {
        if self.ool_addr == 0 || self.ool_size == 0 {
            None
        } else {
            Some(unsafe {
                core::slice::from_raw_parts(self.ool_addr as *const u8, self.ool_size as usize)
            })
        }
    }

    /// Returns `(vaddr, size)` and transfers responsibility for `SYS_MUNMAP` to the caller.
    #[inline]
    pub fn take_ool_mapping(&mut self) -> Option<(u64, u64)> {
        if self.ool_addr == 0 || self.ool_size == 0 {
            None
        } else {
            let out = (self.ool_addr, self.ool_size);
            self.ool_addr = 0;
            self.ool_size = 0;
            Some(out)
        }
    }
}

impl Drop for MachReceivedMsg {
    fn drop(&mut self) {
        self.reply_port.take();
        if self.ool_addr != 0 && self.ool_size != 0 {
            let _ = unsafe { syscall2(SYS_MUNMAP, self.ool_addr, self.ool_size) };
            self.ool_addr = 0;
            self.ool_size = 0;
        }
    }
}

/// Safe RAII wrapper over a process-local `MachPortName` right (`Receive`, `Send`, or `SendOnce`).
pub struct MachPort {
    name: MachPortName,
}

impl MachPort {
    /// Allocates a new Mach port with both `Receive` and `Send` rights.
    #[inline]
    pub fn allocate() -> Result<Self, i64> {
        let res = unsafe { syscall0(SYS_MACH_PORT_ALLOCATE) };
        if res > 0 {
            Ok(Self { name: res as MachPortName })
        } else {
            Err(res)
        }
    }

    /// Wraps a raw `MachPortName` returned by another mechanism (transferring ownership).
    #[inline]
    pub const unsafe fn from_raw(name: MachPortName) -> Self {
        Self { name }
    }

    #[inline]
    pub const fn raw(&self) -> MachPortName {
        self.name
    }

    /// Consumes the wrapper without deallocating the kernel right.
    #[inline]
    pub fn into_raw(self) -> MachPortName {
        let name = self.name;
        core::mem::forget(self);
        name
    }

    /// Sends a Mach message to `self` with optional reply port (`MakeSendOnce`),
    /// inline payload (`<= 4096` bytes), and optional zero-copy `OOL` page descriptor.
    pub fn send(
        &self,
        id: u32,
        reply_port: Option<&MachPort>,
        inline: &[u8],
        ool: Option<MachOolBuf<'_>>,
    ) -> Result<(), i64> {
        if inline.len() > MAX_MACH_INLINE_BYTES {
            return Err(-22);
        }
        let hdr_size = core::mem::size_of::<MachMsgHeader>();
        let ool_size = core::mem::size_of::<MachMsgOolDesc>();
        let is_complex = ool.is_some();
        let total = hdr_size + if is_complex { ool_size } else { 0 } + inline.len();
        let mut wire = vec![0u8; total];

        let local = reply_port.map(|p| p.raw()).unwrap_or(MACH_PORT_NULL);
        let local_disp = if local != MACH_PORT_NULL {
            MACH_MSG_TYPE_MAKE_SEND_ONCE
        } else {
            0
        };
        let hdr = MachMsgHeader {
            bits: mach_msgh_bits(MACH_MSG_TYPE_COPY_SEND, local_disp, is_complex),
            size: total as u32,
            remote_port: self.name,
            local_port: local,
            id,
            reserved: 0,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                (&hdr as *const MachMsgHeader) as *const u8,
                wire.as_mut_ptr(),
                hdr_size,
            );
        }
        let mut cursor = hdr_size;
        if let Some(o) = ool {
            let desc = MachMsgOolDesc {
                addr: o.data.as_ptr() as u64,
                size: o.data.len() as u64,
                deallocate: o.deallocate,
                reserved: [0; 7],
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (&desc as *const MachMsgOolDesc) as *const u8,
                    wire.as_mut_ptr().add(cursor),
                    ool_size,
                );
            }
            cursor += ool_size;
        }
        if !inline.is_empty() {
            wire[cursor..cursor + inline.len()].copy_from_slice(inline);
        }

        let res = unsafe {
            syscall6(
                SYS_MACH_MSG,
                wire.as_ptr() as u64,
                MACH_SEND_MSG as u64,
                total as u64,
                0,
                MACH_PORT_NULL as u64,
                0,
            )
        };
        if res >= 0 {
            Ok(())
        } else {
            Err(res)
        }
    }

    /// Sends a one-shot reply to a `SendOnce` port (consuming it or matching server response).
    pub fn send_reply(self, id: u32, inline: &[u8], ool: Option<MachOolBuf<'_>>) -> Result<(), i64> {
        if inline.len() > MAX_MACH_INLINE_BYTES {
            return Err(-22);
        }
        let hdr_size = core::mem::size_of::<MachMsgHeader>();
        let ool_size = core::mem::size_of::<MachMsgOolDesc>();
        let is_complex = ool.is_some();
        let total = hdr_size + if is_complex { ool_size } else { 0 } + inline.len();
        let mut wire = vec![0u8; total];

        let hdr = MachMsgHeader {
            bits: mach_msgh_bits(MACH_MSG_TYPE_MOVE_SEND_ONCE, 0, is_complex),
            size: total as u32,
            remote_port: self.name,
            local_port: MACH_PORT_NULL,
            id,
            reserved: 0,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                (&hdr as *const MachMsgHeader) as *const u8,
                wire.as_mut_ptr(),
                hdr_size,
            );
        }
        let mut cursor = hdr_size;
        if let Some(o) = ool {
            let desc = MachMsgOolDesc {
                addr: o.data.as_ptr() as u64,
                size: o.data.len() as u64,
                deallocate: o.deallocate,
                reserved: [0; 7],
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (&desc as *const MachMsgOolDesc) as *const u8,
                    wire.as_mut_ptr().add(cursor),
                    ool_size,
                );
            }
            cursor += ool_size;
        }
        if !inline.is_empty() {
            wire[cursor..cursor + inline.len()].copy_from_slice(inline);
        }

        let res = unsafe {
            syscall6(
                SYS_MACH_MSG,
                wire.as_ptr() as u64,
                MACH_SEND_MSG as u64,
                total as u64,
                0,
                MACH_PORT_NULL as u64,
                0,
            )
        };
        // Kernel consumes SendOnce even if delivery completed or errored.
        core::mem::forget(self);
        if res >= 0 {
            Ok(())
        } else {
            Err(res)
        }
    }

    /// Receives a message from `self` (`Receive` right). `timeout_ms == Some(0)` polls without blocking.
    pub fn recv(&self, max_inline: usize, timeout_ms: Option<u64>) -> Result<MachReceivedMsg, i64> {
        let hdr_size = core::mem::size_of::<MachMsgHeader>();
        let ool_size = core::mem::size_of::<MachMsgOolDesc>();
        let cap = hdr_size + ool_size + max_inline.min(MAX_MACH_INLINE_BYTES);
        let mut wire = vec![0u8; cap];

        let mut opt = MACH_RCV_MSG;
        let to = if let Some(ms) = timeout_ms {
            opt |= MACH_RCV_TIMEOUT;
            ms
        } else {
            0
        };

        let res = unsafe {
            syscall6(
                SYS_MACH_MSG,
                wire.as_mut_ptr() as u64,
                opt as u64,
                0,
                cap as u64,
                self.name as u64,
                to,
            )
        };
        if res < 0 {
            return Err(res);
        }

        parse_received_wire(&wire)
    }

    /// Atomic Mach RPC (`MACH_SEND_MSG | MACH_RCV_MSG`): sends a request to `self` with an
    /// ephemeral reply port and waits for the reply in one `SYS_MACH_MSG` call.
    pub fn rpc_call(
        &self,
        id: u32,
        inline: &[u8],
        ool: Option<MachOolBuf<'_>>,
        max_reply_inline: usize,
        timeout_ms: Option<u64>,
    ) -> Result<MachReceivedMsg, i64> {
        if inline.len() > MAX_MACH_INLINE_BYTES {
            return Err(-22);
        }
        let reply = MachPort::allocate()?;
        let hdr_size = core::mem::size_of::<MachMsgHeader>();
        let ool_size = core::mem::size_of::<MachMsgOolDesc>();
        let is_complex = ool.is_some();
        let send_len = hdr_size + if is_complex { ool_size } else { 0 } + inline.len();
        let rcv_cap = (hdr_size + ool_size + max_reply_inline.min(MAX_MACH_INLINE_BYTES)).max(send_len);
        let mut wire = vec![0u8; rcv_cap];

        let hdr = MachMsgHeader {
            bits: mach_msgh_bits(MACH_MSG_TYPE_COPY_SEND, MACH_MSG_TYPE_MAKE_SEND_ONCE, is_complex),
            size: send_len as u32,
            remote_port: self.name,
            local_port: reply.raw(),
            id,
            reserved: 0,
        };
        unsafe {
            core::ptr::copy_nonoverlapping(
                (&hdr as *const MachMsgHeader) as *const u8,
                wire.as_mut_ptr(),
                hdr_size,
            );
        }
        let mut cursor = hdr_size;
        if let Some(o) = ool {
            let desc = MachMsgOolDesc {
                addr: o.data.as_ptr() as u64,
                size: o.data.len() as u64,
                deallocate: o.deallocate,
                reserved: [0; 7],
            };
            unsafe {
                core::ptr::copy_nonoverlapping(
                    (&desc as *const MachMsgOolDesc) as *const u8,
                    wire.as_mut_ptr().add(cursor),
                    ool_size,
                );
            }
            cursor += ool_size;
        }
        if !inline.is_empty() {
            wire[cursor..cursor + inline.len()].copy_from_slice(inline);
        }

        let mut opt = MACH_SEND_MSG | MACH_RCV_MSG;
        let to = if let Some(ms) = timeout_ms {
            opt |= MACH_RCV_TIMEOUT;
            ms
        } else {
            0
        };

        let res = unsafe {
            syscall6(
                SYS_MACH_MSG,
                wire.as_mut_ptr() as u64,
                opt as u64,
                send_len as u64,
                rcv_cap as u64,
                reply.raw() as u64,
                to,
            )
        };
        if res < 0 {
            return Err(res);
        }

        parse_received_wire(&wire)
    }
}

impl Drop for MachPort {
    fn drop(&mut self) {
        if self.name != MACH_PORT_NULL && self.name != MACH_PORT_DEAD {
            let _ = unsafe { syscall1(SYS_MACH_PORT_DEALLOCATE, self.name as u64) };
            self.name = MACH_PORT_NULL;
        }
    }
}

fn parse_received_wire(wire: &[u8]) -> Result<MachReceivedMsg, i64> {
    let hdr_size = core::mem::size_of::<MachMsgHeader>();
    let ool_size = core::mem::size_of::<MachMsgOolDesc>();
    if wire.len() < hdr_size {
        return Err(-22);
    }
    let header: MachMsgHeader =
        unsafe { core::ptr::read_unaligned(wire.as_ptr() as *const MachMsgHeader) };
    let total = (header.size as usize).min(wire.len());
    let is_complex = (header.bits & MACH_MSGH_BITS_COMPLEX) != 0;

    let mut cursor = hdr_size;
    let mut ool_addr = 0u64;
    let mut ool_len = 0u64;
    if is_complex && total >= hdr_size + ool_size {
        let desc: MachMsgOolDesc =
            unsafe { core::ptr::read_unaligned(wire.as_ptr().add(hdr_size) as *const MachMsgOolDesc) };
        ool_addr = desc.addr;
        ool_len = desc.size;
        cursor += ool_size;
    }

    let inline = if total > cursor {
        wire[cursor..total].to_vec()
    } else {
        Vec::new()
    };
    let reply_port = if header.local_port != MACH_PORT_NULL && header.local_port != MACH_PORT_DEAD {
        Some(unsafe { MachPort::from_raw(header.local_port) })
    } else {
        None
    };

    Ok(MachReceivedMsg {
        header,
        inline,
        reply_port,
        ool_addr,
        ool_size: ool_len,
    })
}

/// Registers a service port under `name` in the kernel Bootstrap Registry.
#[inline]
pub fn bootstrap_register(name: &str, port: &MachPort) -> Result<(), i64> {
    if name.is_empty() {
        return Err(-22);
    }
    let res = unsafe {
        syscall3(
            SYS_BOOTSTRAP_REGISTER,
            name.as_ptr() as u64,
            name.len() as u64,
            port.raw() as u64,
        )
    };
    if res >= 0 {
        Ok(())
    } else {
        Err(res)
    }
}

/// Looks up a named service port in the kernel Bootstrap Registry, returning a `Send` right.
#[inline]
pub fn bootstrap_look_up(name: &str) -> Result<MachPort, i64> {
    if name.is_empty() {
        return Err(-22);
    }
    let res = unsafe { syscall2(SYS_BOOTSTRAP_LOOKUP, name.as_ptr() as u64, name.len() as u64) };
    if res > 0 {
        Ok(unsafe { MachPort::from_raw(res as MachPortName) })
    } else {
        Err(res)
    }
}
