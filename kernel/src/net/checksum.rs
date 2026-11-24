pub fn internet_checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    let mut i = 0usize;

    while i + 1 < data.len() {
        let word = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        sum = sum.wrapping_add(word);
        i += 2;
    }

    if i < data.len() {
        let word = ((data[i] as u16) << 8) as u32;
        sum = sum.wrapping_add(word);
    }

    fold_checksum(sum)
}

pub fn transport_checksum(
    src_ip: [u8; 4],
    dst_ip: [u8; 4],
    protocol: u8,
    segment: &[u8],
) -> u16 {
    let mut sum = 0u32;

    // Pseudo-header: src IP, dst IP, zero + proto, segment length
    sum = sum.wrapping_add(u16::from_be_bytes([src_ip[0], src_ip[1]]) as u32);
    sum = sum.wrapping_add(u16::from_be_bytes([src_ip[2], src_ip[3]]) as u32);
    sum = sum.wrapping_add(u16::from_be_bytes([dst_ip[0], dst_ip[1]]) as u32);
    sum = sum.wrapping_add(u16::from_be_bytes([dst_ip[2], dst_ip[3]]) as u32);
    sum = sum.wrapping_add(protocol as u32);
    sum = sum.wrapping_add(segment.len() as u32);

    let mut i = 0usize;
    while i + 1 < segment.len() {
        let word = u16::from_be_bytes([segment[i], segment[i + 1]]) as u32;
        sum = sum.wrapping_add(word);
        i += 2;
    }
    if i < segment.len() {
        sum = sum.wrapping_add(((segment[i] as u16) << 8) as u32);
    }

    fold_checksum(sum)
}

fn fold_checksum(mut sum: u32) -> u16 {
    while (sum >> 16) != 0 {
        sum = (sum & 0xFFFF).wrapping_add(sum >> 16);
    }
    !(sum as u16)
}
