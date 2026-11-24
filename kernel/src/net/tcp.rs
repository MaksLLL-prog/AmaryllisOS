use crate::interrupts::timer;
use crate::net::{self, checksum, ipv4};
use crate::task::preempt::PreemptMutex;

const TCP_HEADER_LEN: usize = 20;
const TCP_FLAG_FIN: u8 = 0x01;
const TCP_FLAG_SYN: u8 = 0x02;
const TCP_FLAG_RST: u8 = 0x04;
const TCP_FLAG_PSH: u8 = 0x08;
const TCP_FLAG_ACK: u8 = 0x10;

const TCP_RX_RING_CAP: usize = 4096;

pub const MAX_TCP_SOCKETS: usize = 8;

#[derive(Clone, Copy, PartialEq, Eq)]
enum TcpState {
    Closed,
    SynSent,
    Established,
    RemoteFin,
    Reset,
}

#[derive(Clone, Copy)]
struct TcpSocket {
    in_use: bool,
    state: TcpState,
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    seq: u32,
    ack: u32,
    peer_acked_seq: u32,
    rx_buf: [u8; TCP_RX_RING_CAP],
    rx_len: usize,
}

impl TcpSocket {
    const fn empty() -> Self {
        Self {
            in_use: false,
            state: TcpState::Closed,
            local_port: 0,
            remote_ip: [0; 4],
            remote_port: 0,
            seq: 0,
            ack: 0,
            peer_acked_seq: 0,
            rx_buf: [0; TCP_RX_RING_CAP],
            rx_len: 0,
        }
    }
}

static TCP_SOCKETS: PreemptMutex<[TcpSocket; MAX_TCP_SOCKETS]> =
    PreemptMutex::new([const { TcpSocket::empty() }; MAX_TCP_SOCKETS]);

pub fn handle_packet(ip: &ipv4::Ipv4Header, payload: &[u8]) {
    if payload.len() < TCP_HEADER_LEN {
        return;
    }

    let src_port = u16::from_be_bytes([payload[0], payload[1]]);
    let dst_port = u16::from_be_bytes([payload[2], payload[3]]);
    let seg_seq = u32::from_be_bytes([payload[4], payload[5], payload[6], payload[7]]);
    let seg_ack = u32::from_be_bytes([payload[8], payload[9], payload[10], payload[11]]);
    let data_offset = ((payload[12] >> 4) as usize) * 4;
    let flags = payload[13];

    if data_offset < TCP_HEADER_LEN || data_offset > payload.len() {
        return;
    }

    let data = &payload[data_offset..];
    let mut ack_to_send: Option<(u16, [u8; 4], u16, u32, u32, u8)> = None;

    {
        let mut pool = TCP_SOCKETS.lock();
        let Some(sock) = pool.iter_mut().find(|s| {
            s.in_use
                && s.state != TcpState::Closed
                && s.remote_ip == ip.src_ip
                && s.remote_port == src_port
                && s.local_port == dst_port
        }) else {
            return;
        };

        if (flags & TCP_FLAG_RST) != 0 {
            sock.state = TcpState::Reset;
            return;
        }

        if (flags & TCP_FLAG_ACK) != 0 {
            if seg_ack.wrapping_sub(sock.peer_acked_seq) <= 0x1000_0000 {
                sock.peer_acked_seq = seg_ack;
            }
        }

        match sock.state {
            TcpState::SynSent => {
                if (flags & (TCP_FLAG_SYN | TCP_FLAG_ACK)) == (TCP_FLAG_SYN | TCP_FLAG_ACK) {
                    sock.ack = seg_seq.wrapping_add(1);
                    sock.seq = seg_ack;
                    sock.peer_acked_seq = seg_ack;
                    sock.state = TcpState::Established;
                    ack_to_send = Some((
                        sock.local_port,
                        sock.remote_ip,
                        sock.remote_port,
                        sock.seq,
                        sock.ack,
                        TCP_FLAG_ACK,
                    ));
                }
            }
            TcpState::Established => {
                if !data.is_empty() && seg_seq == sock.ack {
                    let avail = TCP_RX_RING_CAP.saturating_sub(sock.rx_len);
                    let to_copy = data.len().min(avail);
                    if to_copy > 0 {
                        let start = sock.rx_len;
                        sock.rx_buf[start..start + to_copy].copy_from_slice(&data[..to_copy]);
                        sock.rx_len += to_copy;
                        sock.ack = sock.ack.wrapping_add(to_copy as u32);
                    }
                    ack_to_send = Some((
                        sock.local_port,
                        sock.remote_ip,
                        sock.remote_port,
                        sock.seq,
                        sock.ack,
                        TCP_FLAG_ACK,
                    ));
                } else if !data.is_empty() && seg_seq != sock.ack {
                    ack_to_send = Some((
                        sock.local_port,
                        sock.remote_ip,
                        sock.remote_port,
                        sock.seq,
                        sock.ack,
                        TCP_FLAG_ACK,
                    ));
                }

                if (flags & TCP_FLAG_FIN) != 0 {
                    sock.ack = sock.ack.wrapping_add(1);
                    sock.state = TcpState::RemoteFin;
                    ack_to_send = Some((
                        sock.local_port,
                        sock.remote_ip,
                        sock.remote_port,
                        sock.seq,
                        sock.ack,
                        TCP_FLAG_ACK,
                    ));
                }
            }
            _ => {}
        }
    }

    if let Some((lp, rip, rp, seq, ack, fl)) = ack_to_send {
        let _ = send_tcp_segment(lp, rip, rp, seq, ack, fl, &[]);
    }
}

