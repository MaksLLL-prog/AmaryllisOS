use core::fmt::Write;

use crate::net::{self, dns, tcp};

pub struct ParsedHttpUrl<'a> {
    pub host: &'a str,
    pub port: u16,
    pub path: &'a str,
}

pub fn parse_url(raw: &str) -> Result<ParsedHttpUrl<'_>, &'static str> {
    let trimmed = raw.trim();
    let without_scheme = if let Some(rest) = trimmed.strip_prefix("http://") {
        rest
    } else if trimmed.contains("://") {
        return Err("Only http:// URLs are supported");
    } else {
        trimmed
    };

    if without_scheme.is_empty() {
        return Err("Empty host in URL");
    }

    let (authority, path) = match without_scheme.find('/') {
        Some(idx) => (&without_scheme[..idx], &without_scheme[idx..]),
        None => (without_scheme, "/"),
    };

    let (host, port) = match authority.find(':') {
        Some(idx) => {
            let h = &authority[..idx];
            let p_str = &authority[idx + 1..];
            let p = p_str.parse::<u16>().map_err(|_| "Invalid port number")?;
            (h, p)
        }
        None => (authority, 80),
    };

    if host.is_empty() {
        return Err("Missing hostname");
    }

    Ok(ParsedHttpUrl { host, port, path })
}

struct ReqBuf {
    data: [u8; 512],
    len: usize,
}

impl ReqBuf {
    const fn new() -> Self {
        Self {
            data: [0; 512],
            len: 0,
        }
    }
}

impl Write for ReqBuf {
    fn write_str(&mut self, s: &str) -> core::fmt::Result {
        let bytes = s.as_bytes();
        let avail = self.data.len().saturating_sub(self.len);
        let n = bytes.len().min(avail);
        self.data[self.len..self.len + n].copy_from_slice(&bytes[..n]);
        self.len += n;
        Ok(())
    }
}

pub fn http_get(
    url: &str,
    on_chunk: &mut dyn FnMut(&[u8]),
) -> Result<([u8; 4], usize), &'static str> {
    if !crate::drivers::net_is_ready() {
        return Err("No active network interface initialized");
    }
    if net::NET_STATE.lock().ip == [0, 0, 0, 0] {
        return Err("No IP address assigned (run 'dhcp' first)");
    }

    let parsed = parse_url(url)?;
    let target_ip = dns::resolve_a(parsed.host)?;

    let mut req = ReqBuf::new();
    let _ = write!(
        req,
        "GET {} HTTP/1.1\r\nHost: {}\r\nUser-Agent: AmaryllisOS/0.2\r\nAccept: */*\r\nConnection: close\r\n\r\n",
        parsed.path,
        parsed.host
    );

    let total = tcp::http_exchange(
        target_ip,
        parsed.port,
        &req.data[..req.len],
        on_chunk,
    )?;

    Ok((target_ip, total))
}
