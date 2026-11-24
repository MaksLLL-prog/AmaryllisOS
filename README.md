# Amaryllis OS

**A custom x86_64 operating system built from the ground up in Rust.**

Amaryllis OS is an independent bare-metal operating system project featuring its own hybrid kernel, hardware abstraction infrastructure, memory management, process execution environment, networking stack, storage subsystem, and graphical desktop environment.

Its kernel, **ANU (Amaryllis is Not Unix)**, is written in pure `#![no_std]` Rust with `alloc`, targeting the x86_64 architecture.

Inspired by the architectural principles behind Apple's XNU, ANU combines ideas from Mach, BSD, IOKit, and Libkern into an independently designed kernel architecture.

The project aims to bring together low-level performance, usability, visual consistency, and system reliability.

> **Performance → Convenience → Beauty → Reliability.**

---

## Project Overview

| Property | Value |
|---|---|
| Project | Amaryllis OS |
| Version | v0.11 |
| Kernel | ANU — Amaryllis is Not Unix |
| Kernel architecture | Hybrid |
| Target architecture | x86_64 |
| Language | Rust |
| Kernel environment | `no_std` + `alloc` |
| Codebase | 73,150 lines |
| Source files | 143 |
| License | Apache License 2.0 |
| Graphics | Custom Amaryllis Desktop |
| Architectural inspiration | Apple XNU |

*Codebase statistics represent the project's reported current snapshot.*

---

## Architecture

ANU is a custom hybrid kernel designed around several major subsystems:

- Architecture and interrupt management
- Symmetric multiprocessing (SMP)
- Physical and virtual memory management
- Preemptive task scheduling
- Kernel threads and user processes
- System calls and IPC
- VFS and block-device infrastructure
- Device management and driver work loops
- Networking
- Kernel diagnostics

The architecture takes inspiration from four major XNU components:

| XNU concept | ANU direction |
|---|---|
| Mach | Virtual memory, scheduling, processes and IPC |
| BSD | Userland interfaces, filesystem and networking |
| IOKit | Device registry, drivers and asynchronous work loops |
| Libkern | Kernel infrastructure, allocation and shared utilities |

ANU is not a port or implementation of XNU. It is an independent Rust kernel inspired by its architectural approach.

---

## Implemented Features

### 1. x86_64 Architecture, Interrupts and SMP

ANU implements a 64-bit execution environment with hardware-level interrupt and processor management.

**CPU architecture:**
- GDT and TSS configuration.
- Dedicated IST stacks and RSP0.
- Complete IDT exception handling.
- Normalized interrupt contexts.
- Ring 3 exception handling without automatically panicking the kernel.
- Fast `syscall` / `sysret` transitions.
- GS normalization through `SWAPGS`.

**ACPI support:**
- RSDP discovery.
- RSDT / XSDT parsing.
- FADT for hardware reboot and shutdown.
- MADT processor and interrupt-controller discovery.
- MCFG PCI configuration support.
- HPET discovery.

**Symmetric Multiprocessing:**
- Real multi-core CPU initialization.
- AP startup through `INIT-SIPI-SIPI`.
- 16-bit Real Mode → 32-bit Protected Mode → 64-bit Long Mode trampoline.
- AP trampoline located at physical address `0x8000`.
- MADT-based processor discovery.
- Isolated `PerCpu` structures (`PER_CPU[16]`).
- Dedicated GDT, TSS, stacks and Local APIC timers for each CPU.

**Interrupt controllers:**
- Local APIC.
- Calibrated 1000 Hz periodic APIC timer.
- IOAPIC GSI routing.
- ACPI Interrupt Source Override resolution.
- Shared PCI IRQ vectors (`0x50–0x57`).
- PS/2 IRQ routing.
- Interrupt storm protection.
- Correct interrupt polarity handling.
- Lock-free LAPIC EOI operation.

**Inter-Processor Interrupts:**
- TLB Shootdown IPI (`0xF0`).
- Reschedule IPI (`0xF1`).

---

### 2. Memory Management

ANU implements a physical and virtual memory architecture with hardware-enforced page permissions.

**Physical Memory Manager:**
- Bitmap-based physical frame allocator.
- 4 KiB physical frames.
- Physical page allocation and deallocation.
- Process memory reclamation.

**Virtual Memory Manager:**
- Four-level x86_64 paging: PML4 → PDPT → PD → PT.
- Independent CR3 address spaces.
- Dynamic synchronization of shared kernel PML4 entries.
- NXE and hardware-enforced non-executable pages.
- W^X memory protection.
- 64 MiB kernel heap.

**Virtual memory features:**
- `VmRegion` virtual memory regions.
- Demand Paging.
- Lazy allocation through `sys_mmap`.
- Automatic user stack growth.
- User stack growth from 16 to 64 pages.
- Hardware-unmapped 4 KiB Guard Pages.
- Protected kernel thread stacks.
- Hardened `copy_from_user` and `copy_to_user`.
- Lazy page allocation during user-memory access.

