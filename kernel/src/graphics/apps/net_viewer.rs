use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

use crate::drivers;
use crate::graphics::alerts::{self, AlertSeverity, AlertSubsystem};
use crate::graphics::theme::{self, ACCENT_ERR, ACCENT_MINT, ACCENT_WARN};
use crate::graphics::widgets::{text_width, Canvas, Rect};
use crate::input::{KeyCode, KeyEvent};
use crate::net;
use crate::task::kthread::{self, NetJobOutcome, NetJobRequest};

use super::DesktopAction;

const LINE_H: i32 = 18;

#[derive(Clone)]
enum NetViewerPendingOp {
    Dhcp,
    PingGateway([u8; 4]),
    PingAfterDhcp,
    FetchAfterDhcp(String),
    HttpGet(String),
}

pub struct NetViewerApp {
    url_input: String,
    raw_mode: bool,
    status_line: String,
    headers: Vec<String>,
    raw_lines: Vec<String>,
    rendered_lines: Vec<String>,
    scroll: usize,
    last_visible_rows: Cell<usize>,
    last_wrap_cols: Cell<usize>,
    pending_job: Option<(u64, NetViewerPendingOp)>,
}

impl NetViewerApp {
    pub fn new() -> Self {
        let net_ok = drivers::net_is_ready();
        let nic_lbl = drivers::net_driver_label();
        let st = net::NET_STATE.lock();
        let ip = st.ip;
        drop(st);

        let init_status = if !net_ok {
            String::from("No NIC active — boot QEMU with -device e1000 or -device rtl8139")
        } else if ip == [0, 0, 0, 0] {
            format!("{} Link UP — press DHCP or Go to acquire IP and fetch HTTP", nic_lbl)
        } else {
            format!("{} Ready — IP {}.{}.{}.{}", nic_lbl, ip[0], ip[1], ip[2], ip[3])
        };

        let mut app = Self {
            url_input: String::from("http://example.com/"),
            raw_mode: false,
            status_line: init_status,
            headers: Vec::new(),
            raw_lines: Vec::new(),
            rendered_lines: Vec::new(),
            scroll: 0,
            last_visible_rows: Cell::new(14),
            last_wrap_cols: Cell::new(72),
            pending_job: None,
        };
        app.set_welcome_body();
        app
    }

    fn set_welcome_body(&mut self) {
        self.rendered_lines.clear();
        self.raw_lines.clear();
        self.rendered_lines.push(String::from("Amaryllis Net / HTTP Viewer"));
        self.rendered_lines.push(String::from("------------------------------------------------"));
        self.rendered_lines.push(String::from("1. Edit URL above (type directly and press Enter or click Go)."));
        self.rendered_lines.push(String::from("2. Click DHCP to configure IPv4 via active Ethernet NIC."));
        self.rendered_lines.push(String::from("3. Click Ping GW to test ICMP Echo latency to gateway."));
        self.rendered_lines.push(String::from("4. Toggle Text / Raw to inspect HTTP headers & HTML output."));
        self.raw_lines = self.rendered_lines.clone();
    }

