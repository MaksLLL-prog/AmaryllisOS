use crate::interrupts::timer;
use crate::net::{self, udp};
use crate::task::preempt::PreemptMutex;

pub const DHCP_SERVER_PORT: u16 = 67;
pub const DHCP_CLIENT_PORT: u16 = 68;

const DHCP_MAGIC_COOKIE: [u8; 4] = [0x63, 0x82, 0x53, 0x63];
const DHCP_MSG_DISCOVER: u8 = 1;
const DHCP_MSG_OFFER: u8 = 2;
const DHCP_MSG_REQUEST: u8 = 3;
const DHCP_MSG_ACK: u8 = 5;

#[derive(Clone, Copy)]
pub struct DhcpLease {
    pub ip: [u8; 4],
    pub netmask: [u8; 4],
    pub gateway: [u8; 4],
    pub dns: [u8; 4],
    pub server_ip: [u8; 4],
    pub lease_seconds: u32,
}

#[derive(Clone, Copy)]
struct DhcpMailbox {
    waiting_xid: u32,
    received: bool,
    msg_type: u8,
    lease: DhcpLease,
}

impl DhcpMailbox {
    const fn empty() -> Self {
        Self {
            waiting_xid: 0,
            received: false,
            msg_type: 0,
            lease: DhcpLease {
                ip: [0; 4],
                netmask: [255, 255, 255, 0],
                gateway: [0; 4],
                dns: [0; 4],
                server_ip: [0; 4],
                lease_seconds: 0,
            },
        }
    }
}

static DHCP_MAILBOX: PreemptMutex<DhcpMailbox> = PreemptMutex::new(DhcpMailbox::empty());

pub fn handle_packet(data: &[u8]) {
    if data.len() < 240 {
        return;
    }
    // BOOTREPLY = 2, htype = 1, hlen = 6
    if data[0] != 2 || data[1] != 1 || data[2] != 6 {
        return;
    }
    if data[236..240] != DHCP_MAGIC_COOKIE {
        return;
    }

    let xid = u32::from_be_bytes([data[4], data[5], data[6], data[7]]);
    let mut yiaddr = [0u8; 4];
    let mut siaddr = [0u8; 4];
    yiaddr.copy_from_slice(&data[16..20]);
    siaddr.copy_from_slice(&data[20..24]);

    let mut msg_type = 0u8;
    let mut netmask = [255, 255, 255, 0];
    let mut gateway = [0u8; 4];
    let mut dns = [0u8; 4];
    let mut server_id = siaddr;
    let mut lease_seconds = 86400u32;

    let mut i = 240usize;
    while i < data.len() {
        let opt = data[i];
        i += 1;
        if opt == 0 {
            continue;
        }
        if opt == 255 {
            break;
        }
        if i >= data.len() {
            break;
        }
        let opt_len = data[i] as usize;
        i += 1;
        if i + opt_len > data.len() {
            break;
        }
        let val = &data[i..i + opt_len];
        match opt {
            53 if opt_len >= 1 => msg_type = val[0],
            1 if opt_len >= 4 => netmask.copy_from_slice(&val[..4]),
            3 if opt_len >= 4 => gateway.copy_from_slice(&val[..4]),
            6 if opt_len >= 4 => dns.copy_from_slice(&val[..4]),
            51 if opt_len >= 4 => {
                lease_seconds = u32::from_be_bytes([val[0], val[1], val[2], val[3]]);
            }
            54 if opt_len >= 4 => server_id.copy_from_slice(&val[..4]),
            _ => {}
        }
        i += opt_len;
    }

    let mut mailbox = DHCP_MAILBOX.lock();
    if mailbox.waiting_xid != 0 && mailbox.waiting_xid == xid && !mailbox.received {
        mailbox.received = true;
        mailbox.msg_type = msg_type;
        mailbox.lease = DhcpLease {
            ip: yiaddr,
            netmask,
            gateway,
            dns,
            server_ip: server_id,
            lease_seconds,
        };
    }
}

