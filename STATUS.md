# Amaryllis OS — Project Status

**Version:** v0.11
**Kernel:** ANU — Amaryllis is Not Unix
**Last updated:** November 24, 2026
**Development status:** Active development
**Target architecture:** x86_64
**Primary language:** Rust (`#![no_std]` + `alloc`)
**License:** Apache License 2.0

---

## 1. Project Overview

Amaryllis OS is an independent bare-metal operating system developed from scratch in Rust.

Its hybrid kernel, ANU (Amaryllis is Not Unix), combines architectural ideas inspired by Mach, BSD, IOKit, and Libkern into an independently designed x86_64 kernel.

The project includes its own memory management, SMP infrastructure, preemptive scheduler, kernel threads, userland execution environment, storage subsystem, networking stack, device infrastructure, and graphical desktop environment.

### Project Statistics

| Metric | Value |
|---|---:|
| Version | v0.11 |
| Total codebase | 73,150 lines |
| Source and configuration files | 143 |
| Architecture | x86_64 |
| Kernel language | Rust |
| Kernel environment | `no_std` + `alloc` |
| License | Apache 2.0 |

*Statistics represent the current reported project snapshot.*

---

## 2. Overall Development Status

| Subsystem | Status |
|---|---|
| ANU kernel architecture | Implemented |
| x86_64 architecture | Implemented |
| ACPI | Implemented |
| SMP | Implemented |
| Local APIC / IOAPIC | Implemented |
| Inter-Processor Interrupts | Implemented |
| Physical Memory Manager | Implemented |
| Virtual Memory Manager | Implemented |
| Demand Paging | Implemented |
| Guard Pages | Implemented |
| Preemptive scheduler | Implemented |
| Kernel threads | Implemented |
| Ring 3 execution | Implemented |
| ELF64 loader | Implemented |
| System calls | Implemented |
| Amaryllis SDK | Implemented |
| VFS | Implemented |
| RAMFS | Implemented |
| FAT32 | Implemented |
| Unified Page / Block Cache | Implemented |
| VirtIO-Blk | Implemented |
| AHCI SATA | Implemented |
| IORegistry / IOWorkLoop | Implemented |
| Intel e1000 | Implemented |
| TCP/IP stack | Implemented |
| Intel AC'97 audio | Implemented |
| Kernel diagnostics | Implemented |
| Amaryllis Desktop | Implemented |
| Built-in applications | Implemented |
| Copy-on-Write memory | Planned |
| Mach-inspired IPC Ports | Planned |
| Native FmFS filesystem | Planned |

*Implemented indicates that the corresponding functionality has been developed. It does not imply complete hardware compatibility, production-level validation, or feature parity with mature operating systems.*

---

## 3. Completed Development

### 3.1. ANU Kernel Architecture

- Established ANU as the independent kernel architecture of Amaryllis OS.
- Adopted a hybrid architecture inspired by XNU.
- Developed a modular Rust kernel using `no_std` and `alloc`.
- Established separate architecture, memory, task, userland, storage, driver, networking, and graphics subsystems.

### 3.2. x86_64 Architecture and SMP

**Implemented:**

- GDT and TSS.
- Dedicated IST stacks and RSP0.
- IDT exception handling.
- Ring 3 exception recovery.
- `SWAPGS` and per-CPU context management.
- `syscall` / `sysret`.
- ACPI parsing: RSDP, RSDT/XSDT, FADT, MADT, MCFG, HPET.
- Hardware reboot and shutdown through FADT.
- Local APIC periodic timer at 1000 Hz.
- IOAPIC GSI routing.
- ACPI Interrupt Source Override handling.
- Shared PCI interrupt vectors.
- PS/2 interrupt routing.
- Interrupt storm protection.
- Lock-free LAPIC EOI operation.

**SMP implementation:**

- AP startup using INIT-SIPI-SIPI.
- 16-bit Real Mode → 32-bit Protected Mode → 64-bit Long Mode trampoline.
- Physical trampoline address `0x8000`.
- MADT-based CPU discovery.
- Per-CPU structures (`PER_CPU[16]`).
- Independent GDT, TSS, stacks, and Local APIC timers.
- TLB Shootdown IPI (`0xF0`).
- Reschedule IPI (`0xF1`).

### 3.3. Memory Management

**Implemented:**

