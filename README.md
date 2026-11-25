# Amaryllis OS

**Amaryllis OS** is an independent bare-metal operating system developed from scratch in Rust.

It is powered by **ANU (Amaryllis is Not Unix)** — an independently designed x86_64 hybrid kernel inspired by architectural concepts from Mach, BSD, IOKit, and Libkern.

The project includes its own memory management, SMP infrastructure, preemptive scheduler, kernel threads, userland execution environment, storage subsystem, networking stack, device infrastructure, and graphical desktop environment.

> **Amaryllis OS is an experimental operating system under active development.**

---

## Project Overview

| Property | Value |
|---|---|
| Version | v0.11.0 |
| Kernel | ANU — Amaryllis is Not Unix |
| Architecture | x86_64 |
| Language | Rust |
| Kernel environment | `#![no_std]` + `alloc` |
| Development status | Active development |
| Desktop | Amaryllis Desktop |
| License | Apache License 2.0 |

## Architecture

ANU is a custom hybrid kernel combining architectural ideas from several established operating system designs.

| Inspiration | Architectural concepts |
|---|---|
| Mach | Virtual memory, task and thread concepts, IPC direction |
| BSD | Process interfaces, file descriptors, networking concepts |
| IOKit | Device registry, driver infrastructure, work-loop architecture |
| Libkern | Kernel-level utilities and infrastructure |

These are architectural influences, not imported implementations. ANU is independently developed in Rust.

The kernel is organized into separate subsystems for architecture, memory, scheduling, userland, storage, drivers, networking, and graphics.

---

## Implemented Features

### 1. x86_64 Architecture and SMP

**Architecture support:**

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

- 4-core SMP configuration.
- AP startup using INIT-SIPI-SIPI.
- 16-bit Real Mode → 32-bit Protected Mode → 64-bit Long Mode trampoline.
- Physical trampoline address `0x8000`.
- MADT-based CPU discovery.
- Per-CPU structures (`PER_CPU[16]`).
- Independent GDT, TSS, stacks, and Local APIC timers.
- TLB Shootdown IPI (`0xF0`).
- Reschedule IPI (`0xF1`).

### 2. Memory Management

**Implemented:**

- Bitmap-based Physical Memory Manager.
- Physical frame allocation and deallocation.
- Physical frame reference counting (PMM refcount).
- Copy-on-Write (CoW) memory management.
- CoW page fault handling through `#PF`.
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
- 4 KiB guard pages for user and kernel thread stacks.
- Hardened `copy_from_user` and `copy_to_user`.

CoW allows processes to share physical memory pages until a write requires a private copy.

### 3. Scheduling and Kernel Threads

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

### 4. Userland and System Interfaces

**Implemented:**

- Ring 3 execution.
- Independent process address spaces.
- ELF64 loader.
- SysV ABI `argc` / `argv`.
- `syscall` / `sysret` interface.
- `brk`, `mmap`, and `munmap`.
- Process memory reclamation.
- `fork()` with Copy-on-Write address-space duplication.
- `waitpid()` for child-process synchronization.
- VFS file and directory descriptors.
- File operations through system calls.
- Process creation through `SYS_SPAWN_ELF`.
- Socket and networking system calls.
- Wait queues.
- POSIX-style pipes with 16 KiB buffers.
- Shared Memory for Zero-Copy graphical surfaces.

### 5. Amaryllis SDK

The standalone `amaryllis-sdk` crate provides interfaces for external ELF applications.

**Implemented modules:**

| Module | Purpose |
|---|---|
| `alloc_heap` | Application heap allocator |
| `env` | Environment and application arguments |
| `fs` | Filesystem interfaces |
| `io` | Input/output abstractions |
| `proc` | Process interfaces |
| `time` | Time-related functionality |
| `net` | Networking |
| `ipc` | Inter-process communication |
| `gui` | Graphical interfaces |
| `gui::ShmWindow` | Shared-memory graphical windows |

Additional functionality:

