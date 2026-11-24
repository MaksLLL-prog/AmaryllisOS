use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use crate::drivers::registry::{self, DriverStatus};
use crate::graphics::alerts;
use crate::graphics::theme::{self, ACCENT_ERR, ACCENT_WARN};
use crate::graphics::widgets::{Canvas, Rect};
use crate::memory::heap::GLOBAL_ALLOCATOR;
use crate::memory::PhysicalFrameAllocator;
use crate::process::PROCESS_MANAGER;
use crate::task::kthread::{self, KThreadSnapshot};
use crate::userland::sched::{self, UserProcSnapshot};

use super::DesktopAction;

const SAMPLE_COUNT: usize = 40;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum MonitorTab {
    Overview,
    Processes,
    Drivers,
    Alerts,
}

pub struct MonitorApp {
    tab: MonitorTab,
    heap_samples: [u8; SAMPLE_COUNT],
    sample_cursor: usize,
    last_sample_tick: u64,
    cached_procs: Vec<UserProcSnapshot>,
    cached_kthreads: Vec<KThreadSnapshot>,
}

impl MonitorApp {
    pub fn new() -> Self {
        Self {
            tab: MonitorTab::Overview,
            heap_samples: [0; SAMPLE_COUNT],
            sample_cursor: 0,
            last_sample_tick: 0,
            cached_procs: sched::snapshot_processes(),
            cached_kthreads: kthread::snapshot_kthreads(),
        }
    }

    /// Samples heap usage and active Ring 3 processes every 250 ms; returns `true` when refreshed.
    pub fn tick(&mut self, now_ticks: u64) -> bool {
        if now_ticks.saturating_sub(self.last_sample_tick) < 25 {
            return false;
        }
        self.last_sample_tick = now_ticks;

        let total = GLOBAL_ALLOCATOR.total_size().max(1);
        let free = GLOBAL_ALLOCATOR.free_bytes();
        let used = total.saturating_sub(free);
        let pct = ((used * 100) / total).min(100) as u8;

        self.heap_samples[self.sample_cursor] = pct;
        self.sample_cursor = (self.sample_cursor + 1) % SAMPLE_COUNT;
        self.cached_procs = sched::snapshot_processes();
        self.cached_kthreads = kthread::snapshot_kthreads();
        true
    }

    pub fn select_alerts_tab(&mut self) {
        self.tab = MonitorTab::Alerts;
    }

    pub fn select_processes_tab(&mut self) {
        self.tab = MonitorTab::Processes;
        self.cached_procs = sched::snapshot_processes();
        self.cached_kthreads = kthread::snapshot_kthreads();
    }

    pub fn on_click(&mut self, area: Rect, mx: i32, my: i32) -> DesktopAction {
        let t0 = Rect::new(area.x + 12, area.y + 8, 92, 28);
        let t1 = Rect::new(area.x + 110, area.y + 8, 126, 28);
        let t2 = Rect::new(area.x + 242, area.y + 8, 92, 28);
        let t3 = Rect::new(area.x + 340, area.y + 8, 120, 28);

        if t0.contains(mx, my) {
            self.tab = MonitorTab::Overview;
            return DesktopAction::None;
        }
        if t1.contains(mx, my) {
            self.tab = MonitorTab::Processes;
            self.cached_procs = sched::snapshot_processes();
            self.cached_kthreads = kthread::snapshot_kthreads();
            return DesktopAction::None;
        }
        if t2.contains(mx, my) {
            self.tab = MonitorTab::Drivers;
            return DesktopAction::None;
        }
        if t3.contains(mx, my) {
            self.tab = MonitorTab::Alerts;
            return DesktopAction::None;
        }

        if self.tab == MonitorTab::Processes {
            let content = Rect::new(area.x + 12, area.y + 44, area.w - 24, area.h - 54);
            let max_rows = (((content.h - 138) / 30).max(1)) as usize;
            for (i, proc) in self.cached_procs.iter().take(max_rows).enumerate() {
                let row_y = content.y + 34 + (i as i32) * 30;
                let kill_btn = Rect::new(content.right() - 74, row_y + 2, 62, 24);
                if kill_btn.contains(mx, my) {
                    return DesktopAction::KillProcess(proc.pid);
                }
            }
        }

        if self.tab == MonitorTab::Alerts {
            let btn_inspect = Rect::new(area.right() - 142, area.y + 8, 130, 28);
            if btn_inspect.contains(mx, my) {
                return DesktopAction::OpenLatestAlert;
            }
        }
        DesktopAction::None
    }

