use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::cell::Cell;

use crate::graphics::alerts::{self, AlertSeverity, AlertSubsystem};
use crate::graphics::icons::{
    file_grid_icon, file_list_icon, FileIconKind, LucideIcon, GRID_ICON_SIZE, LIST_ICON_SIZE,
};
use crate::graphics::theme::{self, ACCENT_ERR, ACCENT_WARN};
use crate::graphics::widgets::{text_width, text_width_bold, Canvas, Rect};
use crate::input::{KeyCode, KeyEvent};
use crate::interrupts::timer;
use crate::storage::vfs::{self, VfsDirEntry, VfsError, VfsNodeKind};

use super::{BuiltinAppKind, DesktopAction};

const SIDEBAR_W: i32 = 144;
const TOOLBAR_H: i32 = 38;
const BREADCRUMB_H: i32 = 30;
const STATUS_H: i32 = 24;
const ROW_H: i32 = 26;
const GRID_CELL_W: i32 = 90;
const GRID_CELL_H: i32 = 76;
const MENU_W: i32 = 184;
const MENU_ITEM_H: i32 = 26;

#[derive(Clone, Copy, PartialEq, Eq)]
enum PromptMode {
    None,
    NewFile,
    NewDir,
    Rename,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum ViewMode {
    List,
    Grid,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SortMode {
    Name,
    Size,
}

#[derive(Clone)]
struct FileClipboard {
    src_path: String,
    name: String,
    is_cut: bool,
}

#[derive(Clone, Copy)]
struct FileContextMenu {
    rect: Rect,
    entry_idx: Option<usize>,
}

pub struct FilesApp {
    current_path: String,
    history: Vec<String>,
    history_idx: usize,
    entries: Vec<VfsDirEntry>,
    selected_idx: Option<usize>,
    last_click_idx: Option<usize>,
    last_click_ms: u64,
    view_mode: ViewMode,
    sort_mode: SortMode,
    clipboard: Option<FileClipboard>,
    preview_title: String,
    preview_lines: Vec<String>,
    status_msg: String,
    scroll: usize,
    last_max_visible: Cell<usize>,
    last_grid_cols: Cell<usize>,
    prompt_mode: PromptMode,
    prompt_input: String,
    rename_src_path: Option<String>,
    context_menu: Option<FileContextMenu>,
}

impl FilesApp {
    pub fn new() -> Self {
        let mut initial_hist = Vec::new();
        initial_hist.push(String::from("/"));
        let mut app = Self {
            current_path: String::from("/"),
            history: initial_hist,
            history_idx: 0,
            entries: Vec::new(),
            selected_idx: None,
            last_click_idx: None,
            last_click_ms: 0,
            view_mode: ViewMode::List,
            sort_mode: SortMode::Name,
            clipboard: None,
            preview_title: String::from("Inspector"),
            preview_lines: Vec::new(),
            status_msg: String::from("Double-click to open  |  Ctrl+C/X/V  |  F2 Rename"),
            scroll: 0,
            last_max_visible: Cell::new(12),
            last_grid_cols: Cell::new(4),
            prompt_mode: PromptMode::None,
            prompt_input: String::new(),
            rename_src_path: None,
            context_menu: None,
        };
        app.refresh();
        app
    }

    fn sort_entries(&mut self) {
        self.entries.retain(|e| e.name != "." && e.name != "..");
        let sort_mode = self.sort_mode;
        self.entries.sort_by(|a, b| {
            let a_dir = a.kind == VfsNodeKind::Directory;
            let b_dir = b.kind == VfsNodeKind::Directory;
            if a_dir != b_dir {
                return b_dir.cmp(&a_dir);
            }
            match sort_mode {
                SortMode::Name => cmp_ascii_ci(&a.name, &b.name),
                SortMode::Size => b.size.cmp(&a.size).then_with(|| cmp_ascii_ci(&a.name, &b.name)),
            }
        });
    }

    pub fn refresh(&mut self) {
        let path = self.current_path.clone();
        match vfs::with_vfs(|state| {
            let abs = state.resolve_path(&path)?;
            let list = state.list_dir(&abs)?;
            let info = state.volume_info_for(&abs);
            Ok::<_, VfsError>((abs, list, info))
        }) {
            Ok((abs, list, info_res)) => {
                self.current_path = abs;
                self.entries = list;
                self.sort_entries();
                self.selected_idx = None;
                self.scroll = 0;
                if let Ok(info) = info_res {
                    let ro = if info.read_only { "RO" } else { "RW" };
                    let mount_tag = if self.current_path.starts_with("/disk") {
                        "/disk"
                    } else {
                        "/ (RAMFS)"
                    };
                    self.status_msg = format!(
                        "{}: {} ({})  |  {} items  |  Ctrl+C/X/V  F2 Rename",
                        mount_tag,
                        info.fs_type,
                        ro,
                        self.entries.len()
                    );
                }
            }
            Err(err) => {
                self.entries.clear();
                self.status_msg = format!("VFS Error: {}", err.message());
                alerts::report_system_error(
                    AlertSeverity::Warning,
                    AlertSubsystem::StorageVfs,
                    "VFS READ",
                    "Failed to list directory",
                    err.message(),
                    "Root '/' uses writable RAMFS; '/disk' mounts ATA FAT32/FAT16 drives via 'mount'.",
                    false,
                );
            }
        }
    }

    fn navigate_to(&mut self, path: &str) {
        let resolved = vfs::with_vfs(|s| s.resolve_path(path)).unwrap_or_else(|_| String::from(path));
        if self.current_path != resolved {
            self.history.truncate(self.history_idx + 1);
            if self.history.last().map(|s| s.as_str()) != Some(resolved.as_str()) {
                self.history.push(resolved.clone());
                if self.history.len() > 48 {
                    self.history.remove(0);
                }
            }
            self.history_idx = self.history.len().saturating_sub(1);
        }
        self.current_path = resolved;
        self.context_menu = None;
        self.prompt_mode = PromptMode::None;
        self.refresh();
    }

    pub fn can_go_back(&self) -> bool {
        self.history_idx > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.history_idx + 1 < self.history.len()
    }

    pub fn go_back(&mut self) {
        if self.history_idx > 0 {
            self.history_idx -= 1;
            self.current_path = self.history[self.history_idx].clone();
            self.context_menu = None;
            self.refresh();
        }
    }

    pub fn go_forward(&mut self) {
        if self.history_idx + 1 < self.history.len() {
            self.history_idx += 1;
            self.current_path = self.history[self.history_idx].clone();
            self.context_menu = None;
            self.refresh();
        }
    }

    pub fn go_up(&mut self) {
        if self.current_path == "/" {
            return;
        }
        if let Some(pos) = self.current_path.rfind('/') {
            let parent = if pos == 0 { "/" } else { &self.current_path[..pos] };
            let target = String::from(parent);
            self.navigate_to(&target);
        }
    }

    pub fn dismiss_context_menu(&mut self) -> bool {
        if self.context_menu.is_some() {
            self.context_menu = None;
            true
        } else {
            false
        }
    }

    fn full_child_path(&self, name: &str) -> String {
        if self.current_path == "/" {
            format!("/{}", name)
        } else {
            format!("{}/{}", self.current_path, name)
        }
    }

    pub fn begin_new_file(&mut self) {
        self.context_menu = None;
        self.prompt_mode = PromptMode::NewFile;
        self.prompt_input = String::from("note.txt");
        self.rename_src_path = None;
        self.status_msg = String::from("New file name (Enter to create, Esc to cancel)");
    }

    pub fn begin_new_dir(&mut self) {
        self.context_menu = None;
        self.prompt_mode = PromptMode::NewDir;
        self.prompt_input = String::from("folder");
        self.rename_src_path = None;
        self.status_msg = String::from("New folder name (Enter to create, Esc to cancel)");
    }

    pub fn begin_rename(&mut self) {
        self.context_menu = None;
        let Some(idx) = self.selected_idx else {
            self.status_msg = String::from("Select a file or folder to rename (F2)");
            return;
        };
        let Some(entry) = self.entries.get(idx).cloned() else {
            return;
        };
        let src = self.full_child_path(&entry.name);
        if src == "/disk" || src == "/" {
            self.status_msg = String::from("Cannot rename mount root '/disk'");
            return;
        }
        self.prompt_mode = PromptMode::Rename;
        self.prompt_input = entry.name;
        self.rename_src_path = Some(src);
        self.status_msg = String::from("Rename item (Enter to apply, Esc to cancel)");
    }

    pub fn copy_selected(&mut self) {
        self.context_menu = None;
        let Some(idx) = self.selected_idx else {
            self.status_msg = String::from("Select a file or folder to copy");
            return;
        };
        let Some(entry) = self.entries.get(idx).cloned() else {
            return;
        };
        let src = self.full_child_path(&entry.name);
        if src == "/disk" {
            self.status_msg = String::from("Cannot copy mount point '/disk'");
            return;
        }
        self.status_msg = format!("Copied '{}' to file clipboard", entry.name);
        self.clipboard = Some(FileClipboard {
            src_path: src,
            name: entry.name,
            is_cut: false,
        });
    }

    pub fn cut_selected(&mut self) {
        self.context_menu = None;
        let Some(idx) = self.selected_idx else {
            self.status_msg = String::from("Select a file or folder to cut");
            return;
        };
        let Some(entry) = self.entries.get(idx).cloned() else {
            return;
        };
        let src = self.full_child_path(&entry.name);
        if src == "/disk" {
            self.status_msg = String::from("Cannot move mount point '/disk'");
            return;
        }
        self.status_msg = format!("Cut '{}' — open target folder & press Ctrl+V", entry.name);
        self.clipboard = Some(FileClipboard {
            src_path: src,
            name: entry.name,
            is_cut: true,
        });
    }

    pub fn paste_clipboard(&mut self) {
        self.context_menu = None;
        let Some(cb) = self.clipboard.clone() else {
            self.status_msg = String::from("File clipboard is empty");
            return;
        };

        let mut dst = self.full_child_path(&cb.name);
        if cb.is_cut && dst == cb.src_path {
            self.clipboard = None;
            self.status_msg = String::from("Item is already in this folder");
            return;
        }

        if !cb.is_cut && vfs::with_vfs(|s| s.stat(&dst).is_ok()) {
            dst = self.unique_copy_path(&cb.name);
        }

        let op_res = if cb.is_cut {
            vfs::with_vfs(|s| s.rename_path(&cb.src_path, &dst))
        } else {
            vfs::with_vfs(|s| s.copy_path(&cb.src_path, &dst))
        };

        match op_res {
            Ok(()) => {
                if cb.is_cut {
                    self.clipboard = None;
                }
                self.refresh();
                self.status_msg = format!(
                    "{} '{}' -> '{}'",
                    if cb.is_cut { "Moved" } else { "Copied" },
                    cb.src_path,
                    dst
                );
            }
            Err(e) => {
                self.status_msg = format!("Paste failed: {}", e.message());
                alerts::report_system_error(
                    AlertSeverity::Warning,
                    AlertSubsystem::StorageVfs,
                    if cb.is_cut { "VFS MOVE" } else { "VFS COPY" },
                    &format!("Failed to paste '{}'", cb.src_path),
                    e.message(),
                    "Ensure target volume is writable (RAMFS '/' or FAT32 '/disk') and names are valid.",
                    false,
                );
            }
        }
    }

    fn unique_copy_path(&self, orig_name: &str) -> String {
        let (stem, ext) = if let Some(dot) = orig_name.rfind('.') {
            if dot > 0 {
                (&orig_name[..dot], &orig_name[dot..])
            } else {
                (orig_name, "")
            }
        } else {
            (orig_name, "")
        };

        for n in 1usize..=99 {
            let candidate_name = if n == 1 {
                format!("{}_copy{}", stem, ext)
            } else {
                format!("{}_copy{}{}", stem, n, ext)
            };
            let full = self.full_child_path(&candidate_name);
            if vfs::with_vfs(|s| s.stat(&full).is_err()) {
                return full;
            }
        }
        self.full_child_path(&format!("{}_copy", stem))
    }

    fn delete_selected(&mut self) {
        self.context_menu = None;
        let Some(idx) = self.selected_idx else {
            self.status_msg = String::from("Select a file or folder to delete");
            return;
        };
        let Some(entry) = self.entries.get(idx).cloned() else {
            return;
        };
        let path = self.full_child_path(&entry.name);
        if path == "/disk" || path == "/" {
            self.status_msg = String::from("Cannot delete '/disk' mount point");
            return;
        }
        match vfs::with_vfs(|s| s.remove_recursive(&path)) {
            Ok(()) => {
                self.preview_title = String::from("Inspector");
                self.preview_lines.clear();
                self.refresh();
                self.status_msg = format!("Deleted '{}'", path);
            }
            Err(e) => {
                self.status_msg = format!("Delete failed: {}", e.message());
                alerts::report_system_error(
                    AlertSeverity::Warning,
                    AlertSubsystem::StorageVfs,
                    "VFS REMOVE",
                    &format!("Could not remove '{}'", path),
                    e.message(),
                    "Ensure the filesystem is writable (RAMFS or FAT32).",
                    false,
                );
            }
        }
    }

    fn open_selected_entry(&mut self) -> DesktopAction {
        let Some(idx) = self.selected_idx else {
            return DesktopAction::None;
        };
        let Some(entry) = self.entries.get(idx).cloned() else {
            return DesktopAction::None;
        };
        if entry.kind == VfsNodeKind::Directory {
            let next = self.full_child_path(&entry.name);
            self.navigate_to(&next);
            return DesktopAction::None;
        }
        let path = self.full_child_path(&entry.name);
        let lower = entry.name.to_ascii_lowercase();
        if lower.ends_with(".elf") {
            DesktopAction::ExecElf(path)
        } else if lower.ends_with(".wav") {
            DesktopAction::PlayWav(path)
        } else if lower.ends_with(".bmp") || lower.ends_with(".tga") || lower.ends_with(".ppm") {
            DesktopAction::SetCustomWallpaper(path)
        } else {
            DesktopAction::OpenEditor(path)
        }
    }

    fn edit_selected_in_editor(&self) -> DesktopAction {
        if let Some(idx) = self.selected_idx {
            if let Some(entry) = self.entries.get(idx) {
                if entry.kind == VfsNodeKind::File {
                    return DesktopAction::OpenEditor(self.full_child_path(&entry.name));
                }
            }
        }
        DesktopAction::OpenApp(BuiltinAppKind::Editor)
    }

    fn commit_prompt(&mut self) -> DesktopAction {
        let mode = self.prompt_mode;
        let name = String::from(self.prompt_input.trim());
        let rename_src = self.rename_src_path.take();
        self.prompt_mode = PromptMode::None;
        self.prompt_input.clear();

        if name.is_empty() || name.contains('/') || name == "." || name == ".." {
            self.status_msg = String::from("Cancelled: enter a valid file or folder name");
            return DesktopAction::None;
        }
        let target = self.full_child_path(&name);
        match mode {
            PromptMode::NewFile => match vfs::with_vfs(|s| s.write_file(&target, b"")) {
                Ok(()) => {
                    self.refresh();
                    self.status_msg = format!("Created '{}' — opening in Text Editor", target);
                    return DesktopAction::OpenEditor(target);
                }
                Err(e) => {
                    self.status_msg = format!("Create file failed: {}", e.message());
                    alerts::report_system_error(
                        AlertSeverity::Warning,
                        AlertSubsystem::StorageVfs,
                        "VFS WRITE",
                        &format!("Failed to create '{}'", target),
                        e.message(),
                        "Root '/' uses writable RAMFS; '/disk' requires a writable FAT32 volume.",
                        false,
                    );
                }
            },
            PromptMode::NewDir => match vfs::with_vfs(|s| s.create_dir(&target)) {
                Ok(()) => {
                    self.refresh();
                    self.status_msg = format!("Created directory '{}'", target);
                }
                Err(e) => {
                    self.status_msg = format!("Create folder failed: {}", e.message());
                    alerts::report_system_error(
                        AlertSeverity::Warning,
                        AlertSubsystem::StorageVfs,
                        "VFS MKDIR",
                        &format!("Failed to create directory '{}'", target),
                        e.message(),
                        "Root '/' uses writable RAMFS; '/disk' requires a writable FAT32 volume.",
                        false,
                    );
                }
            },
            PromptMode::Rename => {
                if let Some(src) = rename_src {
                    match vfs::with_vfs(|s| s.rename_path(&src, &target)) {
                        Ok(()) => {
                            self.refresh();
                            if let Some(pos) = self.entries.iter().position(|e| e.name == name) {
                                self.selected_idx = Some(pos);
                                let e = self.entries[pos].clone();
                                self.load_preview(&e);
                            }
                            self.status_msg = format!("Renamed to '{}'", name);
                        }
                        Err(e) => {
                            self.status_msg = format!("Rename failed: {}", e.message());
                            alerts::report_system_error(
                                AlertSeverity::Warning,
                                AlertSubsystem::StorageVfs,
                                "VFS RENAME",
                                &format!("Failed to rename '{}'", src),
                                e.message(),
                                "Ensure target name is unused and volume is writable.",
                                false,
                            );
                        }
                    }
                }
            }
            PromptMode::None => {}
        }
        DesktopAction::None
    }

    fn load_preview(&mut self, entry: &VfsDirEntry) {
        self.preview_title = entry.name.clone();
        self.preview_lines.clear();

        if entry.kind == VfsNodeKind::Directory {
            self.preview_lines.push(String::from("Type: Folder"));
            self.preview_lines.push(format!("Cluster: {}", entry.first_cluster));
            self.preview_lines.push(String::from("Double-click to open."));
            return;
        }

        self.preview_lines.push(format!("Size: {} bytes", entry.size));
        self.preview_lines.push(format!("Cluster: {}", entry.first_cluster));
        self.preview_lines.push(String::from("------------------"));

        let path = self.full_child_path(&entry.name);
        if entry.name.ends_with(".elf") {
            self.preview_lines.push(String::from("Ring 3 ELF64 Binary"));
            self.preview_lines.push(String::from("Double-click to run"));
            self.preview_lines.push(String::from("in background."));
            return;
        }
        if entry.name.ends_with(".wav") {
            self.preview_lines.push(String::from("RIFF WAV PCM Audio"));
            self.preview_lines.push(String::from("Double-click to play."));
            return;
        }

        let to_read = (entry.size as usize).min(1024);
        let mut buf = alloc::vec![0u8; to_read];
        match vfs::with_vfs(|state| state.read_file_chunk(&path, 0, &mut buf)) {
            Ok(n) => {
                let text = core::str::from_utf8(&buf[..n]).unwrap_or("<binary data>");
                for line in text.lines().take(12) {
                    self.preview_lines.push(String::from(line));
                }
                if text.is_empty() {
                    self.preview_lines.push(String::from("<empty file>"));
                }
            }
            Err(e) => {
                self.preview_lines.push(format!("Error reading: {}", e.message()));
            }
        }
    }

    fn content_view_rect(area: Rect) -> Rect {
        let preview_w = 176;
        Rect::new(
            area.x + SIDEBAR_W + 6,
            area.y + TOOLBAR_H + BREADCRUMB_H + 4,
            (area.w - SIDEBAR_W - preview_w - 14).max(140),
            (area.h - TOOLBAR_H - BREADCRUMB_H - STATUS_H - 8).max(80),
        )
    }

    fn breadcrumb_chips(&self, area: Rect) -> Vec<(Rect, String, String)> {
        let mut out = Vec::new();
        let nav_x = area.x + SIDEBAR_W + 6;
        let bar_y = area.y + TOOLBAR_H + 4;
        // [<] [>] [^] take 3 * 26 + 8 = 86px
        let mut cx = nav_x + 90;
        let max_right = area.right() - 122;

        let root_w = (text_width("/", 1) + 14).max(24);
        if cx + root_w <= max_right {
            out.push((
                Rect::new(cx, bar_y + 1, root_w, 22),
                String::from("/"),
                String::from("/"),
            ));
            cx += root_w + 14;
        }

        let mut acc = String::new();
        for seg in self.current_path.split('/').filter(|s| !s.is_empty()) {
            acc.push('/');
            acc.push_str(seg);
            let w = (text_width(seg, 1) + 14).min(120).max(28);
            if cx + w > max_right {
                break;
            }
            out.push((
                Rect::new(cx, bar_y + 1, w, 22),
                String::from(seg),
                acc.clone(),
            ));
            cx += w + 14;
        }
        out
    }

    fn hit_test_entry(&self, view_rect: Rect, mx: i32, my: i32) -> Option<usize> {
        if !view_rect.contains(mx, my) {
            return None;
        }
        match self.view_mode {
            ViewMode::List => {
                let row = ((my - (view_rect.y + 6)) / ROW_H).max(0) as usize;
                let idx = self.scroll + row;
                if idx < self.entries.len() {
                    Some(idx)
                } else {
                    None
                }
            }
            ViewMode::Grid => {
                let cols = ((view_rect.w - 18) / GRID_CELL_W).max(1) as usize;
                let lx = (mx - (view_rect.x + 6)).max(0);
                let ly = (my - (view_rect.y + 6)).max(0);
                let col = (lx / GRID_CELL_W) as usize;
                let row = (ly / GRID_CELL_H) as usize;
                if col >= cols {
                    return None;
                }
                let idx = self.scroll + row * cols + col;
                if idx < self.entries.len() {
                    Some(idx)
                } else {
                    None
                }
            }
        }
    }

    fn toolbar_rects(area: Rect) -> [Rect; 7] {
        let x0 = area.x + SIDEBAR_W + 8;
        let y0 = area.y + 6;
        [
            Rect::new(x0, y0, 62, 26),            // 0: +File
            Rect::new(x0 + 67, y0, 62, 26),       // 1: +Dir
            Rect::new(x0 + 134, y0, 56, 26),      // 2: Copy
            Rect::new(x0 + 195, y0, 48, 26),      // 3: Cut
            Rect::new(x0 + 248, y0, 60, 26),      // 4: Paste
            Rect::new(x0 + 313, y0, 64, 26),      // 5: Rename
            Rect::new(x0 + 382, y0, 52, 26),      // 6: Del
        ]
    }

    pub fn on_wheel(&mut self, delta: i8) {
        let step = match self.view_mode {
            ViewMode::List => 2usize,
            ViewMode::Grid => self.last_grid_cols.get().max(1),
        };
        let max_vis = self.last_max_visible.get().max(1);
        let max_scroll = self.entries.len().saturating_sub(max_vis);
        if delta > 0 {
            self.scroll = (self.scroll + step).min(max_scroll);
        } else if delta < 0 {
            self.scroll = self.scroll.saturating_sub(step);
        }
    }

    pub fn on_key(&mut self, evt: KeyEvent) -> DesktopAction {
        if self.prompt_mode != PromptMode::None {
            match evt.code {
                KeyCode::Enter => return self.commit_prompt(),
                KeyCode::Escape => {
                    self.prompt_mode = PromptMode::None;
                    self.prompt_input.clear();
                    self.rename_src_path = None;
                    self.status_msg = String::from("Cancelled");
                }
                KeyCode::Backspace => {
                    self.prompt_input.pop();
                }
                KeyCode::Char(c) if c.is_ascii() && !c.is_ascii_control() && c != '/' => {
                    if self.prompt_input.len() < 48 {
                        self.prompt_input.push(c);
                    }
                }
                _ => {}
            }
            return DesktopAction::None;
        }

        if self.context_menu.is_some() && evt.code == KeyCode::Escape {
            self.context_menu = None;
            return DesktopAction::None;
        }

        if evt.modifiers.ctrl {
            match evt.code {
                KeyCode::Char('c') | KeyCode::Char('C') => {
                    self.copy_selected();
                    return DesktopAction::None;
                }
                KeyCode::Char('x') | KeyCode::Char('X') => {
                    self.cut_selected();
                    return DesktopAction::None;
                }
                KeyCode::Char('v') | KeyCode::Char('V') => {
                    self.paste_clipboard();
                    return DesktopAction::None;
                }
                KeyCode::Char('r') | KeyCode::Char('R') => {
                    self.refresh();
                    return DesktopAction::None;
                }
                _ => {}
            }
        }

        if evt.modifiers.alt {
            match evt.code {
                KeyCode::Left => {
                    self.go_back();
                    return DesktopAction::None;
                }
                KeyCode::Right => {
                    self.go_forward();
                    return DesktopAction::None;
                }
                KeyCode::Up => {
                    self.go_up();
                    return DesktopAction::None;
                }
                _ => {}
            }
        }

        let step = match self.view_mode {
            ViewMode::List => 1usize,
            ViewMode::Grid => self.last_grid_cols.get().max(1),
        };

        match evt.code {
            KeyCode::Up => {
                if let Some(idx) = self.selected_idx {
                    let next = idx.saturating_sub(step);
                    self.selected_idx = Some(next);
                    if let Some(e) = self.entries.get(next).cloned() {
                        self.load_preview(&e);
                    }
                } else if !self.entries.is_empty() {
                    self.selected_idx = Some(0);
                    let e = self.entries[0].clone();
                    self.load_preview(&e);
                }
            }
            KeyCode::Down => {
                if let Some(idx) = self.selected_idx {
                    let next = (idx + step).min(self.entries.len().saturating_sub(1));
                    self.selected_idx = Some(next);
                    if let Some(e) = self.entries.get(next).cloned() {
                        self.load_preview(&e);
                    }
                } else if !self.entries.is_empty() {
                    self.selected_idx = Some(0);
                    let e = self.entries[0].clone();
                    self.load_preview(&e);
                }
            }
            KeyCode::Left if self.view_mode == ViewMode::Grid => {
                if let Some(idx) = self.selected_idx {
                    if idx > 0 {
                        self.selected_idx = Some(idx - 1);
                        let e = self.entries[idx - 1].clone();
                        self.load_preview(&e);
                    }
                }
            }
            KeyCode::Right if self.view_mode == ViewMode::Grid => {
                if let Some(idx) = self.selected_idx {
                    if idx + 1 < self.entries.len() {
                        self.selected_idx = Some(idx + 1);
                        let e = self.entries[idx + 1].clone();
                        self.load_preview(&e);
                    }
                }
            }
            KeyCode::Backspace => self.go_up(),
            KeyCode::Enter => return self.open_selected_entry(),
            KeyCode::F2 => self.begin_rename(),
            KeyCode::F5 => self.refresh(),
            KeyCode::Delete => self.delete_selected(),
            _ => {}
        }
        DesktopAction::None
    }

    /// Returns an optional previously-open menu rectangle so WM can guarantee clean background invalidation.
    pub fn on_right_click(&mut self, area: Rect, mx: i32, my: i32) -> Option<Rect> {
        let prev_rect = self.context_menu.map(|cm| cm.rect);
        let view_rect = Self::content_view_rect(area);
        if !view_rect.contains(mx, my) {
            self.context_menu = None;
            return prev_rect;
        }

        let entry_idx = self.hit_test_entry(view_rect, mx, my);
        if let Some(idx) = entry_idx {
            self.selected_idx = Some(idx);
            let e = self.entries[idx].clone();
            self.load_preview(&e);
        }

        let item_count = 9i32;
        let menu_h = 10 + item_count * MENU_ITEM_H;
        let cx = mx.min(area.right() - MENU_W - 6).max(area.x + 6);
        let cy = my.min(area.bottom() - menu_h - 6).max(area.y + 6);
        let rect = Rect::new(cx, cy, MENU_W, menu_h);
        self.context_menu = Some(FileContextMenu { rect, entry_idx });
        prev_rect
    }

    pub fn on_click(&mut self, area: Rect, mx: i32, my: i32) -> DesktopAction {
        if let Some(cm) = self.context_menu {
            self.context_menu = None;
            if cm.rect.contains(mx, my) {
                let row = ((my - cm.rect.y - 5) / MENU_ITEM_H).clamp(0, 8);
                match row {
                    0 => {
                        if cm.entry_idx.is_some() {
                            return self.open_selected_entry();
                        }
                    }
                    1 => {
                        if cm.entry_idx.is_some() {
                            self.begin_rename();
                        }
                    }
                    2 => {
                        if cm.entry_idx.is_some() {
                            self.copy_selected();
                        }
                    }
                    3 => {
                        if cm.entry_idx.is_some() {
                            self.cut_selected();
                        }
                    }
                    4 => self.paste_clipboard(),
                    5 => self.begin_new_file(),
                    6 => self.begin_new_dir(),
                    7 => {
                        if cm.entry_idx.is_some() {
                            self.delete_selected();
                        }
                    }
                    _ => self.refresh(),
                }
                return DesktopAction::None;
            }
        }

        if self.prompt_mode != PromptMode::None {
            let pbox = Rect::new(
                area.x + SIDEBAR_W + 10,
                area.y + TOOLBAR_H + BREADCRUMB_H + 6,
                area.w - SIDEBAR_W - 20,
                34,
            );
            let btn_ok = Rect::new(pbox.right() - 122, pbox.y + 4, 56, 26);
            let btn_cancel = Rect::new(pbox.right() - 60, pbox.y + 4, 54, 26);
            if btn_ok.contains(mx, my) {
                return self.commit_prompt();
            }
            if btn_cancel.contains(mx, my) {
                self.prompt_mode = PromptMode::None;
                self.prompt_input.clear();
                self.rename_src_path = None;
                self.status_msg = String::from("Cancelled");
                return DesktopAction::None;
            }
            if pbox.contains(mx, my) {
                return DesktopAction::None;
            }
        }

        // Toolbar buttons
        let tb = Self::toolbar_rects(area);
        let btn_run = Rect::new(area.right() - 62, area.y + 6, 54, 26);
        if tb[0].contains(mx, my) {
            self.begin_new_file();
            return DesktopAction::None;
        }
        if tb[1].contains(mx, my) {
            self.begin_new_dir();
            return DesktopAction::None;
        }
        if tb[2].contains(mx, my) {
            self.copy_selected();
            return DesktopAction::None;
        }
        if tb[3].contains(mx, my) {
            self.cut_selected();
            return DesktopAction::None;
        }
        if tb[4].contains(mx, my) {
            self.paste_clipboard();
            return DesktopAction::None;
        }
        if tb[5].contains(mx, my) {
            self.begin_rename();
            return DesktopAction::None;
        }
        if tb[6].contains(mx, my) {
            self.delete_selected();
            return DesktopAction::None;
        }
        if btn_run.contains(mx, my) {
            return self.open_selected_entry();
        }

        // Breadcrumb bar: [<] [>] [^] + chips + [Sort] [View]
        let nav_x = area.x + SIDEBAR_W + 6;
        let bar_y = area.y + TOOLBAR_H + 4;
        let btn_back = Rect::new(nav_x, bar_y, 26, 24);
        let btn_fwd = Rect::new(nav_x + 28, bar_y, 26, 24);
        let btn_up = Rect::new(nav_x + 56, bar_y, 26, 24);
        let btn_sort = Rect::new(area.right() - 114, bar_y, 52, 24);
        let btn_view = Rect::new(area.right() - 58, bar_y, 50, 24);

        if btn_back.contains(mx, my) {
            self.go_back();
            return DesktopAction::None;
        }
        if btn_fwd.contains(mx, my) {
            self.go_forward();
            return DesktopAction::None;
        }
        if btn_up.contains(mx, my) {
            self.go_up();
            return DesktopAction::None;
        }
        if btn_sort.contains(mx, my) {
            self.sort_mode = match self.sort_mode {
                SortMode::Name => SortMode::Size,
                SortMode::Size => SortMode::Name,
            };
            self.sort_entries();
            self.status_msg = String::from(match self.sort_mode {
                SortMode::Name => "Sorted by Name",
                SortMode::Size => "Sorted by Size",
            });
            return DesktopAction::None;
        }
        if btn_view.contains(mx, my) {
            self.view_mode = match self.view_mode {
                ViewMode::List => ViewMode::Grid,
                ViewMode::Grid => ViewMode::List,
            };
            self.scroll = 0;
            return DesktopAction::None;
        }

        for (chip_r, _, target_path) in self.breadcrumb_chips(area) {
            if chip_r.contains(mx, my) {
                self.navigate_to(&target_path);
                return DesktopAction::None;
            }
        }

        // Sidebar items
        let side_items = [
            Rect::new(area.x + 8, area.y + 42, SIDEBAR_W - 16, 28),
            Rect::new(area.x + 8, area.y + 74, SIDEBAR_W - 16, 28),
            Rect::new(area.x + 8, area.y + 106, SIDEBAR_W - 16, 28),
            Rect::new(area.x + 8, area.y + 138, SIDEBAR_W - 16, 28),
            Rect::new(area.x + 8, area.y + 170, SIDEBAR_W - 16, 28),
            Rect::new(area.x + 8, area.y + 202, SIDEBAR_W - 16, 28),
        ];
        if side_items[0].contains(mx, my) {
            self.navigate_to("/");
            return DesktopAction::None;
        }
        if side_items[1].contains(mx, my) {
            self.navigate_to("/disk");
            return DesktopAction::None;
        }
        if side_items[2].contains(mx, my) {
            self.navigate_to("/home");
            return DesktopAction::None;
        }
        if side_items[3].contains(mx, my) {
            return DesktopAction::RunShellCommand(String::from("mount auto"));
        }
        if side_items[4].contains(mx, my) {
            return DesktopAction::OpenApp(BuiltinAppKind::Editor);
        }
        if side_items[5].contains(mx, my) {
            return DesktopAction::OpenApp(BuiltinAppKind::Terminal);
        }

        // Main directory view (single-click selects; double-click opens!)
        let view_rect = Self::content_view_rect(area);
        if let Some(idx) = self.hit_test_entry(view_rect, mx, my) {
            let now_ms = timer::uptime_ms();
            let is_double = self.last_click_idx == Some(idx)
                && now_ms.saturating_sub(self.last_click_ms) <= 480;
            self.last_click_idx = Some(idx);
            self.last_click_ms = now_ms;
            self.selected_idx = Some(idx);
            let entry = self.entries[idx].clone();
            self.load_preview(&entry);
            if is_double {
                self.last_click_idx = None;
                return self.open_selected_entry();
            }
        } else if view_rect.contains(mx, my) {
            self.selected_idx = None;
            self.last_click_idx = None;
        }

        DesktopAction::None
    }

    pub fn draw(&self, canvas: &mut Canvas<'_>, area: Rect) {
        let pal = theme::palette();
        canvas.fill_rect(area, pal.win_bg_active);

        // Sidebar
        let sidebar = Rect::new(area.x, area.y, SIDEBAR_W, area.h);
        canvas.fill_rect(sidebar, pal.surface_sidebar);
        canvas.fill_rect(Rect::new(area.x + SIDEBAR_W - 1, area.y, 1, area.h), pal.surface_border);

        canvas.draw_text_bold(area.x + 12, area.y + 12, "LOCATIONS", pal.text_muted);

        let on_disk = self.current_path == "/disk" || self.current_path.starts_with("/disk/");
        let on_home = self.current_path == "/home" || self.current_path.starts_with("/home/");
        let on_root = !on_disk && !on_home;

        let side_items = [
            (Rect::new(area.x + 8, area.y + 42, SIDEBAR_W - 16, 28), "/ (RAMFS)", LucideIcon::Folder, on_root),
            (Rect::new(area.x + 8, area.y + 74, SIDEBAR_W - 16, 28), "/disk (FAT)", LucideIcon::HardDrive, on_disk),
            (Rect::new(area.x + 8, area.y + 106, SIDEBAR_W - 16, 28), "/home", LucideIcon::Home, on_home),
            (Rect::new(area.x + 8, area.y + 138, SIDEBAR_W - 16, 28), "Remount", LucideIcon::RefreshCw, false),
            (Rect::new(area.x + 8, area.y + 170, SIDEBAR_W - 16, 28), "Text Editor", LucideIcon::Pencil, false),
            (Rect::new(area.x + 8, area.y + 202, SIDEBAR_W - 16, 28), "Terminal", LucideIcon::Terminal, false),
        ];
        for (rect, label, icon, active) in side_items {
            if active {
                canvas.fill_rounded_rect(rect, 7, pal.surface_selected);
            }
            let ic_col = if active { pal.accent_primary } else { pal.text_secondary };
            canvas.draw_lucide_icon(rect.x + 7, rect.y + 6, icon, ic_col);
            canvas.draw_text_clipped(
                rect.x + 28,
                rect.y + 6,
                label,
                if active { pal.text_primary } else { pal.text_primary },
                1,
                rect,
            );
        }

        // Toolbar strip
        let toolbar = Rect::new(area.x + SIDEBAR_W, area.y, area.w - SIDEBAR_W, TOOLBAR_H);
        canvas.fill_rect(toolbar, pal.surface_toolbar);
        canvas.fill_rect(Rect::new(toolbar.x, toolbar.bottom() - 1, toolbar.w, 1), pal.surface_border);

        let tb = Self::toolbar_rects(area);
        let has_sel = self.selected_idx.is_some();
        let has_cb = self.clipboard.is_some();
        let tb_meta = [
            (tb[0], "+File", LucideIcon::FilePlus, pal.accent_primary, true),
            (tb[1], "+Dir", LucideIcon::FolderPlus, pal.accent_green, true),
            (tb[2], "Copy", LucideIcon::Copy, pal.text_primary, has_sel),
            (tb[3], "Cut", LucideIcon::Scissors, pal.text_primary, has_sel),
            (tb[4], "Paste", LucideIcon::ClipboardPaste, pal.accent_primary, has_cb),
            (tb[5], "Ren", LucideIcon::Pencil, pal.text_primary, has_sel),
            (tb[6], "Del", LucideIcon::Trash2, ACCENT_ERR, has_sel),
        ];
        for (r, txt, icon, fg, enabled) in tb_meta {
            if r.right() <= area.right() - 70 {
                canvas.fill_rounded_rect(r, 6, pal.surface_card);
                canvas.stroke_rounded_rect(r, 6, pal.surface_border, 185);
                let col = if enabled { fg } else { pal.text_muted };
                canvas.draw_lucide_icon(r.x + 5, r.y + 5, icon, col);
                canvas.draw_text_clipped(r.x + 23, r.y + 5, txt, col, 1, r);
            }
        }
        let btn_run = Rect::new(area.right() - 62, area.y + 6, 54, 26);
        canvas.fill_rounded_rect(btn_run, 6, pal.accent_primary);
        canvas.draw_lucide_icon(btn_run.x + 5, btn_run.y + 5, LucideIcon::Play, pal.on_accent);
        canvas.draw_text_clipped(btn_run.x + 23, btn_run.y + 5, "Run", pal.on_accent, 1, btn_run);

        // Breadcrumb & Navigation bar
        let nav_x = area.x + SIDEBAR_W + 6;
        let bar_y = area.y + TOOLBAR_H + 4;
        let nav_btns = [
            (Rect::new(nav_x, bar_y, 26, 24), LucideIcon::ArrowLeft, self.can_go_back()),
            (Rect::new(nav_x + 28, bar_y, 26, 24), LucideIcon::ArrowRight, self.can_go_forward()),
            (Rect::new(nav_x + 56, bar_y, 26, 24), LucideIcon::ArrowUp, self.current_path != "/"),
        ];
        for (r, icon, enabled) in nav_btns {
            canvas.fill_rounded_rect(r, 5, pal.surface_card);
            canvas.stroke_rounded_rect(r, 5, pal.surface_border, 175);
            canvas.draw_lucide_icon(
                r.x + 5,
                r.y + 4,
                icon,
                if enabled { pal.text_primary } else { pal.text_muted },
            );
        }

        let chips = self.breadcrumb_chips(area);
        let chip_len = chips.len();
        for (idx, (chip_r, label, _)) in chips.iter().enumerate() {
            let is_last = idx + 1 == chip_len;
            canvas.fill_rounded_rect(
                *chip_r,
                5,
                if is_last { pal.surface_selected } else { pal.surface_card },
            );
            canvas.stroke_rounded_rect(*chip_r, 5, pal.surface_border, 165);
            canvas.draw_text_clipped(
                chip_r.x + 7,
                chip_r.y + 3,
                label,
                if is_last { pal.text_primary } else { pal.text_secondary },
                1,
                *chip_r,
            );
            if !is_last {
                canvas.draw_lucide_icon(chip_r.right() - 1, chip_r.y + 3, LucideIcon::ChevronRight, pal.text_muted);
            }
        }

        let btn_sort = Rect::new(area.right() - 114, bar_y, 52, 24);
        canvas.fill_rounded_rect(btn_sort, 5, pal.surface_card);
        canvas.stroke_rounded_rect(btn_sort, 5, pal.surface_border, 175);
        canvas.draw_lucide_icon(btn_sort.x + 4, btn_sort.y + 4, LucideIcon::ArrowUpDown, pal.text_secondary);
        let sort_lbl = match self.sort_mode {
            SortMode::Name => "Name",
            SortMode::Size => "Size",
        };
        canvas.draw_text_clipped(btn_sort.x + 21, btn_sort.y + 4, sort_lbl, pal.text_secondary, 1, btn_sort);

        let btn_view = Rect::new(area.right() - 58, bar_y, 50, 24);
        canvas.fill_rounded_rect(btn_view, 5, pal.surface_selected);
        canvas.stroke_rounded_rect(btn_view, 5, pal.accent_primary, 180);
        let (v_icon, v_lbl) = match self.view_mode {
            ViewMode::List => (LucideIcon::List, "List"),
            ViewMode::Grid => (LucideIcon::LayoutGrid, "Grid"),
        };
        canvas.draw_lucide_icon(btn_view.x + 4, btn_view.y + 4, v_icon, pal.accent_primary);
        canvas.draw_text_clipped(btn_view.x + 21, btn_view.y + 4, v_lbl, pal.text_primary, 1, btn_view);

        // Main File List / Grid container
        let view_rect = Self::content_view_rect(area);
        canvas.fill_rounded_rect(view_rect, 8, pal.surface_panel);
        canvas.stroke_rounded_rect(view_rect, 8, pal.surface_border, 185);

        let cut_src = self.clipboard.as_ref().filter(|c| c.is_cut).map(|c| c.src_path.as_str());

        match self.view_mode {
            ViewMode::List => {
                let max_rows = ((view_rect.h - 10) / ROW_H).max(1) as usize;
                self.last_max_visible.set(max_rows);

                for (i, entry) in self.entries.iter().skip(self.scroll).take(max_rows).enumerate() {
                    let abs_idx = self.scroll + i;
                    let row_rect = Rect::new(
                        view_rect.x + 5,
                        view_rect.y + 5 + (i as i32) * ROW_H,
                        view_rect.w - 18,
                        ROW_H - 2,
                    );
                    if self.selected_idx == Some(abs_idx) {
                        canvas.fill_rounded_rect(row_rect, 6, pal.surface_selected);
                    }
                    let icon_kind = entry_icon_kind(entry);
                    canvas.blit_argb_icon(
                        row_rect.x + 6,
                        row_rect.y + 4,
                        LIST_ICON_SIZE,
                        LIST_ICON_SIZE,
                        file_list_icon(icon_kind),
                    );
                    let child_full = self.full_child_path(&entry.name);
                    let is_cut = cut_src == Some(child_full.as_str());
                    let name_col = if is_cut { pal.text_muted } else { pal.text_primary };
                    canvas.draw_text_clipped(
                        row_rect.x + 28,
                        row_rect.y + 4,
                        &entry.name,
                        name_col,
                        1,
                        Rect::new(row_rect.x + 28, row_rect.y, (row_rect.w - 98).max(20), row_rect.h),
                    );
                    let meta = if is_cut {
                        String::from("[CUT]")
                    } else if entry.kind == VfsNodeKind::Directory {
                        String::from("DIR")
                    } else {
                        format_size(entry.size)
                    };
                    let meta_w = text_width(&meta, 1);
                    canvas.draw_text_clipped(
                        row_rect.right() - meta_w - 8,
                        row_rect.y + 4,
                        &meta,
                        if is_cut { ACCENT_WARN } else { pal.text_muted },
                        1,
                        row_rect,
                    );
                }

                let sb_track = Rect::new(view_rect.right() - 10, view_rect.y + 4, 7, view_rect.h - 8);
                canvas.draw_scrollbar_v(
                    sb_track,
                    self.entries.len(),
                    max_rows,
                    self.scroll,
                    pal.text_muted,
                    pal.surface_border,
                );
            }
            ViewMode::Grid => {
                let cols = ((view_rect.w - 18) / GRID_CELL_W).max(1) as usize;
                let rows = ((view_rect.h - 10) / GRID_CELL_H).max(1) as usize;
                let max_vis = cols * rows;
                self.last_grid_cols.set(cols);
                self.last_max_visible.set(max_vis);

                for (i, entry) in self.entries.iter().skip(self.scroll).take(max_vis).enumerate() {
                    let abs_idx = self.scroll + i;
                    let col = i % cols;
                    let row = i / cols;
                    let cell_r = Rect::new(
                        view_rect.x + 6 + (col as i32) * GRID_CELL_W,
                        view_rect.y + 6 + (row as i32) * GRID_CELL_H,
                        GRID_CELL_W - 6,
                        GRID_CELL_H - 6,
                    );
                    if self.selected_idx == Some(abs_idx) {
                        canvas.fill_rounded_rect(cell_r, 8, pal.surface_selected);
                        canvas.stroke_rounded_rect(cell_r, 8, pal.accent_primary, 190);
                    }
                    let icon_kind = entry_icon_kind(entry);
                    canvas.blit_argb_icon(
                        cell_r.x + (cell_r.w - GRID_ICON_SIZE as i32) / 2,
                        cell_r.y + 7,
                        GRID_ICON_SIZE,
                        GRID_ICON_SIZE,
                        file_grid_icon(icon_kind),
                    );
                    let child_full = self.full_child_path(&entry.name);
                    let is_cut = cut_src == Some(child_full.as_str());
                    let lbl = truncate_name(&entry.name, 11);
                    let tw = text_width(&lbl, 1);
                    let tx = cell_r.x + ((cell_r.w - tw) / 2).max(4);
                    canvas.draw_text_clipped(
                        tx,
                        cell_r.y + 44,
                        &lbl,
                        if is_cut { pal.text_muted } else { pal.text_primary },
                        1,
                        cell_r,
                    );
                }

                let sb_track = Rect::new(view_rect.right() - 10, view_rect.y + 4, 7, view_rect.h - 8);
                canvas.draw_scrollbar_v(
                    sb_track,
                    self.entries.len(),
                    max_vis,
                    self.scroll,
                    pal.text_muted,
                    pal.surface_border,
                );
            }
        }

        // Inspector panel
        let insp_rect = Rect::new(
            view_rect.right() + 6,
            view_rect.y,
            (area.right() - view_rect.right() - 10).max(120),
            view_rect.h,
        );
        canvas.fill_rounded_rect(insp_rect, 8, pal.surface_card);
        canvas.stroke_rounded_rect(insp_rect, 8, pal.surface_border, 180);
        canvas.draw_text_bold_clipped(
            insp_rect.x + 10,
            insp_rect.y + 8,
            &self.preview_title,
            pal.accent_primary,
            insp_rect,
        );
        for (li, line) in self.preview_lines.iter().take(12).enumerate() {
            canvas.draw_text_clipped(
                insp_rect.x + 10,
                insp_rect.y + 30 + (li as i32) * 18,
                line,
                pal.text_secondary,
                1,
                insp_rect,
            );
        }

        // Inline New File / New Folder / Rename prompt overlay
        if self.prompt_mode != PromptMode::None {
            let pbox = Rect::new(
                area.x + SIDEBAR_W + 10,
                area.y + TOOLBAR_H + BREADCRUMB_H + 6,
                area.w - SIDEBAR_W - 20,
                34,
            );
            canvas.draw_soft_shadow(pbox, 8, 4, 65);
            canvas.fill_rounded_rect(pbox, 8, pal.modal_bg);
            canvas.stroke_rounded_rect(pbox, 8, pal.accent_primary, 220);
            let (lbl, ok_lbl) = match self.prompt_mode {
                PromptMode::NewFile => ("File:", "Create"),
                PromptMode::NewDir => ("Dir:", "Create"),
                PromptMode::Rename => ("Name:", "Save"),
                PromptMode::None => ("", "OK"),
            };
            canvas.draw_text_bold_clipped(pbox.x + 10, pbox.y + 9, lbl, pal.accent_primary, pbox);
            let inp_r = Rect::new(pbox.x + 62, pbox.y + 4, (pbox.w - 192).max(40), 26);
            canvas.fill_rounded_rect(inp_r, 5, pal.modal_input_bg);
            canvas.stroke_rounded_rect(inp_r, 5, pal.surface_border, 200);
            canvas.draw_text_clipped(inp_r.x + 8, inp_r.y + 5, &self.prompt_input, pal.text_primary, 1, inp_r);

            let btn_ok = Rect::new(pbox.right() - 122, pbox.y + 4, 56, 26);
            let btn_cancel = Rect::new(pbox.right() - 60, pbox.y + 4, 54, 26);
            canvas.fill_rounded_rect(btn_ok, 5, pal.accent_primary);
            let ok_w = text_width_bold(ok_lbl);
            canvas.draw_text_bold_clipped(
                btn_ok.x + (btn_ok.w - ok_w) / 2,
                btn_ok.y + 5,
                ok_lbl,
                0xFFFFFFFF,
                btn_ok,
            );
            canvas.fill_rounded_rect(btn_cancel, 5, pal.surface_card);
            canvas.draw_text_clipped(btn_cancel.x + 6, btn_cancel.y + 5, "Cancel", pal.text_secondary, 1, btn_cancel);
        }

        // Context Menu overlay on right-click
        if let Some(cm) = self.context_menu {
            let menu_r = cm.rect;
            canvas.draw_soft_shadow(menu_r, 8, 5, 76);
            canvas.draw_glass_panel(menu_r, 8, pal.modal_bg, pal.modal_alpha, pal.modal_border);

            let has_entry = cm.entry_idx.is_some();
            let has_clip = self.clipboard.is_some();
            let items = [
                (
                    if has_entry { "Open / Run" } else { "(No selection)" },
                    LucideIcon::Play,
                    if has_entry { pal.text_primary } else { pal.text_muted },
                ),
                (
                    "Rename (F2)",
                    LucideIcon::Pencil,
                    if has_entry { pal.text_primary } else { pal.text_muted },
                ),
                (
                    "Copy (Ctrl+C)",
                    LucideIcon::Copy,
                    if has_entry { pal.text_primary } else { pal.text_muted },
                ),
                (
                    "Cut (Ctrl+X)",
                    LucideIcon::Scissors,
                    if has_entry { pal.text_primary } else { pal.text_muted },
                ),
                (
                    "Paste (Ctrl+V)",
                    LucideIcon::ClipboardPaste,
                    if has_clip { pal.accent_primary } else { pal.text_muted },
                ),
                ("New File...", LucideIcon::FilePlus, pal.text_primary),
                ("New Folder...", LucideIcon::FolderPlus, pal.text_primary),
                (
                    "Delete",
                    LucideIcon::Trash2,
                    if has_entry { ACCENT_ERR } else { pal.text_muted },
                ),
                ("Refresh", LucideIcon::RefreshCw, pal.text_primary),
            ];
            for (idx, (txt, icon, col)) in items.iter().enumerate() {
                let row_r = Rect::new(
                    menu_r.x + 6,
                    menu_r.y + 5 + (idx as i32) * MENU_ITEM_H,
                    menu_r.w - 12,
                    MENU_ITEM_H - 2,
                );
                canvas.draw_lucide_icon(row_r.x + 6, row_r.y + 4, *icon, *col);
                canvas.draw_text_clipped(row_r.x + 28, row_r.y + 4, txt, *col, 1, row_r);
            }
        }

        // Bottom status strip
        canvas.draw_text_clipped(
            area.x + SIDEBAR_W + 12,
            area.bottom() - 20,
            &self.status_msg,
            pal.text_muted,
            1,
            area,
        );
    }
}

fn entry_icon_kind(entry: &VfsDirEntry) -> FileIconKind {
    if entry.kind == VfsNodeKind::Directory {
        FileIconKind::Folder
    } else if entry.name.ends_with(".elf") {
        FileIconKind::Elf
    } else if entry.name.ends_with(".wav") {
        FileIconKind::Wav
    } else {
        FileIconKind::Text
    }
}

fn format_size(sz: u32) -> String {
    if sz < 1024 {
        format!("{} B", sz)
    } else if sz < 1024 * 1024 {
        format!("{}.{} KB", sz / 1024, ((sz % 1024) * 10) / 1024)
    } else {
        format!("{}.{} MB", sz / (1024 * 1024), ((sz % (1024 * 1024)) * 10) / (1024 * 1024))
    }
}

fn truncate_name(s: &str, max_chars: usize) -> String {
    let count = s.chars().count();
    if count <= max_chars {
        return String::from(s);
    }
    let mut out: String = s.chars().take(max_chars.saturating_sub(1)).collect();
    out.push('…');
    out
}

fn cmp_ascii_ci(a: &str, b: &str) -> core::cmp::Ordering {
    let mut ia = a.bytes().map(|c| c.to_ascii_lowercase());
    let mut ib = b.bytes().map(|c| c.to_ascii_lowercase());
    loop {
        match (ia.next(), ib.next()) {
            (Some(x), Some(y)) => {
                if x != y {
                    return x.cmp(&y);
                }
            }
            (None, Some(_)) => return core::cmp::Ordering::Less,
            (Some(_), None) => return core::cmp::Ordering::Greater,
            (None, None) => return core::cmp::Ordering::Equal,
        }
    }
}