- `no_std` + `alloc`.
- Global allocator.
- `Vec`, `String`, `Box`, and `BTreeMap` support.
- Application arguments.
- Filesystem interfaces.
- Networking interfaces.
- GUI integration.
- Process interfaces.
- Linker configuration.
- Runnable example applications.

**SDK examples:**

- `hello`
- `multitask_counter`
- `gui_plasma`
- `net_http_client`
- `fs_alloc_demo`
- `ipc_shm_gui`
- `fork_demo`

The `fork_demo` example demonstrates:

- Calling `fork()`.
- Copy-on-Write memory behavior.
- Isolated writes to the process stack.
- Waiting for child-process completion through `waitpid()`.

See [`amaryllis-sdk/README.md`](amaryllis-sdk/README.md) for SDK usage and examples.

### 6. Storage and Filesystems

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

**Planned:** FmFS — the native Amaryllis filesystem.

### 7. Device Infrastructure

The device infrastructure is inspired by Apple's IOKit architecture but is independently implemented.

**Implemented:**

- Unified device registry.
- IORegistry device tree.
- Runtime device inspection through `ioreg`.
- Lock-free IRQ Top-Half → IOWorkLoop Bottom-Half architecture.
- Background processing through kernel threads.
- Asynchronous device operations.

**Available drivers:**

- VirtIO-Blk.
- AHCI SATA.
- Intel e1000.
- Intel AC'97.
- PS/2 keyboard and mouse.

### 8. Networking

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

### 9. Audio

**Implemented:**

- Intel AC'97 driver.
- BDL DMA playback.
- Mixer volume control.
- Variable sample rate configuration.
- Non-blocking audio mixer.
- WAV streaming from FAT32/VFS.
- Background audio processing.

### 10. Kernel Diagnostics

**Implemented:**

- `klog` kernel logging.
- `dmesg`.
- Ring-buffer telemetry.
- Atomic IRQ counters.
- Preemption counters.
- Page fault counters.
- Runtime subsystem diagnostics.

Available diagnostic commands include:

` smp `, `apic`, `ioreg`, `kthread`, `diskinfo`, `virtio`, `netinfo`.

### 11. Amaryllis Desktop

Amaryllis includes an independently developed graphical desktop environment.

**Graphical infrastructure:**

- Custom window manager.
- Double-buffered compositor.
- Dirty-rectangle rendering.
- Occlusion-aware rendering.
- Wallpaper caching.
- Window dragging, resizing, and snapping.
- Floating Amaryllis Capsule.
- Application launcher.
- Universal search.
- System Island.
- Control Center.
- Calendar.
- Power menu.
- Active-window menu.
- Light and Dark themes.
- Vector-style typography.
- Glyph caching.
- Cyrillic support.
- TOML-based personalization.

**Performance target:** 120 FPS, with approximately 8 ms frame pacing.

**Built-in applications:**

1. Files
2. Terminal
3. Text Editor
4. Audio Player
5. Net Viewer
6. System Settings
7. Activity Monitor

---

## Development Roadmap

The next development phase focuses on expanding the IPC architecture, native filesystem, and overall system reliability.

### 1. Mach-Inspired IPC

- [ ] Typed IPC message ports.
- [ ] Send / Receive permissions.
- [ ] Zero-Copy Out-of-Line page transfer.
- [ ] Bootstrap service registry.
- [ ] Service discovery infrastructure inspired by `launchd`.
- [ ] Mach Exception Ports.

### 2. FmFS — Native Amaryllis Filesystem

- [ ] Extent-based storage architecture.
- [ ] Filesystem access permissions.
- [ ] Metadata checksums.
- [ ] Journaling.
- [ ] Copy-on-Write filesystem mechanisms.
- [ ] VFS integration.
- [ ] Unified Page Cache integration.

FmFS is planned as the native filesystem direction for Amaryllis OS, eventually replacing FAT32 as the primary filesystem.

### 3. Reliability and Compatibility

- [ ] Expanded SMP stress testing.
- [ ] Memory pressure and CoW stress testing.
- [ ] Process lifecycle and fault-recovery testing.
- [ ] Storage consistency testing.
- [ ] Expanded physical hardware compatibility.
- [ ] Long-running workload validation.