    pub fn draw(
        &self,
        canvas: &mut Canvas<'_>,
        area: Rect,
        allocator: Option<&PhysicalFrameAllocator<'_>>,
        fps_val: u32,
    ) {
        let pal = theme::palette();
        canvas.fill_rect(area, pal.win_bg_active);

        let tabs = [
            (Rect::new(area.x + 12, area.y + 8, 92, 28), String::from("Overview"), self.tab == MonitorTab::Overview),
            (
                Rect::new(area.x + 110, area.y + 8, 126, 28),
                format!("Tasks ({}+{})", self.cached_procs.len(), self.cached_kthreads.len()),
                self.tab == MonitorTab::Processes,
            ),
            (Rect::new(area.x + 242, area.y + 8, 92, 28), String::from("Drivers"), self.tab == MonitorTab::Drivers),
            (
                Rect::new(area.x + 340, area.y + 8, 120, 28),
                format!("Alerts ({})", alerts::alert_count()),
                self.tab == MonitorTab::Alerts,
            ),
        ];

        for (r, label, active) in tabs {
            canvas.fill_rounded_rect(r, 6, if active { pal.surface_selected } else { pal.surface_card });
            canvas.stroke_rounded_rect(
                r,
                6,
                if active { pal.accent_primary } else { pal.surface_border },
                190,
            );
            canvas.draw_text_clipped(
                r.x + 10,
                r.y + 6,
                &label,
                if active { pal.accent_primary } else { pal.text_secondary },
                1,
                r,
            );
        }

        if self.tab == MonitorTab::Alerts {
            let btn = Rect::new(area.right() - 142, area.y + 8, 130, 28);
            canvas.fill_rounded_rect(btn, 6, pal.surface_card);
            canvas.stroke_rounded_rect(btn, 6, ACCENT_ERR, 190);
            canvas.draw_text_clipped(btn.x + 10, btn.y + 6, "Inspect Latest", ACCENT_ERR, 1, btn);
        }

        let content = Rect::new(area.x + 12, area.y + 44, area.w - 24, area.h - 54);

        match self.tab {
            MonitorTab::Overview => self.draw_overview(canvas, content, allocator, fps_val),
            MonitorTab::Processes => self.draw_processes(canvas, content),
            MonitorTab::Drivers => self.draw_drivers(canvas, content),
            MonitorTab::Alerts => self.draw_alerts(canvas, content),
        }
    }

