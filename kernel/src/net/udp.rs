use crate::net::{checksum, dhcp, ethernet, ipv4, NET_STATE};
use crate::task::preempt::PreemptMutex;

const UDP_HEADER_LEN: usize = 8;
const MAX_UDP_RX_PAYLOAD: usize = 1024;

struct UdpRxMailbox {
    listening_port: u16,
    has_packet: bool,
    src_ip: [u8; 4],
    src_port: u16,
    len: usize,
    data: [u8; MAX_UDP_RX_PAYLOAD],
}

impl UdpRxMailbox {
    const fn empty() -> Self {
        Self {
            listening_port: 0,
            has_packet: false,
            src_ip: [0; 4],
            src_port: 0,
            len: 0,
            data: [0; MAX_UDP_RX_PAYLOAD],
        }
    }
}

static UDP_MAILBOX: PreemptMutex<UdpRxMailbox> = PreemptMutex::new(UdpRxMailbox::empty());

pub const MAX_UDP_SOCKETS: usize = 4;

#[derive(Clone, Copy)]
struct UdpSocketSlot {
    in_use: bool,
    bound_port: u16,
    has_packet: bool,
    src_ip: [u8; 4],
    src_port: u16,
    len: usize,
    data: [u8; MAX_UDP_RX_PAYLOAD],
}

impl UdpSocketSlot {
    const fn empty() -> Self {
        Self {
            in_use: false,
            bound_port: 0,
            has_packet: false,
            src_ip: [0; 4],
            src_port: 0,
            len: 0,
            data: [0; MAX_UDP_RX_PAYLOAD],
        }
    }
}

static UDP_SOCKETS: PreemptMutex<[UdpSocketSlot; MAX_UDP_SOCKETS]> =
    PreemptMutex::new([const { UdpSocketSlot::empty() }; MAX_UDP_SOCKETS]);

pub fn alloc_socket() -> Result<(usize, u16), &'static str> {
    let mut pool = UDP_SOCKETS.lock();
    let idx = pool
        .iter()
        .position(|s| !s.in_use)
        .ok_or("No free UDP socket slots")?;
    let port = 52000u16 + (idx as u16) * 32;
    let slot = &mut pool[idx];
    *slot = UdpSocketSlot::empty();
    slot.in_use = true;
    slot.bound_port = port;
    Ok((idx, port))
}

pub fn set_bound_port(handle: usize, port: u16) -> Result<(), &'static str> {
    if handle >= MAX_UDP_SOCKETS {
        return Err("Invalid UDP handle");
    }
    let mut pool = UDP_SOCKETS.lock();
    if !pool[handle].in_use {
        return Err("UDP socket closed");
    }
    pool[handle].bound_port = port;
    pool[handle].has_packet = false;
    pool[handle].len = 0;
    Ok(())
}

pub fn get_bound_port(handle: usize) -> Option<u16> {
    if handle >= MAX_UDP_SOCKETS {
        return None;
    }
    let pool = UDP_SOCKETS.lock();
    if pool[handle].in_use {
        Some(pool[handle].bound_port)
    } else {
        None
    }
}

pub fn recv_socket(handle: usize, out: &mut [u8]) -> Option<([u8; 4], u16, usize)> {
    if handle >= MAX_UDP_SOCKETS {
        return None;
    }
    crate::net::poll_rx();
    let mut pool = UDP_SOCKETS.lock();
    let slot = &mut pool[handle];
    if !slot.in_use || !slot.has_packet {
        return None;
    }
    let n = slot.len.min(out.len());
    out[..n].copy_from_slice(&slot.data[..n]);
    let res = (slot.src_ip, slot.src_port, n);
    slot.has_packet = false;
    slot.len = 0;
    Some(res)
}

pub fn free_socket(handle: usize) {
    if handle < MAX_UDP_SOCKETS {
        UDP_SOCKETS.lock()[handle] = UdpSocketSlot::empty();
    }
}

pub fn bind_port(port: u16) {
    let mut box_state = UDP_MAILBOX.lock();
    box_state.listening_port = port;
    box_state.has_packet = false;
    box_state.len = 0;
}

pub fn unbind_port() {
    let mut box_state = UDP_MAILBOX.lock();
    box_state.listening_port = 0;
    box_state.has_packet = false;
    box_state.len = 0;
}

pub fn take_received(out: &mut [u8]) -> Option<([u8; 4], u16, usize)> {
    let mut box_state = UDP_MAILBOX.lock();
    if !box_state.has_packet {
        return None;
    }
    let n = box_state.len.min(out.len());
    out[..n].copy_from_slice(&box_state.data[..n]);
    let res = (box_state.src_ip, box_state.src_port, n);
    box_state.has_packet = false;
    box_state.len = 0;
    Some(res)
}

