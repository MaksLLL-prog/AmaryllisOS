use crate::console::Console;
use crate::drivers::e1000;
use crate::graphics::Color;
use crate::interrupts::timer;
use crate::net::{self, dhcp, dns, http, icmp};

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);
const RED: Color = (220, 90, 90);

pub fn cmd_netinfo(console: &mut Console<'_>) {
    net::poll_rx();
    let nic = e1000::stats();
    let state = net::NET_STATE.lock();

    console.write("\nNetwork Interface Information:\n", GREEN);
    console.write("  Driver:     ", GRAY);
    console.write("Intel 82540EM (e1000)\n", WHITE);

    console.write("  Status:     ", GRAY);
    if !nic.initialized {
        console.write("NOT PRESENT (start QEMU with -netdev user,id=net0 -device e1000,netdev=net0)\n", YELLOW);
        return;
    }
    console.write("ACTIVE\n", GREEN);

    console.write("  Link:       ", GRAY);
    if nic.link_up {
        console.write("UP\n", GREEN);
    } else {
        console.write("DOWN\n", YELLOW);
    }

    console.write("  MAC:        ", GRAY);
    write_mac(console, nic.mac);
    console.write("\n", WHITE);

    console.write("  IPv4:       ", GRAY);
    write_ip(console, state.ip);
    if state.dhcp_configured {
        console.write(" (DHCP)", GREEN);
    } else {
        console.write(" (unconfigured - run 'dhcp')", YELLOW);
    }
    console.write("\n", WHITE);

    console.write("  Netmask:    ", GRAY);
    write_ip(console, state.netmask);
    console.write("\n", WHITE);

    console.write("  Gateway:    ", GRAY);
    write_ip(console, state.gateway);
    console.write("\n", WHITE);

    console.write("  DNS Server: ", GRAY);
    write_ip(console, state.dns);
    console.write("\n", WHITE);

    console.write("  MMIO Phys:  ", GRAY);
    write_hex64(console, nic.mmio_phys);
    console.write("\n", WHITE);

    console.write("  TX Packets: ", GRAY);
    write_decimal(console, nic.tx_packets);
    console.write(" (", GRAY);
    write_decimal(console, nic.tx_bytes);
    console.write(" bytes, errors: ", GRAY);
    write_decimal(console, nic.tx_errors);
    console.write(")\n", GRAY);

    console.write("  RX Packets: ", GRAY);
    write_decimal(console, nic.rx_packets);
    console.write(" (", GRAY);
    write_decimal(console, nic.rx_bytes);
    console.write(" bytes)\n", GRAY);
}

pub fn cmd_dhcp(console: &mut Console<'_>) {
    if !e1000::is_ready() {
        console.write("Network driver e1000 is not initialized.\n", RED);
        return;
    }

    console.write("Sending DHCPDISCOVER...\n", GRAY);
    match dhcp::discover_and_configure() {
        Ok(lease) => {
            console.write("[ OK ] DHCP lease acquired!\n", GREEN);
            console.write("  IP Address: ", GRAY);
            write_ip(console, lease.ip);
            console.write("\n", WHITE);
            console.write("  Netmask:    ", GRAY);
            write_ip(console, lease.netmask);
            console.write("\n", WHITE);
            console.write("  Gateway:    ", GRAY);
            write_ip(console, lease.gateway);
            console.write("\n", WHITE);
            console.write("  DNS:        ", GRAY);
            write_ip(console, lease.dns);
            console.write("\n", WHITE);
        }
        Err(err) => {
            console.write("DHCP failed: ", RED);
            console.write(err, WHITE);
            console.write("\n", WHITE);
        }
    }
}

pub fn cmd_ping(console: &mut Console<'_>, arguments: &str) {
    let target = arguments.trim();
    if target.is_empty() {
        console.write("Usage: ping <ip_or_hostname>\n", YELLOW);
        return;
    }

    if !e1000::is_ready() {
        console.write("Network driver e1000 is not initialized.\n", RED);
        return;
    }

    if net::NET_STATE.lock().ip == [0, 0, 0, 0] {
        console.write("No IP configured; running DHCP first...\n", GRAY);
        if let Err(err) = dhcp::discover_and_configure() {
            console.write("DHCP failed: ", RED);
            console.write(err, WHITE);
            console.write("\n", WHITE);
            return;
        }
    }

    let target_ip = match dns::resolve_a(target) {
        Ok(ip) => ip,
        Err(err) => {
            console.write("Cannot resolve host '", RED);
            console.write(target, WHITE);
            console.write("': ", RED);
            console.write(err, WHITE);
            console.write("\n", WHITE);
            return;
        }
    };

    console.write("PING ", GREEN);
    console.write(target, WHITE);
    console.write(" (", GRAY);
    write_ip(console, target_ip);
    console.write(") 32 bytes of data:\n", GRAY);

    let mut received = 0u64;
    let id = 0x414D;

    for seq in 1..=4u16 {
        match icmp::send_echo_and_wait(target_ip, id, seq, 1500) {
            Ok(reply) => {
                received += 1;
                console.write("  Reply from ", GREEN);
                write_ip(console, reply.from_ip);
                console.write(": bytes=", WHITE);
                write_decimal(console, reply.bytes as u64);
                console.write(" seq=", WHITE);
                write_decimal(console, reply.seq as u64);
                console.write(" time=", WHITE);
                write_decimal(console, reply.rtt_ms);
                console.write("ms TTL=", WHITE);
                write_decimal(console, reply.ttl as u64);
                console.write("\n", WHITE);
            }
            Err(err) => {
                console.write("  seq=", YELLOW);
                write_decimal(console, seq as u64);
                console.write(" ", YELLOW);
                console.write(err, YELLOW);
                console.write("\n", YELLOW);
            }
        }
        if seq < 4 {
            timer::sleep_ms(250);
        }
    }

    console.write("Ping statistics: 4 transmitted, ", GRAY);
    write_decimal(console, received);
    console.write(" received\n", GRAY);
}

