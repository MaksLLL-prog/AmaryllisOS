use crate::interrupts::timer;
use crate::net::{self, udp};

const DNS_PORT: u16 = 53;
const DNS_CLIENT_PORT: u16 = 53531;

pub fn resolve_a(hostname: &str) -> Result<[u8; 4], &'static str> {
    if let Some(ip) = net::parse_ipv4(hostname) {
        return Ok(ip);
    }

    let dns_ip = net::NET_STATE.lock().dns;
    if dns_ip == [0, 0, 0, 0] {
        return Err("No DNS server configured (run dhcp first)");
    }

    let mut pkt = [0u8; 256];
    let tx_id = 0xA140u16 ^ (timer::ticks() as u16);

    // DNS Header (12 bytes)
    pkt[0..2].copy_from_slice(&tx_id.to_be_bytes());
    pkt[2..4].copy_from_slice(&0x0100u16.to_be_bytes()); // RD=1 standard query
    pkt[4..6].copy_from_slice(&1u16.to_be_bytes()); // QDCOUNT=1
    pkt[6..12].fill(0);

    // QNAME labels
    let mut pos = 12usize;
    for label in hostname.split('.') {
        if label.is_empty() || label.len() > 63 || pos + 1 + label.len() + 5 > pkt.len() {
            return Err("Invalid hostname for DNS");
        }
        pkt[pos] = label.len() as u8;
        pos += 1;
        pkt[pos..pos + label.len()].copy_from_slice(label.as_bytes());
        pos += label.len();
    }
    pkt[pos] = 0; // root terminator
    pos += 1;
    pkt[pos..pos + 2].copy_from_slice(&1u16.to_be_bytes()); // QTYPE = A
    pkt[pos + 2..pos + 4].copy_from_slice(&1u16.to_be_bytes()); // QCLASS = IN
    pos += 4;

    udp::bind_port(DNS_CLIENT_PORT);
    let send_res = udp::send_datagram(DNS_CLIENT_PORT, dns_ip, DNS_PORT, &pkt[..pos]);
    if let Err(err) = send_res {
        udp::unbind_port();
        return Err(err);
    }

    let mut rx_buf = [0u8; 512];
    let deadline = timer::uptime_ms().saturating_add(2500);
    while timer::uptime_ms() < deadline {
        net::poll_rx();
        if let Some((_src_ip, src_port, rx_len)) = udp::take_received(&mut rx_buf) {
            if src_port == DNS_PORT && rx_len >= 12 {
                let rx_id = u16::from_be_bytes([rx_buf[0], rx_buf[1]]);
                if rx_id == tx_id {
                    udp::unbind_port();
                    return parse_dns_a_answer(&rx_buf[..rx_len]);
                }
            }
        }
        timer::sleep_ms(10);
    }

    udp::unbind_port();
    Err("DNS query timed out")
}

fn skip_dns_name(buf: &[u8], mut pos: usize) -> Option<usize> {
    let mut hops = 0usize;
    while pos < buf.len() && hops < 32 {
        let b = buf[pos];
        if b == 0 {
            return Some(pos + 1);
        }
        if (b & 0xC0) == 0xC0 {
            if pos + 2 <= buf.len() {
                return Some(pos + 2);
            }
            return None;
        }
        pos += 1 + (b as usize);
        hops += 1;
    }
    None
}

fn parse_dns_a_answer(buf: &[u8]) -> Result<[u8; 4], &'static str> {
    if buf.len() < 12 {
        return Err("Truncated DNS response");
    }
    let flags = u16::from_be_bytes([buf[2], buf[3]]);
    if (flags & 0x000F) != 0 {
        return Err("DNS lookup returned error status");
    }
    let qdcount = u16::from_be_bytes([buf[4], buf[5]]) as usize;
    let ancount = u16::from_be_bytes([buf[6], buf[7]]) as usize;

    let mut pos = 12usize;
    for _ in 0..qdcount {
        pos = skip_dns_name(buf, pos).ok_or("Malformed DNS question")?;
        pos = pos.checked_add(4).ok_or("Malformed DNS question")?;
        if pos > buf.len() {
            return Err("Malformed DNS packet");
        }
    }

    for _ in 0..ancount {
        pos = skip_dns_name(buf, pos).ok_or("Malformed DNS answer")?;
        if pos + 10 > buf.len() {
            return Err("Truncated DNS answer record");
        }
        let rtype = u16::from_be_bytes([buf[pos], buf[pos + 1]]);
        let rclass = u16::from_be_bytes([buf[pos + 2], buf[pos + 3]]);
        let rdlength = u16::from_be_bytes([buf[pos + 8], buf[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlength > buf.len() {
            return Err("Truncated DNS RDATA");
        }
        if rtype == 1 && rclass == 1 && rdlength == 4 {
            return Ok([buf[pos], buf[pos + 1], buf[pos + 2], buf[pos + 3]]);
        }
        pos += rdlength;
    }

    Err("No A record in DNS response")
}