    fn draw_overview(
        &self,
        canvas: &mut Canvas<'_>,
        area: Rect,
        allocator: Option<&PhysicalFrameAllocator<'_>>,
        fps_val: u32,
    ) {
        let pal = theme::palette();
        let total_heap = GLOBAL_ALLOCATOR.total_size().max(1);
        let free_heap = GLOBAL_ALLOCATOR.free_bytes();
        let used_heap = total_heap.saturating_sub(free_heap);
        let heap_pct = ((used_heap * 100) / total_heap).min(100);

        let pmm_line = if let Some(alloc_ref) = allocator {
            let pmm_alloc = alloc_ref.allocated_frames();
            let pmm_free = alloc_ref.free_frames();
            let pmm_total = (pmm_alloc + pmm_free).max(1);
            let pmm_pct = ((pmm_alloc * 100) / pmm_total).min(100);
            format!(
                "Physical RAM: {} / {} frames ({}% used)  |  PMM: {}",
                pmm_alloc,
                pmm_total,
                pmm_pct,
                if alloc_ref.check_integrity() { "OK" } else { "ERR" }
            )
        } else {
            String::from("Physical Frames: Active Kernel Allocator")
        };

        let hcard = Rect::new(area.x, area.y, area.w, 118);
        canvas.fill_rounded_rect(hcard, 8, pal.surface_card);
        canvas.stroke_rounded_rect(hcard, 8, pal.surface_border, 190);

        canvas.draw_text_clipped(
            hcard.x + 12,
            hcard.y + 10,
            &format!(
                "Kernel Heap: {} KiB / {} KiB ({}%)   |   Compositor: {} FPS",
                used_heap / 1024,
                total_heap / 1024,
                heap_pct,
                fps_val.min(60)
            ),
            pal.accent_green,
            1,
            hcard,
        );

        let bar = Rect::new(hcard.x + 12, hcard.y + 34, hcard.w - 24, 10);
        canvas.fill_rounded_rect(bar, 4, pal.surface_panel);
        let fill_w = ((bar.w as usize * heap_pct) / 100).max(4) as i32;
        canvas.fill_rounded_rect(Rect::new(bar.x, bar.y, fill_w.min(bar.w), bar.h), 4, pal.accent_green);

        let spark = Rect::new(hcard.x + 12, hcard.y + 52, hcard.w - 24, 54);
        canvas.fill_rounded_rect(spark, 6, pal.surface_panel);
        let col_w = (spark.w / SAMPLE_COUNT as i32).max(2);
        for i in 0..SAMPLE_COUNT {
            let idx = (self.sample_cursor + i) % SAMPLE_COUNT;
            let val = self.heap_samples[idx] as i32;
            let bh = ((val * (spark.h - 4)) / 100).max(2);
            let bx = spark.x + 2 + (i as i32) * col_w;
            if bx + col_w - 2 <= spark.right() {
                canvas.fill_rect(
                    Rect::new(bx, spark.bottom() - bh - 2, (col_w - 2).max(1), bh),
                    pal.accent_primary,
                );
            }
        }

        let pcard = Rect::new(area.x, area.y + 126, area.w, area.h - 126);
        canvas.fill_rounded_rect(pcard, 8, pal.surface_card);
        canvas.stroke_rounded_rect(pcard, 8, pal.surface_border, 190);

        canvas.draw_text_clipped(
            pcard.x + 12,
            pcard.y + 12,
            &pmm_line,
            pal.text_primary,
            1,
            pcard,
        );

        let proc_count = PROCESS_MANAGER.lock().process_count();
        let r3_count = self.cached_procs.len();
        let net_state = crate::net::NET_STATE.lock();
        let ip = net_state.ip;
        let net_txt = if crate::drivers::e1000::is_ready() {
            format!("Intel e1000 READY ({}.{}.{}.{})", ip[0], ip[1], ip[2], ip[3])
        } else {
            String::from("Intel e1000 NOT DETECTED")
        };
        drop(net_state);

        let ac97_txt = if crate::drivers::ac97::is_ready() {
            String::from("Intel AC'97 Async Mixer ACTIVE")
        } else {
            String::from("Intel AC'97 fallback PC speaker")
        };

        canvas.draw_text_clipped(
            pcard.x + 12,
            pcard.y + 36,
            &format!(
                "Ring 3 Tasks: {}  |  KThreads: {}  |  PCBs: {}  |  Alerts: {}",
                r3_count,
                self.cached_kthreads.len(),
                proc_count,
                alerts::alert_count()
            ),
            pal.text_secondary,
            1,
            pcard,
        );
        canvas.draw_text_clipped(pcard.x + 12, pcard.y + 60, &net_txt, pal.accent_primary, 1, pcard);
        canvas.draw_text_clipped(pcard.x + 12, pcard.y + 84, &ac97_txt, pal.accent_green, 1, pcard);

        let cache = crate::storage::cache::stats_snapshot();
        let total_io = cache.hits + cache.misses;
        let hit_pct = if total_io > 0 {
            (cache.hits * 100) / total_io
        } else {
            0
        };
        let cache_txt = format!(
            "Page Cache: {}/{} KiB ({}% hit, {} dirty)  |  {} ({}s DMA)",
            cache.cached_pages * 4,
            cache.max_pages * 4,
            hit_pct,
            cache.dirty_pages,
            cache.backend.as_str(),
            cache.max_dma_sectors
        );
        canvas.draw_text_clipped(pcard.x + 12, pcard.y + 108, &cache_txt, pal.text_primary, 1, pcard);

        let kstats = crate::klog::stats();
        let smp = crate::arch::smp::summary();
        let iokit_st = crate::drivers::iokit::stats();
        let apic_timer_lbl = if crate::arch::apic::is_timer_active() {
            format!("LAPIC {}Hz ({} IRQs)", crate::arch::apic::timer_hz(), kstats.apic_timer_irqs)
        } else {
            String::from("PIT 100Hz fallback")
        };
        let sched_txt = format!(
            "{}  |  Preempt: K={} R3={}  |  Lazy #PF={} Guard={}",
            apic_timer_lbl,
            kstats.kthread_preempts,
            kstats.user_preempts,
            kstats.pf_lazy_commits,
            kstats.pf_guard_faults
        );
        canvas.draw_text_clipped(pcard.x + 12, pcard.y + 132, &sched_txt, pal.accent_primary, 1, pcard);

        let smp_txt = format!(
            "SMP: {} online CPUs (BSP + {} APs)  |  TLB Shootdowns: {}  |  IOAPIC: {} vecs / {} IRQs",
            smp.online_cpus,
            smp.ap_booted,
            smp.tlb_shootdowns,
            iokit_st.routed_vectors,
            iokit_st.total_ioapic_irqs,
        );
        canvas.draw_text_clipped(pcard.x + 12, pcard.y + 154, &smp_txt, pal.accent_green, 1, pcard);
    }

