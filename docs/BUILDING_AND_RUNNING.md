# Amaryllis OS — Building, Running & Development Guide

**Version:** 0.11.0  
**Kernel:** ANU (Amaryllis is Not Unix)  
**Architecture:** x86_64  
**Language:** Rust  
**License:** Apache-2.0

---

# Table of Contents

1. Introduction
2. Project Overview
3. System Requirements
4. Development Environment Setup
5. Repository Structure
6. Build System
7. Building Amaryllis OS
8. Running in QEMU
9. QEMU Virtual Hardware
10. Boot Process
11. First Boot
12. ANU Kernel Architecture
13. CPU and SMP
14. Memory Management
15. Scheduling and Kernel Threads
16. Interrupts and Device Architecture
17. Storage and VFS
18. Networking
19. Audio
20. Userland and Ring 3
21. Amaryllis SDK
22. Inter-Process Communication
23. Graphics and Desktop Environment
24. Shell Command Reference
25. Debugging and Diagnostics
26. Testing and Reliability
27. Troubleshooting
28. Known Limitations
29. Development Workflow
30. License

---

# 1. Introduction

Amaryllis OS is an independent operating system project written primarily in Rust.

It is built from the ground up for the x86_64 architecture and uses a custom kernel called **ANU — Amaryllis is Not Unix**.

The project explores how a modern operating system can combine:

- A hybrid kernel architecture.
- Memory-safe systems programming.
- Multiprocessor execution.
- Preemptive multitasking.
- Independent user processes.
- Modern storage and networking.
- A custom graphical desktop environment.
- A dedicated application development SDK.

Amaryllis is not a Linux distribution, a modified BSD system, or a port of an existing operating system kernel.

It is an independent implementation with its own kernel, system-call interface, userland, SDK, device architecture and graphical environment.

## 1.1 Project philosophy

The development priorities are:

1. Performance.
2. Convenience.
3. Beauty.
4. Reliability.

The goal is not simply to make an operating system boot.

The goal is to build a coherent platform where the kernel, drivers, applications, filesystem, networking and graphical environment work together.

## 1.2 Why Rust?

Rust provides useful mechanisms for systems programming:

- Ownership and borrowing.
- Compile-time memory safety.
- Explicit mutability.
- Strong type checking.
- Abstractions with predictable runtime overhead.
- Concurrency primitives.

However, bare-metal operating system development still requires careful use of `unsafe`.

Hardware access, page tables, interrupt handling, context switching and low-level memory management cannot be made automatically safe by the language.

Rust reduces certain classes of programming errors, but it does not eliminate logical bugs, race conditions, hardware failures or incorrect unsafe code.

---

# 2. Project Overview

Amaryllis OS consists of several major components.

| Component | Responsibility |
|---|---|
| ANU Kernel | Core operating system functionality |
| Bootloader integration | Loads the kernel |
| Memory subsystem | Physical and virtual memory |
| Scheduler | Processes, threads and CPU scheduling |
| IOKit-inspired architecture | Device management and I/O work loops |
| Storage subsystem | Block devices, partitions and filesystems |
| Networking subsystem | Network protocols and sockets |
| Userland | Ring 3 applications |
| Amaryllis SDK | APIs for user applications |
| Desktop environment | Windows, compositor and system UI |

## 2.1 Kernel architecture

ANU uses a custom hybrid architecture inspired by ideas from:

- Mach.
- BSD.
- IOKit.
- Libkern.

These projects provide architectural inspiration, not source-code compatibility.

The kernel keeps important operating system functionality in kernel space while using separate user processes for applications.

## 2.2 Versioning

The current project version is:

```text
Amaryllis OS 0.11.0
```

This is a development-stage operating system.

A version number does not imply complete hardware compatibility, production readiness or full POSIX compatibility.

---

# 3. System Requirements

## 3.1 Recommended development hardware

| Component | Recommendation |
|---|---|
| CPU | Modern x86_64 processor |
| RAM | 8 GB minimum, 16 GB recommended |
| Storage | SSD |
| Virtualization | Intel VT-x or AMD-V |
| Display | QEMU-compatible virtual display |
| Network | Standard Ethernet or Wi-Fi host connection |

## 3.2 Required software

| Software | Purpose |
|---|---|
| Rust Nightly | Kernel and SDK compilation |
| Cargo | Build system |
| rust-src | Rust source component |
| llvm-tools-preview | LLVM-related toolchain utilities |
| QEMU | Virtual hardware and execution |
| Git | Source control |

## 3.3 Rust toolchain

Amaryllis requires Nightly Rust because the kernel uses unstable language and compiler features.

Install Nightly:

```bash
rustup toolchain install nightly
```

Install required components:

```bash
rustup component add rust-src llvm-tools-preview --toolchain nightly
```

Install the target:

```bash
rustup target add x86_64-unknown-none --toolchain nightly
```

Verify:

```bash
rustc +nightly --version
cargo +nightly --version
rustup target list --installed --toolchain nightly
```

The kernel target is:

```text
x86_64-unknown-none
```

---

# 4. Development Environment Setup

## 4.1 Arch Linux

Install the required packages:

```bash
sudo pacman -Syu
sudo pacman -S git rustup qemu-desktop
```

Initialize Rust if necessary:

```bash
rustup default stable
```

Install Nightly and its components:

```bash
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview --toolchain nightly
rustup target add x86_64-unknown-none --toolchain nightly
```

Verify QEMU:

```bash
qemu-system-x86_64 --version
```

Check KVM:

```bash
ls -l /dev/kvm
```

If necessary, add the current user to the KVM group:

```bash
sudo usermod -aG kvm "$USER"
```

Log out and back in after changing group membership.

## 4.2 Ubuntu

Install the required software:

```bash
sudo apt update
sudo apt install git curl build-essential qemu-system-x86 qemu-utils
```

Install Rust using rustup if it is not already installed.

Then configure Nightly:

```bash
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview --toolchain nightly
rustup target add x86_64-unknown-none --toolchain nightly
```

Verify:

```bash
qemu-system-x86_64 --version
cargo +nightly --version
```