pub fn cmd_httpget(console: &mut Console<'_>, arguments: &str) {
    let url = arguments.trim();
    if url.is_empty() {
        console.write("Usage: httpget <http://host[:port]/path>\n", YELLOW);
        return;
    }

    if !e1000::is_ready() {
        console.write("Network driver e1000 is not initialized.\n", RED);
        return;
    }

    if net::NET_STATE.lock().ip == [0, 0, 0, 0] {
        console.write("No IP configured; running DHCP first...\n", GRAY);
        if let Err(err) = dhcp::discover_and_configure() {
            console.write("DHCP failed: ", RED);
            console.write(err, WHITE);
            console.write("\n", WHITE);
            return;
        }
    }

    console.write("HTTP GET ", GREEN);
    console.write(url, WHITE);
    console.write("\n", WHITE);

    const MAX_DISPLAY_BYTES: usize = 8192;
    let mut printed = 0usize;
    let mut truncated = false;

    let res = http::http_get(url, &mut |chunk| {
        if printed >= MAX_DISPLAY_BYTES {
            truncated = true;
            return;
        }
        let allowed = (MAX_DISPLAY_BYTES - printed).min(chunk.len());
        print_http_bytes(console, &chunk[..allowed]);
        printed += allowed;
        if allowed < chunk.len() {
            truncated = true;
        }
    });

    match res {
        Ok((ip, total)) => {
            console.write("\n--- HTTP done (", GREEN);
            write_ip(console, ip);
            console.write(", ", GREEN);
            write_decimal(console, total as u64);
            console.write(" bytes received) ---\n", GREEN);
            if truncated {
                console.write("[Output truncated to 8 KiB on console]\n", YELLOW);
            }
        }
        Err(err) => {
            console.write("httpget failed: ", RED);
            console.write(err, WHITE);
            console.write("\n", WHITE);
        }
    }
}

fn print_http_bytes(console: &mut Console<'_>, bytes: &[u8]) {
    let mut buf = [0u8; 64];
    let mut len = 0usize;

    for &b in bytes {
        if b == b'\r' {
            continue;
        }
        let ch = if b == b'\n' || b == b'\t' || (b >= 0x20 && b < 0x7F) {
            b
        } else {
            b'.'
        };
        buf[len] = ch;
        len += 1;
        if len == buf.len() {
            if let Ok(s) = core::str::from_utf8(&buf[..len]) {
                console.write(s, WHITE);
            }
            len = 0;
        }
    }
    if len > 0 {
        if let Ok(s) = core::str::from_utf8(&buf[..len]) {
            console.write(s, WHITE);
        }
    }
}

fn write_mac(console: &mut Console<'_>, mac: [u8; 6]) {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = [0u8; 17];
    for i in 0..6 {
        let pos = i * 3;
        out[pos] = HEX[(mac[i] >> 4) as usize];
        out[pos + 1] = HEX[(mac[i] & 0x0F) as usize];
        if i < 5 {
            out[pos + 2] = b':';
        }
    }
    if let Ok(s) = core::str::from_utf8(&out) {
        console.write(s, WHITE);
    }
}

fn write_ip(console: &mut Console<'_>, ip: [u8; 4]) {
    for (i, &octet) in ip.iter().enumerate() {
        write_decimal(console, octet as u64);
        if i < 3 {
            console.write(".", WHITE);
        }
    }
}

fn write_decimal(console: &mut Console<'_>, mut number: u64) {
    if number == 0 {
        console.write("0", WHITE);
        return;
    }
    let mut buffer = [0u8; 20];
    let mut index = buffer.len();
    while number > 0 {
        index -= 1;
        buffer[index] = b'0' + (number % 10) as u8;
        number /= 10;
    }
    if let Ok(text) = core::str::from_utf8(&buffer[index..]) {
        console.write(text, WHITE);
    }
}

fn write_hex64(console: &mut Console<'_>, value: u64) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    console.write("0x", GRAY);
    let mut buffer = [0u8; 16];
    for index in 0..16 {
        let shift = (15 - index) * 4;
        buffer[index] = HEX[((value >> shift) & 0xF) as usize];
    }
    if let Ok(text) = core::str::from_utf8(&buffer) {
        console.write(text, WHITE);
    }
}