- Bitmap-based Physical Memory Manager.
- 4 KiB physical frame allocation and deallocation.
- Four-level x86_64 paging.
- Independent process address spaces.
- CR3 switching.
- Dynamic synchronization of shared kernel PML4 entries.
- NXE and W^X protection.
- 64 MiB kernel heap.
- `VmRegion` virtual memory regions.
- Demand Paging.
- Lazy allocation through `sys_mmap`.
- Automatic user stack growth.
- User stack growth from 16 to 64 pages.
- 4 KiB Guard Pages for user and kernel thread stacks.
- Hardened `copy_from_user` and `copy_to_user`.

### 3.4. Scheduling and Kernel Threads

**Implemented:**

- Preemptive fair scheduler.
- Virtual runtime accounting (`vruntime_ms`).
- CPU context switching.
- FPU/SSE context preservation (`fxsave64` / `fxrstor64`).
- `PreemptGuard`.
- `PreemptMutex`.
- Preemptive kernel threads.
- Dedicated 64 KiB kernel thread stacks.
- 4 KiB guard pages.
- Kernel thread execution on AP processors.
- Background workers for networking, storage, and audio.

### 3.5. Userland and System Interfaces

**Implemented:**

- Ring 3 execution.
- Independent process address spaces.
- ELF64 loader.
- SysV ABI `argc` / `argv`.
- `syscall` / `sysret` interface.
- `brk`, `mmap`, and `munmap`.
- Process memory reclamation.
- VFS file and directory descriptors.
- File operations through system calls.
- Process creation through `SYS_SPAWN_ELF`.
- Socket and networking system calls.
- Wait queues.
- POSIX-style pipes with 16 KiB buffers.
- Shared Memory for Zero-Copy graphical surfaces.

### 3.6. Amaryllis SDK

A standalone `amaryllis-sdk` crate provides interfaces for external ELF applications.

**Implemented modules:**

- `fs`
- `env`
- `net`
- `ipc`
- `time`
- `gui`
- `gui::ShmWindow`

Additional functionality:

- `no_std` + `alloc`.
- Global allocator.
- `Vec`, `String`, `Box`, and `BTreeMap` support.
- Application arguments.
- Filesystem interfaces.
- Networking interfaces.
- GUI integration.
- Linker configuration.
- Runnable example applications.

### 3.7. Storage and Filesystems

**Implemented:**

- Virtual Filesystem (VFS).
- Unified filesystem tree rooted at `/`.
- Writable RAMFS.
- FAT32 read and write support.
- Long File Name (LFN).
- File creation and overwriting.
- Directory creation and removal.
- Generic `BlockDevice` interface.
- MBR partition parsing.
- ATA PIO.
- VirtIO-Blk.
- AHCI SATA.

**Unified Page / Block Cache:**

- 4 KiB cache pages.
- Clock replacement algorithm.
- Read-ahead up to 16 KiB.
- Write-back flushing.
- Multi-block DMA transfers up to 128 sectors (64 KiB).
- Reported cache hit rate of approximately 98%.

### 3.8. Device Infrastructure

**Implemented:**

- Unified device registry.
- IORegistry device tree.
- Runtime device inspection through `ioreg`.
- Lock-free IRQ Top-Half → IOWorkLoop Bottom-Half architecture.
- Background processing through kernel threads.
- Asynchronous device operations.

The driver infrastructure is inspired by Apple's IOKit architecture but is independently implemented.

**Available drivers:**

- VirtIO-Blk.
- AHCI SATA.
- Intel e1000.
- Intel AC'97.
- PS/2 keyboard and mouse.

### 3.9. Networking

**Implemented network stack:**

| Layer | Protocols |
|---|---|
| Link | Ethernet |
| Network | ARP, IPv4, ICMP |
| Transport | UDP, TCP |
| Configuration | DHCP |
| Name resolution | DNS |
| Application | HTTP |

Additional functionality:

- Intel e1000 MMIO/DMA driver.
- TX/RX descriptor rings.
- ICMP ping.
- DHCP client.
- DNS A-record resolution.
- TCP client.
- HTTP/1.1 GET.
- Socket interfaces for Ring 3 applications.
- Background networking worker.

### 3.10. Audio

**Implemented:**

- Intel AC'97 driver.
- BDL DMA playback.
- Mixer volume control.
- Variable sample rate configuration.
- Non-blocking audio mixer.
- WAV streaming from FAT32/VFS.
- Background audio processing.