## 4.3 Windows

Install:

- Git for Windows.
- Rust through rustup.
- QEMU for Windows.

Install Nightly:

```powershell
rustup toolchain install nightly
rustup component add rust-src llvm-tools-preview --toolchain nightly
rustup target add x86_64-unknown-none --toolchain nightly
```

Make sure the QEMU installation directory is included in the Windows `PATH`.

Verify:

```powershell
qemu-system-x86_64.exe --version
```

The current launcher invokes:

```rust
Command::new("qemu-system-x86_64")
```

Therefore, the executable must be discoverable through `PATH`.

The current QEMU configuration uses KVM and a PulseAudio audio backend. These options are Linux-oriented. Windows users may need a host-specific launcher configuration and an appropriate QEMU audio backend.

## 4.4 Virtual machines and WSL

KVM requires access to hardware virtualization features exposed by the host.

If the outer hypervisor does not expose virtualization extensions, `-enable-kvm` may fail.

In this situation, use QEMU TCG software emulation.

TCG is slower than KVM and can significantly affect the performance of SMP workloads.

---

# 5. Repository Structure

The repository is organized as a Cargo workspace.

```text
AmaryllisOS/
│
├── .cargo/
│   └── config.toml
│
├── amaryllis-sdk/
│   ├── .cargo/
│   ├── src/
│   ├── examples/
│   └── Cargo.toml
│
├── kernel/
│   ├── src/
│   └── Cargo.toml
│
├── docs/
│
├── src/
│   └── main.rs
│
├── build.rs
├── Cargo.toml
├── Cargo.lock
├── LICENSE
└── README.md
```

## 5.1 Root project

The root Cargo package coordinates the build and launches QEMU.

Its responsibilities include:

- Building the kernel artifact.
- Running the build script.
- Generating the BIOS boot image.
- Providing the image path to the launcher.
- Starting QEMU.

## 5.2 Kernel directory

The `kernel/` directory contains the ANU kernel.

This is the core of Amaryllis OS.

It contains hardware initialization, memory management, scheduling, device support, storage, networking, userland and graphical subsystems.

## 5.3 SDK directory

The `amaryllis-sdk/` directory contains the userland development library.

It provides APIs for applications that execute outside kernel space.

## 5.4 Documentation

The `docs/` directory contains project documentation.

---

# 6. Build System

Amaryllis uses Cargo and a multi-package workspace.

## 6.1 Cargo workspace

The workspace contains:

```toml
[workspace]
resolver = "3"
members = ["kernel", "amaryllis-sdk"]
```

The main packages are:

- `amaryllis_os`
- `kernel`
- `amaryllis-sdk`

## 6.2 Build script

The root `build.rs` performs boot image generation.

Its responsibilities are:

1. Read Cargo's output directory.
2. Locate the compiled kernel binary.
3. Create a BIOS boot configuration.
4. Set minimum framebuffer dimensions.
5. Create a BIOS disk image using `bootloader`.
6. Export the generated image path through `BIOS_PATH`.

The framebuffer minimum dimensions are configured as:

```rust
boot_config.frame_buffer.minimum_framebuffer_width = Some(1600);
boot_config.frame_buffer.minimum_framebuffer_height = Some(900);
```

The bootloader dependency is:

```toml
bootloader = "0.11.17"
```

The BIOS image is generated automatically during the Cargo build process.

## 6.3 Build profiles

The root project uses optimized development and release profiles.

The release profile uses:

```toml
[profile.release]
opt-level = 3
panic = "abort"
codegen-units = 1
```

The SDK additionally uses:

```toml
lto = true
strip = "debuginfo"
```

## 6.4 Cargo configuration

The root `.cargo/config.toml` contains:

```toml
[unstable]
bindeps = true
```

This enables the unstable Cargo feature required by the workspace's binary dependency configuration.

The SDK also has its own Cargo configuration. Keep it intact when modifying the root build configuration.

---

# 7. Building Amaryllis OS

## 7.1 Clone the repository

```bash
git clone https://github.com/MaksLLL-prog/AmaryllisOS.git
cd AmaryllisOS
```

## 7.2 Build the kernel and workspace

```bash
cargo +nightly build --release
```

Cargo compiles the workspace according to its package configuration.

The build script generates the BIOS boot image after the kernel artifact is available.

## 7.3 Build and launch

The normal development command is:

```bash
cargo +nightly run --release
```

This command builds the project and starts the root launcher.

The launcher retrieves the image path using:

```rust
env!("BIOS_PATH")
```

It then starts QEMU.

## 7.4 Build only

```bash
cargo +nightly build --release
```

## 7.5 Check the workspace

```bash
cargo +nightly check
```

A successful check does not replace an actual kernel build or runtime test.

---

# 8. Running in QEMU

QEMU provides the virtual hardware used to test Amaryllis OS.

The current launcher configuration is:

```rust
use std::process::Command;

fn main() {
    println!("Starting AmaryllisOS...");

    let mut args = vec![
        "-enable-kvm".to_string(),
        "-cpu".to_string(),
        "host".to_string(),
        "-smp".to_string(),
        "4".to_string(),
        "-drive".to_string(),
        format!("format=raw,file={}", env!("BIOS_PATH")),
        "-m".to_string(),
        "1G".to_string(),
        "-serial".to_string(),
        "stdio".to_string(),
        "-netdev".to_string(),
        "user,id=net0".to_string(),
        "-device".to_string(),
        "e1000,netdev=net0".to_string(),
        "-audiodev".to_string(),
        "pa,id=snd0".to_string(),
        "-device".to_string(),
        "AC97,audiodev=snd0".to_string(),
    ];

    if std::path::Path::new("disk_virtio.img").exists() {
        args.extend([
            "-drive".to_string(),
            "file=disk_virtio.img,if=virtio,format=raw".to_string(),
        ]);
    }

    let status = Command::new("qemu-system-x86_64")
        .args(&args)
        .status()
        .expect("Failed to start QEMU");

    println!("QEMU exited with: {status}");
}
```

## 8.1 Current virtual machine configuration