---

### 3. Scheduler, Kernel Threads and Userland

ANU features a preemptive multitasking system with support for parallel kernel execution.

**Scheduler:**
- Preemptive fair scheduler.
- Virtual runtime accounting (`vruntime_ms`).
- CPU context switching.
- FPU/SSE context preservation through `fxsave64` and `fxrstor64`.
- `PreemptGuard`.
- `PreemptMutex`.
- Protection against interrupt-related scheduling deadlocks.

**Kernel Threads:**
- Preemptive kernel threads.
- Independent 64 KiB kernel stacks.
- 4 KiB guard pages.
- Execution across AP processors.
- Background workers for asynchronous subsystem operations.

**Userland:**
- ELF64 executable loader.
- SysV ABI `argc` / `argv` initialization.
- Ring 3 execution.
- `syscall` / `sysret` interface.
- `brk`, `mmap`, and `munmap`.
- Up to 64 MiB of process-managed virtual memory through the supported interfaces.
- Independent process address spaces.

**Inter-Process Communication:**
- POSIX-style pipes with 16 KiB buffers.
- Shared Memory (SHM).
- Zero-Copy graphical surface sharing.

---

### 4. Amaryllis SDK

The standalone `amaryllis-sdk` crate provides a foundation for external applications.

**SDK characteristics:**
- `no_std` + `alloc`.
- Global allocator.
- Application-facing system interfaces.
- Independent ELF application development.

**Available modules:**

| Module | Purpose |
|---|---|
| `fs` | Filesystem operations |
| `env` | Environment interfaces |
| `net` | Networking |
| `ipc` | Inter-process communication |
| `time` | Time-related interfaces |
| `gui` | Graphical application interfaces |
| `gui::ShmWindow` | Shared-memory graphical windows |

The SDK is intended to decouple applications from internal kernel implementation details.

---

### 5. Storage and Filesystems

ANU provides a unified storage architecture with caching and asynchronous device operations.

**Filesystem infrastructure:**
- Virtual Filesystem (VFS).
- Unified filesystem tree rooted at `/`.
- Writable RAMFS.
- FAT32 filesystem.
- FAT32 read and write support.
- Long File Name (LFN) support.

**Unified Page / Block Cache:**
- 4 KiB cache pages.
- `CachedBlockDevice`.
- Clock replacement algorithm.
- Read-ahead up to 16 KiB.
- Write-back cache flushing.
- Reported cache hit rate of approximately 98%.

**Block I/O:**
- Multi-block DMA transfers.
- Up to 128 sectors (64 KiB) per operation.
- VirtIO-Blk with split virtqueues.
- AHCI SATA with 16-entry PRDT.

---

### 6. Device Infrastructure — IOKit-Inspired Architecture

ANU uses an object-oriented device infrastructure inspired by the architectural ideas of Apple's IOKit.

**IORegistry:**
- Hierarchical device object tree.
- Device registration and discovery.
- Runtime device inspection through `ioreg`.

**IOWorkLoop:**
- Lock-free IRQ Top-Half → IOWorkLoop Bottom-Half architecture.
- Deferred interrupt processing.
- Background kernel-thread execution.
- Dedicated workers such as `knet_worker`, `kiokit_audio`, and `kiokit_blk`.

This architecture separates time-critical interrupt handling from longer-running device operations.

**Implemented hardware drivers:**
- VirtIO-Blk.
- AHCI SATA.
- Intel e1000 Ethernet.
- Intel AC'97 audio.
- PS/2 keyboard and mouse.

---

### 7. Networking

ANU includes an independently implemented TCP/IP networking stack.

**Network hardware:**
- Intel e1000.
- MMIO device access.
- DMA-based packet operations.

**Network protocols:**

| Layer | Implemented protocols |
|---|---|
| Link | Ethernet |
| Network | ARP, IPv4, ICMP |
| Transport | UDP, TCP |
| Configuration | DHCP |
| Name resolution | DNS |
| Application | HTTP |

Additional functionality:
- ICMP ping.
- DHCP configuration.
- DNS resolution.
- TCP communication.
- HTTP requests.
- Socket interfaces for Ring 3 applications.

---

### 8. Audio

ANU includes an asynchronous audio subsystem.

Implemented features:
- Intel AC'97 driver.
- Buffer Descriptor List (BDL).
- DMA-based audio playback.
- Non-blocking audio mixer.
- Background audio processing through kernel threads.

---

### 9. Kernel Diagnostics

ANU provides internal diagnostic and telemetry infrastructure.

