# Amaryllis OS

**A custom experimental operating system written in Rust.**

Amaryllis OS is a personal bare-metal operating system project focused on building a modular kernel, low-level hardware interaction, memory management, process execution, and eventually a complete operating system environment.

The project is written primarily in Rust and targets the x86_64 architecture.

> The long-term goal is to build a lightweight, responsive, and carefully designed operating system with its own architecture and user experience.

---

## Project Statistics

Current source snapshot:

| Metric | Value |
|---|---:|
| Rust source files | 52 |
| Total lines | 23,472 |
| Non-empty lines | 18,993 |
| Blank lines | 4,479 |
| Architecture | x86_64 |
| Language | Rust |

Statistics cover `kernel/src` and its Rust source files only.

## Technology Stack

- **Language:** Rust
- **Target:** `x86_64-unknown-none`
- **Toolchain:** Rust Nightly
- **Bootloader:** `bootloader` 0.11.17
- **Bootloader API:** `bootloader_api`
- **Virtualization / Emulation:** QEMU
- **Kernel model:** `no_std`, `no_main`
- **Synchronization:** `spin`

## Architecture

The kernel is organized into several subsystems.

```text
kernel/src/
│
├── main.rs
├── pci.rs
│
├── console/
│   └── console.rs
│
├── graphics/
│   └── renderer.rs
│
├── input/
│   ├── keyboard.rs
│   └── mouse.rs
│
├── interrupts/
│   ├── pic.rs
│   └── timer.rs
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
│   └── ide/
│
├── process/
│   ├── manager.rs
│   ├── process.rs
│   └── integration_tests.rs
│
├── shell/
│   ├── commands.rs
│   ├── parser.rs
│   └── process_commands.rs
│
├── storage/
│   ├── block.rs
│   ├── ide/
│   ├── ata/
│   ├── partition/
│   └── fat16/
│
└── task/
    ├── context.rs
    ├── scheduler.rs
    └── task.rs
```

## Implemented Subsystems

### Memory Management

One of the most developed parts of the kernel.

Implemented components:

- Physical Memory Manager (PMM).
- Bootstrap and dynamic PMM allocation.
- Bitmap-based memory tracking.
- Heap allocator.
- Virtual Memory Manager (VMM).
- Page-table management.
- CR3 handling.
- Address Space creation and destruction.
- Huge-page handling.
- Memory diagnostics.
- PMM and heap stress tests.
- Address-space validation and destruction tests.

Memory management has undergone extensive testing, including invalid page-table entries, duplicate tables, cycles, huge pages, and transactional destruction.

### PCI

Implemented PCI subsystem:

- PCI configuration-space access.
- Bus/device/function enumeration.
- PCI device inventory.
- Device inspection.
- Device queries.
- Device manager.
- BAR resource discovery.
- Resource validation.
- Driver matching.
- PCI command-line diagnostics.

### Storage and Filesystem

Storage architecture is being separated from PCI.

Implemented components:

- Generic `BlockDevice` trait.
- Block-device error model.
- MBR partition parsing.
- FAT16 filesystem structures.
- Generic `Fat16Volume<D: BlockDevice>`.
- IDE block-device adapter.

The ATA implementation is currently located under the PCI IDE module and has not yet been fully moved into the new storage architecture.

### Task Management

Implemented components:

- Task structures.
- CPU context representation.
- Scheduler.
- Task states:
  - Ready
  - Running
  - Blocked
  - Finished
- Task switching infrastructure.
- Task counters.
- Blocking and unblocking.
- Task termination.
- Sleep and wake-up handling.
- Shell task-management commands.

### Process Management

Implemented components:

- Process structures.
- Process manager.
- Process integration tests.
- Shell process commands.

### Graphics and Console

Implemented components:

- Framebuffer renderer.
- Color system.
- Text console.
- Console output.
- Kernel command-line interface.

### Input and Interrupts

Implemented components:

- Keyboard input.
- Mouse input.
- Programmable Interrupt Controller (PIC).
- Programmable Interval Timer (PIT).
- Timer-driven kernel operations.

### Shell

The kernel includes an interactive command-line shell.

Available functionality includes:

- Command parsing.
- System information.
- Device diagnostics.
- Memory diagnostics.
- Task management.
- Process commands.
- Storage and filesystem commands.

Previously implemented commands include:

`help`, `clear`, `about`, `version`, `echo`, `whoami`, `neofetch`, `uptime`, `tasks`, `block`, `unblock`, `kill`, and `sleep`.

---

## Development Environment

The project has been developed and tested using:

- Windows 11
- PowerShell 5.1
- Rust Nightly
- Cargo
- QEMU 11.1.0

### Build

```powershell
cargo +nightly build
```

### Run

```powershell
cargo +nightly run
```

The current build configuration targets:

```text
x86_64-unknown-none
```

---

## Development Roadmap

### 1. Architecture and Reorganization

Core project organization and subsystem separation.

**Status: Completed**

### 2. Memory Management

PMM, heap, VMM, address spaces, and memory validation.

**Status: Completed**

### 3. Kernel Architecture

- PCI architecture.
- Storage and FAT16 separation.
- Interrupts and input separation.
- Task and process architecture.
- Boot, initialization, and runtime organization.
- Final architecture audit.

**Status: In Progress**

### 4. Storage and Filesystem

- ATA driver.
- Block-device abstraction.
- Partition management.
- FAT16 filesystem.
- Filesystem operations.

**Status: In Progress**

### 5. Drivers

Expand hardware support and introduce a more complete driver architecture.

**Status: Planned**

### 6. Userland

Develop the userspace environment and system interfaces.

**Status: Planned**

### 7. Amaryllis Graphical System

Build the graphical environment and the user-facing operating system experience.

**Status: Planned**

---

## Project Philosophy

Amaryllis OS follows three important priorities:

1. Performance.
2. Convenience.
3. Visual quality.

The goal is not simply to reproduce an existing operating system, but to create an independent system with a consistent architecture, responsive behavior, and a carefully designed user experience.

---

## Current Status

Amaryllis OS is an actively developed experimental operating system.

The project already contains substantial kernel infrastructure, including memory management, PCI, task scheduling, process management, input, interrupts, graphics, console, shell, and early storage support.

The next major objective is to finish kernel architecture separation and establish a clean foundation for filesystem support and future drivers.

**Project:** Amaryllis OS  
**Language:** Rust  
**Architecture:** x86_64  
**Type:** Bare-metal operating system