| Setting | Value |
|---|---|
| Emulator | QEMU x86_64 |
| Acceleration | KVM |
| CPU model | Host |
| Virtual CPUs | 4 |
| RAM | 1 GiB |
| Boot image | Generated BIOS image |
| Serial output | stdio |
| Network backend | User networking |
| Network adapter | Intel e1000 |
| Audio backend | PulseAudio |
| Audio controller | AC'97 |
| Additional disk | Optional VirtIO block device |

## 8.2 KVM acceleration

The following arguments enable KVM:

```text
-enable-kvm
-cpu host
```

KVM allows QEMU to use hardware virtualization instead of translating guest CPU execution entirely through software emulation.

## 8.3 TCG fallback

When KVM is unavailable, use:

```text
-accel tcg
-cpu max
```

Do not combine the TCG configuration with `-enable-kvm`.

TCG is a software emulator and is generally slower.

## 8.4 Serial console

The launcher uses:

```text
-serial stdio
```

This connects the emulated serial port to the terminal running QEMU.

Serial output is useful for:

- Kernel boot messages.
- Diagnostics.
- Panic information.
- Debugging early initialization.

---

# 9. QEMU Virtual Hardware

The current QEMU setup provides several virtual devices.

## 9.1 Virtual CPU configuration

```text
-smp 4
```

This creates four virtual CPUs.

The kernel can use these CPUs for SMP initialization and multiprocessor execution.

## 9.2 Memory

```text
-m 1G
```

The guest receives 1 GiB of RAM.

## 9.3 Intel e1000 network adapter

```text
-netdev user,id=net0
-device e1000,netdev=net0
```

The e1000 device provides an emulated Intel Ethernet controller.

The user networking backend provides a virtual network connection through the host.

## 9.4 AC'97 audio

```text
-audiodev pa,id=snd0
-device AC97,audiodev=snd0
```

This creates an AC'97 audio device and connects it to the host's PulseAudio backend.

## 9.5 VirtIO block device

If `disk_virtio.img` exists in the working directory, the launcher attaches it:

```text
-drive file=disk_virtio.img,if=virtio,format=raw
```

The disk image must already exist and contain a layout supported by the guest storage stack.

## 9.6 Creating a disk image

For example:

```bash
qemu-img create -f raw disk_virtio.img 256M
```

This creates an empty 256 MiB image. It does not automatically create a partition table or filesystem.

---

# 10. Boot Process

Amaryllis uses a BIOS boot image generated through the `bootloader` crate.

At a high level, the boot flow is:

1. QEMU initializes its virtual machine.
2. QEMU loads the generated BIOS disk image.
3. The bootloader performs its boot-time responsibilities.
4. The kernel is loaded and receives its boot information.
5. ANU begins kernel initialization.
6. Hardware and kernel subsystems are initialized.
7. The system proceeds toward its operational state.

The exact low-level sequence depends on the bootloader implementation and kernel initialization code.

---

# 11. First Boot

After starting:

```bash
cargo +nightly run --release
```

QEMU should launch with the configured virtual hardware.

Start with:

```text
help
```

Then inspect the hardware and kernel subsystems:

```text
smp
apic
ioreg
diskinfo
netinfo
```

Try the kernel-thread demonstration:

```text
kthread demo
```

Try the ELF demonstration:

```text
mkelf fork
exec /fork_demo.elf
```

The exact output depends on the current kernel configuration and available devices.

---

# 12. ANU Kernel Architecture

ANU is the custom kernel at the center of Amaryllis OS.

Its architecture combines kernel-resident services with isolated Ring 3 applications.

## 12.1 Main architectural areas

| Subsystem | Purpose |
|---|---|
| Architecture | x86_64 CPU and hardware support |
| ACPI | Platform description and hardware discovery |
| Interrupts | CPU and device interrupt handling |
| SMP | Multiprocessor initialization and coordination |
| Memory | Physical and virtual memory management |
| Scheduler | Process and thread scheduling |
| Userland | User-mode process execution |
| PCI | PCI device discovery and resources |
| IOKit-inspired layer | Device registry and I/O work loops |
| Storage | Block devices, partitions and filesystems |
| Networking | Network protocols and sockets |
| Graphics | Framebuffer and desktop rendering |
| Telemetry | Kernel logging and diagnostics |

## 12.2 Hybrid architecture

The kernel is not a pure microkernel.

Important functionality remains in kernel space, while application code executes in isolated user processes.

## 12.3 Kernel and userland separation

Kernel code has privileged access to hardware and kernel memory.

Ring 3 applications execute with restricted CPU privileges and interact with the kernel through system calls.

---

# 13. CPU and SMP

Amaryllis supports symmetric multiprocessing on x86_64.

The current QEMU configuration provides four virtual CPUs.

## 13.1 ACPI discovery

The kernel includes ACPI parsing for:

- RSDP.
- RSDT.
- XSDT.
- FADT.
- MADT.
- MCFG.
- HPET.

These tables provide information about the platform's hardware configuration.

## 13.2 Local APIC

The kernel uses the Local APIC for processor-local interrupt handling.

The current timer configuration uses a 1000 Hz timer frequency.

A timer tick is approximately:

\[
T = \frac{1}{1000} = 1\text{ ms}
\]

## 13.3 I/O APIC

The I/O APIC handles external interrupt routing.

The kernel includes:

- GSI routing.
- ACPI interrupt source overrides.
- Interrupt routing configuration.

## 13.4 Application processors

Additional CPUs are started using the INIT-SIPI-SIPI startup sequence.

The startup trampoline is located at physical address:

```text
0x8000
```

The kernel maintains per-CPU structures, including:

- CPU-local state.
- GDT.
- TSS.
- Kernel stacks.
- Local APIC timer state.

The per-CPU storage is sized for up to 16 CPU entries.

## 13.5 Inter-processor interrupts

Current dedicated vectors include:

| Vector | Purpose |
|---|---|
| `0xF0` | TLB Shootdown IPI |
| `0xF1` | Reschedule IPI |

### TLB Shootdown

When address-space changes require remote processors to invalidate stale translations, the kernel can coordinate this using TLB Shootdown IPIs.