**Implemented components:**
- `klog` kernel logging.
- `dmesg` diagnostic output.
- Ring-buffer telemetry.
- Atomic interrupt counters.
- Preemption counters.
- Page fault counters.
- Runtime subsystem diagnostics.

**Shell diagnostic commands include:**

`sm​p`, `apic`, `ioreg`, `kthread`, `diskinfo`, `virtio`, `netinfo`.

---

## Amaryllis Desktop

Amaryllis includes its own graphical desktop environment, designed as an independent visual system rather than a direct reproduction of macOS.

The design follows a consistent monochrome visual language with an emphasis on minimalism, responsiveness, and accessibility.

### Desktop Components

**Amaryllis Capsule**

A floating bottom capsule that combines:
- 3×3 application launcher.
- Taskbar with application indicators.
- Universal search inspired by Spotlight.
- System island.
- Expandable Control Center.
- Calendar.
- ACPI power menu.

**Window Management:**
- Frameless top-left active-window menu.
- Window titles aligned to the left.
- Window controls aligned to the right.
- Minimal window decoration.

**Compositor:**
- 120 FPS target.
- Approximately 8 ms frame pacing.
- `sti` / `hlt` event-driven idle handling.
- Occlusion-aware rendering through `begin_frame_with_occluders`.
- Dirty-rectangle rendering.
- Wallpaper caching.
- Cursor rendering without trails.

**Typography and Appearance:**
- Vector font rendering through `fontdue`.
- Glyph caching.
- Cyrillic support.
- Colloid and Lucide icons.
- Monochrome Black and White themes.
- TOML-based personalization service integrated with VFS.

### Built-in Applications

1. Files
2. Terminal
3. Text Editor
4. Audio Player
5. Net Viewer
6. System Settings
7. Activity Monitor

---

## Roadmap

The next development phase focuses on expanding ANU's Mach-inspired process, memory, and IPC architecture, followed by a modern native filesystem.

### 1. Advanced Memory Management — Mach VM

- Physical frame reference counting in PMM.
- Copy-on-Write (CoW).
- File-backed `mmap` integrated with Page Cache.
- `fork()` and `waitpid()`.
- POSIX signals.

### 2. Mach-Inspired IPC

- Typed IPC message ports.
- Send and Receive permissions.
- Zero-Copy Out-of-Line (OOL) page transfer.
- Bootstrap service registry.
- Service discovery infrastructure inspired by `launchd`.
- Mach Exception Ports.

### 3. FmFS — Amaryllis Filesystem

Development of a native filesystem designed for Amaryllis OS.

Planned features:
- Extent-based storage architecture.
- Filesystem access permissions.
- Metadata checksums.
- Journaling.
- Copy-on-Write mechanisms.
- Integration with VFS and Unified Page Cache.

FmFS is planned as a modern replacement for FAT32 as the primary native filesystem.

---

## Development Philosophy

Amaryllis OS is built around four priorities:

1. **Performance** — efficient execution, low overhead, and responsive interactions.
2. **Convenience** — consistent system interfaces and intuitive workflows.
3. **Beauty** — a coherent visual identity and carefully designed desktop environment.
4. **Reliability** — predictable behavior, memory safety, fault handling, and robust subsystem integration.

The project is not intended to be a Linux distribution or a macOS clone.

It is an attempt to build an independent operating system with its own kernel architecture, system interfaces, device infrastructure, filesystem direction, and desktop experience.

---

## Building and Testing

Amaryllis OS targets bare-metal x86_64 hardware and virtualized environments.

The project uses a Rust Nightly toolchain and a custom build configuration.

**Target:**

`x86_64-unknown-none`

**Primary development tools:**
- Rust Nightly
- Cargo
- QEMU
- x86_64 virtual hardware

Build and launch commands depend on the current repository configuration and build scripts.

---

## Project Status

**Amaryllis OS v0.11 is an actively developed experimental operating system.**

The project already includes:

- A custom 64-bit Rust hybrid kernel.
- Real SMP and inter-processor interrupts.
- Demand Paging and protected address spaces.
- Preemptive scheduling and kernel threads.
- ELF64 userland execution.
- IPC and Shared Memory.
- VFS, FAT32, RAMFS, and unified caching.
- VirtIO-Blk and AHCI storage drivers.
- IOKit-inspired device infrastructure.
- An Ethernet and TCP/IP networking stack.
- AC'97 audio.
- A custom 120 FPS-targeted desktop environment.
- Seven built-in graphical applications.

The next major milestone is to deepen the Mach-inspired architecture through Copy-on-Write memory, advanced IPC ports, POSIX process interfaces, and the native FmFS filesystem.

---

## License

Amaryllis OS is licensed under the **Apache License 2.0**.

See `LICENSE` for details.

---

**Amaryllis OS — Built from the ground up.**
**ANU — Amaryllis is Not Unix.**