    pub fn tick(&mut self) -> bool {
        let Some((job_id, op)) = self.pending_job.clone() else {
            return false;
        };
        let Some(outcome) = kthread::take_net_job_result(job_id) else {
            return false;
        };
        self.pending_job = None;

        match (op, outcome) {
            (NetViewerPendingOp::Dhcp, NetJobOutcome::Dhcp(res)) => match res {
                Ok(lease) => {
                    self.status_line = format!(
                        "DHCP OK: IP {}.{}.{}.{}  GW {}.{}.{}.{}  DNS {}.{}.{}.{}",
                        lease.ip[0], lease.ip[1], lease.ip[2], lease.ip[3],
                        lease.gateway[0], lease.gateway[1], lease.gateway[2], lease.gateway[3],
                        lease.dns[0], lease.dns[1], lease.dns[2], lease.dns[3]
                    );
                    self.headers.clear();
                    self.headers.push(self.status_line.clone());
                }
                Err(err) => {
                    self.status_line = format!("DHCP failed: {}", err);
                    alerts::report_system_error(
                        AlertSeverity::Warning,
                        AlertSubsystem::Network,
                        "NET DHCP",
                        "DHCP discovery failed",
                        err,
                        "Check QEMU networking (-netdev user,id=n0 -device e1000,netdev=n0).",
                        false,
                    );
                }
            },
            (NetViewerPendingOp::PingAfterDhcp, NetJobOutcome::Dhcp(res)) => match res {
                Ok(lease) => {
                    let gw = if lease.gateway != [0, 0, 0, 0] {
                        lease.gateway
                    } else {
                        [10, 0, 2, 2]
                    };
                    let id = kthread::submit_net_job(NetJobRequest::Ping {
                        target_ip: gw,
                        seq: 1,
                        timeout_ms: 1500,
                    });
                    self.pending_job = Some((id, NetViewerPendingOp::PingGateway(gw)));
                    self.status_line = format!(
                        "DHCP OK — pinging {}.{}.{}.{} asynchronously...",
                        gw[0], gw[1], gw[2], gw[3]
                    );
                }
                Err(err) => {
                    self.status_line = format!("DHCP failed before ping: {}", err);
                }
            },
            (NetViewerPendingOp::FetchAfterDhcp(target), NetJobOutcome::Dhcp(res)) => match res {
                Ok(_) => {
                    let id = kthread::submit_net_job(NetJobRequest::HttpGet {
                        url: target.clone(),
                        max_bytes: 24 * 1024,
                    });
                    self.pending_job = Some((id, NetViewerPendingOp::HttpGet(target.clone())));
                    self.status_line = format!("DHCP OK — fetching {}...", target);
                }
                Err(err) => {
                    self.status_line = format!("DHCP failed: {}", err);
                }
            },
            (NetViewerPendingOp::PingGateway(gw), NetJobOutcome::Ping(res)) => match res {
                Ok(rep) => {
                    self.status_line = format!(
                        "PING {}.{}.{}.{}: {} bytes ttl={} time={} ms",
                        rep.from_ip[0], rep.from_ip[1], rep.from_ip[2], rep.from_ip[3],
                        rep.bytes, rep.ttl, rep.rtt_ms
                    );
                }
                Err(err) => {
                    self.status_line = format!(
                        "Ping {}.{}.{}.{} failed: {}",
                        gw[0], gw[1], gw[2], gw[3], err
                    );
                }
            },
            (NetViewerPendingOp::HttpGet(target), NetJobOutcome::HttpGet { result }) => {
                match result {
                    Ok((peer_ip, total, raw_bytes)) => {
                        self.parse_http_response(peer_ip, total, &raw_bytes);
                    }
                    Err(err) => {
                        self.status_line = format!("HTTP error: {}", err);
                        alerts::report_system_error(
                            AlertSeverity::Warning,
                            AlertSubsystem::Network,
                            "HTTP GET",
                            &format!("Request failed for {}", target),
                            &err,
                            "Only plaintext HTTP (port 80) is supported; HTTPS is not implemented.",
                            false,
                        );
                    }
                }
            }
            _ => {}
        }
        true
    }

    pub fn run_dhcp(&mut self) {
        if !drivers::net_is_ready() {
            self.status_line = String::from("Network controller is not ready");
            return;
        }
        if self.pending_job.is_some() {
            self.status_line = String::from("Network task already running on knet_worker...");
            return;
        }
        self.scroll = 0;
        let id = kthread::submit_net_job(NetJobRequest::Dhcp);
        self.pending_job = Some((id, NetViewerPendingOp::Dhcp));
        self.status_line = String::from("DHCP: Discovering IPv4 lease (async)...");
    }