### Reschedule IPI

A reschedule IPI allows one CPU to request scheduling activity on another CPU.

---

# 14. Memory Management

Memory management is one of the central components of ANU.

## 14.1 Physical memory manager

The physical memory manager uses a bitmap-based allocation mechanism.

Its responsibilities include:

- Tracking physical frames.
- Allocating frames.
- Releasing frames.
- Maintaining frame reference counts.
- Providing physical memory to higher-level allocators.

The current frame size is 4096 bytes (4 KiB).

## 14.2 Virtual memory

The kernel uses four-level x86_64 paging.

Each process can have its own address space.

The kernel switches address spaces through CR3.

The implementation also maintains the shared kernel mappings required when switching to a process page table.

## 14.3 Dynamic kernel mappings

The bootloader's dynamic mapping configuration means that assumptions about fixed kernel page-table locations can be incorrect.

ANU synchronizes the required shared kernel PML4 entries between address spaces.

## 14.4 NX and W^X

The kernel supports NXE and uses W^X-oriented memory protections.

The general W^X principle is that a memory region should not be writable and executable at the same time.

## 14.5 Kernel heap

The kernel has a configured heap size of:

```text
64 MiB
```

The heap is used for dynamic kernel allocations.

## 14.6 Virtual memory regions

The VM subsystem tracks virtual memory regions.

The implementation includes support for:

- Demand Paging.
- Lazy `mmap`.
- Stack growth.
- Guard pages.
- Process-specific address spaces.

## 14.7 Demand Paging

Demand Paging allows certain pages to be allocated or populated only when they are first accessed.

This can reduce unnecessary physical memory allocation.

## 14.8 Stack growth and guard pages

User stacks support growth through page-fault handling.

Guard pages provide unmapped regions that help detect invalid stack accesses.

## 14.9 Copy-on-Write

ANU implements Copy-on-Write memory management.

CoW allows processes to initially share physical pages rather than immediately duplicating all memory.

When a process attempts to write to a shared CoW page, the page-fault handler can create a private copy.

The physical memory manager tracks frame references to determine whether a page is shared.

The general sequence is:

1. A process creates a child address space through `fork()`.
2. Eligible memory pages are shared.
3. Shared mappings are marked for Copy-on-Write.
4. A process attempts to write to a protected page.
5. A page fault occurs.
6. The kernel checks whether the fault corresponds to a CoW mapping.
7. The kernel allocates and copies a frame when necessary.
8. The process receives a writable private mapping.

CoW reduces the cost of process creation when the child does not immediately modify all inherited memory.

## 14.10 User memory access

The kernel includes hardened user-memory copying mechanisms:

- `copy_from_user`
- `copy_to_user`

These are important because system-call arguments can contain pointers supplied by untrusted Ring 3 applications.

---

# 15. Scheduling and Kernel Threads

ANU includes a preemptive scheduler.

## 15.1 Preemptive scheduling

A preemptive scheduler can interrupt a running task and select another task for execution.

The kernel uses a virtual-runtime-based scheduling approach.

The `vruntime_ms` value is used to track scheduling progress.

## 15.2 Context switching

The scheduler maintains CPU execution context information.

The implementation also handles FPU/SSE context state.

Correct context switching is essential because each process and thread must resume with its own execution state.

## 15.3 Preemption protection

The kernel includes synchronization mechanisms such as:

- `PreemptGuard`
- `PreemptMutex`

These help protect critical sections against unwanted preemption.

## 15.4 Kernel threads

ANU supports preemptive kernel threads.

Kernel threads execute privileged kernel code and are scheduled by the kernel.

The current implementation uses the following stack configuration:

| Parameter | Size |
|---|---:|
| Kernel thread stack | **512 KiB** |
| Guard region | **16 KiB** |
| Guard region in pages | **4 × 4 KiB pages** |

The kernel-thread stack allocation uses an isolated virtual-memory slot associated with the PML4 address:

```text
0x0A00_0000_0000
```

This layout separates the kernel-thread stack region from other virtual-memory areas.

The guard region provides an unmapped boundary intended to detect stack overflows before they silently corrupt neighboring memory.

Kernel-thread stacks are configured independently from ordinary user-process stacks.

The implementation also supports execution on AP cores.

Examples of kernel worker names include:

```text
knet_worker
kiokit_audio
kiokit_blk
```

These workers are associated with network, audio and block-device work.

## 15.5 Scheduling considerations

A working scheduler does not automatically guarantee fairness or stability under every workload.

Important areas for testing include:

- Multiple runnable processes.
- Frequent blocking and waking.
- CPU-bound workloads.
- I/O-bound workloads.
- Concurrent memory allocation.
- Process termination during scheduling.
- CPU migration.
- Lock contention.

---

# 16. Interrupts and Device Architecture

Hardware interrupts allow devices and processors to notify the kernel about events.

ANU uses interrupt handling and deferred work to separate urgent interrupt processing from longer operations.

## 16.1 Interrupt handling

Interrupt handlers should perform only the work required to acknowledge and capture the event.

Long-running operations are better suited to deferred processing.

## 16.2 Top-Half and Bottom-Half

The device architecture uses an IRQ Top-Half and I/O Bottom-Half approach.

The Top-Half handles urgent interrupt work.

The Bottom-Half performs deferred processing in kernel-thread context.

## 16.3 IOKit-inspired device architecture

The kernel includes an IOKit-inspired device architecture.

Important concepts include:

- IORegistry.
- Device discovery.
- Driver registration.
- I/O work loops.
- Deferred interrupt processing.

Use:

```text
ioreg
```

to inspect the available device registry.

## 16.4 PCI

The PCI subsystem provides device discovery and resource information.

PCI devices can expose:

- Vendor IDs.
- Device IDs.
- Class codes.
- BAR resources.
- Interrupt information.

## 16.5 Current driver coverage

| Device | Implementation |
|---|---|
| Intel e1000 | Network controller |
| AC'97 | Audio controller |
| VirtIO-Blk | Block storage |
| AHCI | SATA storage |
| PS/2 | Keyboard and related legacy input |

