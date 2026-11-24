use crate::interrupts::timer;
use crate::net::{self, ethernet};

const ARP_HTYPE_ETHERNET: u16 = 1;
const ARP_PTYPE_IPV4: u16 = 0x0800;
const ARP_OP_REQUEST: u16 = 1;
const ARP_OP_REPLY: u16 = 2;
const ARP_PACKET_LEN: usize = 28;

pub fn handle_packet(payload: &[u8]) {
    if payload.len() < ARP_PACKET_LEN {
        return;
    }

    let htype = u16::from_be_bytes([payload[0], payload[1]]);
    let ptype = u16::from_be_bytes([payload[2], payload[3]]);
    let hlen = payload[4];
    let plen = payload[5];
    let opcode = u16::from_be_bytes([payload[6], payload[7]]);

    if htype != ARP_HTYPE_ETHERNET || ptype != ARP_PTYPE_IPV4 || hlen != 6 || plen != 4 {
        return;
    }

    let mut sender_mac = [0u8; 6];
    let mut sender_ip = [0u8; 4];
    let mut target_ip = [0u8; 4];

    sender_mac.copy_from_slice(&payload[8..14]);
    sender_ip.copy_from_slice(&payload[14..18]);
    target_ip.copy_from_slice(&payload[24..28]);

    // Always learn sender IP -> MAC mapping if valid.
    if sender_ip != [0, 0, 0, 0] {
        net::arp_insert(sender_ip, sender_mac);
    }

    if opcode == ARP_OP_REQUEST {
        let (our_mac, our_ip) = {
            let state = net::NET_STATE.lock();
            (state.mac, state.ip)
        };
        if our_ip != [0, 0, 0, 0] && target_ip == our_ip {
            let _ = send_arp_packet(ARP_OP_REPLY, our_mac, our_ip, sender_mac, sender_ip);
        }
    }
}

pub fn send_arp_packet(
    opcode: u16,
    sender_mac: [u8; 6],
    sender_ip: [u8; 4],
    target_mac: [u8; 6],
    target_ip: [u8; 4],
) -> Result<(), &'static str> {
    let mut pkt = [0u8; ARP_PACKET_LEN];
    pkt[0..2].copy_from_slice(&ARP_HTYPE_ETHERNET.to_be_bytes());
    pkt[2..4].copy_from_slice(&ARP_PTYPE_IPV4.to_be_bytes());
    pkt[4] = 6;
    pkt[5] = 4;
    pkt[6..8].copy_from_slice(&opcode.to_be_bytes());
    pkt[8..14].copy_from_slice(&sender_mac);
    pkt[14..18].copy_from_slice(&sender_ip);
    pkt[18..24].copy_from_slice(&target_mac);
    pkt[24..28].copy_from_slice(&target_ip);

    let dst_eth = if opcode == ARP_OP_REQUEST {
        ethernet::BROADCAST_MAC
    } else {
        target_mac
    };
    ethernet::send(dst_eth, sender_mac, ethernet::ETHERTYPE_ARP, &pkt)
}

pub fn resolve_mac(target_ip: [u8; 4]) -> Result<[u8; 6], &'static str> {
    if target_ip == [255, 255, 255, 255] {
        return Ok(ethernet::BROADCAST_MAC);
    }

    if let Some(mac) = net::arp_lookup(target_ip) {
        return Ok(mac);
    }

    let (our_mac, our_ip) = {
        let state = net::NET_STATE.lock();
        (state.mac, state.ip)
    };

    for _attempt in 0..3 {
        send_arp_packet(ARP_OP_REQUEST, our_mac, our_ip, [0; 6], target_ip)?;
        for _ in 0..40 {
            net::poll_rx();
            if let Some(mac) = net::arp_lookup(target_ip) {
                return Ok(mac);
            }
            timer::sleep_ms(10);
        }
    }

    Err("ARP resolution timeout")
}