    fn draw_processes(&self, canvas: &mut Canvas<'_>, area: Rect) {
        let pal = theme::palette();
        canvas.fill_rounded_rect(area, 8, pal.surface_panel);
        canvas.stroke_rounded_rect(area, 8, pal.surface_border, 180);

        canvas.draw_text_bold_clipped(area.x + 12, area.y + 8, "PID", pal.text_muted, area);
        canvas.draw_text_bold_clipped(area.x + 56, area.y + 8, "PROGRAM", pal.text_muted, area);
        canvas.draw_text_bold_clipped(area.x + 206, area.y + 8, "STATE", pal.text_muted, area);
        canvas.draw_text_bold_clipped(area.x + 286, area.y + 8, "VRUNTIME", pal.text_muted, area);
        canvas.draw_text_bold_clipped(area.x + 368, area.y + 8, "PR / PF", pal.text_muted, area);
        canvas.draw_text_bold_clipped(area.x + 448, area.y + 8, "MEM", pal.text_muted, area);

        let max_rows = (((area.h - 138) / 30).max(1)) as usize;
        if self.cached_procs.is_empty() {
            canvas.draw_text_clipped(
                area.x + 16,
                area.y + 40,
                "No active Ring 3 ELF64 processes.",
                pal.text_secondary,
                1,
                area,
            );
            canvas.draw_text_clipped(
                area.x + 16,
                area.y + 62,
                "Launch '/counter.elf', '/lazy_demo.elf', or '/gui_demo.elf' via Terminal or Spotlight.",
                pal.text_muted,
                1,
                area,
            );
        } else {
            for (i, proc) in self.cached_procs.iter().take(max_rows).enumerate() {
                let y = area.y + 34 + (i as i32) * 30;
                let row_r = Rect::new(area.x + 8, y, area.w - 16, 26);
                canvas.fill_rounded_rect(row_r, 6, pal.surface_card);
                canvas.stroke_rounded_rect(row_r, 6, pal.surface_border, 140);

                canvas.draw_text_clipped(area.x + 14, y + 5, &format!("{}", proc.pid), pal.text_primary, 1, row_r);
                canvas.draw_text_clipped(area.x + 56, y + 5, &proc.name, pal.accent_primary, 1, row_r);

                let st_col = if proc.state_label == "Running" || proc.state_label == "Ready" {
                    pal.accent_green
                } else {
                    ACCENT_WARN
                };
                canvas.draw_text_clipped(area.x + 206, y + 5, proc.state_label, st_col, 1, row_r);
                canvas.draw_text_clipped(
                    area.x + 286,
                    y + 5,
                    &format!("{}ms", proc.vruntime_ms),
                    pal.text_secondary,
                    1,
                    row_r,
                );
                canvas.draw_text_clipped(
                    area.x + 368,
                    y + 5,
                    &format!("{} / {}", proc.preempts, proc.lazy_faults),
                    pal.text_secondary,
                    1,
                    row_r,
                );
                canvas.draw_text_clipped(area.x + 448, y + 5, &format!("{}K", proc.pages * 4), pal.text_secondary, 1, row_r);

                let kill_btn = Rect::new(area.right() - 74, y + 2, 62, 22);
                canvas.fill_rounded_rect_alpha(kill_btn, 5, ACCENT_ERR, 45);
                canvas.stroke_rounded_rect(kill_btn, 5, ACCENT_ERR, 190);
                canvas.draw_text_clipped(kill_btn.x + 14, kill_btn.y + 3, "Kill", ACCENT_ERR, 1, kill_btn);
            }
        }

        // Preemptive Kernel Worker / IOWorkLoop Threads section at bottom
        let kt_top = area.bottom() - 114;
        if kt_top > area.y + 92 {
            canvas.fill_rect(Rect::new(area.x + 8, kt_top - 6, area.w - 16, 1), pal.surface_border);
            canvas.draw_text_bold_clipped(
                area.x + 12,
                kt_top,
                "PREEMPTIVE KERNEL & IOWorkLoop THREADS (SMP)",
                pal.text_muted,
                area,
            );
            for (j, kt) in self.cached_kthreads.iter().take(3).enumerate() {
                let ky = kt_top + 20 + (j as i32) * 28;
                let row_r = Rect::new(area.x + 8, ky, area.w - 16, 24);
                canvas.fill_rounded_rect(row_r, 6, pal.surface_card);
                canvas.stroke_rounded_rect(row_r, 6, pal.surface_border, 130);
                let cpu_lbl = match kt.running_on_cpu {
                    Some(c) => format!("*{}", c),
                    None => format!("C{}", kt.last_cpu),
                };
                canvas.draw_text_clipped(area.x + 14, ky + 4, &format!("K{} {}", kt.id, cpu_lbl), pal.text_muted, 1, row_r);
                canvas.draw_text_clipped(area.x + 68, ky + 4, kt.name, pal.accent_green, 1, row_r);
                canvas.draw_text_clipped(area.x + 206, ky + 4, kt.state_label, pal.text_secondary, 1, row_r);
                canvas.draw_text_clipped(area.x + 286, ky + 4, &format!("{}ms", kt.cpu_ms), pal.text_secondary, 1, row_r);
                canvas.draw_text_clipped(
                    area.x + 368,
                    ky + 4,
                    &format!("{}sw / {}pr", kt.switches, kt.preempts),
                    pal.text_secondary,
                    1,
                    row_r,
                );
                canvas.draw_text_clipped(
                    area.x + 472,
                    ky + 4,
                    if kt.guarded_stack { "Guard" } else { "Heap" },
                    if kt.guarded_stack { pal.accent_green } else { pal.text_muted },
                    1,
                    row_r,
                );
            }
        }
    }