The presence of a driver does not imply support for every hardware revision.

---

# 17. Storage and VFS

Amaryllis includes a storage subsystem with a virtual filesystem layer.

## 17.1 VFS

The Virtual Filesystem provides a common interface for accessing files and directories.

It separates filesystem operations from the underlying storage implementation.

## 17.2 RAMFS

The kernel includes a writable RAMFS.

RAMFS stores files in memory rather than on a persistent block device.

Its contents are not inherently persistent across a full system restart.

## 17.3 FAT32

The storage subsystem includes FAT32 read and write support.

The implementation also includes Long File Name support.

## 17.4 Block devices

The storage layer uses a common block-device abstraction.

Supported storage paths include:

- VirtIO-Blk.
- AHCI SATA.

## 17.5 Caching

The storage subsystem includes a 4 KiB page/block cache.

Its mechanisms include:

- Clock-based cache replacement.
- Read-ahead.
- Write-back caching.

The configured read-ahead size is:

```text
16 KiB
```

The project has reported approximately 98% cache hits in a particular workload.

This figure is workload-dependent and should not be interpreted as a universal performance guarantee.

## 17.6 DMA

The storage implementation supports DMA transfers up to:

```text
128 sectors
64 KiB
```

The exact transfer size also depends on device and driver constraints.

## 17.7 Storage commands

Use:

```text
diskinfo
```

to inspect storage information.

---

# 18. Networking

Amaryllis includes a kernel networking stack and Ring 3 socket interfaces.

## 18.1 Network device

The current QEMU configuration uses:

```text
e1000
```

The virtual network backend is:

```text
-netdev user,id=net0
```

## 18.2 Protocol support

| Protocol | Purpose |
|---|---|
| Ethernet | Link-layer framing |
| ARP | IPv4 address resolution |
| IPv4 | Network-layer packet transport |
| ICMP | Network diagnostics |
| UDP | Datagram transport |
| DHCP | Dynamic network configuration |
| DNS | Domain-name resolution |
| TCP | Reliable transport |
| HTTP | Application-layer requests |

## 18.3 Network initialization

The e1000 driver provides the device interface.

The networking subsystem builds higher-level protocol functionality on top of the network device.

Use:

```text
netinfo
```

to inspect network information.

## 18.4 Ring 3 networking

The SDK exposes networking-related system-call constants and APIs.

The current syscall interface includes operations for:

- Socket creation.
- Connect.
- Send.
- Receive.
- Bind.

It also includes higher-level network operations such as DNS resolution and HTTP GET.

## 18.5 HTTP example

The SDK contains:

```text
net_http_client
```

This example demonstrates the HTTP client workflow.

## 18.6 Current limitations

Do not assume support for IPv6, TLS, a complete POSIX socket implementation or every TCP feature.

---

# 19. Audio

Amaryllis includes an AC'97 audio driver and a basic audio subsystem.

## 19.1 AC'97

The QEMU launcher creates an AC'97 device:

```text
-device AC97,audiodev=snd0
```

The audio backend is configured through:

```text
-audiodev pa,id=snd0
```

## 19.2 DMA

The audio implementation uses Buffer Descriptor Lists (BDL) for DMA-oriented audio transfers.

## 19.3 Mixer

The current audio subsystem includes a nonblocking mixer.

The audio path uses deferred work through the kernel's I/O architecture.

---

# 20. Userland and Ring 3

Amaryllis supports user applications executing in Ring 3.

This is a major architectural boundary between privileged kernel execution and ordinary application execution.

## 20.1 ELF64 applications

The kernel includes an ELF64 loader.

It loads executable segments into a process address space and prepares the process for execution.

The loader handles the application entry point and process startup state.

## 20.2 SysV startup arguments

The process startup implementation supports the SysV-style `argc` and `argv` convention.

This allows applications to receive command-line arguments during startup.

## 20.3 Process memory

User processes have independent address spaces.

The kernel supports:

- ELF segment mappings.
- User stacks.
- Heap growth through `brk`.
- Memory mappings through `mmap`.
- Unmapping through `munmap`.
- Demand Paging.
- Copy-on-Write memory.
- Process memory reclamation.

## 20.4 Process creation and lifecycle

The kernel implements process creation and lifecycle operations, including:

- ELF process loading.
- `spawn`.
- `fork`.
- `waitpid`.
- Process termination.
- Address-space reclamation.

The SDK exposes public Rust wrappers for `fork()` and `waitpid()` through:

```text
amaryllis-sdk/src/proc.rs
```

This allows Ring 3 applications to use process creation and child-process waiting through the SDK instead of invoking raw system calls directly.

### Fork

`fork()` creates a child process using the kernel's process and memory-management implementation.

The kernel uses Copy-on-Write for eligible shared memory pages.

This avoids eagerly duplicating all process memory during creation.

### Waitpid

`waitpid()` provides a way for a parent process to wait for a child process and retrieve its termination status.

The exact supported flags and semantics are defined by the current kernel and SDK implementation.

### Example

The SDK includes:

```text
amaryllis-sdk/examples/fork_demo.rs
```

The example demonstrates the process creation workflow using the SDK.

Build and run it as described in the SDK section below.

## 20.5 System calls

Ring 3 applications cannot directly execute privileged kernel operations.

They request kernel services through the system-call interface.

The SDK wraps these operations in Rust APIs.

The current syscall interface includes operations for:

- File I/O.
- Memory mapping.
- Process operations.
- Pipes.
- Networking.
- Time.
- GUI.
- Shared memory.

Refer to:

```text
amaryllis-sdk/src/syscall.rs
```

for syscall numbers and wrappers.

## 20.6 File descriptors

The userland interface includes file descriptors for I/O operations.

The SDK provides abstractions for reading and writing through file descriptors.

The kernel maintains the underlying descriptor state.

---

# 21. Amaryllis SDK

The Amaryllis SDK is the primary Rust interface for Ring 3 application development.

Its package name is:

```text
amaryllis-sdk
```

Its library name is:

```text
amaryllis_sdk
```

