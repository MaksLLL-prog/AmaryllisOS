pub mod arp;
pub mod checksum;
pub mod dhcp;
pub mod dns;
pub mod ethernet;
pub mod http;
pub mod icmp;
pub mod ipv4;
pub mod tcp;
pub mod udp;

use crate::drivers;
use crate::task::preempt::PreemptMutex;

const ARP_CACHE_SIZE: usize = 16;

#[derive(Clone, Copy)]
struct ArpEntry {
    ip: [u8; 4],
    mac: [u8; 6],
    valid: bool,
}

impl ArpEntry {
    const EMPTY: Self = Self {
        ip: [0; 4],
        mac: [0; 6],
        valid: false,
    };
}

pub struct NetState {
    pub mac: [u8; 6],
    pub ip: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    pub dhcp_configured: bool,
    arp_cache: [ArpEntry; ARP_CACHE_SIZE],
    arp_next_evict: usize,
}

impl NetState {
    pub const fn new() -> Self {
        Self {
            mac: [0; 6],
            ip: [0, 0, 0, 0],
            netmask: [255, 255, 255, 0],
            gateway: [0, 0, 0, 0],
            dns: [0, 0, 0, 0],
            dhcp_configured: false,
            arp_cache: [ArpEntry::EMPTY; ARP_CACHE_SIZE],
            arp_next_evict: 0,
        }
    }

    pub fn next_hop(&self, dst_ip: [u8; 4]) -> [u8; 4] {
        if self.gateway == [0, 0, 0, 0] {
            return dst_ip;
        }
        let same_subnet = (0..4).all(|i| (dst_ip[i] & self.netmask[i]) == (self.ip[i] & self.netmask[i]));
        if same_subnet {
            dst_ip
        } else {
            self.gateway
        }
    }
}

pub static NET_STATE: PreemptMutex<NetState> = PreemptMutex::new(NetState::new());

pub fn set_mac(mac: [u8; 6]) {
    NET_STATE.lock().mac = mac;
}

pub fn arp_lookup(ip: [u8; 4]) -> Option<[u8; 6]> {
    let state = NET_STATE.lock();
    for entry in &state.arp_cache {
        if entry.valid && entry.ip == ip {
            return Some(entry.mac);
        }
    }
    None
}

pub fn arp_insert(ip: [u8; 4], mac: [u8; 6]) {
    let mut state = NET_STATE.lock();
    for entry in &mut state.arp_cache {
        if entry.valid && entry.ip == ip {
            entry.mac = mac;
            return;
        }
    }
    for entry in &mut state.arp_cache {
        if !entry.valid {
            *entry = ArpEntry {
                ip,
                mac,
                valid: true,
            };
            return;
        }
    }
    let idx = state.arp_next_evict % ARP_CACHE_SIZE;
    state.arp_cache[idx] = ArpEntry {
        ip,
        mac,
        valid: true,
    };
    state.arp_next_evict = (idx + 1) % ARP_CACHE_SIZE;
}

pub fn poll_rx() -> usize {
    if !drivers::net_is_ready() {
        return 0;
    }

    let mut processed = 0usize;
    let mut frame_buf = [0u8; 1600];

    for _ in 0..16 {
        let Some(len) = drivers::net_poll_frame(&mut frame_buf) else {
            break;
        };
        processed += 1;
        if let Some(eth) = ethernet::parse(&frame_buf[..len]) {
            match eth.ethertype {
                ethernet::ETHERTYPE_ARP => arp::handle_packet(eth.payload),
                ethernet::ETHERTYPE_IPV4 => ipv4::handle_packet(eth.src_mac, eth.payload),
                _ => {}
            }
        }
    }

    processed
}

pub fn parse_ipv4(s: &str) -> Option<[u8; 4]> {
    let mut octets = [0u8; 4];
    let mut count = 0usize;
    for part in s.trim().split('.') {
        if count >= 4 {
            return None;
        }
        let val = part.parse::<u8>().ok()?;
        octets[count] = val;
        count += 1;
    }
    if count == 4 {
        Some(octets)
    } else {
        None
    }
}