    pub fn run_ping_gateway(&mut self) {
        if !drivers::net_is_ready() {
            self.status_line = String::from("Network controller is not ready");
            return;
        }
        if self.pending_job.is_some() {
            self.status_line = String::from("Network task already running on knet_worker...");
            return;
        }
        let (ip, gw_cfg) = {
            let st = net::NET_STATE.lock();
            (st.ip, st.gateway)
        };
        if ip == [0, 0, 0, 0] {
            let id = kthread::submit_net_job(NetJobRequest::Dhcp);
            self.pending_job = Some((id, NetViewerPendingOp::PingAfterDhcp));
            self.status_line = String::from("Acquiring DHCP lease before ping (async)...");
            return;
        }
        let gw = if gw_cfg != [0, 0, 0, 0] { gw_cfg } else { [10, 0, 2, 2] };
        let id = kthread::submit_net_job(NetJobRequest::Ping {
            target_ip: gw,
            seq: 1,
            timeout_ms: 1500,
        });
        self.pending_job = Some((id, NetViewerPendingOp::PingGateway(gw)));
        self.status_line = format!("Pinging {}.{}.{}.{} (async)...", gw[0], gw[1], gw[2], gw[3]);
    }

    pub fn fetch_url(&mut self) {
        if !drivers::net_is_ready() {
            self.status_line = String::from("Network interface is not ready");
            return;
        }
        if self.pending_job.is_some() {
            self.status_line = String::from("Network task already running on knet_worker...");
            return;
        }

        let target = String::from(self.url_input.trim());
        if target.is_empty() {
            self.status_line = String::from("Enter a valid http:// URL");
            return;
        }

        if net::NET_STATE.lock().ip == [0, 0, 0, 0] {
            let id = kthread::submit_net_job(NetJobRequest::Dhcp);
            self.pending_job = Some((id, NetViewerPendingOp::FetchAfterDhcp(target.clone())));
            self.status_line = format!("DHCP + GET {} (async)...", target);
        } else {
            let id = kthread::submit_net_job(NetJobRequest::HttpGet {
                url: target.clone(),
                max_bytes: 24 * 1024,
            });
            self.pending_job = Some((id, NetViewerPendingOp::HttpGet(target.clone())));
            self.status_line = format!("Fetching {} (async)...", target);
        }
    }

    fn parse_http_response(&mut self, peer_ip: [u8; 4], total_bytes: usize, raw: &[u8]) {
        self.scroll = 0;
        self.headers.clear();
        self.raw_lines.clear();
        self.rendered_lines.clear();

        let split_pos = raw
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| (p, p + 4))
            .or_else(|| raw.windows(2).position(|w| w == b"\n\n").map(|p| (p, p + 2)));

        let (hdr_bytes, body_bytes) = match split_pos {
            Some((h_end, b_start)) => (&raw[..h_end], &raw[b_start..]),
            None => (&b""[..], raw),
        };

        let hdr_str = String::from_utf8_lossy(hdr_bytes);
        for line in hdr_str.lines().take(16) {
            self.headers.push(String::from(line.trim_end_matches('\r')));
        }
        let first_hdr = self.headers.first().cloned().unwrap_or_else(|| String::from("HTTP Response"));
        self.status_line = format!(
            "{}  |  {}.{}.{}.{}  |  {} bytes",
            first_hdr, peer_ip[0], peer_ip[1], peer_ip[2], peer_ip[3], total_bytes
        );

        let wrap_cols = self.last_wrap_cols.get().clamp(40, 110);

        let raw_text = String::from_utf8_lossy(body_bytes);
        for line in raw_text.lines() {
            wrap_push(&mut self.raw_lines, line.trim_end_matches('\r'), wrap_cols);
        }
        if self.raw_lines.is_empty() {
            self.raw_lines.push(String::from("<empty body>"));
        }

