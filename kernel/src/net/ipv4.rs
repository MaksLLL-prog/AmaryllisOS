use core::sync::atomic::{AtomicU16, Ordering};

use crate::net::{self, arp, checksum, ethernet, icmp, tcp, udp};

pub const IP_HEADER_MIN_LEN: usize = 20;
pub const PROTO_ICMP: u8 = 1;
pub const PROTO_TCP: u8 = 6;
pub const PROTO_UDP: u8 = 17;

static NEXT_IP_ID: AtomicU16 = AtomicU16::new(1);

pub struct Ipv4Header {
    pub header_len: usize,
    pub total_len: usize,
    pub protocol: u8,
    pub src_ip: [u8; 4],
    pub dst_ip: [u8; 4],
    pub ttl: u8,
}

pub fn parse_header(packet: &[u8]) -> Option<Ipv4Header> {
    if packet.len() < IP_HEADER_MIN_LEN {
        return None;
    }
    let version = packet[0] >> 4;
    let ihl = (packet[0] & 0x0F) as usize;
    let header_len = ihl * 4;
    if version != 4 || header_len < IP_HEADER_MIN_LEN || packet.len() < header_len {
        return None;
    }

    let total_len = u16::from_be_bytes([packet[2], packet[3]]) as usize;
    if total_len < header_len || total_len > packet.len() {
        return None;
    }

    let ttl = packet[8];
    let protocol = packet[9];
    let mut src_ip = [0u8; 4];
    let mut dst_ip = [0u8; 4];
    src_ip.copy_from_slice(&packet[12..16]);
    dst_ip.copy_from_slice(&packet[16..20]);

    Some(Ipv4Header {
        header_len,
        total_len,
        protocol,
        src_ip,
        dst_ip,
        ttl,
    })
}

pub fn handle_packet(src_mac: [u8; 6], packet: &[u8]) {
    let Some(hdr) = parse_header(packet) else {
        return;
    };

    if hdr.src_ip != [0, 0, 0, 0] {
        net::arp_insert(hdr.src_ip, src_mac);
    }

    let (our_ip, _our_mac) = {
        let state = net::NET_STATE.lock();
        (state.ip, state.mac)
    };

    let is_broadcast = hdr.dst_ip == [255, 255, 255, 255];
    let is_unconfigured_dhcp = our_ip == [0, 0, 0, 0] && hdr.protocol == PROTO_UDP;
    if hdr.dst_ip != our_ip && !is_broadcast && !is_unconfigured_dhcp {
        return;
    }

    let payload = &packet[hdr.header_len..hdr.total_len];
    match hdr.protocol {
        PROTO_ICMP => icmp::handle_packet(&hdr, payload),
        PROTO_UDP => udp::handle_packet(&hdr, payload),
        PROTO_TCP => tcp::handle_packet(&hdr, payload),
        _ => {}
    }
}

pub fn send_packet(
    dst_ip: [u8; 4],
    protocol: u8,
    payload: &[u8],
) -> Result<(), &'static str> {
    if IP_HEADER_MIN_LEN + payload.len() > 1500 {
        return Err("IPv4 packet exceeds 1500-byte MTU");
    }

    let (our_mac, our_ip, next_hop_ip) = {
        let state = net::NET_STATE.lock();
        let next_hop = state.next_hop(dst_ip);
        (state.mac, state.ip, next_hop)
    };

    let dst_mac = if dst_ip == [255, 255, 255, 255] {
        ethernet::BROADCAST_MAC
    } else {
        arp::resolve_mac(next_hop_ip)?
    };

    let total_len = (IP_HEADER_MIN_LEN + payload.len()) as u16;
    let id = NEXT_IP_ID.fetch_add(1, Ordering::Relaxed);

    let mut buf = [0u8; 1500];
    buf[0] = 0x45; // IPv4, IHL=5
    buf[1] = 0;
    buf[2..4].copy_from_slice(&total_len.to_be_bytes());
    buf[4..6].copy_from_slice(&id.to_be_bytes());
    buf[6..8].copy_from_slice(&0x4000u16.to_be_bytes()); // Don't fragment
    buf[8] = 64; // TTL
    buf[9] = protocol;
    buf[10..12].copy_from_slice(&[0, 0]);
    buf[12..16].copy_from_slice(&our_ip);
    buf[16..20].copy_from_slice(&dst_ip);

    let csum = checksum::internet_checksum(&buf[..IP_HEADER_MIN_LEN]);
    buf[10..12].copy_from_slice(&csum.to_be_bytes());
    buf[IP_HEADER_MIN_LEN..total_len as usize].copy_from_slice(payload);

    ethernet::send(
        dst_mac,
        our_mac,
        ethernet::ETHERTYPE_IPV4,
        &buf[..total_len as usize],
    )
}
