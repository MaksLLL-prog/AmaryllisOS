# Amaryllis OS — Project Status

**Last updated:** Stage 4 Step C+ — Ring 3 Heap (`SYS_BRK`/`SYS_MMAP`), VFS Syscalls, SysV `argv`, Wait Queues, Cooperative Kernel Threads (`kthread`) & Full `alloc`/`fs` in `amaryllis-sdk`  
**Project type:** Bare-metal operating system  
**Primary language:** Rust  
**Architecture:** x86_64  
**Target:** `x86_64-unknown-none`  
**Development status:** Active development — Stage 4 Steps A, B & C+ implemented

---

## 1. Project Overview

Amaryllis OS is an experimental operating system developed from scratch in Rust.

The project focuses on building an independent kernel with modular architecture, low-level hardware support, memory management, task scheduling, process management, storage, and a complete 60 FPS graphical desktop environment.

The current implementation is a bare-metal kernel using `no_std` and `no_main`.

## 2. Current Statistics

Statistics collected from `kernel/src`:

| Metric | Count |
|---|---:|
| Rust source files | 107 (kernel) + 16 (`amaryllis-sdk`) |
| Total lines | 51,042 (`kernel/src`) + 1,910 (`amaryllis-sdk`) |

## 3. Development Environment

| Component | Version / Configuration |
|---|---|
| Host OS | Windows 11 |
| Shell | Windows PowerShell 5.1 |
| Rust | Nightly |
| Rust compiler | 1.102.0-nightly (2026-11-08) |
| Target | `x86_64-unknown-none` |
| Bootloader | `bootloader` 0.11.17 |
| Emulator | QEMU 11.1.0 |
| Project directory | `AmaryllisOS` |

### Build command

```powershell
cargo +nightly build
```

### Run command

```powershell
cargo +nightly run
```

## 4. Completed Work

### 4.1 Architecture and Reorganization

**Status: Completed**

The kernel has been divided into separate subsystems:

- Memory
- PCI
- Interrupts
- Input
- Graphics
- Console
- Shell
- Task
- Process
- Storage

The project has moved away from keeping all functionality in a single source file, although `main.rs` is still large and requires further decomposition.

### 4.2 Memory Management

**Status: Completed — tests passed**

Implemented:

- Physical Memory Manager.
- Bootstrap PMM.
- Dynamic PMM.
- Bitmap-based physical memory tracking.
- Heap allocator.
- Virtual Memory Manager.
- Page-table management.
- CR3 operations.
- Address Space creation and destruction.
- Huge-page handling.
- Memory diagnostics.
- PMM tests and stress tests.
- Heap tests and stress tests.
- VMM tests.
- Address Space validation tests.

Additional validation:

- Duplicate page-table detection.
- Recursive page-table cycle detection.
- Invalid physical table frames.
- Zero physical table addresses.
- Non-present entries.
- Huge-page leaf handling.
- Invalid lower-level frames.
- Transactional address-space destruction.

**Result:** Memory Management was closed after the user confirmed the complete test sequence passed.

### 4.3 PCI Subsystem

**Status: Implemented; architectural separation still under review**

Implemented:

- PCI configuration-space access.
- PCI bus enumeration.
- Device inventory.
- Device inspection.
- Device queries.
- Device manager.
- BAR discovery.
- Resource validation.
- Driver matching.
- PCI diagnostics.
- IDE controller detection.

The PCI subsystem is one of the largest parts of the current codebase.

### 4.4 Task Management

**Status: Implemented; basic functionality confirmed**

Implemented:

- Task structure.
- CPU context representation.
- Scheduler.
- Fixed task capacity.
- Task states:
  - `Ready`
  - `Running`
  - `Blocked`
  - `Finished`
- Task counters.
- Blocking and unblocking.
- Task termination.
- Sleep and wake-up handling.
- Shell task-management commands.

The user confirmed that the task and sleep system worked after fixing an earlier scheduler crash.