The SDK is designed for:

```rust
#![no_std]
```

applications using `alloc`.

## 21.1 SDK modules

| Module | Responsibility |
|---|---|
| `alloc_heap` | Application heap support |
| `env` | Environment-related functionality |
| `fs` | Filesystem operations |
| `io` | Input and output |
| `proc` | Process-related operations |
| `time` | Time-related functionality |
| `net` | Networking |
| `ipc` | Inter-process communication |
| `gui` | Graphical application interfaces |
| `gui::ShmWindow` | Shared-memory-backed GUI windows |

## 21.2 Process API

The public process API is located in:

```text
amaryllis-sdk/src/proc.rs
```

It provides process-related functionality, including:

- `getpid()`
- `yield_now()`
- `exit(code)`
- `spawn(cmdline)`
- `spawn_with_args(cmdline)`
- `active_count()`
- `fork()`
- `waitpid(...)`

The `fork()` and `waitpid()` wrappers expose the corresponding kernel functionality to Ring 3 applications.

Consult the current source for exact signatures, arguments, return types and supported options.

## 21.3 Building SDK examples

From the repository root:

```bash
cargo +nightly build --release \
    -p amaryllis-sdk \
    --examples \
    --target x86_64-unknown-none
```

The examples include:

| Example | Description |
|---|---|
| `hello` | Basic user application |
| `multitask_counter` | Multitasking demonstration |
| `gui_plasma` | Graphical plasma demonstration |
| `net_http_client` | HTTP client |
| `fs_alloc_demo` | Filesystem and allocation |
| `ipc_shm_gui` | Shared-memory GUI |
| `fork_demo` | Process creation and fork demonstration |

The normal Cargo target directory is typically:

```text
target/x86_64-unknown-none/release/examples/
```

The exact location depends on workspace target-directory configuration.

## 21.4 Running an application

An ELF application must first be available inside the guest filesystem.

The shell supports execution through:

```text
exec /path/to/application.elf
```

For example:

```text
exec /hello.elf
```

Applications stored under `/disk` can be launched through their `/disk/...` paths.

The built-in `mkelf` command can also be used for supported ELF demonstrations.

## 21.5 SDK development considerations

When writing a new application:

1. Use the SDK rather than depending directly on kernel internals.
2. Keep the application compatible with the target architecture.
3. Avoid assuming that the host operating system's standard library is available.
4. Use the provided system-call wrappers.
5. Handle error return values.
6. Validate pointers and buffer lengths.
7. Test process termination and resource cleanup.
8. Test parent-child process lifecycle when using `fork()` and `waitpid()`.

The SDK is a developing interface. APIs may change as the kernel evolves.

---

# 22. Inter-Process Communication

ANU includes IPC mechanisms for communication between processes.

## 22.1 Pipes

The SDK exposes:

```rust
ipc::pipe()
```

This creates a reader and writer pair:

```rust
let (mut reader, mut writer) = amaryllis_sdk::ipc::pipe()?;
```

The reader provides:

```rust
reader.read(&mut buffer)
```

The writer provides:

```rust
writer.write(data)
```

Both objects own file descriptors and close them when dropped.

Pipes are useful for streaming data between processes.

The current implementation uses POSIX-style 16 KiB pipes.

## 22.2 Shared memory

The SDK exposes a `SharedMem` abstraction.

Shared memory allows multiple processes to access a common memory region.

The current interface includes:

```rust
SharedMem::create_or_open(...)
SharedMem::map_id(...)
```

The abstraction provides:

- Shared-memory identifiers.
- Virtual addresses.
- Region sizes.
- Read access.
- Optional mutable access.

The requested size is rounded up to a 4 KiB page boundary.

## 22.3 Shared-memory windows

The GUI subsystem supports shared-memory-backed graphical surfaces.

This allows applications to render into memory that the compositor can consume without requiring every frame to be copied through a conventional message buffer.

The SDK contains:

```text
ipc_shm_gui
```

as an example of this approach.

## 22.4 IPC design considerations

Shared memory is efficient but requires synchronization.

Applications must avoid reading partially written data or modifying memory while another process is using it without an appropriate protocol.

The kernel must also enforce valid mappings and permissions.

## 22.5 Future IPC direction

The project roadmap includes a more complete Mach-inspired IPC system.

Planned concepts include:

- Typed ports.
- Send and receive permissions.
- Out-of-line memory transfer.
- Bootstrap service registry.
- Service discovery.
- Exception ports.

These are roadmap items and should not be confused with the currently implemented pipes and shared-memory interfaces.

---

# 23. Graphics and Desktop Environment

Amaryllis includes a custom graphical environment.

It is designed as part of the operating system rather than as a conventional Linux desktop environment.

## 23.1 Framebuffer rendering

The graphical system uses framebuffer-based rendering.

The boot configuration requests a minimum framebuffer resolution of:

```text
1600 × 900
```

This is a minimum request to the bootloader configuration, not a guarantee that every host or emulator will provide exactly that mode.

## 23.2 Compositor

The compositor is responsible for assembling the visible desktop.

The current implementation includes work on:

- Window composition.
- Dirty rectangles.
- Occlusion culling.
- Wallpaper caching.
- Cursor rendering.
- Frame pacing.

A 120 FPS target is part of the graphical design.

It should not be interpreted as a guaranteed measured frame rate on all hardware.

## 23.3 Desktop components

The desktop environment includes:

- Taskbar.
- System Island.
- Control Center.
- Calendar.
- Search interface.
- Application launcher.
- Window management.
- Power menu.

The launcher uses a 3×3 layout.

## 23.4 Visual system

The graphical environment includes:

- Black and white themes.
- TOML-based personalization.
- Font rendering through `fontdue`.
- Glyph caching.
- Cyrillic text support.
- Colloid-inspired icons.
- Lucide-style iconography.

## 23.5 Built-in applications

| Application | Purpose |
|---|---|
| Files | File management |
| Terminal | Shell access |
| Text Editor | Text editing |
| Audio Player | Audio playback interface |
| Net Viewer | Network information |
| System Settings | System configuration |
| Activity Monitor | System activity |

