//! TCP/UDP BSD-like sockets, ICMP Ping, DNS A-record lookup, and HTTP/1.1 GET for Ring 3.

use crate::syscall::{
    syscall1, syscall2, syscall3, syscall4, EAGAIN, SYS_BIND, SYS_CLOSE, SYS_CONNECT,
    SYS_DNS_RESOLVE, SYS_HTTP_GET, SYS_NET_PING, SYS_RECV, SYS_SEND, SYS_SOCKET,
};
use crate::time::sleep_ms;

pub const AF_INET: u64 = 2;
pub const SOCK_STREAM: u64 = 1;
pub const SOCK_DGRAM: u64 = 2;

#[repr(C)]
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq)]
pub struct SockAddrIn {
    pub ip: [u8; 4],
    pub port: u16,
    pub pad: u16,
}

impl SockAddrIn {
    pub const fn new(ip: [u8; 4], port: u16) -> Self {
        Self { ip, port, pad: 0 }
    }
}

/// Sends an ICMP Echo Request to `ip` and returns round-trip time in ms.
pub fn ping(ip: [u8; 4], seq: u16, timeout_ms: u64) -> Result<u64, i64> {
    let packed = u32::from_be_bytes(ip) as u64;
    let res = unsafe { syscall3(SYS_NET_PING, packed, seq as u64, timeout_ms) };
    if res >= 0 {
        Ok(res as u64)
    } else {
        Err(res)
    }
}

/// Resolves an IPv4 A record via AmaryllisOS DNS client (`SYS_DNS_RESOLVE`).
pub fn dns_resolve(host: &str) -> Result<[u8; 4], i64> {
    if host.is_empty() {
        return Err(-22);
    }
    let mut ip = [0u8; 4];
    let res = unsafe {
        syscall3(
            SYS_DNS_RESOLVE,
            host.as_ptr() as u64,
            host.len() as u64,
            ip.as_mut_ptr() as u64,
        )
    };
    if res == 0 {
        Ok(ip)
    } else {
        Err(res)
    }
}

/// Performs an HTTP/1.1 GET request into caller-provided `out` slice and returns bytes received.
pub fn http_get(url: &str, out: &mut [u8]) -> Result<usize, i64> {
    if url.is_empty() || out.is_empty() {
        return Err(-22);
    }
    let res = unsafe {
        syscall4(
            SYS_HTTP_GET,
            url.as_ptr() as u64,
            url.len() as u64,
            out.as_mut_ptr() as u64,
            out.len() as u64,
        )
    };
    if res >= 0 {
        Ok(res as usize)
    } else {
        Err(res)
    }
}

/// Ring 3 TCPv4 client socket backed by kernel `SYS_SOCKET` / `SYS_CONNECT` / `SYS_SEND` / `SYS_RECV`.
pub struct TcpStream {
    fd: i64,
}

impl TcpStream {
    pub fn connect(ip: [u8; 4], port: u16) -> Result<Self, i64> {
        let fd = unsafe { syscall2(SYS_SOCKET, AF_INET, SOCK_STREAM) };
        if fd < 0 {
            return Err(fd);
        }
        let addr = SockAddrIn::new(ip, port);
        let rc = unsafe {
            syscall3(
                SYS_CONNECT,
                fd as u64,
                (&addr as *const SockAddrIn) as u64,
                core::mem::size_of::<SockAddrIn>() as u64,
            )
        };
        if rc < 0 {
            unsafe {
                let _ = syscall1(SYS_CLOSE, fd as u64);
            }
            return Err(rc);
        }
        Ok(Self { fd })
    }

    pub fn write(&self, data: &[u8]) -> Result<usize, i64> {
        if data.is_empty() {
            return Ok(0);
        }
        let n = unsafe {
            syscall4(
                SYS_SEND,
                self.fd as u64,
                data.as_ptr() as u64,
                data.len() as u64,
                0,
            )
        };
        if n >= 0 {
            Ok(n as usize)
        } else {
            Err(n)
        }
    }

    /// Non-blocking single `SYS_RECV` attempt (returns `Err(EAGAIN)` if no data yet, `Ok(0)` on EOF).
    pub fn try_read(&self, buf: &mut [u8]) -> Result<usize, i64> {
        if buf.is_empty() {
            return Ok(0);
        }
        let n = unsafe {
            syscall4(
                SYS_RECV,
                self.fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                0,
            )
        };
        if n >= 0 {
            Ok(n as usize)
        } else {
            Err(n)
        }
    }

    /// Polls cooperatively for data until at least 1 byte arrives, EOF, or `timeout_ms` expires.
    pub fn read_timeout(&self, buf: &mut [u8], timeout_ms: u64) -> Result<usize, i64> {
        let mut waited = 0u64;
        loop {
            match self.try_read(buf) {
                Ok(n) => return Ok(n),
                Err(EAGAIN) => {
                    if waited >= timeout_ms {
                        return Err(EAGAIN);
                    }
                    sleep_ms(10);
                    waited += 10;
                }
                Err(e) => return Err(e),
            }
        }
    }

    pub fn close(mut self) {
        if self.fd >= 0 {
            unsafe {
                let _ = syscall1(SYS_CLOSE, self.fd as u64);
            }
            self.fd = -1;
        }
    }
}

impl Drop for TcpStream {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe {
                let _ = syscall1(SYS_CLOSE, self.fd as u64);
            }
            self.fd = -1;
        }
    }
}

/// Ring 3 UDPv4 datagram socket backed by `SYS_SOCKET` / `SYS_BIND` / `SYS_SEND` / `SYS_RECV`.
pub struct UdpSocket {
    fd: i64,
}

impl UdpSocket {
    pub fn bind(local_port: u16) -> Result<Self, i64> {
        let fd = unsafe { syscall2(SYS_SOCKET, AF_INET, SOCK_DGRAM) };
        if fd < 0 {
            return Err(fd);
        }
        if local_port != 0 {
            let rc = unsafe { syscall2(SYS_BIND, fd as u64, local_port as u64) };
            if rc < 0 {
                unsafe {
                    let _ = syscall1(SYS_CLOSE, fd as u64);
                }
                return Err(rc);
            }
        }
        Ok(Self { fd })
    }

    pub fn send_to(&self, data: &[u8], ip: [u8; 4], port: u16) -> Result<usize, i64> {
        let addr = SockAddrIn::new(ip, port);
        let n = unsafe {
            syscall4(
                SYS_SEND,
                self.fd as u64,
                data.as_ptr() as u64,
                data.len() as u64,
                (&addr as *const SockAddrIn) as u64,
            )
        };
        if n >= 0 {
            Ok(n as usize)
        } else {
            Err(n)
        }
    }

    pub fn recv_from(&self, buf: &mut [u8]) -> Result<(usize, SockAddrIn), i64> {
        let mut from = SockAddrIn::default();
        let n = unsafe {
            syscall4(
                SYS_RECV,
                self.fd as u64,
                buf.as_mut_ptr() as u64,
                buf.len() as u64,
                (&mut from as *mut SockAddrIn) as u64,
            )
        };
        if n >= 0 {
            Ok((n as usize, from))
        } else {
            Err(n)
        }
    }

    pub fn close(mut self) {
        if self.fd >= 0 {
            unsafe {
                let _ = syscall1(SYS_CLOSE, self.fd as u64);
            }
            self.fd = -1;
        }
    }
}

impl Drop for UdpSocket {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe {
                let _ = syscall1(SYS_CLOSE, self.fd as u64);
            }
            self.fd = -1;
        }
    }
}