### 3.11. Kernel Diagnostics

**Implemented:**

- `klog` kernel logging.
- `dmesg`.
- Ring-buffer telemetry.
- Atomic IRQ counters.
- Preemption counters.
- Page fault counters.
- Runtime subsystem diagnostics.

Available diagnostic commands include:

`sm​p`, `apic`, `ioreg`, `kthread`, `diskinfo`, `virtio`, `netinfo`.

### 3.12. Amaryllis Desktop

**Implemented graphical infrastructure:**

- Custom window manager.
- Double-buffered compositor.
- Dirty-rectangle rendering.
- Occlusion-aware rendering.
- Wallpaper caching.
- Window dragging, resizing, and snapping.
- Floating Amaryllis Capsule.
- Application launcher.
- Universal search.
- System island.
- Control Center.
- Calendar.
- Power menu.
- Active-window menu.
- Light and Dark themes.
- Vector-style typography.
- Glyph caching.
- Cyrillic support.
- TOML-based personalization.

**Performance target:** 120 FPS.

**Built-in applications:**

1. Files
2. Terminal
3. Text Editor
4. Audio Player
5. Net Viewer
6. System Settings
7. Activity Monitor

---

## 4. Current Roadmap

The next development phase focuses on expanding the memory model, process interfaces, IPC architecture, and native filesystem.

### 4.1. Mach-Inspired Virtual Memory

- [ ] Physical frame reference counting in PMM.
- [ ] Copy-on-Write (CoW).
- [ ] File-backed `mmap` integrated with Page Cache.
- [ ] `fork()`.
- [ ] `waitpid()`.
- [ ] POSIX signals.

### 4.2. Mach-Inspired IPC

- [ ] Typed IPC message ports.
- [ ] Send / Receive permissions.
- [ ] Zero-Copy Out-of-Line page transfer.
- [ ] Bootstrap service registry.
- [ ] Service discovery infrastructure inspired by `launchd`.
- [ ] Mach Exception Ports.

### 4.3. FmFS — Native Amaryllis Filesystem

- [ ] Extent-based storage architecture.
- [ ] Filesystem access permissions.
- [ ] Metadata checksums.
- [ ] Journaling.
- [ ] Copy-on-Write mechanisms.
- [ ] VFS integration.
- [ ] Unified Page Cache integration.

FmFS is planned as the native filesystem direction for Amaryllis OS, eventually replacing FAT32 as the primary filesystem.

---

## 5. Development Priorities

The project follows four core principles:

1. **Performance** — efficient execution and low system overhead.
2. **Convenience** — consistent interfaces and intuitive workflows.
3. **Beauty** — a coherent visual identity and desktop experience.
4. **Reliability** — predictable behavior, robust memory management, and fault handling.

Current development priorities:

- Expand VM capabilities.
- Strengthen SMP and concurrency validation.
- Improve process and IPC infrastructure.
- Expand hardware compatibility.
- Improve storage reliability.
- Continue developing the native filesystem.
- Validate the kernel under sustained workloads.

---

## 6. Important Development Notes

- A successful compilation does not automatically prove runtime correctness.
- Implemented functionality should be distinguished from fully validated functionality.
- SMP, interrupt handling, memory management, and scheduling changes require careful regression testing.
- Avoid unnecessary rewrites of working subsystems.
- Preserve memory-management validation results.
- Maintain clear subsystem boundaries and avoid circular dependencies.
- Test changes in QEMU and on physical hardware where applicable.
- Update this document after major milestones.

---

## 7. Current Project State

Amaryllis OS v0.11 has progressed beyond its initial kernel and graphical prototype stages.

The project now contains a custom x86_64 hybrid kernel with SMP, virtual memory, preemptive scheduling, kernel threads, Ring 3 execution, an application SDK, storage and networking subsystems, device infrastructure, and an independent graphical desktop.

The fundamental architecture is in place. The current challenge is expanding its capabilities while improving correctness, robustness, and compatibility.

The next major milestones are:

1. Copy-on-Write and advanced virtual memory.
2. Mach-inspired IPC ports and service discovery.
3. Native FmFS filesystem.

**Amaryllis OS is an actively developed experimental operating system.**

---

**Amaryllis OS — Built from the ground up.**
**ANU — Amaryllis is Not Unix.**