fn build_dhcp_packet(
    xid: u32,
    mac: [u8; 6],
    msg_type: u8,
    requested_ip: Option<[u8; 4]>,
    server_id: Option<[u8; 4]>,
    out: &mut [u8; 320],
) -> usize {
    out.fill(0);
    out[0] = 1; // BOOTREQUEST
    out[1] = 1; // Ethernet
    out[2] = 6; // MAC length
    out[3] = 0; // hops
    out[4..8].copy_from_slice(&xid.to_be_bytes());
    out[10..12].copy_from_slice(&0x8000u16.to_be_bytes()); // Broadcast flag
    out[28..34].copy_from_slice(&mac);
    out[236..240].copy_from_slice(&DHCP_MAGIC_COOKIE);

    let mut pos = 240usize;
    // Option 53: DHCP Message Type
    out[pos] = 53;
    out[pos + 1] = 1;
    out[pos + 2] = msg_type;
    pos += 3;

    if let Some(req_ip) = requested_ip {
        out[pos] = 50;
        out[pos + 1] = 4;
        out[pos + 2..pos + 6].copy_from_slice(&req_ip);
        pos += 6;
    }

    if let Some(srv) = server_id {
        out[pos] = 54;
        out[pos + 1] = 4;
        out[pos + 2..pos + 6].copy_from_slice(&srv);
        pos += 6;
    }

    // Option 55: Parameter Request List (Subnet Mask, Router, DNS)
    out[pos] = 55;
    out[pos + 1] = 3;
    out[pos + 2] = 1;
    out[pos + 3] = 3;
    out[pos + 4] = 6;
    pos += 5;

    // Option 255: End
    out[pos] = 255;
    pos += 1;

    pos.max(300)
}

fn wait_for_reply(xid: u32, expected_type: u8, timeout_ms: u64) -> Option<DhcpLease> {
    let deadline = timer::uptime_ms().saturating_add(timeout_ms);
    while timer::uptime_ms() < deadline {
        net::poll_rx();
        {
            let mut mailbox = DHCP_MAILBOX.lock();
            if mailbox.received && mailbox.waiting_xid == xid {
                let msg_type = mailbox.msg_type;
                let lease = mailbox.lease;
                mailbox.received = false;
                if msg_type == expected_type {
                    return Some(lease);
                }
            }
        }
        timer::sleep_ms(10);
    }
    None
}

pub fn discover_and_configure() -> Result<DhcpLease, &'static str> {
    if !crate::drivers::net_is_ready() {
        return Err("No active network driver");
    }

    let mac = net::NET_STATE.lock().mac;
    let xid = 0x414D_5200u32 ^ (timer::ticks() as u32) ^ ((mac[4] as u32) << 8) ^ (mac[5] as u32);

    let mut pkt = [0u8; 320];
    let len = build_dhcp_packet(xid, mac, DHCP_MSG_DISCOVER, None, None, &mut pkt);

    {
        let mut mailbox = DHCP_MAILBOX.lock();
        *mailbox = DhcpMailbox::empty();
        mailbox.waiting_xid = xid;
    }

    let mut offer = None;
    for _attempt in 0..2 {
        udp::send_broadcast_raw(
            [0, 0, 0, 0],
            DHCP_CLIENT_PORT,
            [255, 255, 255, 255],
            DHCP_SERVER_PORT,
            &pkt[..len],
        )?;
        offer = wait_for_reply(xid, DHCP_MSG_OFFER, 2000);
        if offer.is_some() {
            break;
        }
    }

    let offer = match offer {
        Some(lease) => lease,
        None => {
            DHCP_MAILBOX.lock().waiting_xid = 0;
            return Err("No DHCPOFFER received");
        }
    };

    let req_len = build_dhcp_packet(
        xid,
        mac,
        DHCP_MSG_REQUEST,
        Some(offer.ip),
        Some(offer.server_ip),
        &mut pkt,
    );

    {
        let mut mailbox = DHCP_MAILBOX.lock();
        mailbox.waiting_xid = xid;
        mailbox.received = false;
    }

    udp::send_broadcast_raw(
        [0, 0, 0, 0],
        DHCP_CLIENT_PORT,
        [255, 255, 255, 255],
        DHCP_SERVER_PORT,
        &pkt[..req_len],
    )?;

    let ack = wait_for_reply(xid, DHCP_MSG_ACK, 2000);
    DHCP_MAILBOX.lock().waiting_xid = 0;

    let final_lease = ack.unwrap_or(offer);
    {
        let mut state = net::NET_STATE.lock();
        state.ip = final_lease.ip;
        state.netmask = final_lease.netmask;
        state.gateway = final_lease.gateway;
        state.dns = if final_lease.dns != [0; 4] {
            final_lease.dns
        } else {
            final_lease.gateway
        };
        state.dhcp_configured = true;
    }

    Ok(final_lease)
}