        let plain = html_to_plain_text(&raw_text);
        for line in plain.lines() {
            wrap_push(&mut self.rendered_lines, line, wrap_cols);
        }
        if self.rendered_lines.is_empty() {
            self.rendered_lines = self.raw_lines.clone();
        }
    }

    pub fn on_wheel(&mut self, delta: i8) {
        let total = if self.raw_mode {
            self.raw_lines.len()
        } else {
            self.rendered_lines.len()
        };
        let vis = self.last_visible_rows.get().max(1);
        let max_scroll = total.saturating_sub(vis);
        if delta > 0 {
            self.scroll = (self.scroll + 3).min(max_scroll);
        } else if delta < 0 {
            self.scroll = self.scroll.saturating_sub(3);
        }
    }

    pub fn on_key(&mut self, evt: KeyEvent) -> DesktopAction {
        let total = if self.raw_mode {
            self.raw_lines.len()
        } else {
            self.rendered_lines.len()
        };
        let vis = self.last_visible_rows.get().max(1);
        let max_scroll = total.saturating_sub(vis);

        match evt.code {
            KeyCode::Enter => self.fetch_url(),
            KeyCode::Backspace => {
                self.url_input.pop();
            }
            KeyCode::Up => {
                self.scroll = self.scroll.saturating_sub(1);
            }
            KeyCode::Down => {
                self.scroll = (self.scroll + 1).min(max_scroll);
            }
            KeyCode::PageUp => {
                self.scroll = self.scroll.saturating_sub(vis);
            }
            KeyCode::PageDown => {
                self.scroll = (self.scroll + vis).min(max_scroll);
            }
            KeyCode::Char(c) if c.is_ascii() && !c.is_ascii_control() => {
                if evt.modifiers.ctrl && (c == 'l' || c == 'L') {
                    self.url_input.clear();
                } else if self.url_input.len() < 120 {
                    self.url_input.push(c);
                }
            }
            _ => {}
        }
        DesktopAction::None
    }

    pub fn on_click(&mut self, area: Rect, mx: i32, my: i32) -> DesktopAction {
        let y0 = area.y + 8;
        let btn_go = Rect::new(area.right() - 252, y0, 44, 28);
        let btn_dhcp = Rect::new(area.right() - 202, y0, 56, 28);
        let btn_ping = Rect::new(area.right() - 140, y0, 72, 28);
        let btn_mode = Rect::new(area.right() - 62, y0, 50, 28);

        if btn_go.contains(mx, my) {
            self.fetch_url();
            return DesktopAction::None;
        }
        if btn_dhcp.contains(mx, my) {
            self.run_dhcp();
            return DesktopAction::None;
        }
        if btn_ping.contains(mx, my) {
            self.run_ping_gateway();
            return DesktopAction::None;
        }
        if btn_mode.contains(mx, my) {
            self.raw_mode = !self.raw_mode;
            self.scroll = 0;
            return DesktopAction::None;
        }

        // Quick preset bookmarks row
        let preset_y = area.y + 42;
        let presets = [
            ("example.com", "http://example.com/"),
            ("10.0.2.2", "http://10.0.2.2/"),
            ("neverssl.com", "http://neverssl.com/"),
        ];
        let mut px = area.x + 12;
        for (lbl, url) in presets {
            let w = text_width(lbl, 1) + 18;
            let pr = Rect::new(px, preset_y, w, 22);
            if pr.contains(mx, my) {
                self.url_input = String::from(url);
                self.fetch_url();
                return DesktopAction::None;
            }
            px += w + 8;
        }

        DesktopAction::None
    }

    pub fn draw(&self, canvas: &mut Canvas<'_>, area: Rect) {
        let pal = theme::palette();
        canvas.fill_rect(area, pal.win_bg_active);

        // Top URL & action toolbar
        let bar = Rect::new(area.x, area.y, area.w, 70);
        canvas.fill_rect(bar, pal.surface_toolbar);
        canvas.fill_rect(Rect::new(area.x, bar.bottom() - 1, area.w, 1), pal.surface_border);

        let url_r = Rect::new(area.x + 12, area.y + 8, (area.w - 272).max(120), 28);
        canvas.fill_rounded_rect(url_r, 6, pal.modal_input_bg);
        canvas.stroke_rounded_rect(url_r, 6, pal.accent_primary, 180);
        canvas.draw_text_clipped(url_r.x + 10, url_r.y + 6, &self.url_input, pal.text_primary, 1, url_r);

        let y0 = area.y + 8;
        let btns = [
            (Rect::new(area.right() - 252, y0, 44, 28), "Go", pal.accent_primary, 0xFFFFFFFF),
            (Rect::new(area.right() - 202, y0, 56, 28), "DHCP", pal.surface_card, pal.accent_green),
            (Rect::new(area.right() - 140, y0, 72, 28), "Ping GW", pal.surface_card, pal.text_primary),
            (
                Rect::new(area.right() - 62, y0, 50, 28),
                if self.raw_mode { "Text" } else { "Raw" },
                pal.surface_card,
                pal.accent_primary,
            ),
        ];
        for (r, txt, bg, fg) in btns {
            canvas.fill_rounded_rect(r, 6, bg);
            canvas.stroke_rounded_rect(r, 6, pal.surface_border, 180);
            let tx = r.x + ((r.w - text_width(txt, 1)) / 2).max(4);
            canvas.draw_text_bold_clipped(tx, r.y + 6, txt, fg, r);
        }

        // Preset buttons + live network summary on row 2
        let preset_y = area.y + 42;
        let presets = ["example.com", "10.0.2.2", "neverssl.com"];
        let mut px = area.x + 12;
        for lbl in presets {
            let w = text_width(lbl, 1) + 18;
            let pr = Rect::new(px, preset_y, w, 22);
            canvas.fill_rounded_rect(pr, 5, pal.surface_card);
            canvas.stroke_rounded_rect(pr, 5, pal.surface_border, 160);
            canvas.draw_text_clipped(pr.x + 9, pr.y + 3, lbl, pal.text_secondary, 1, pr);
            px += w + 8;
        }

        let st = net::NET_STATE.lock();
        let ip = st.ip;
        let gw = st.gateway;
        drop(st);
        let nic = drivers::net_stats();
        let link_str = format!(
            "{}  IP {}.{}.{}.{}  GW {}.{}.{}.{}  TX/RX {}/{}",
            if nic.link_up { "UP" } else { "DOWN" },
            ip[0], ip[1], ip[2], ip[3],
            gw[0], gw[1], gw[2], gw[3],
            nic.tx_packets, nic.rx_packets
        );
        canvas.draw_text_clipped(
            px + 6,
            preset_y + 3,
            &link_str,
            if nic.link_up { pal.accent_green } else { ACCENT_WARN },
            1,
            Rect::new(px + 6, preset_y, (area.right() - px - 14).max(40), 22),
        );

        // Response body card
        let body_r = Rect::new(area.x + 12, area.y + 78, area.w - 24, (area.h - 108).max(80));
        canvas.fill_rounded_rect(body_r, 8, pal.surface_panel);
        canvas.stroke_rounded_rect(body_r, 8, pal.surface_border, 180);

        let lines = if self.raw_mode {
            &self.raw_lines
        } else {
            &self.rendered_lines
        };

        let vis_rows = ((body_r.h - 16) / LINE_H).max(1) as usize;
        let cols = ((body_r.w - 28) / 7).max(32) as usize;
        self.last_visible_rows.set(vis_rows);
        self.last_wrap_cols.set(cols);

        for (i, line) in lines.iter().skip(self.scroll).take(vis_rows).enumerate() {
            let ly = body_r.y + 8 + (i as i32) * LINE_H;
            let col = if i == 0 && self.scroll == 0 && !self.raw_mode {
                pal.accent_primary
            } else {
                pal.text_primary
            };
            canvas.draw_text_clipped(
                body_r.x + 12,
                ly,
                line,
                col,
                1,
                Rect::new(body_r.x + 12, ly, body_r.w - 30, LINE_H),
            );
        }

        canvas.draw_scrollbar_v(
            Rect::new(body_r.right() - 10, body_r.y + 4, 7, body_r.h - 8),
            lines.len(),
            vis_rows,
            self.scroll,
            pal.text_muted,
            pal.surface_border,
        );

        let st_col = if self.pending_job.is_some() {
            pal.accent_primary
        } else if self.status_line.contains("failed") || self.status_line.contains("error") {
            ACCENT_ERR
        } else if self.status_line.contains("OK") || self.status_line.contains("200") {
            ACCENT_MINT
        } else {
            pal.text_muted
        };
        canvas.draw_text_clipped(area.x + 12, area.bottom() - 22, &self.status_line, st_col, 1, area);
    }
}

