# AmaryllisOS

### An independent x86_64 operating system built from the ground up in Rust.

Amaryllis OS is an experimental bare-metal operating system featuring its own hybrid kernel, hardware abstraction infrastructure, memory management, preemptive multitasking, user-space execution environment, networking stack, storage subsystem, and graphical desktop.

At its core is **ANU (Amaryllis is Not Unix)** — an independently designed hybrid kernel written in Rust using `#![no_std]` and `alloc`, targeting x86_64.

ANU takes architectural inspiration from Apple's XNU, combining ideas associated with Mach, BSD, IOKit, and Libkern into an independent kernel architecture.

Amaryllis OS aims to bring together low-level performance, intuitive interaction, visual consistency, and system reliability.

> **Performance → Convenience → Beauty → Reliability.**

---

## Project Overview

| Property | Value |
|---|---|
| Project | Amaryllis OS |
| Version | v0.11 |
| Kernel | ANU — Amaryllis is Not Unix |
| Architecture | Hybrid |
| Target architecture | x86_64 |
| Primary language | Rust |
| Kernel environment | `no_std` + `alloc` |
| Reported codebase | 73,150 lines |
| Reported source files | 143 |
| License | Apache License 2.0 |
| Desktop | Custom Amaryllis Desktop |
| Architectural inspiration | Apple XNU |

*Codebase statistics represent the reported project snapshot and may change as development continues.*

---

## Architecture

ANU is an independently developed hybrid kernel organized around several major subsystems:

- x86_64 architecture and interrupt management
- Symmetric multiprocessing (SMP)
- Physical and virtual memory management
- Preemptive task scheduling
- Kernel threads and user processes
- System calls and inter-process communication
- Virtual filesystem and block-device infrastructure
- Device management and asynchronous driver work loops
- Networking and audio
- Kernel diagnostics and telemetry

### Architectural inspiration

ANU draws inspiration from several architectural concepts found in XNU:

| XNU concept | ANU implementation direction |
|---|---|
| Mach | Virtual memory, scheduling, processes, and IPC |
| BSD | User-space interfaces, filesystem, and networking |
| IOKit | Device registry, driver infrastructure, and asynchronous work loops |
| Libkern | Kernel infrastructure, allocation, and shared utilities |

ANU is **not a port or implementation of XNU**. It is an independent Rust kernel inspired by selected architectural principles.

---

## Implemented Features

### 1. x86_64 Architecture, Interrupts, and SMP

ANU implements a 64-bit execution environment with hardware-level processor and interrupt management.

**CPU architecture**

- GDT and TSS configuration
- Dedicated IST stacks and RSP0
- IDT exception handling
- Normalized interrupt contexts
- Ring 3 exception handling without automatically panicking the kernel
- Fast `syscall` / `sysret` transitions
- GS normalization through `SWAPGS`

**ACPI support**

- RSDP discovery
- RSDT / XSDT parsing
- FADT hardware reboot and shutdown
- MADT processor and interrupt-controller discovery
- MCFG PCI configuration support
- HPET discovery

**Symmetric multiprocessing**

- Multi-core CPU initialization
- AP startup through `INIT-SIPI-SIPI`
- 16-bit Real Mode → 32-bit Protected Mode → 64-bit Long Mode trampoline
- AP trampoline at physical address `0x8000`
- MADT-based processor discovery
- Per-CPU structures (`PER_CPU[16]`)
- Dedicated GDT, TSS, stacks, and Local APIC timers for each CPU

**Interrupt controllers**

- Local APIC
- Calibrated 1000 Hz periodic APIC timer
- IOAPIC GSI routing
- ACPI Interrupt Source Override resolution
- Shared PCI IRQ vectors (`0x50–0x57`)
- PS/2 IRQ routing
- Interrupt storm protection
- Interrupt polarity handling
- Lock-free LAPIC EOI operation

**Inter-Processor Interrupts**

- TLB Shootdown IPI (`0xF0`)
- Reschedule IPI (`0xF1`)

### 2. Memory Management

ANU implements physical and virtual memory management with hardware-enforced page permissions.

**Physical Memory Manager**

- Bitmap-based physical frame allocator
- 4 KiB physical frames
- Physical page allocation and deallocation
- Process memory reclamation

**Virtual Memory Manager**

- Four-level x86_64 paging: PML4 → PDPT → PD → PT
- Independent CR3 address spaces
- Dynamic synchronization of shared kernel PML4 entries
- NXE and hardware-enforced non-executable pages
- W^X memory protection
- 64 MiB kernel heap

**Virtual memory features**

