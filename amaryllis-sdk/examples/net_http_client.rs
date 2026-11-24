#![no_std]
#![no_main]

use amaryllis_sdk::{entry, net, println, proc};

entry!(main);

fn main() -> i32 {
    println!("[net_http_client PID={}] Starting network checks...", proc::getpid());

    // 1. Ping QEMU user-mode gateway (10.0.2.2)
    match net::ping([10, 0, 2, 2], 1, 1500) {
        Ok(rtt) => println!("ICMP ping 10.0.2.2 OK: rtt = {} ms", rtt),
        Err(e) => println!("ICMP ping 10.0.2.2 failed (err={})", e),
    }

    // 2. DNS lookup for example.com
    let ip = match net::dns_resolve("example.com") {
        Ok(ip) => {
            println!("DNS A example.com -> {}.{}.{}.{}", ip[0], ip[1], ip[2], ip[3]);
            Some(ip)
        }
        Err(e) => {
            println!("DNS lookup failed (err={})", e);
            None
        }
    };

    // 3. Raw BSD-like TCP stream + HTTP/1.1 request if DNS resolved
    if let Some(ip) = ip {
        match net::TcpStream::connect(ip, 80) {
            Ok(stream) => {
                let req = b"GET / HTTP/1.1\r\nHost: example.com\r\nConnection: close\r\n\r\n";
                if let Ok(sent) = stream.write(req) {
                    println!("TCP sent {} bytes to {}:80", sent, ip[0]);
                    let mut resp = [0u8; 512];
                    match stream.read_timeout(&mut resp, 3000) {
                        Ok(n) if n > 0 => {
                            let preview_len = n.min(200);
                            if let Ok(text) = core::str::from_utf8(&resp[..preview_len]) {
                                println!("TCP response ({} bytes preview):\n{}", n, text);
                            }
                        }
                        Ok(_) => println!("TCP connection closed immediately."),
                        Err(e) => println!("TCP recv error: {}", e),
                    }
                }
                stream.close();
            }
            Err(e) => println!("TCP connect failed (err={}), trying SYS_HTTP_GET...", e),
        }
    }

    // 4. Direct HTTP GET syscall test
    let mut http_buf = [0u8; 1024];
    match net::http_get("http://example.com/", &mut http_buf) {
        Ok(n) => {
            let show = n.min(180);
            println!(
                "SYS_HTTP_GET ok ({} bytes): {}",
                n,
                core::str::from_utf8(&http_buf[..show]).unwrap_or("<non-utf8>")
            );
        }
        Err(e) => println!("SYS_HTTP_GET failed (err={})", e),
    }

    0
}