fn send_tcp_segment(
    local_port: u16,
    remote_ip: [u8; 4],
    remote_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> Result<(), &'static str> {
    let total_len = TCP_HEADER_LEN + payload.len();
    if total_len > 1460 {
        return Err("TCP segment exceeds MSS");
    }

    let src_ip = net::NET_STATE.lock().ip;
    let mut seg = [0u8; 1480];

    seg[0..2].copy_from_slice(&local_port.to_be_bytes());
    seg[2..4].copy_from_slice(&remote_port.to_be_bytes());
    seg[4..8].copy_from_slice(&seq.to_be_bytes());
    seg[8..12].copy_from_slice(&ack.to_be_bytes());
    seg[12] = 0x50;
    seg[13] = flags;
    seg[14..16].copy_from_slice(&(TCP_RX_RING_CAP as u16).to_be_bytes());
    seg[16..18].copy_from_slice(&[0, 0]);
    seg[18..20].copy_from_slice(&[0, 0]);
    if !payload.is_empty() {
        seg[TCP_HEADER_LEN..total_len].copy_from_slice(payload);
    }

    let csum = checksum::transport_checksum(src_ip, remote_ip, ipv4::PROTO_TCP, &seg[..total_len]);
    seg[16..18].copy_from_slice(&csum.to_be_bytes());

    ipv4::send_packet(remote_ip, ipv4::PROTO_TCP, &seg[..total_len])
}

/// Allocates a kernel TCP socket slot and transmits SYN asynchronously.
pub fn open_and_start_connect(
    remote_ip: [u8; 4],
    remote_port: u16,
) -> Result<usize, &'static str> {
    let tick_seed = timer::uptime_ms() as u32;
    let (handle, local_port, isn) = {
        let mut pool = TCP_SOCKETS.lock();
        let idx = pool
            .iter()
            .position(|s| !s.in_use)
            .ok_or("No free kernel TCP socket slots")?;
        let local_port = 49152u16 + ((idx as u16) * 512) + ((tick_seed as u16) & 0x01FF);
        let isn = 0x1020_3000u32
            .wrapping_add(tick_seed.wrapping_mul(131))
            .wrapping_add((idx as u32) << 16);

        let sock = &mut pool[idx];
        *sock = TcpSocket::empty();
        sock.in_use = true;
        sock.state = TcpState::SynSent;
        sock.local_port = local_port;
        sock.remote_ip = remote_ip;
        sock.remote_port = remote_port;
        sock.seq = isn;
        sock.peer_acked_seq = isn;
        (idx, local_port, isn)
    };

    if let Err(err) = send_tcp_segment(local_port, remote_ip, remote_port, isn, 0, TCP_FLAG_SYN, &[]) {
        let mut pool = TCP_SOCKETS.lock();
        pool[handle] = TcpSocket::empty();
        return Err(err);
    }

    Ok(handle)
}

/// Polls connection handshake status: `Ok(true)` if Established, `Ok(false)` if still connecting.
pub fn poll_connect(handle: usize) -> Result<bool, &'static str> {
    if handle >= MAX_TCP_SOCKETS {
        return Err("Invalid TCP socket handle");
    }
    net::poll_rx();
    let pool = TCP_SOCKETS.lock();
    let sock = &pool[handle];
    if !sock.in_use {
        return Err("TCP socket closed");
    }
    match sock.state {
        TcpState::Established | TcpState::RemoteFin => Ok(true),
        TcpState::SynSent => Ok(false),
        TcpState::Reset => Err("TCP connection refused (RST)"),
        TcpState::Closed => Err("TCP socket closed"),
    }
}