- `VmRegion` virtual memory regions
- Demand Paging
- Lazy allocation through `sys_mmap`
- Automatic user stack growth
- User stack growth from 16 to 64 pages
- Hardware-unmapped 4 KiB guard pages
- Protected kernel thread stacks
- Hardened `copy_from_user` and `copy_to_user`
- Lazy page allocation during user-memory access

### 3. Scheduler, Kernel Threads, and Userland

ANU features preemptive multitasking and parallel kernel execution across available processors.

**Scheduler**

- Preemptive fair scheduler
- Virtual runtime accounting (`vruntime_ms`)
- CPU context switching
- FPU/SSE context preservation through `fxsave64` and `fxrstor64`
- `PreemptGuard`
- `PreemptMutex`
- Protection against interrupt-related scheduling deadlocks

**Kernel threads**

- Preemptive kernel threads
- Independent 64 KiB kernel stacks
- 4 KiB guard pages
- Execution across AP processors
- Background workers for asynchronous subsystem operations

**Userland**

- ELF64 executable loader
- SysV ABI `argc` / `argv` initialization
- Ring 3 execution
- `syscall` / `sysret` interface
- `brk`, `mmap`, and `munmap`
- Up to 64 MiB of process-managed virtual memory through supported interfaces
- Independent process address spaces

**Inter-process communication**

- POSIX-style pipes with 16 KiB buffers
- Shared Memory (SHM)
- Zero-Copy graphical surface sharing

### 4. Amaryllis SDK

The standalone `amaryllis-sdk` crate provides an application development interface for external Ring 3 programs.

**SDK characteristics**

- `no_std` + `alloc`
- Global allocator
- Application-facing system interfaces
- Standalone ELF64 application development

**Available modules**

| Module | Purpose |
|---|---|
| `fs` | Filesystem operations |
| `env` | Application environment and arguments |
| `net` | Networking |
| `ipc` | Inter-process communication |
| `time` | Time-related interfaces |
| `gui` | Graphical application interfaces |
| `gui::ShmWindow` | Shared-memory graphical windows |

The SDK is designed to separate applications from internal kernel implementation details.

See [`amaryllis-sdk/README.md`](amaryllis-sdk/README.md) for SDK setup, examples, and application development.

### 5. Storage and Filesystems

ANU provides a unified storage architecture with caching and asynchronous device operations.

**Filesystem infrastructure**

- Virtual Filesystem (VFS)
- Unified filesystem tree rooted at `/`
- Writable RAMFS
- FAT32 filesystem
- FAT32 read and write support
- Long File Name (LFN) support

**Unified Page / Block Cache**

- 4 KiB cache pages
- `CachedBlockDevice`
- Clock replacement algorithm
- Read-ahead up to 16 KiB
- Write-back cache flushing
- Reported cache hit rate of approximately 98%

**Block I/O**

- Multi-block DMA transfers
- Up to 128 sectors (64 KiB) per operation
- VirtIO-Blk with split virtqueues
- AHCI SATA with 16-entry PRDT

### 6. Device Infrastructure — IOKit-Inspired Architecture

ANU uses a device infrastructure inspired by architectural ideas from Apple's IOKit.

**IORegistry**

- Hierarchical device object tree
- Device registration and discovery
- Runtime device inspection through `ioreg`

**IOWorkLoop**

- Lock-free IRQ Top-Half → IOWorkLoop Bottom-Half architecture
- Deferred interrupt processing
- Background kernel-thread execution
- Dedicated workers such as `knet_worker`, `kiokit_audio`, and `kiokit_blk`

This architecture separates time-critical interrupt handling from longer-running device operations.

**Implemented hardware drivers**

- VirtIO-Blk
- AHCI SATA
- Intel e1000 Ethernet
- Intel AC'97 audio
- PS/2 keyboard and mouse

### 7. Networking

ANU includes an independently implemented TCP/IP networking stack.

**Network hardware**

- Intel e1000
- MMIO device access
- DMA-based packet operations

**Network protocols**

| Layer | Implemented protocols |
|---|---|
| Link | Ethernet |
| Network | ARP, IPv4, ICMP |
| Transport | UDP, TCP |
| Configuration | DHCP |
| Name resolution | DNS |
| Application | HTTP |

**Additional functionality**

- ICMP ping
- DHCP configuration
- DNS resolution
- TCP communication
- HTTP requests
- Socket interfaces for Ring 3 applications

### 8. Audio

ANU includes an asynchronous audio subsystem.

- Intel AC'97 driver
- Buffer Descriptor List (BDL)
- DMA-based audio playback
- Non-blocking audio mixer
- Background audio processing through kernel threads

### 9. Kernel Diagnostics