fn wrap_push(out: &mut Vec<String>, line: &str, max_chars: usize) {
    if line.is_empty() {
        out.push(String::new());
        return;
    }
    let mut cur = String::new();
    for ch in line.chars() {
        cur.push(ch);
        if cur.chars().count() >= max_chars {
            out.push(cur);
            cur = String::new();
        }
    }
    if !cur.is_empty() {
        out.push(cur);
    }
}

fn html_to_plain_text(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    let mut tag_buf = String::new();
    let mut suppress_depth = 0usize;
    let mut prev_space = false;

    let chars: Vec<char> = html.chars().collect();
    let mut i = 0usize;
    while i < chars.len() {
        let ch = chars[i];
        if ch == '<' {
            in_tag = true;
            tag_buf.clear();
            i += 1;
            continue;
        }
        if in_tag {
            if ch == '>' {
                in_tag = false;
                let tl = tag_buf.trim().to_ascii_lowercase();
                if tl.starts_with("script") || tl.starts_with("style") {
                    suppress_depth += 1;
                } else if tl.starts_with("/script") || tl.starts_with("/style") {
                    suppress_depth = suppress_depth.saturating_sub(1);
                } else if suppress_depth == 0
                    && (tl.starts_with("p")
                        || tl.starts_with("/p")
                        || tl.starts_with("br")
                        || tl.starts_with("div")
                        || tl.starts_with("/div")
                        || tl.starts_with("h1")
                        || tl.starts_with("/h1")
                        || tl.starts_with("h2")
                        || tl.starts_with("/h2")
                        || tl.starts_with("li"))
                {
                    if !out.ends_with('\n') {
                        out.push('\n');
                    }
                    prev_space = false;
                }
            } else if tag_buf.len() < 32 {
                tag_buf.push(ch);
            }
            i += 1;
            continue;
        }

        if suppress_depth > 0 {
            i += 1;
            continue;
        }

        if ch == '&' {
            let rem: String = chars[i..chars.len().min(i + 8)].iter().collect();
            if rem.starts_with("&amp;") {
                out.push('&');
                i += 5;
                prev_space = false;
                continue;
            } else if rem.starts_with("&lt;") {
                out.push('<');
                i += 4;
                prev_space = false;
                continue;
            } else if rem.starts_with("&gt;") {
                out.push('>');
                i += 4;
                prev_space = false;
                continue;
            } else if rem.starts_with("&quot;") {
                out.push('"');
                i += 6;
                prev_space = false;
                continue;
            } else if rem.starts_with("&nbsp;") {
                if !prev_space {
                    out.push(' ');
                    prev_space = true;
                }
                i += 6;
                continue;
            }
        }

        if ch == '\r' {
            i += 1;
            continue;
        }
        if ch.is_whitespace() {
            if ch == '\n' {
                if !out.ends_with("\n\n") && !out.is_empty() {
                    out.push('\n');
                }
            } else if !prev_space && !out.ends_with('\n') {
                out.push(' ');
            }
            prev_space = true;
        } else {
            out.push(ch);
            prev_space = false;
        }
        i += 1;
    }
    out
}