The exact functionality of each application depends on its current implementation.

## 23.6 GUI application development

The SDK provides GUI-related APIs.

The example:

```text
gui_plasma
```

demonstrates graphical rendering.

The example:

```text
ipc_shm_gui
```

demonstrates the shared-memory GUI path.

---

# 24. Shell Command Reference

The kernel shell provides commands for interacting with the system.

| Command | Purpose |
|---|---|
| `help` | Display available commands |
| `smp` | Inspect multiprocessor information |
| `apic` | Inspect APIC information |
| `ioreg` | Inspect the device registry |
| `diskinfo` | Display storage information |
| `netinfo` | Display network information |
| `kthread demo` | Run the kernel-thread demonstration |
| `mkelf fork` | Create the fork demonstration ELF |
| `exec PATH` | Execute an ELF application |

## 24.1 Help

```text
help
```

Displays the available commands supported by the current build.

## 24.2 SMP

```text
smp
```

Displays multiprocessor-related information.

## 24.3 APIC

```text
apic
```

Displays APIC-related information.

## 24.4 I/O Registry

```text
ioreg
```

Displays the device registry.

## 24.5 Storage

```text
diskinfo
```

Displays information about detected storage devices.

## 24.6 Networking

```text
netinfo
```

Displays network information.

## 24.7 Kernel threads

```text
kthread demo
```

Runs the kernel-thread demonstration.

## 24.8 ELF demonstration

```text
mkelf fork
exec /fork_demo.elf
```

This workflow creates and launches the fork demonstration ELF through the shell.

---

# 25. Debugging and Diagnostics

Kernel development requires more than successful compilation.

A system can compile correctly and still contain runtime problems involving memory, scheduling, interrupts or hardware.

## 25.1 Serial output

The QEMU launcher connects serial output to the terminal:

```text
-serial stdio
```

This is one of the most important debugging facilities during early boot.

## 25.2 Kernel logging

Amaryllis includes a kernel logging system with a ring buffer.

The logging infrastructure supports diagnostic output and the `dmesg`-oriented telemetry workflow.

## 25.3 Panic investigation

When the kernel panics or stops responding:

1. Capture the serial output.
2. Identify the last successful initialization message.
3. Check whether the failure occurs before or after memory initialization.
4. Inspect the relevant subsystem.
5. Reproduce with the smallest possible workload.
6. Compare behavior with and without SMP where appropriate.

A triple fault may reset the virtual machine without providing a normal kernel panic message.

## 25.4 QEMU debugging

QEMU supports debugger-oriented options.

For example:

```bash
qemu-system-x86_64 \
    -S \
    -gdb tcp::1234 \
    ...
```

The ellipsis represents the rest of the normal QEMU configuration.

Connect a compatible GDB session to the endpoint and use symbols from the relevant kernel build.

Do not expose a debugging endpoint to untrusted networks.

## 25.5 Build diagnostics

Useful checks include:

```bash
cargo +nightly check
cargo +nightly check -p kernel
cargo +nightly check -p amaryllis-sdk
```

A successful check does not replace runtime testing.

---

# 26. Testing and Reliability

A successful boot is only the beginning of operating-system validation.

The most important tests exercise interactions between subsystems.

## 26.1 SMP testing

Test:

- Booting with multiple CPUs.
- Starting and stopping kernel threads.
- Concurrent process execution.
- Rescheduling between CPUs.
- Inter-processor interrupts.
- CPU-local state.
- Lock contention.

## 26.2 Memory testing

Test:

- Physical frame allocation and release.
- Repeated allocation under memory pressure.
- Address-space creation and destruction.
- Demand Paging.
- Stack growth.
- Guard-page faults.
- Copy-on-Write.
- Page reference counting.
- TLB Shootdown.

CoW tests should include both parent and child writing to previously shared pages.

Verify that terminating a process releases its mappings and frame references correctly.

## 26.3 Kernel-thread testing

With the current 512 KiB stack and 16 KiB guard region, test:

- Kernel threads on the bootstrap processor.
- Kernel threads on AP cores.
- Stack allocation and reclamation.
- Guard-region fault detection.
- Context switching.
- Deep call chains.
- Concurrent worker activity.

The larger stack provides additional room for kernel-thread execution, but it does not eliminate stack-overflow risks.

## 26.4 Process testing

Test:

- ELF loading.
- Process creation.
- User-mode entry.
- System-call transitions.
- `fork()` behavior.
- Copy-on-Write after `fork()`.
- `waitpid()` behavior.
- Process termination.
- Address-space reclamation.
- Parent-child lifecycle.

The `fork_demo` SDK example is a useful starting point for testing the public process API.

## 26.5 Storage testing

Test:

- Device detection.
- Block reads.
- Block writes.
- Filesystem mounting.
- Directory operations.
- Long File Names.
- Cache consistency.
- Read-ahead behavior.
- Write-back behavior.
- Device error handling.

Do not use important personal data for experimental filesystem testing.

## 26.6 Networking testing

Test each protocol independently where possible:

1. Verify e1000 initialization.
2. Verify Ethernet transmission and reception.
3. Verify ARP.
4. Verify IPv4 packet handling.
5. Verify ICMP.
6. Verify DHCP.
7. Verify DNS.
8. Verify UDP.
9. Verify TCP.
10. Verify HTTP.
11. Verify the Ring 3 network interface.

## 26.7 GUI testing

Test:

- Window creation.
- Window destruction.
- Surface updates.
- Shared-memory access.
- Dirty rectangle updates.
- Occlusion handling.
- Cursor movement.
- Text rendering.
- Cyrillic text.
- Application switching.
- Long-running graphical workloads.

## 26.8 Reliability principle

Do not treat a subsystem as fully validated merely because one demonstration works.

A demonstration proves that a particular execution path works under particular conditions.

Stress testing, error handling and regression testing are necessary before making broader stability claims.

---

# 27. Troubleshooting

## 27.1 Rust target not installed

Error:

```text
can't find crate for core
```

Install the target:

```bash
rustup target add x86_64-unknown-none --toolchain nightly
```