---

## Building and Running

### Requirements

- Rust Nightly.
- Cargo.
- QEMU (`qemu-system-x86_64`).
- `x86_64-unknown-none` Rust target.

Install the required Rust target:

```bash
rustup target add x86_64-unknown-none --toolchain nightly
```

### Build

Run from the repository root:

```bash
cargo +nightly build
```

The root package uses Cargo artifact dependencies to compile the kernel as an `x86_64-unknown-none` binary.

During the build, `build.rs` automatically:

1. Retrieves the compiled kernel ELF artifact.
2. Configures the bootloader framebuffer with a minimum resolution of 1600×900.
3. Creates a BIOS-bootable disk image using `bootloader 0.11.17`.
4. Exposes the generated image path through the `BIOS_PATH` environment variable.

The BIOS image is generated inside Cargo's build output directory.

### Run on Linux

After building, locate the generated BIOS image:

```bash
BIOS_IMAGE=$(find target -type f -name bios.img -print -quit)
```

Launch it with QEMU:

```bash
qemu-system-x86_64 \
  -drive format=raw,file="$BIOS_IMAGE" \
  -m 512M
```

The current launcher in `src/main.rs` uses a Windows-specific QEMU executable path. On Linux, use the direct QEMU command above or update the launcher to use `qemu-system-x86_64` from `PATH`.

### Build Configuration

The root `.cargo/config.toml` enables Cargo's unstable binary dependency support:

```toml
[unstable]
bindeps = true
```

The release profile uses optimized compilation with a single code generation unit:

```toml
[profile.release]
opt-level = 3
codegen-units = 1
panic = "abort"
```

This configuration is intended to produce a more optimized final binary.

**Boot configuration:** The current build creates a BIOS boot image. UEFI boot support is not established by this configuration.

---

## Development Priorities

The project follows four core principles:

1. **Performance** — efficient execution and low system overhead.
2. **Convenience** — consistent interfaces and intuitive workflows.
3. **Beauty** — a coherent visual identity and desktop experience.
4. **Reliability** — predictable behavior, robust memory management, and fault handling.

Current development priorities:

- Strengthen SMP and concurrency validation.
- Improve process and IPC infrastructure.
- Expand hardware compatibility.
- Improve storage reliability.
- Continue developing the native filesystem.
- Validate CoW under memory pressure and concurrent workloads.
- Validate the kernel under sustained workloads.

---

## Development Notes

- A successful compilation does not automatically prove runtime correctness.
- Implemented functionality should be distinguished from fully validated functionality.
- SMP, interrupt handling, memory management, and scheduling changes require careful regression testing.
- CoW requires careful validation of page reference counts, write faults, and address-space teardown.
- Avoid unnecessary rewrites of working subsystems.
- Preserve memory-management validation results.
- Maintain clear subsystem boundaries and avoid circular dependencies.
- Test changes in QEMU and on physical hardware where applicable.
- Update project documentation after major milestones.

*Implemented indicates that the corresponding functionality has been developed. It does not imply complete hardware compatibility, production-level validation, or feature parity with mature operating systems.*

---

## Current Project State

Amaryllis OS v0.11.0 has progressed beyond its initial kernel and graphical prototype stages.

The project now includes:

- A custom x86_64 hybrid kernel.
- 4-core SMP configuration and inter-processor communication.
- Physical memory reference counting and Copy-on-Write.
- Process creation through `fork()` and synchronization through `waitpid()`.
- Preemptive scheduling and kernel threads.
- Ring 3 execution and an application SDK.
- Storage and networking subsystems.
- Device infrastructure and audio support.
- An independent graphical desktop environment.

The fundamental architecture is in place. The current challenge is expanding its capabilities while improving correctness, robustness, and compatibility.

The next major milestones are Mach-inspired IPC Ports, native FmFS, and deeper system reliability validation.

---

## License

Amaryllis OS is licensed under the **Apache License 2.0**.

See [`LICENSE`](LICENSE) for the full license text.

---

**Amaryllis OS — Built from the ground up.**

**ANU — Amaryllis is Not Unix.**