    fn draw_drivers(&self, canvas: &mut Canvas<'_>, area: Rect) {
        let pal = theme::palette();
        canvas.fill_rounded_rect(area, 8, pal.surface_panel);
        canvas.stroke_rounded_rect(area, 8, pal.surface_border, 180);

        let iokit_st = crate::drivers::iokit::stats();
        canvas.draw_text_bold_clipped(
            area.x + 12,
            area.y + 8,
            &format!(
                "IOKit IOAPIC: {} vecs | {} IRQs | WorkLoops: net={} aud={} blk={}",
                iokit_st.routed_vectors,
                iokit_st.total_ioapic_irqs,
                iokit_st.net_signals,
                iokit_st.audio_signals,
                iokit_st.blk_signals,
            ),
            pal.accent_primary,
            area,
        );
        canvas.fill_rect(Rect::new(area.x + 8, area.y + 28, area.w - 16, 1), pal.surface_border);

        let count = registry::count();
        let max_rows = ((area.h - 42) / 22).max(1) as usize;

        for i in 0..count.min(max_rows) {
            let Some(dev) = registry::get(i) else { continue };
            let y = area.y + 34 + (i as i32) * 22;
            let status_color = match dev.status {
                DriverStatus::Active => pal.accent_green,
                DriverStatus::Failed => ACCENT_ERR,
                DriverStatus::NotPresent => ACCENT_WARN,
                DriverStatus::Registered => pal.text_secondary,
            };
            canvas.draw_text_clipped(area.x + 12, y, dev.status.name(), status_color, 1, area);
            canvas.draw_text_clipped(area.x + 104, y, dev.name, pal.text_primary, 1, area);
            canvas.draw_text_clipped(area.x + 236, y, dev.details_str(), pal.text_secondary, 1, area);
        }
    }