### 4.5 Process Management

**Status: Implemented**

Current components:

- Process structures.
- Process manager.
- Process integration tests.
- Shell process commands.

Further integration and architectural validation remain part of the roadmap.

### 4.6 Graphics and Console

**Status: Implemented**

Current components:

- Framebuffer renderer.
- Color definitions.
- Text console.
- Console output.
- Kernel shell integration.

### 4.7 Input and Interrupts

**Status: Basic implementation present**

Current components:

- Keyboard input.
- Mouse input.
- PIC.
- PIT.
- Timer-driven kernel operations.

Further architectural separation and integration remain unfinished.

### 4.8 Shell

**Status: Implemented**

The kernel has an interactive command-line interface.

Previously implemented commands include:

```text
help
clear
about
version
echo
whoami
neofetch
uptime
tasks
block
unblock
kill
sleep
```

Additional PCI, storage, filesystem, memory, and process diagnostics are also present.

### 4.9 Storage and Filesystem

**Status: Implemented (FAT16 RO, FAT32 R/W + LFN, VFS, ATA PIO)**

Implemented:

- Generic `BlockDevice` trait with `read_sector`, `write_sector`, and `flush`.
- `BlockDeviceError`.
- Neutral hardware IDE channel descriptors in `kernel/src/hardware.rs` decoupling PCI from Storage.
- Low-level ATA PIO driver (`LBA28` & `LBA48` read/write/cache-flush) in `kernel/src/storage/ata/`.
- IDE runtime controller registry and generic `IdeBlockDevice` adapter in `kernel/src/storage/ide/`.
- Storage & ATA CLI diagnostics in `kernel/src/storage/cli.rs`.
- MBR partition and BPB parsing (`FAT12`/`FAT16`/`FAT32`) in `kernel/src/storage/partition/`.
- Generic `Fat16Volume<D: BlockDevice>` (read-only).
- Generic `Fat32Volume<D: BlockDevice>` (full FAT32 support: 32-bit FAT chains, FSInfo, directory traversal, LFN VFAT + 8.3 read/write, file create/overwrite, `mkdir`, `rm`).
- Virtual File System (`kernel/src/storage/vfs/`) with `FileSystem` trait, path normalization, current working directory, and `KERNEL_VFS`.
- Interactive shell filesystem commands (`ls`, `cd`, `pwd`, `cat`, `write`, `mkdir`, `rm`, `diskinfo`).

### 4.10 Drivers, Networking & Audio (Stage 2)