## 27.2 Nightly toolchain unavailable

```bash
rustup toolchain install nightly
```

## 27.3 QEMU command not found

Verify:

```bash
which qemu-system-x86_64
```

Install QEMU or correct the host `PATH`.

## 27.4 KVM initialization failure

Check:

```bash
ls -l /dev/kvm
```

Verify that hardware virtualization is enabled and exposed to the current environment.

If KVM is unavailable, use TCG:

```text
-accel tcg
-cpu max
```

Remove `-enable-kvm` when switching to TCG.

## 27.5 No audio output

Check:

- Whether the host audio service is running.
- Whether PulseAudio compatibility is available.
- Whether the QEMU audio backend is supported.
- Whether the AC'97 device is initialized by the guest.

## 27.6 No network connectivity

Check:

- Whether QEMU created the e1000 device.
- Whether the driver initialized.
- Whether the guest network configuration completed.
- Whether DHCP succeeded.
- Whether DNS resolution works.

Use:

```text
netinfo
```

## 27.7 Additional disk not detected

The launcher only attaches the additional disk if:

```text
disk_virtio.img
```

exists in the current working directory.

Verify:

```bash
ls -lh disk_virtio.img
```

## 27.8 BIOS image not found

The BIOS image is generated by the root build script.

Build through Cargo:

```bash
cargo +nightly build --release
```

## 27.9 Kernel hangs during boot

Capture serial output and identify the last successful initialization stage.

Potential areas to inspect include:

- Bootloader handoff.
- Physical memory initialization.
- Page-table setup.
- GDT and TSS initialization.
- Interrupt configuration.
- AP startup.
- Device initialization.
- Scheduler startup.

## 27.10 Kernel-thread stack faults

The kernel-thread stack region uses a 512 KiB stack and a 16 KiB guard region in the isolated virtual-memory slot associated with:

```text
0x0A00_0000_0000
```

When investigating a kernel-thread stack fault, inspect:

- Stack allocation.
- Guard-region mappings.
- Virtual-address reservation.
- Per-CPU stack selection.
- Context-switch stack pointers.
- Stack reclamation.

Do not assume that every page fault in this region is an ordinary user-stack growth event.

## 27.11 `fork()` or `waitpid()` application errors

Check that the application is built against the current `amaryllis-sdk`.

The public wrappers are located in:

```text
amaryllis-sdk/src/proc.rs
```

Also verify that the application uses the correct process API signatures and handles returned errors.

For a reference implementation, inspect:

```text
amaryllis-sdk/examples/fork_demo.rs
```

---

# 28. Known Limitations

Amaryllis OS is an actively developed independent operating system.

## 28.1 Hardware compatibility

The current driver set is limited.

The existence of e1000, AC'97, VirtIO-Blk, AHCI and PS/2 support does not imply universal support for physical hardware.

## 28.2 Networking

The current networking implementation should not be treated as a complete modern networking stack.

IPv6, TLS and a complete POSIX networking API should not be assumed.

## 28.3 Userland compatibility

Amaryllis uses its own kernel and SDK interfaces.

It is not a drop-in replacement for Linux or another Unix-like operating system.

Existing Linux binaries are not automatically compatible.

## 28.4 POSIX compatibility

The presence of familiar operations such as `read`, `write`, `mmap`, `brk`, `fork` or `waitpid` does not imply complete POSIX compatibility.

The actual supported behavior is defined by the current kernel and SDK implementation.

## 28.5 SDK completeness

The SDK exposes a growing set of kernel services.

Some interfaces may have different semantics from their Unix or POSIX counterparts.

The public process API currently includes `fork()` and `waitpid()` wrappers in `amaryllis-sdk/src/proc.rs`.

The `fork_demo` example demonstrates their use.

## 28.6 Reliability

Concurrency, memory management, interrupt handling and storage are complex areas.

The kernel should be considered a development project until extensive regression and stress testing has been completed.

---

# 29. Development Workflow

## 29.1 General workflow

A recommended development cycle:

1. Update the local repository.
2. Inspect the subsystem you intend to modify.
3. Make a focused change.
4. Run the relevant Cargo checks.
5. Build the release configuration.
6. Boot in QEMU.
7. Run the relevant shell or application tests.
8. Inspect serial output and kernel logs.
9. Commit the change.

## 29.2 Update the repository

```bash
git pull
```

## 29.3 Inspect changes

```bash
git status
git diff
```

## 29.4 Check the workspace

```bash
cargo +nightly check
```

## 29.5 Build release

```bash
cargo +nightly build --release
```

## 29.6 Run the operating system

```bash
cargo +nightly run --release
```

## 29.7 Build SDK examples

```bash
cargo +nightly build --release \
    -p amaryllis-sdk \
    --examples \
    --target x86_64-unknown-none
```

## 29.8 Commit changes

```bash
git add .
git commit -m "Describe the change"
```

Use focused commits with descriptive messages.

Avoid committing generated build output, temporary disk images or unrelated local files.

## 29.9 Contributing principles

When changing the kernel:

- Keep hardware-specific code separated from generic interfaces.
- Avoid unnecessary allocations in interrupt context.
- Keep critical sections short.
- Consider SMP behavior even when testing on one CPU.
- Validate user pointers at the kernel boundary.
- Handle allocation failures.
- Release resources during process termination.
- Keep driver operations consistent with the storage and device abstractions.
- Add regression tests for previously fixed bugs.

When changing the SDK:

- Keep the public interface understandable.
- Document system-call behavior.
- Return errors instead of silently ignoring failures.
- Avoid exposing unnecessary kernel implementation details.
- Keep examples synchronized with the actual public API.
- Test process lifecycle APIs, including `fork()` and `waitpid()`.

---

# 30. License

Amaryllis OS is distributed under the Apache License 2.0.

See the repository's `LICENSE` file for the complete license text.

Repository:

https://github.com/MaksLLL-prog/AmaryllisOS

---

# Amaryllis OS

**ANU — Amaryllis is Not Unix**

An independent operating system built with Rust.

Performance. Convenience. Beauty. Reliability.