pub fn send_data(handle: usize, data: &[u8]) -> Result<usize, &'static str> {
    if handle >= MAX_TCP_SOCKETS {
        return Err("Invalid TCP socket handle");
    }
    let mut sent = 0usize;
    while sent < data.len() {
        let chunk_len = (data.len() - sent).min(1400);
        let (lp, rip, rp, seq, ack) = {
            let mut pool = TCP_SOCKETS.lock();
            let sock = &mut pool[handle];
            if !sock.in_use || sock.state != TcpState::Established {
                if sent > 0 {
                    return Ok(sent);
                }
                return Err("TCP socket is not connected");
            }
            let s = sock.seq;
            let a = sock.ack;
            sock.seq = sock.seq.wrapping_add(chunk_len as u32);
            (sock.local_port, sock.remote_ip, sock.remote_port, s, a)
        };
        send_tcp_segment(
            lp,
            rip,
            rp,
            seq,
            ack,
            TCP_FLAG_PSH | TCP_FLAG_ACK,
            &data[sent..sent + chunk_len],
        )?;
        sent += chunk_len;
    }
    Ok(sent)
}

/// Non-blocking drain from TCP socket RX ring:
/// - `Ok(n > 0)`: copied `n` bytes
/// - `Ok(0)`: still connected, no data yet
/// - `Err("EOF")`: connection closed/finished and buffer drained
pub fn recv_data(handle: usize, out: &mut [u8]) -> Result<usize, &'static str> {
    if handle >= MAX_TCP_SOCKETS {
        return Err("Invalid TCP socket handle");
    }
    net::poll_rx();
    let mut pool = TCP_SOCKETS.lock();
    let sock = &mut pool[handle];
    if !sock.in_use {
        return Err("EOF");
    }
    if sock.rx_len > 0 {
        let n = sock.rx_len.min(out.len());
        out[..n].copy_from_slice(&sock.rx_buf[..n]);
        if n < sock.rx_len {
            sock.rx_buf.copy_within(n..sock.rx_len, 0);
        }
        sock.rx_len -= n;
        return Ok(n);
    }
    match sock.state {
        TcpState::Established | TcpState::SynSent => Ok(0),
        TcpState::RemoteFin | TcpState::Reset | TcpState::Closed => Err("EOF"),
    }
}

pub fn close_socket(handle: usize) {
    if handle >= MAX_TCP_SOCKETS {
        return;
    }
    let fin_info = {
        let mut pool = TCP_SOCKETS.lock();
        let sock = &mut pool[handle];
        if !sock.in_use {
            return;
        }
        let info = if sock.state == TcpState::Established || sock.state == TcpState::RemoteFin {
            Some((
                sock.local_port,
                sock.remote_ip,
                sock.remote_port,
                sock.seq,
                sock.ack,
            ))
        } else {
            None
        };
        *sock = TcpSocket::empty();
        info
    };

    if let Some((lp, rip, rp, seq, ack)) = fin_info {
        let _ = send_tcp_segment(lp, rip, rp, seq, ack, TCP_FLAG_FIN | TCP_FLAG_ACK, &[]);
    }
}

pub fn http_exchange(
    remote_ip: [u8; 4],
    remote_port: u16,
    request: &[u8],
    on_chunk: &mut dyn FnMut(&[u8]),
) -> Result<usize, &'static str> {
    let handle = open_and_start_connect(remote_ip, remote_port)?;

    // Wait up to 4s for SYN+ACK
    let connect_deadline = timer::uptime_ms().saturating_add(4000);
    loop {
        match poll_connect(handle) {
            Ok(true) => break,
            Ok(false) => {}
            Err(e) => {
                close_socket(handle);
                return Err(e);
            }
        }
        if timer::uptime_ms() >= connect_deadline {
            close_socket(handle);
            return Err("TCP connect timed out");
        }
        timer::sleep_ms(10);
    }

    if let Err(err) = send_data(handle, request) {
        close_socket(handle);
        return Err(err);
    }

    let mut total_received = 0usize;
    let mut last_activity = timer::uptime_ms();
    let max_idle_ms = 2500u64;
    let overall_deadline = timer::uptime_ms().saturating_add(10_000);
    let mut tmp = [0u8; 1024];

    loop {
        match recv_data(handle, &mut tmp) {
            Ok(n) if n > 0 => {
                on_chunk(&tmp[..n]);
                total_received += n;
                last_activity = timer::uptime_ms();
                continue;
            }
            Ok(_) => {}
            Err(_) => break,
        }

        let now = timer::uptime_ms();
        if now >= overall_deadline || (total_received > 0 && now.saturating_sub(last_activity) >= max_idle_ms) {
            break;
        }
        timer::sleep_ms(10);
    }

    close_socket(handle);
    Ok(total_received)
}
