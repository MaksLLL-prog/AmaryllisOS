use crate::interrupts::timer;
use crate::net::{self, checksum, ipv4};
use crate::task::preempt::PreemptMutex;

const ICMP_ECHO_REPLY: u8 = 0;
const ICMP_ECHO_REQUEST: u8 = 8;

#[derive(Clone, Copy)]
struct PendingPingReply {
    waiting: bool,
    received: bool,
    from_ip: [u8; 4],
    id: u16,
    seq: u16,
    ttl: u8,
    bytes: usize,
    recv_ms: u64,
}

impl PendingPingReply {
    const fn empty() -> Self {
        Self {
            waiting: false,
            received: false,
            from_ip: [0; 4],
            id: 0,
            seq: 0,
            ttl: 0,
            bytes: 0,
            recv_ms: 0,
        }
    }
}

static PING_STATE: PreemptMutex<PendingPingReply> = PreemptMutex::new(PendingPingReply::empty());

#[derive(Clone, Copy)]
pub struct PingReply {
    pub from_ip: [u8; 4],
    pub seq: u16,
    pub ttl: u8,
    pub bytes: usize,
    pub rtt_ms: u64,
}

pub fn handle_packet(ip: &ipv4::Ipv4Header, payload: &[u8]) {
    if payload.len() < 8 {
        return;
    }

    let icmp_type = payload[0];
    let code = payload[1];
    let id = u16::from_be_bytes([payload[4], payload[5]]);
    let seq = u16::from_be_bytes([payload[6], payload[7]]);

    if icmp_type == ICMP_ECHO_REQUEST && code == 0 {
        if payload.len() <= 1480 {
            let mut reply = [0u8; 1480];
            let len = payload.len();
            reply[..len].copy_from_slice(&payload[..len]);
            reply[0] = ICMP_ECHO_REPLY;
            reply[2] = 0;
            reply[3] = 0;
            let csum = checksum::internet_checksum(&reply[..len]);
            reply[2..4].copy_from_slice(&csum.to_be_bytes());
            let _ = ipv4::send_packet(ip.src_ip, ipv4::PROTO_ICMP, &reply[..len]);
        }
        return;
    }

    if icmp_type == ICMP_ECHO_REPLY && code == 0 {
        let mut st = PING_STATE.lock();
        if st.waiting && st.id == id && st.seq == seq {
            st.received = true;
            st.from_ip = ip.src_ip;
            st.ttl = ip.ttl;
            st.bytes = payload.len().saturating_sub(8);
            st.recv_ms = timer::uptime_ms();
        }
    }
}

pub fn send_echo_and_wait(
    target_ip: [u8; 4],
    id: u16,
    seq: u16,
    timeout_ms: u64,
) -> Result<PingReply, &'static str> {
    let payload_data = b"AmaryllisOS-ICMP-Echo-Payload-01"; // 32 bytes
    let total_len = 8 + payload_data.len();
    let mut pkt = [0u8; 40];
    pkt[0] = ICMP_ECHO_REQUEST;
    pkt[1] = 0;
    pkt[2] = 0;
    pkt[3] = 0;
    pkt[4..6].copy_from_slice(&id.to_be_bytes());
    pkt[6..8].copy_from_slice(&seq.to_be_bytes());
    pkt[8..total_len].copy_from_slice(payload_data);

    let csum = checksum::internet_checksum(&pkt[..total_len]);
    pkt[2..4].copy_from_slice(&csum.to_be_bytes());

    {
        let mut st = PING_STATE.lock();
        *st = PendingPingReply::empty();
        st.waiting = true;
        st.id = id;
        st.seq = seq;
    }

    let start_ms = timer::uptime_ms();
    if let Err(err) = ipv4::send_packet(target_ip, ipv4::PROTO_ICMP, &pkt[..total_len]) {
        PING_STATE.lock().waiting = false;
        return Err(err);
    }

    let deadline = start_ms.saturating_add(timeout_ms);
    loop {
        net::poll_rx();
        {
            let mut st = PING_STATE.lock();
            if st.received {
                let rtt = st.recv_ms.saturating_sub(start_ms);
                let reply = PingReply {
                    from_ip: st.from_ip,
                    seq,
                    ttl: st.ttl,
                    bytes: st.bytes,
                    rtt_ms: rtt,
                };
                st.waiting = false;
                return Ok(reply);
            }
        }
        if timer::uptime_ms() >= deadline {
            PING_STATE.lock().waiting = false;
            return Err("Request timed out");
        }
        timer::sleep_ms(10);
    }
}