**Status: Implemented (Unified Driver Registry, Intel e1000 + Net Stack, Intel AC'97 + WAV)**

Implemented:

- Unified driver registry (`kernel/src/drivers/registry.rs`) with `KernelDriver`, `NetworkDevice`, `AudioDevice` traits, `DRIVER_REGISTRY`, and `drivers` shell command.
- Hardware discovery extended (`kernel/src/hardware.rs` + `kernel/src/pci.rs`) to detect Intel e1000 NIC and Intel AC'97 audio controller and enable PCI Bus Mastering / MMIO / I/O space.
- Intel 82540EM (e1000) NIC driver (`kernel/src/drivers/e1000.rs`) with VMM MMIO mapping and DMA TX/RX descriptor rings.
- Minimal TCP/IP network stack (`kernel/src/net/`): Ethernet II, ARP, IPv4, ICMP (`ping`), UDP, DHCPv4 client (`dhcp`), DNS A-record resolver, TCPv4 client, and HTTP/1.1 GET (`httpget`).
- Intel AC'97 audio driver (`kernel/src/drivers/ac97.rs`) with mixer volume control, VRA sample rate setup, BDL DMA playback, tone generator (`beep`), and streaming WAV player from FAT32/VFS (`play <file.wav>`).

### 4.11 Userland, GDT/TSS, Syscalls & ELF64 Loader (Stage 3)

**Status: Implemented (GDT + TSS, SYSCALL/SYSRET, Ring 3 Exception Recovery, ELF64 Loader & Demos)**

Implemented:

- GDT & Task State Segment (`kernel/src/arch/gdt.rs`): Ring 0 and Ring 3 segments ordered strictly for x86_64 `IA32_STAR` (`KernelCode`, `KernelData`, `UserData`, `UserCode`), dedicated `RSP0` Ring 3->Ring 0 interrupt stack, and `IST0` Double Fault stack.
- Extended IDT exception handlers (`kernel/src/interrupts/mod.rs`) covering `#DE`, `#UD`, `#DF` (IST0), `#NP`, `#SS`, `#GP`, `#PF`: exceptions triggered from Ring 3 are trapped cleanly and recover back to the kernel shell without rebooting or leaking frames.
- `SYSCALL` / `SYSRET`, Preemptive Ring 3 Scheduler & Wait Queues (`kernel/src/userland/syscall.rs`, `kernel/src/userland/sched.rs`): configures `IA32_EFER` (`SCE`/`NXE`), `IA32_STAR`, `IA32_LSTAR`, `IA32_FMASK`; runs up to 8 concurrent Ring 3 processes via 1000 Hz PIT `IRQ0` time-slicing (`CR3` + `UserCpuContext` switching) with non-blocking `WaitReason` queues (`Sleep`, `GuiEvent`, `TcpConnect`, `NetJob`), dynamic Ring 3 heap (`SYS_BRK`, `SYS_MMAP`, `SYS_MUNMAP`), VFS file/dir descriptors (`SYS_OPEN`, `SYS_STAT`, `SYS_FSTAT`, `SYS_LSEEK`, `SYS_GETDENTS`, `SYS_MKDIR`, `SYS_UNLINK`), `SYS_SPAWN_ELF` with arguments, and socket/network syscalls (`SYS_SOCKET`, `SYS_CONNECT`, `SYS_SEND`, `SYS_RECV`, `SYS_BIND`, `SYS_NET_PING`, `SYS_DNS_RESOLVE`, `SYS_HTTP_GET`).
- Cooperative Kernel Worker Threads (`kernel/src/task/kthread.rs`, `kernel/src/interrupts/timer.rs`): `64 KiB` stack-switched `knet_worker` servicing async DHCP/ICMP Ping/DNS/HTTP jobs; `timer::sleep_ms` transparently yields inside `kthread` so Desktop 60 FPS rendering and Ring 3 execution never stall on I/O.
- Streaming ELF64 loader & process runner (`kernel/src/userland/elf.rs`, `kernel/src/userland/runner.rs`): parses and loads `PT_LOAD` segments from `KERNEL_VFS`, sets up `brk_base` and SysV `argc`/`argv` (`rdi`/`rsi` + stack) for `exec`/`execbg`, streams live stdout to Desktop Terminal, and reclaims all frames (including `brk`/`mmap`), FDs, and GUI windows on exit/fault/`killr3`.
- Built-in Ring 3 ELF64 generator (`kernel/src/userland/demo.rs`), pre-populated RAMFS demos (`/hello.elf`, `/counter.elf`, `/gui_demo.elf`, `/net_demo.elf`, `/fault.elf`), shell commands (`mkelf`, `elfinfo`, `exec`, `execbg`, `ps`, `killr3`), and standalone Rust SDK crate (`AmaryllisOS/amaryllis-sdk/`) with `#[global_allocator]` (`Vec`, `String`, `Box`, `BTreeMap`), `fs` VFS APIs, `env::args()`, `io`, `time`, `proc`, `gui`, `net`, static `linker.ld`, and 5 runnable examples (`hello`, `multitask_counter`, `gui_plasma`, `net_http_client`, `fs_alloc_demo`).

### 4.12 Amaryllis Graphical System (Stage 4 Steps A–C+)

**Status: Implemented (Compositor, Window Manager, Built-in Apps, Async NetViewer, KThread Monitor, Light/Dark Themes, Preemptive Ring 3 GUI & Alert Inspector)**

Implemented:

- Direct GUI boot from `kernel_main` (removed `SCROLL TEST` boot spam; exiting GUI drops cleanly into the interactive text shell).
- CMOS RTC (`0x70`/`0x71`) real-time clock & calendar driver (`kernel/src/arch/rtc.rs`) with UIP polling, BCD/12h decoding, and Sakamoto weekday calculation.
- Full 4-byte PS/2 IntelliMouse packet decoding (`dx`, `dy`, `buttons`, `wheel`, right-click edge detection) + extended `KeyEvent` (`Alt`/`Ctrl`/`Super`/arrows) in `kernel/src/input/`.
- High-speed 60 FPS XRGB dirty-rectangle `Compositor` (`kernel/src/graphics/{theme.rs, widgets.rs, compositor.rs, renderer.rs}`) with 16 MiB kernel heap double-buffering, procedural 4-layer silk-wave `wallpaper_cache: Vec<u32>` (`copy_from_slice`), live Light (macOS-style) & Dark theme palette switching (`ThemeMode` + `ThemePalette`), per-window `surface_cache`, vertical scrollbars (`draw_scrollbar_v`), dirty-region clipping (`Canvas::clip`), shift-based (`>> 8`) integer alpha blending, and `u64` / `copy_nonoverlapping` transfers.
- Custom zero-dependency crisp `8x16` 4x4-supersampled anti-aliased ASCII font (`kernel/src/graphics/font.rs`) with proportional UI advances and aligned monospace Terminal rendering.
- Enhanced System Error & Ring 3 Fault Inspector modal (`kernel/src/graphics/alerts.rs`) adapted to Light/Dark theme palettes, translating `#PF`/`#GP`/`#UD`/`#DE`/`#SS`/`#NP` vectors, `CR2`, `RIP`/`RSP`, PMM integrity, and VFS/ELF/Net/Audio errors into human-readable causes and recovery hints.
- macOS-inspired Window Manager (`kernel/src/graphics/wm.rs`): 28px translucent Top Bar with working dropdown menus (`Amaryllis`, `File`, `Edit`, `View`, `Window`), vector tray icons, alert bell badge, real CMOS RTC date/time, 60x50 glossy floating Dock with 42x42 layered icons, vertical separator (`|`), hover tooltips & active dot indicators, Spotlight launcher (`Alt+Space` / `Super`), window dragging/resizing/snapping, cooperative `kthread` stepping, and Ring 3 GUI syscalls (`SYS_CREATE_WINDOW=300`, `SYS_WINDOW_BLIT=301`, `SYS_POLL_GUI_EVENT=302`).
- Global kernel clipboard (`kernel/src/graphics/clipboard.rs`) shared between `Editor` and `Terminal` (`Ctrl+C`, `Ctrl+V`, `Ctrl+A`), plus built-in apps (`kernel/src/graphics/apps/`): **Text Editor** (`editor.rs` with FAT32/RAMFS R/W, `Ctrl+S`, gutter line numbers, scrollbar), **Files** (`files.rs` with right-click context menu, `+ File` / `+ Dir`, `Delete`), **Audio Player** (`audio.rs` with non-blocking AC'97 mixer), **Net / HTTP Viewer** (`net_viewer.rs` non-blocking via `knet_worker`), **Settings** (`settings.rs`), **Amaryllis Terminal** (with live Ring 3 stdout streaming), and **Activity Monitor** (with compositor FPS readout & live `Tasks` tab displaying Ring 3 processes with `[Kill]` and cooperative `kthread` workers).
- Q12 fixed-point C2 smooth procedural wave curves, orthogonal subpixel Euclidean anti-aliasing, and 4x4 ordered Bayer dithering in `build_amaryllis_wallpaper` eliminating dark-mode banding and edge stair-stepping.

---

## 5. Current Source Structure

```text
kernel/src/
│
├── main.rs
├── pci.rs
│
├── console/
│   ├── console.rs
│   └── mod.rs
│
├── graphics/
│   ├── renderer.rs
│   └── mod.rs
│
├── input/
│   ├── keyboard.rs
│   ├── mouse.rs
│   └── mod.rs
│
├── interrupts/
│   ├── pic.rs
│   ├── timer.rs
│   └── mod.rs
│
├── memory/
│   ├── address_space.rs
│   ├── cr3.rs
│   ├── diagnostics.rs
│   ├── heap.rs
│   ├── vmm.rs
│   └── pmm/
│
├── pci/
│   ├── cli.rs
│   ├── device_manager.rs
│   ├── driver.rs
│   ├── inspect.rs
│   ├── query.rs
│   ├── resources.rs
│   ├── validation.rs
│   ├── ide.rs
│   └── ide/
│       ├── ata.rs
│       └── cli.rs
│
├── process/
│   ├── manager.rs
│   ├── process.rs
│   ├── integration_tests.rs
│   └── mod.rs
│
├── shell/
│   ├── commands.rs
│   ├── parser.rs
│   ├── process_commands.rs
│   └── mod.rs
│
├── storage/
│   ├── block.rs
│   ├── mod.rs
│   ├── ata/
│   ├── ide/
│   ├── partition/
│   └── fat16/
│
└── task/
    ├── context.rs
    ├── scheduler.rs
    ├── task.rs
    └── mod.rs
```

Note: `storage/ata/` exists but is currently empty. The ATA implementation is still located in `pci/ide/ata.rs`.

---

## 6. Current Architecture Objective

The next architectural objective is to complete the separation of PCI and Storage.

The intended structure is:

```text
kernel/src/
│
├── memory/
│
├── pci/
│   ├── core/
│   ├── resources/
│   ├── drivers/
│   └── diagnostics/
│
└── storage/
    ├── block.rs
    ├── ide/
    ├── ata/
    ├── partition/
    └── fat16/
```

### Desired dependency rules

- PCI discovers hardware and provides hardware resource information.
- Storage handles block devices and storage operations.
- ATA implements low-level ATA PIO operations.
- Partition parsing consumes block-device interfaces.
- FAT16 consumes the generic `BlockDevice` interface.
- PCI must not depend on Storage.
- Storage must not depend directly on PCI.
- The top-level kernel initialization should connect the subsystems.

A neutral shared hardware descriptor may be needed to pass IDE channel resources from PCI to Storage without creating circular dependencies.

This design has been discussed but is not fully implemented.

---

## 7. Exact Stopping Point

**Date:** November 15, 2026

The development session ended during PCI / Storage / FAT16 architectural separation.

### Completed immediately before pausing

- Created `kernel/src/storage/`.
- Added `storage/block.rs`.
- Added `storage/ide/mod.rs`.
- Moved partition code into `storage/partition/mod.rs`.
- Moved FAT16 code into `storage/fat16/mod.rs`.
- Added `mod storage;` to `main.rs`.
- Created `kernel/src/storage/ata/`.
- Inspected the complete existing ATA source file.

### Existing ATA source

```text
kernel/src/pci/ide/ata.rs
```

The file contains 856 lines and includes:

- ATA device types.
- ATA probe status.
- ATA device structures.
- ATA channel device structures.
- Port I/O operations.
- ATA status polling.
- IDENTIFY parsing.
- Device probing.
- Channel probing.
- Detection diagnostics.
- Formatting helpers.
- ATA PIO LBA28 sector reading.
- ATA read error handling.

### Important architectural issue

The current `storage/ide/mod.rs` adapter imports PCI IDE and ATA types from the old PCI module.

This dependency must be removed before the PCI / Storage separation can be considered complete.

### Next planned phase

Proceed to Phase 5 (unified hardware driver abstractions in `kernel/src/drivers/`), followed by Phase 6 (Userland Ring 3, `syscall`/`sysret`, ELF loader over VFS) and Phase 7 (Amaryllis Graphical System).

---

## 8. Remaining Roadmap

### Phase 3 — Kernel Architecture

- [x] Finish PCI / Storage / FAT16 separation.
- [ ] Interrupts / Input separation.
- [ ] Task / Process architecture.
- [ ] Kernel Boot / Init / Runtime organization.
- [ ] Final architecture audit.

### Phase 4 — Storage and Filesystem

- [x] Complete ATA driver separation.
- [x] Complete block-device integration (read/write/flush).
- [x] Complete partition management.
- [x] Add full FAT32 (with LFN read/write), VFS layer, and shell filesystem commands.
- [ ] Validate runtime filesystem stress tests in QEMU.

### Phase 5 — Drivers, Network & Audio

- [x] Establish unified driver interface and registry (`kernel/src/drivers/` + `drivers` command).
- [x] Implement Intel e1000 NIC driver + TCP/IP/DHCP/DNS/ICMP/HTTP stack (`netinfo`, `dhcp`, `ping`, `httpget`).
- [x] Implement Intel AC'97 audio driver + WAV streaming from FAT32 (`soundinfo`, `beep`, `play`).
- [ ] Validate network and audio hardware runtime behavior in QEMU.

### Phase 6 — Userland

- [x] Define userspace architecture (GDT/TSS + Ring 3 + isolated `AddressSpace`).
- [x] Develop system-call interfaces (`SYSCALL`/`SYSRET` MSRs + Ring 3 fault recovery).
- [x] Introduce userspace programs (ELF64 loader + `mkelf`/`elfinfo`/`exec`/`execbg`/`ps`/`killr3` + preemptive Ring 3 scheduler + socket/Ping/DNS/HTTP syscalls + standalone `amaryllis-sdk` Rust crate).

### Phase 7 — Amaryllis Graphical System

- [x] Design the graphical environment (Amaryllis twilight/light glass themes, double-buffered dirty-rect compositor, rounded corners, soft shadows & Bayer-dithered anti-aliased wave wallpaper).
- [x] Develop window management (Z-order, drag/resize/traffic lights, Left/Right/Max window snapping, `Alt+Space`/Super Spotlight).
- [x] Build the complete user-facing experience (`Files`, `Terminal`, `Text Editor`, `Audio Player`, `Net / HTTP Viewer`, `Settings`, `Activity Monitor` with Ring 3 process manager, `/gui_demo.elf` & `amaryllis-sdk` GUI windows, `gui`/`startx`).
- [x] Non-blocking AC'97 background mixer (`audio/mixer.rs`), writable root `RAMFS` + `/disk` mount, Cyrillic 8x16 glyphs & Semibold/Q8 scaled typography.

---

## 9. Repository and Version Control

The following command was executed:

```powershell
git status --short
```

Result:

```text
fatal: not a git repository
```

The current `AmaryllisOS` directory is not initialized as a Git repository.

No Git commit history is currently available.

A Git repository should be initialized before further substantial architectural changes, so future development can be tracked and safely reverted.

---

## 10. Development Notes

- Do not assume that a successful compilation proves runtime correctness.
- Preserve the confirmed Memory Management test results.
- Do not rewrite working subsystems without a clear architectural reason.
- Prefer complete, carefully inspected source files for refactoring.
- Avoid introducing circular dependencies.
- Keep PCI hardware discovery independent from storage operations.
- Validate each architectural change with a build before proceeding.
- Keep this document updated after completing major phases.

---

## Final Status

Amaryllis OS has grown into a substantial experimental kernel with **52 Rust source files and 23,472 lines** in its current source snapshot.

Memory management is completed and tested. PCI, task scheduling, process management, shell, graphics, input, interrupts, and early storage functionality are already present.

The project is currently paused during architectural refinement of PCI and Storage.

**The next session should resume from Section 7 — Exact Stopping Point.**

Amaryllis OS is not finished, but the foundation built so far is substantial and worth preserving.