pub fn handle_packet(ip: &ipv4::Ipv4Header, payload: &[u8]) {
    if payload.len() < UDP_HEADER_LEN {
        return;
    }

    let src_port = u16::from_be_bytes([payload[0], payload[1]]);
    let dst_port = u16::from_be_bytes([payload[2], payload[3]]);
    let udp_len = u16::from_be_bytes([payload[4], payload[5]]) as usize;
    if udp_len < UDP_HEADER_LEN || udp_len > payload.len() {
        return;
    }

    let data = &payload[UDP_HEADER_LEN..udp_len];

    if dst_port == dhcp::DHCP_CLIENT_PORT && src_port == dhcp::DHCP_SERVER_PORT {
        dhcp::handle_packet(data);
        return;
    }

    {
        let mut pool = UDP_SOCKETS.lock();
        if let Some(slot) = pool
            .iter_mut()
            .find(|s| s.in_use && s.bound_port != 0 && s.bound_port == dst_port && !s.has_packet)
        {
            let n = data.len().min(MAX_UDP_RX_PAYLOAD);
            slot.data[..n].copy_from_slice(&data[..n]);
            slot.src_ip = ip.src_ip;
            slot.src_port = src_port;
            slot.len = n;
            slot.has_packet = true;
            return;
        }
    }

    let mut box_state = UDP_MAILBOX.lock();
    if box_state.listening_port != 0
        && box_state.listening_port == dst_port
        && !box_state.has_packet
    {
        let n = data.len().min(MAX_UDP_RX_PAYLOAD);
        box_state.data[..n].copy_from_slice(&data[..n]);
        box_state.src_ip = ip.src_ip;
        box_state.src_port = src_port;
        box_state.len = n;
        box_state.has_packet = true;
    }
}

pub fn send_datagram(
    src_port: u16,
    dst_ip: [u8; 4],
    dst_port: u16,
    payload: &[u8],
) -> Result<(), &'static str> {
    if UDP_HEADER_LEN + payload.len() > 1472 {
        return Err("UDP payload exceeds IPv4 MTU");
    }

    let src_ip = NET_STATE.lock().ip;
    let total_len = UDP_HEADER_LEN + payload.len();
    let mut seg = [0u8; 1480];

    seg[0..2].copy_from_slice(&src_port.to_be_bytes());
    seg[2..4].copy_from_slice(&dst_port.to_be_bytes());
    seg[4..6].copy_from_slice(&(total_len as u16).to_be_bytes());
    seg[6..8].copy_from_slice(&[0, 0]);
    seg[UDP_HEADER_LEN..total_len].copy_from_slice(payload);

    let mut csum = checksum::transport_checksum(src_ip, dst_ip, ipv4::PROTO_UDP, &seg[..total_len]);
    if csum == 0 {
        csum = 0xFFFF;
    }
    seg[6..8].copy_from_slice(&csum.to_be_bytes());

    ipv4::send_packet(dst_ip, ipv4::PROTO_UDP, &seg[..total_len])
}

/// Sends a raw Ethernet + IPv4 + UDP broadcast packet (used by DHCP before IP is assigned).
pub fn send_broadcast_raw(
    src_ip: [u8; 4],
    src_port: u16,
    dst_ip: [u8; 4],
    dst_port: u16,
    payload: &[u8],
) -> Result<(), &'static str> {
    let udp_len = UDP_HEADER_LEN + payload.len();
    let ip_total_len = ipv4::IP_HEADER_MIN_LEN + udp_len;
    if ip_total_len > 1500 {
        return Err("Broadcast UDP packet exceeds MTU");
    }

    let our_mac = NET_STATE.lock().mac;
    let mut pkt = [0u8; 1500];

    // IPv4 header
    pkt[0] = 0x45;
    pkt[1] = 0;
    pkt[2..4].copy_from_slice(&(ip_total_len as u16).to_be_bytes());
    pkt[4..6].copy_from_slice(&1u16.to_be_bytes());
    pkt[6..8].copy_from_slice(&0x4000u16.to_be_bytes());
    pkt[8] = 64;
    pkt[9] = ipv4::PROTO_UDP;
    pkt[10..12].copy_from_slice(&[0, 0]);
    pkt[12..16].copy_from_slice(&src_ip);
    pkt[16..20].copy_from_slice(&dst_ip);
    let ip_csum = checksum::internet_checksum(&pkt[..ipv4::IP_HEADER_MIN_LEN]);
    pkt[10..12].copy_from_slice(&ip_csum.to_be_bytes());

    // UDP header + payload
    let u = ipv4::IP_HEADER_MIN_LEN;
    pkt[u..u + 2].copy_from_slice(&src_port.to_be_bytes());
    pkt[u + 2..u + 4].copy_from_slice(&dst_port.to_be_bytes());
    pkt[u + 4..u + 6].copy_from_slice(&(udp_len as u16).to_be_bytes());
    pkt[u + 6..u + 8].copy_from_slice(&[0, 0]);
    pkt[u + UDP_HEADER_LEN..u + udp_len].copy_from_slice(payload);

    let mut udp_csum = checksum::transport_checksum(
        src_ip,
        dst_ip,
        ipv4::PROTO_UDP,
        &pkt[u..u + udp_len],
    );
    if udp_csum == 0 {
        udp_csum = 0xFFFF;
    }
    pkt[u + 6..u + 8].copy_from_slice(&udp_csum.to_be_bytes());

    ethernet::send(
        ethernet::BROADCAST_MAC,
        our_mac,
        ethernet::ETHERTYPE_IPV4,
        &pkt[..ip_total_len],
    )
}