ANU provides internal diagnostic and telemetry infrastructure.

**Implemented components**

- `klog` kernel logging
- `dmesg` diagnostic output
- Ring-buffer telemetry
- Atomic interrupt counters
- Preemption counters
- Page fault counters
- Runtime subsystem diagnostics

**Shell diagnostic commands**

` s m p `, `apic`, `ioreg`, `kthread`, `diskinfo`, `virtio`, `netinfo`.

---

## Amaryllis Desktop

Amaryllis includes its own graphical desktop environment, designed as an independent visual system rather than a direct reproduction of macOS.

Its design follows a consistent monochrome visual language focused on minimalism, responsiveness, and accessibility.

### Desktop Components

**Amaryllis Capsule**

A floating bottom capsule that combines:

- 3×3 application launcher
- Taskbar with application indicators
- Universal search inspired by Spotlight
- System island
- Expandable Control Center
- Calendar
- ACPI power menu

**Window management**

- Frameless top-left active-window menu
- Window titles aligned to the left
- Window controls aligned to the right
- Minimal window decoration

**Compositor**

- 120 FPS target
- Approximately 8 ms frame pacing
- `sti` / `hlt` event-driven idle handling
- Occlusion-aware rendering through `begin_frame_with_occluders`
- Dirty-rectangle rendering
- Wallpaper caching
- Cursor rendering without trails

**Typography and appearance**

- Vector font rendering through `fontdue`
- Glyph caching
- Cyrillic support
- Colloid and Lucide icons
- Monochrome Black and White themes
- TOML-based personalization service integrated with VFS

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

The next development phase focuses on expanding ANU's Mach-inspired memory, process, and IPC architecture, followed by a native filesystem.

### 1. Advanced Memory Management — Mach VM

- Physical frame reference counting in PMM
- Copy-on-Write (CoW)
- File-backed `mmap` integrated with Page Cache
- `fork()` and `waitpid()`
- POSIX signals

### 2. Mach-Inspired IPC

- Typed IPC message ports
- Send and Receive permissions
- Zero-Copy Out-of-Line (OOL) page transfer
- Bootstrap service registry
- Service discovery infrastructure inspired by `launchd`
- Mach Exception Ports

### 3. FmFS — Native Amaryllis Filesystem

Development of a native filesystem designed specifically for Amaryllis OS.

**Planned features**

- Extent-based storage architecture
- Filesystem access permissions
- Metadata checksums
- Journaling
- Copy-on-Write mechanisms
- Integration with VFS and Unified Page Cache

FmFS is planned as a modern replacement for FAT32 as the primary native filesystem.

---

## Development Philosophy

Amaryllis OS is built around four priorities:

1. **Performance** — efficient execution, low overhead, and responsive interactions.
2. **Convenience** — consistent system interfaces and intuitive workflows.
3. **Beauty** — a coherent visual identity and carefully designed desktop environment.
4. **Reliability** — predictable behavior, memory safety, fault handling, and robust subsystem integration.

Amaryllis OS is not intended to be a Linux distribution or a macOS clone.

It is an attempt to build an independent operating system with its own kernel architecture, system interfaces, device infrastructure, filesystem direction, and desktop experience.

---

## Building and Testing

Amaryllis OS targets bare-metal x86_64 hardware and virtualized environments.

**Primary development tools**

- Rust Nightly
- Cargo
- `x86_64-unknown-none` target
- QEMU
- x86_64 virtual hardware

Install the required Rust target:

```bash
rustup target add x86_64-unknown-none --toolchain nightly
```

The root project uses a custom Cargo workspace and build configuration. Build and launch commands should be taken from the repository's current build scripts and configuration.

---

## Project Status

**Amaryllis OS v0.11 is an actively developed experimental operating system.**

The current implementation includes:

- Custom 64-bit Rust hybrid kernel
- Real SMP and inter-processor interrupts
- Demand Paging and protected address spaces
- Preemptive scheduling and kernel threads
- ELF64 userland execution
- Pipes and Shared Memory
- VFS, FAT32, RAMFS, and unified caching
- VirtIO-Blk and AHCI storage drivers
- IOKit-inspired device infrastructure
- Ethernet and TCP/IP networking stack
- AC'97 audio
- Custom desktop environment with a 120 FPS target
- Seven built-in graphical applications

The next major milestones are Copy-on-Write memory management, advanced IPC ports, POSIX process interfaces, and the native FmFS filesystem.

---

## License

Amaryllis OS is licensed under the **Apache License 2.0**.

See [`LICENSE`](LICENSE) for details.

---

**AmaryllisOS — Built from the ground up.**

**ANU — Amaryllis is Not Unix.**