    fn draw_alerts(&self, canvas: &mut Canvas<'_>, area: Rect) {
        let pal = theme::palette();
        canvas.fill_rounded_rect(area, 8, pal.surface_panel);
        canvas.stroke_rounded_rect(area, 8, pal.surface_border, 180);

        let alert_h = ((area.h * 52) / 100).max(96);
        let list: Vec<_> = alerts::snapshot_history();
        if list.is_empty() {
            canvas.draw_text_clipped(
                area.x + 16,
                area.y + 14,
                "No system alerts or Ring 3 faults captured in this session.",
                pal.accent_green,
                1,
                area,
            );
            canvas.draw_text_clipped(
                area.x + 16,
                area.y + 36,
                "Try 'exec /fault.elf' or 'exec /guard_fault.elf' to inspect guard/#PF isolation.",
                pal.text_secondary,
                1,
                area,
            );
        } else {
            let max_items = ((alert_h - 12) / 48).max(1) as usize;
            for (i, alert) in list.iter().rev().take(max_items).enumerate() {
                let card = Rect::new(area.x + 8, area.y + 8 + (i as i32) * 48, area.w - 16, 44);
                canvas.fill_rounded_rect(card, 6, pal.surface_card);
                canvas.stroke_rounded_rect(card, 6, alert.severity.color(), 150);
                canvas.draw_text_clipped(
                    card.x + 10,
                    card.y + 5,
                    &format!("[{}] {}", alert.code_tag, alert.title),
                    alert.severity.color(),
                    1,
                    card,
                );
                canvas.draw_text_clipped(
                    card.x + 10,
                    card.y + 24,
                    &alert.summary,
                    pal.text_secondary,
                    1,
                    card,
                );
            }
        }

        let klog_y = area.y + alert_h;
        let klog_area = Rect::new(area.x + 8, klog_y, area.w - 16, area.bottom() - klog_y - 8);
        if klog_area.h > 48 {
            canvas.fill_rect(Rect::new(area.x + 8, klog_y - 4, area.w - 16, 1), pal.surface_border);
            let st = crate::klog::stats();
            canvas.draw_text_bold_clipped(
                klog_area.x + 4,
                klog_area.y + 2,
                &format!(
                    "KERNEL LOG (klog: {} events | lazy #PF: {} | copy_user faults: {})",
                    st.total_events, st.pf_lazy_commits, st.copy_user_faults
                ),
                pal.text_muted,
                klog_area,
            );
            let logs = crate::klog::snapshot();
            let max_logs = ((klog_area.h - 24) / 18).max(1) as usize;
            for (i, entry) in logs.iter().rev().take(max_logs).enumerate() {
                let ly = klog_area.y + 22 + (i as i32) * 18;
                let col = match entry.level {
                    crate::klog::KLogLevel::Fault => ACCENT_ERR,
                    crate::klog::KLogLevel::Warn => ACCENT_WARN,
                    crate::klog::KLogLevel::Preempt => pal.accent_green,
                    crate::klog::KLogLevel::Mm => pal.accent_primary,
                    crate::klog::KLogLevel::Info => pal.text_secondary,
                };
                canvas.draw_text_clipped(
                    klog_area.x + 4,
                    ly,
                    &format!("[{}ms] {} [{}] {}", entry.uptime_ms, entry.level.label(), entry.tag, entry.msg),
                    col,
                    1,
                    klog_area,
                );
            }
        }
    }
}
