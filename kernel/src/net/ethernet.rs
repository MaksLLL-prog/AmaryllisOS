use crate::drivers::e1000;

pub const ETH_HEADER_LEN: usize = 14;
pub const ETHERTYPE_IPV4: u16 = 0x0800;
pub const ETHERTYPE_ARP: u16 = 0x0806;

pub const BROADCAST_MAC: [u8; 6] = [0xFF; 6];

pub struct EthernetFrame<'a> {
    pub dst_mac: [u8; 6],
    pub src_mac: [u8; 6],
    pub ethertype: u16,
    pub payload: &'a [u8],
}

pub fn parse(frame: &[u8]) -> Option<EthernetFrame<'_>> {
    if frame.len() < ETH_HEADER_LEN {
        return None;
    }
    let mut dst_mac = [0u8; 6];
    let mut src_mac = [0u8; 6];
    dst_mac.copy_from_slice(&frame[0..6]);
    src_mac.copy_from_slice(&frame[6..12]);
    let ethertype = u16::from_be_bytes([frame[12], frame[13]]);
    Some(EthernetFrame {
        dst_mac,
        src_mac,
        ethertype,
        payload: &frame[ETH_HEADER_LEN..],
    })
}

pub fn send(
    dst_mac: [u8; 6],
    src_mac: [u8; 6],
    ethertype: u16,
    payload: &[u8],
) -> Result<(), &'static str> {
    if ETH_HEADER_LEN + payload.len() > 1514 {
        return Err("Ethernet payload exceeds MTU");
    }

    let mut frame = [0u8; 1518];
    frame[0..6].copy_from_slice(&dst_mac);
    frame[6..12].copy_from_slice(&src_mac);
    frame[12..14].copy_from_slice(&ethertype.to_be_bytes());
    frame[ETH_HEADER_LEN..ETH_HEADER_LEN + payload.len()].copy_from_slice(payload);

    let total_len = (ETH_HEADER_LEN + payload.len()).max(60);
    e1000::send_frame(&frame[..total_len])
}
