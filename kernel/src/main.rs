#![no_std]
#![no_main]
#![feature(abi_x86_interrupt)]
#![feature(alloc_error_handler)]

mod graphics;
mod console;
mod input;
mod shell;
mod interrupts;
mod task;
mod memory;
mod process;
mod hardware;
mod pci;
mod storage;
mod drivers;
mod net;
mod audio;
mod arch;
mod userland;
mod klog;
mod serial;

extern crate alloc;

use alloc::boxed::Box;
use alloc::string::String;
use alloc::vec::Vec;

use core::alloc::Layout;
use core::arch::asm;
use core::panic::PanicInfo;

use bootloader_api::{
    entry_point,
    BootInfo,
    BootloaderConfig,
};
use bootloader_api::config::Mapping;

use graphics::{Color, Renderer};
use console::Console;

// ============================================================
// BOOTLOADER CONFIGURATION
// ============================================================

pub static BOOTLOADER_CONFIG: BootloaderConfig = {
    let mut config = BootloaderConfig::new_default();
    config.mappings.physical_memory = Some(Mapping::Dynamic);
    config.kernel_stack_size = 512 * 1024;
    config
};

entry_point!(kernel_main, config = &BOOTLOADER_CONFIG);

// ============================================================
// SYSTEM CONSTANTS
// ============================================================

const WHITE: Color = (235, 235, 240);
const GREEN: Color = (40, 180, 120);
const GRAY: Color = (140, 140, 150);
const YELLOW: Color = (220, 190, 90);

const BUFFER_SIZE: usize = 128;
const HISTORY_SIZE: usize = 50;

const BLINK_INTERVAL_TICKS: u64 = 50;

// ============================================================
// HEAP MEMORY
// ============================================================

const HEAP_SIZE: usize = 64 * 1024 * 1024;

#[repr(align(4096))]
struct HeapMemory([u8; HEAP_SIZE]);

static mut HEAP_MEMORY: HeapMemory =
    HeapMemory([0; HEAP_SIZE]);

// ============================================================
// KERNEL ENTRY POINT
// ============================================================

fn kernel_main(boot_info: &'static mut BootInfo) -> ! {
    serial::init();
    serial::log("boot", "kernel_main entered");

    let memory_stats =
        memory::analyze(&boot_info.memory_regions);

    let physical_memory_offset =
        match boot_info.physical_memory_offset {
            bootloader_api::info::Optional::Some(offset) => {
                Some(offset)
            }
            bootloader_api::info::Optional::None => None,
        };

    let rsdp_hint = match boot_info.rsdp_addr {
        bootloader_api::info::Optional::Some(addr) => Some(addr),
        bootloader_api::info::Optional::None => None,
    };

    let mut frame_allocator =
        memory::PhysicalFrameAllocator::new(
            &boot_info.memory_regions,
        );

    let framebuffer =
        match boot_info.framebuffer.as_mut() {
            Some(framebuffer) => framebuffer,
            None => {
                serial::log("boot", "ERROR: bootloader provided no framebuffer");
                halt_forever();
            }
        };

    let info = framebuffer.info();
    let fb_slice = framebuffer.buffer_mut();
    serial::register_emergency_fb(
        fb_slice.as_mut_ptr(),
        fb_slice.len(),
        info.width,
        info.height,
        info.stride,
        info.bytes_per_pixel,
        matches!(info.pixel_format, bootloader_api::info::PixelFormat::Bgr),
    );
    serial::log_fmt(
        "boot",
        format_args!(
            "framebuffer {}x{} stride={} bpp={} phys_off={:?}",
            info.width, info.height, info.stride, info.bytes_per_pixel, physical_memory_offset
        ),
    );

    let renderer = Renderer::new(
        fb_slice,
        info.width,
        info.height,
        info.stride,
        info.bytes_per_pixel,
        info.pixel_format,
    );

    let mut console = Console::new(renderer);

    arch::gdt::init();
    userland::init();
    interrupts::init();
    serial::log("boot", "gdt/userland/interrupts initialized");

    initialize_console(&mut console);
    serial::log("boot", "console initialized");

// --------------------------------------------------------
// Physical Memory Manager tests (before heap initialization)
// --------------------------------------------------------

console.write(
    "\nPhysical Memory Manager tests:\n",
    GREEN,
);

let bootstrap_test =
    memory::pmm::tests::run_bootstrap_tests();

display_test_result(
    &mut console,
    "Bootstrap PMM",
    bootstrap_test,
);

let dynamic_test =
    memory::pmm::tests::run_dynamic_tests();

display_test_result(
    &mut console,
    "Dynamic PMM",
    dynamic_test,
);

let metadata_test =
    memory::pmm::tests::run_metadata_tests();

display_test_result(
    &mut console,
    "PMM Metadata",
    metadata_test,
);

let discovery = pci::scan(&mut console);
storage::init(&mut console, &discovery.ide_controllers);
serial::log("boot", "pci & storage initialized");

if input::initialize_mouse() {
    console.write("[ OK ] PS/2 mouse initialized\n", GREEN);
    interrupts::enable_mouse_irq();
    serial::log("boot", "ps2 mouse initialized");
} else {
    console.write("[FAIL] PS/2 mouse initialization failed\n", YELLOW);
    serial::log("boot", "ps2 mouse failed");
}

    display_memory_mapping(
        &mut console,
        physical_memory_offset,
    );

    display_memory_stats(
        &mut console,
        &memory_stats,
    );

    memory::diagnostics::display_memory_map(
    &mut console,
    &boot_info.memory_regions,
);

    // --------------------------------------------------------
    // Virtual memory manager tests
    // --------------------------------------------------------

    if let Some(offset) = physical_memory_offset {
        if let Some(mapping) =
            memory::vmm::PhysicalMemoryMapping::new(offset)
        {
            memory::vmm::sync_shared_kernel_entries(&mapping);

            test_virtual_memory_manager(
                &mut console,
                &mut frame_allocator,
                &mapping,
            );

            test_address_spaces(
                &mut console,
                &mut frame_allocator,
                &mapping,
            );

            test_process_manager(
                &mut console,
                &mut frame_allocator,
                &mapping,
            );

            process::integration_tests::run(
                &mut console,
                &mut frame_allocator,
                &mapping,
            );
            serial::log("boot", "pre-heap vmm & process tests completed");
        }
    }

    // --------------------------------------------------------
    // Heap allocator initialization
    // --------------------------------------------------------

console.write(
    "\nHeap allocator initialization:\n",
    GREEN,
);

let heap_initialized = unsafe {
    let heap_start =
        core::ptr::addr_of_mut!(HEAP_MEMORY.0).cast::<u8>();

    memory::heap::GLOBAL_ALLOCATOR.initialize(
        heap_start,
        HEAP_SIZE,
    )
};
serial::log_fmt("boot", format_args!("heap initialized={}", heap_initialized));

display_test_result(
    &mut console,
    "Global heap initialization",
    heap_initialized,
);

if heap_initialized {
    test_heap_allocator(&mut console);
    test_memory_api(&mut console);

    // --------------------------------------------------------
    // BOOTSTRAP -> DYNAMIC PMM MIGRATION
    // --------------------------------------------------------

    console.write(
        "\nPhysical Memory Manager migration:\n",
        GREEN,
    );

    let migration_success =
        frame_allocator.migrate_to_dynamic();
    serial::log_fmt("boot", format_args!("dynamic pmm migration={}", migration_success));

    display_test_result(
        &mut console,
        "Bootstrap -> Dynamic PMM migration",
        migration_success,
    );

    if migration_success {
        console.write("  Backend: ", GRAY);
        console.write("DYNAMIC\n", GREEN);

        console.write("  Active frames: ", GRAY);
        write_number(
            &mut console,
            frame_allocator.allocated_frames() as u64,
        );
        console.write("\n", WHITE);

        console.write("  Total allocations: ", GRAY);
        write_number(
            &mut console,
            frame_allocator.total_allocations(),
        );
        console.write("\n", WHITE);

        console.write("  Total deallocations: ", GRAY);
        write_number(
            &mut console,
            frame_allocator.total_deallocations(),
        );
        console.write("\n", WHITE);

        display_test_result(
            &mut console,
            "Dynamic PMM integrity",
            frame_allocator.check_integrity(),
        );
    } else {
        console.write(
            "  Bootstrap PMM remains active.\n",
            YELLOW,
        );
    }
} else {
    console.write(
        "\n[WARN] Dynamic PMM migration skipped: heap unavailable.\n",
        YELLOW,
    );
}


        // --------------------------------------------------------
        // DYNAMIC ADDRESS SPACE TESTS
        // --------------------------------------------------------

        if let Some(offset) = physical_memory_offset {
            if let Some(mapping) =
                memory::vmm::PhysicalMemoryMapping::new(offset)
            {
                console.write(
                    "\nDynamic PMM address space tests:\n",
                    GREEN,
                );

                test_address_spaces(
                    &mut console,
                    &mut frame_allocator,
                    &mapping,
                );

                display_test_result(
                    &mut console,
                    "Dynamic PMM integrity after address space tests",
                    frame_allocator.check_integrity(),
                );
            }
        }

    // --------------------------------------------------------
    // Shell initialization
    // --------------------------------------------------------

    console.write("[ OK ] ", GREEN);
    console.write("Command history initialized\n", WHITE);

    console.write("[ OK ] ", GREEN);
    console.write("Line editor initialized\n", WHITE);

    console.write("[ OK ] ", GREEN);
    console.write("Kernel initialization completed\n", WHITE);

    console.write(
        "\nType 'help' to see available commands.\n\n",
        YELLOW,
    );

    let shell_mapping = physical_memory_offset.and_then(|offset| {
        memory::vmm::PhysicalMemoryMapping::new(offset)
    });

    if let Some(ref mapping) = shell_mapping {
        serial::log("boot", "starting acpi::init");
        let acpi_info = arch::acpi::init(mapping, &mut frame_allocator, rsdp_hint);
        serial::log_fmt(
            "boot",
            format_args!(
                "acpi discovered={} tables={} cpus={} ioapics={}",
                acpi_info.discovered,
                acpi_info.table_count,
                acpi_info.cpu_count,
                acpi_info.ioapic_count
            ),
        );
        let apic_snap = arch::apic::init(&acpi_info, mapping, &mut frame_allocator);
        console.write("\nACPI & APIC/SMP Initialization:\n", GREEN);
        if acpi_info.discovered {
            console.write("  [ OK ] ", GREEN);
            console.write("ACPI ", WHITE);
            console.write(if acpi_info.using_xsdt { "XSDT" } else { "RSDT" }, WHITE);
            console.write(" tables=", GRAY);
            write_number(&mut console, acpi_info.table_count as u64);
            console.write(" CPUs=", GRAY);
            write_number(&mut console, acpi_info.cpu_count as u64);
            console.write(" IOAPICs=", GRAY);
            write_number(&mut console, acpi_info.ioapic_count as u64);
            console.write("\n", WHITE);
        } else {
            console.write("  [WARN] ACPI RSDP not located\n", YELLOW);
        }
        console.write("  [ OK ] ", GREEN);
        console.write("BSP APIC ID=", GRAY);
        write_number(&mut console, apic_snap.bsp_apic_id as u64);
        console.write(" LAPIC=", GRAY);
        console.write(if apic_snap.lapic_enabled { "ENABLED" } else { "DISABLED" }, WHITE);
        console.write("\n", WHITE);
    }

    drivers::init_all(
        &mut console,
        &discovery,
        shell_mapping.as_ref(),
        &mut frame_allocator,
        &info,
    );
    serial::log("boot", "drivers::init_all completed");

    if let Some(ref mapping) = shell_mapping {
        let kthread_guarded = task::kthread::init_guarded_stacks(mapping, &mut frame_allocator);
        memory::vmm::sync_shared_kernel_entries(mapping);
        let apic_timer_ok = arch::apic::calibrate_and_enable_timer(1000);
        let acpi_info = arch::acpi::snapshot();
        let smp_sum = arch::smp::init_ap_cores(&acpi_info, mapping, &mut frame_allocator);
        drivers::iokit::start_workloops();
        let iokit_st = drivers::iokit::stats();
        serial::log_fmt(
            "boot",
            format_args!(
                "smp online={} ap_booted={} lapic_timer={} iokit_vecs={}",
                smp_sum.online_cpus, smp_sum.ap_booted, apic_timer_ok, iokit_st.routed_vectors
            ),
        );

        console.write("  [ OK ] ", GREEN);
        console.write("KThread Guarded Stacks: ", GRAY);
        console.write(
            if kthread_guarded { "ACTIVE (4K guard pages)\n" } else { "FALLBACK\n" },
            if kthread_guarded { WHITE } else { YELLOW },
        );
        console.write("  [ OK ] ", GREEN);
        console.write("LAPIC Periodic Timer:   ", GRAY);
        console.write(
            if apic_timer_ok { "1000 Hz (IRQ vector 0x40)\n" } else { "PIT fallback\n" },
            if apic_timer_ok { WHITE } else { YELLOW },
        );
        console.write("  [ OK ] ", GREEN);
        console.write("SMP Multi-Core Bringup: ", GRAY);
        write_number(&mut console, smp_sum.online_cpus as u64);
        console.write(" online CPU(s) (BSP + ", WHITE);
        write_number(&mut console, smp_sum.ap_booted as u64);
        console.write(" APs, IPI TLB shootdown armed)\n", WHITE);

        console.write("  [ OK ] ", GREEN);
        console.write("IOKit & IOAPIC Events:  ", GRAY);
        write_number(&mut console, iokit_st.routed_vectors as u64);
        console.write(" PCI GSI vec(s), WorkLoops ", WHITE);
        console.write(
            if iokit_st.workloops_active { "ARMED\n" } else { "OFF\n" },
            if iokit_st.workloops_active { GREEN } else { YELLOW },
        );

        klog::log_info(
            "boot",
            if apic_timer_ok {
                "Preemptive SMP, LAPIC 1000Hz timer & IOKit WorkLoops active"
            } else {
                "Boot completed with PIT timer fallback"
            },
        );
    }

    graphics::personalization::init_from_vfs();
    serial::log("boot", "launching desktop");

    // Boot directly into Amaryllis Desktop; exiting GUI drops cleanly to the text shell.
    graphics::wm::run_desktop(
        &mut console,
        shell_mapping.as_ref(),
        &mut frame_allocator,
    );

    serial::log("boot", "desktop exited, starting text shell");
    run_shell(
        &mut console,
        shell_mapping.as_ref(),
        &mut frame_allocator,
    );
}

// ============================================================
// CONSOLE INITIALIZATION
// ============================================================


#[inline(never)]
fn initialize_console(console: &mut Console<'_>) {
    console.clear();

    console.draw_rect(
        0,
        0,
        console.width(),
        4,
        GREEN,
    );

    console.write("AmaryllisOS v0.4\n", GREEN);
    console.write("64-bit Rust Operating System\n", GRAY);
    console.write("--------------------------------\n", GRAY);

    display_status(console, "Framebuffer initialized");
    display_status(console, "Graphics renderer ready");
    display_status(console, "Keyboard IRQ ready");
    display_status(console, "PIT timer initialized (1000 Hz)");
    display_status(console, "Physical memory map analyzed");
}



fn display_status(
    console: &mut Console<'_>,
    message: &str,
) {
    console.write("[ OK ] ", GREEN);
    console.write(message, WHITE);
    console.write("\n", WHITE);
}

// ============================================================
// VIRTUAL MEMORY INFORMATION
// ============================================================

fn display_memory_mapping(
    console: &mut Console<'_>,
    physical_memory_offset: Option<u64>,
) {
    console.write(
        "\nVirtual memory information:\n",
        GREEN,
    );

    match physical_memory_offset {
        Some(offset) => {
            console.write(
                "  Physical memory offset: ",
                GRAY,
            );

            write_number(console, offset);
            console.write("\n", WHITE);

            console.write(
                "  Physical mapping: ",
                GRAY,
            );

            console.write("ENABLED\n", GREEN);

            match memory::vmm::PhysicalMemoryMapping::new(offset) {
                Some(mapping) => {
                    let physical_address =
                        memory::vmm::active_level_4_table_address();

                    console.write(
                        "  Active PML4 physical: ",
                        GRAY,
                    );

                    write_number(
                        console,
                        physical_address,
                    );

                    console.write("\n", WHITE);

                    console.write(
                        "  Active PML4 virtual:  ",
                        GRAY,
                    );

                    match mapping.active_level_4_table() {
                        Some(address) => {
                            write_number(
                                console,
                                address.as_u64(),
                            );

                            console.write("\n", WHITE);
                        }

                        None => {
                            console.write(
                                "INVALID ADDRESS\n",
                                YELLOW,
                            );
                        }
                    }

                    match mapping.inspect_active_level_4_table() {
                        Some(stats) => {
                            display_pml4_stats(
                                console,
                                &stats,
                            );
                        }

                        None => {
                            console.write(
                                "\nPML4 statistics unavailable\n",
                                YELLOW,
                            );
                        }
                    }

                    let probe_address =
                        core::ptr::from_mut(console) as u64;

                    display_page_translation(
                        console,
                        &mapping,
                        probe_address,
                    );
                }

                None => {
                    console.write(
                        "  Invalid physical memory offset\n",
                        YELLOW,
                    );
                }
            }
        }

        None => {
            console.write(
                "  Physical memory offset: ",
                GRAY,
            );

            console.write(
                "NOT AVAILABLE\n",
                YELLOW,
            );

            console.write(
                "  Physical mapping: ",
                GRAY,
            );

            console.write(
                "DISABLED\n",
                YELLOW,
            );
        }
    }
}

// ============================================================
// PAGE TABLE WALKER TEST
// ============================================================

fn display_page_translation(
    console: &mut Console<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
    virtual_address: u64,
) {
    console.write(
        "\nPage Table Walker test:\n",
        GREEN,
    );

    console.write(
        "  Virtual address: ",
        GRAY,
    );

    write_number(
        console,
        virtual_address,
    );

    console.write("\n", WHITE);

    let virtual_address =
        match x86_64::VirtAddr::try_new(virtual_address) {
            Ok(address) => address,

            Err(_) => {
                console.write(
                    "  Invalid virtual address\n",
                    YELLOW,
                );

                return;
            }
        };

    match mapping.translate_virtual_address(virtual_address) {
        Some(translation) => {
            console.write(
                "  Physical address: ",
                GRAY,
            );

            write_number(
                console,
                translation.physical_address,
            );

            console.write("\n", WHITE);

            console.write(
                "  Page size: ",
                GRAY,
            );

            write_number(
                console,
                translation.page_size / 1024,
            );

            console.write(" KB\n", WHITE);

            console.write(
                "  Translation: SUCCESS\n",
                GREEN,
            );
        }

        None => {
            console.write(
                "  Translation unavailable\n",
                YELLOW,
            );
        }
    }
}

// ============================================================
// PML4 STATISTICS
// ============================================================

fn display_pml4_stats(
    console: &mut Console<'_>,
    stats: &memory::vmm::Pml4Stats,
) {
    console.write(
        "\nActive PML4 table statistics:\n",
        GREEN,
    );

    console.write(
        "  Total entries:       ",
        GRAY,
    );

    write_number(
        console,
        stats.total_entries as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  Occupied entries:    ",
        GRAY,
    );

    write_number(
        console,
        stats.occupied_entries as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  Present entries:     ",
        GRAY,
    );

    write_number(
        console,
        stats.present_entries as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  Writable entries:    ",
        GRAY,
    );

    write_number(
        console,
        stats.writable_entries as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  User accessible:     ",
        GRAY,
    );

    write_number(
        console,
        stats.user_accessible_entries as u64,
    );

    console.write("\n", WHITE);
}

// ============================================================
// PHYSICAL MEMORY INFORMATION
// ============================================================

fn display_memory_stats(
    console: &mut Console<'_>,
    stats: &memory::MemoryStats,
) {
    console.write(
        "\nPhysical memory information:\n",
        GREEN,
    );

    console.write(
        "  Total regions:    ",
        GRAY,
    );

    write_number(
        console,
        stats.total_regions as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  Usable regions:   ",
        GRAY,
    );

    write_number(
        console,
        stats.usable_regions as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  Reserved regions: ",
        GRAY,
    );

    write_number(
        console,
        stats.reserved_regions as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  Usable memory:    ",
        GRAY,
    );

    write_number(
        console,
        stats.usable_megabytes(),
    );

    console.write(" MB\n", WHITE);

    console.write(
        "  Total mapped:     ",
        GRAY,
    );

    write_number(
        console,
        stats.mapped_megabytes(),
    );

    console.write(" MB\n", WHITE);
}

// ============================================================
// PHYSICAL FRAME ALLOCATOR TESTS
// ============================================================

fn test_frame_allocator(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
) {
    console.write(
        "\nFrame allocator test:\n",
        GREEN,
    );

    let frame_a = allocator.allocate_frame();
    let frame_b = allocator.allocate_frame();
    let frame_c = allocator.allocate_frame();

    let (a, b, c) = match (
        frame_a,
        frame_b,
        frame_c,
    ) {
        (Some(a), Some(b), Some(c)) => {
            (a, b, c)
        }

        _ => {
            console.write(
                "  Frame allocation failed!\n",
                YELLOW,
            );

            return;
        }
    };

    console.write("  Frame A: ", GRAY);
    write_number(console, a);
    console.write("\n", WHITE);

    console.write("  Frame B: ", GRAY);
    write_number(console, b);
    console.write("\n", WHITE);

    console.write("  Frame C: ", GRAY);
    write_number(console, c);
    console.write("\n", WHITE);

    console.write(
        "  Allocated frames: ",
        GRAY,
    );

    write_number(
        console,
        allocator.allocated_frames() as u64,
    );

    console.write("\n", WHITE);

    let invalid_free =
        allocator.deallocate_frame(c + 1);

    display_test_result(
        console,
        "Invalid address protection",
        !invalid_free,
    );

    let never_allocated =
        allocator.deallocate_frame(
            c + memory::FRAME_SIZE,
        );

    display_test_result(
        console,
        "Unallocated frame protection",
        !never_allocated,
    );

    let freed =
        allocator.deallocate_frame(b);

    display_test_result(
        console,
        "Free frame B",
        freed,
    );

    let double_free =
        allocator.deallocate_frame(b);

    display_test_result(
        console,
        "Double free protection",
        !double_free,
    );

    let reused =
        allocator.allocate_frame();

    console.write(
        "  Reused frame: ",
        GRAY,
    );

    let address_reuse = match reused {
        Some(address) => {
            write_number(console, address);
            console.write("\n", WHITE);

            address == b
        }

        None => {
            console.write(
                "Allocation failed\n",
                YELLOW,
            );

            false
        }
    };

    display_test_result(
        console,
        "Address reuse",
        address_reuse,
    );

    console.write(
        "  Free frames: ",
        GRAY,
    );

    write_number(
        console,
        allocator.free_frames() as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  Allocated frames after reuse: ",
        GRAY,
    );

    write_number(
        console,
        allocator.allocated_frames() as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  Total allocations: ",
        GRAY,
    );

    write_number(
        console,
        allocator.total_allocations(),
    );

    console.write("\n", WHITE);

    console.write(
        "  Total deallocations: ",
        GRAY,
    );

    write_number(
        console,
        allocator.total_deallocations(),
    );

    console.write("\n", WHITE);

    console.write(
        "  Available slots: ",
        GRAY,
    );

    write_number(
        console,
        allocator.available_slots() as u64,
    );

    console.write("\n", WHITE);

    display_test_result(
        console,
        "Integrity check",
        allocator.check_integrity(),
    );
}

// ============================================================
// TEST RESULT DISPLAY
// ============================================================

fn display_test_result(
    console: &mut Console<'_>,
    name: &str,
    success: bool,
) {
    console.write("  ", GRAY);
    console.write(name, GRAY);
    console.write(": ", GRAY);

    if success {
        console.write("SUCCESS\n", GREEN);
    } else {
        console.write("FAILED\n", YELLOW);
    }
}

// ============================================================
// VIRTUAL MEMORY MANAGER TESTS
// ============================================================


#[inline(never)]
fn test_virtual_memory_manager(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use x86_64::VirtAddr;
    use x86_64::structures::paging::PageTableFlags;

    console.write(
        "\nVirtual memory manager test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Allocate physical frame
    // --------------------------------------------------------

    let physical_frame =
        match allocator.allocate_frame() {
            Some(frame) => frame,
            None => {
                console.write(
                    "  Physical allocation failed\n",
                    YELLOW,
                );
                return;
            }
        };

    console.write("  Physical frame: ", GRAY);
    write_number(console, physical_frame);
    console.write("\n", WHITE);

    // --------------------------------------------------------
    // 2. Prepare test virtual address
    // --------------------------------------------------------

    let virtual_address =
        match VirtAddr::try_new(0x0000_4000_0000_0000) {
            Ok(address) => address,
            Err(_) => {
                allocator.deallocate_frame(physical_frame);

                console.write(
                    "  Invalid test virtual address\n",
                    YELLOW,
                );
                return;
            }
        };

    // --------------------------------------------------------
    // 3. Check whether address is already mapped
    // --------------------------------------------------------

    if mapping
        .translate_virtual_address(virtual_address)
        .is_some()
    {
        allocator.deallocate_frame(physical_frame);

        console.write(
            "  Test virtual address is already mapped\n",
            YELLOW,
        );
        return;
    }

    // --------------------------------------------------------
    // 4. Record allocator state before mapping
    // --------------------------------------------------------

    let allocated_before_mapping =
        allocator.allocated_frames();

    // --------------------------------------------------------
    // 5. Map virtual address to physical frame
    // --------------------------------------------------------

    let result = mapping.map_page(
        allocator,
        virtual_address,
        physical_frame,
        PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
    );

    if result.is_err() {
        allocator.deallocate_frame(physical_frame);

        console.write(
            "  map_page: FAILED\n",
            YELLOW,
        );
        return;
    }

    display_test_result(
        console,
        "map_page",
        true,
    );

    let allocated_after_mapping =
        allocator.allocated_frames();

    let page_tables_allocated =
        allocated_after_mapping.saturating_sub(
            allocated_before_mapping,
        );

    console.write(
        "  Page-table frames allocated: ",
        GRAY,
    );

    write_number(
        console,
        page_tables_allocated as u64,
    );

    console.write("\n", WHITE);

    // --------------------------------------------------------
    // 6. Verify virtual-to-physical translation
    // --------------------------------------------------------

    let translation =
        mapping.translate_virtual_address(
            virtual_address,
        );

    let translation_success =
        match translation {
            Some(result) => {
                result.physical_address == physical_frame
            }
            None => false,
        };

    display_test_result(
        console,
        "Virtual-to-physical translation",
        translation_success,
    );

    // --------------------------------------------------------
    // 7. Remove mapping and clean empty page tables
    // --------------------------------------------------------

    let unmapped =
        mapping.unmap_page_with_allocator(
            allocator,
            virtual_address,
        );

    let unmap_success =
        match unmapped {
            Ok(address) => address == physical_frame,
            Err(_) => false,
        };

    display_test_result(
        console,
        "unmap_page",
        unmap_success,
    );

    // --------------------------------------------------------
    // 8. Verify mapping removal
    // --------------------------------------------------------

    let removed =
        mapping
            .translate_virtual_address(virtual_address)
            .is_none();

    display_test_result(
        console,
        "Mapping removal",
        removed,
    );

    // --------------------------------------------------------
    // 9. Verify empty page-table cleanup
    // --------------------------------------------------------

    let page_tables_released =
        unmap_success
        && removed
        && allocator.allocated_frames()
            == allocated_before_mapping;

    display_test_result(
        console,
        "Empty page-table cleanup",
        page_tables_released,
    );

    console.write(
        "  Allocated frames before mapping: ",
        GRAY,
    );

    write_number(
        console,
        allocated_before_mapping as u64,
    );

    console.write("\n", WHITE);

    console.write(
        "  Allocated frames after unmapping: ",
        GRAY,
    );

    write_number(
        console,
        allocator.allocated_frames() as u64,
    );

    console.write("\n", WHITE);

    // --------------------------------------------------------
    // 10. Safely release physical data frame
    // --------------------------------------------------------

    if unmap_success && removed {
        let released =
            allocator.deallocate_frame(physical_frame);

        display_test_result(
            console,
            "Physical frame release",
            released,
        );
    } else {
        console.write(
            "  Physical frame NOT released (safety protection)\n",
            YELLOW,
        );
    }

    // --------------------------------------------------------
    // 11. Check allocator integrity
    // --------------------------------------------------------

    display_test_result(
        console,
        "Allocator integrity",
        allocator.check_integrity(),
    );

    // --------------------------------------------------------
    // 12. Test allocation rollback
    // --------------------------------------------------------

    test_vmm_allocation_rollback(
        console,
        allocator,
        mapping,
    );
}
// ============================================================
// ADDRESS SPACE TESTS
// ============================================================


#[inline(never)]
fn test_address_spaces(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use x86_64::VirtAddr;
    use x86_64::structures::paging::PageTableFlags;

    console.write(
        "\nAddress space manager test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Create first address space
    // --------------------------------------------------------

    let mut first = match memory::address_space::AddressSpace::new(
        mapping,
        allocator,
    ) {
        Some(space) => space,
        None => {
            console.write(
                "  First address space creation failed\n",
                YELLOW,
            );
            return;
        }
    };

    display_test_result(
        console,
        "First address space creation",
        true,
    );

    // --------------------------------------------------------
    // 2. Create second address space
    // --------------------------------------------------------

    let mut second = match memory::address_space::AddressSpace::new(
        mapping,
        allocator,
    ) {
        Some(space) => space,
        None => {
            unsafe {
                let _ = first.destroy(mapping, allocator);
            }

            console.write(
                "  Second address space creation failed\n",
                YELLOW,
            );
            return;
        }
    };

    display_test_result(
        console,
        "Second address space creation",
        true,
    );

    // --------------------------------------------------------
    // 3. Verify independent PML4 roots
    // --------------------------------------------------------

    let different_roots =
        first.pml4_physical_address()
            != second.pml4_physical_address();

    display_test_result(
        console,
        "Independent PML4 roots",
        different_roots,
    );

    // --------------------------------------------------------
    // 4. Verify unique IDs
    // --------------------------------------------------------

    let different_ids =
        first.id() != second.id();

    display_test_result(
        console,
        "Unique address space IDs",
        different_ids,
    );

    // --------------------------------------------------------
    // 5. Verify shared kernel mappings
    // --------------------------------------------------------

    let first_kernel_shared =
        first.shares_kernel_mappings(mapping);

    let second_kernel_shared =
        second.shares_kernel_mappings(mapping);

    display_test_result(
        console,
        "First space kernel mappings",
        first_kernel_shared,
    );

    display_test_result(
        console,
        "Second space kernel mappings",
        second_kernel_shared,
    );

    // --------------------------------------------------------
    // 6. Verify empty private user regions
    // --------------------------------------------------------

    let first_user_empty =
        first.user_half_is_empty(mapping);

    let second_user_empty =
        second.user_half_is_empty(mapping);

    display_test_result(
        console,
        "First private region is empty",
        first_user_empty,
    );

    display_test_result(
        console,
        "Second private region is empty",
        second_user_empty,
    );

    // --------------------------------------------------------
    // 7. Display physical addresses
    // --------------------------------------------------------

    let first_root =
        first.pml4_physical_address();

    let second_root =
        second.pml4_physical_address();

    console.write("\n  First PML4:  ", GRAY);
    write_number(console, first_root);
    console.write("\n", WHITE);

    console.write("  Second PML4: ", GRAY);
    write_number(console, second_root);
    console.write("\n", WHITE);

    // --------------------------------------------------------
    // 8. Test CR3 switching
    // --------------------------------------------------------

    test_cr3_switch(
        console,
        mapping,
        first_root,
    );

    // --------------------------------------------------------
    // 9. Create a private mapping in second address space
    // --------------------------------------------------------

    console.write(
        "\nAddress space destruction test:\n",
        GREEN,
    );

    let test_virtual_address =
        match VirtAddr::try_new(0x0000_7000_0000_0000) {
            Ok(address) => address,
            Err(_) => {
                console.write(
                    "  Invalid test virtual address\n",
                    YELLOW,
                );

                unsafe {
                    let _ = second.destroy(mapping, allocator);
                    let _ = first.destroy(mapping, allocator);
                }

                return;
            }
        };

    let data_frame =
        match allocator.allocate_frame() {
            Some(frame) => frame,
            None => {
                console.write(
                    "  Data frame allocation failed\n",
                    YELLOW,
                );

                unsafe {
                    let _ = second.destroy(mapping, allocator);
                    let _ = first.destroy(mapping, allocator);
                }

                return;
            }
        };

    let mapping_result = mapping.map_page_in(
        allocator,
        second_root,
        test_virtual_address,
        data_frame,
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    if mapping_result.is_err() {
        display_test_result(
            console,
            "Private page mapping",
            false,
        );

        allocator.deallocate_frame(data_frame);

        unsafe {
            let _ = second.destroy(mapping, allocator);
            let _ = first.destroy(mapping, allocator);
        }

        return;
    }

    display_test_result(
        console,
        "Private page mapping",
        true,
    );

    // --------------------------------------------------------
    // 10. Verify private mapping
    // --------------------------------------------------------

    let private_translation =
        mapping.translate_virtual_address_in(
            second_root,
            test_virtual_address,
        );

    let private_mapping_valid =
        matches!(
            private_translation,
            Some(translation)
                if translation.physical_address == data_frame
        );

    display_test_result(
        console,
        "Private mapping translation",
        private_mapping_valid,
    );


    // --------------------------------------------------------
    // 11. Destroy second address space
    // --------------------------------------------------------

    let second_destroyed = unsafe {
        second.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "Second address space destroyed",
        second_destroyed,
    );

    display_test_result(
        console,
        "Second address space marked destroyed",
        second.is_destroyed(),
    );

    // --------------------------------------------------------
    // 12. Verify shared kernel mappings survived
    // --------------------------------------------------------

    let kernel_still_shared =
        first.shares_kernel_mappings(mapping);

    display_test_result(
        console,
        "Kernel mappings survived destruction",
        kernel_still_shared,
    );

    // --------------------------------------------------------
    // 13. Verify data frame ownership
    // --------------------------------------------------------

    let data_frame_preserved =
        allocator.owns_frame(data_frame);

    display_test_result(
        console,
        "Mapped data frame preserved",
        data_frame_preserved,
    );

    // Only release the data frame after successful destruction.
    let data_frame_released = if second_destroyed
        && data_frame_preserved
    {
        allocator.deallocate_frame(data_frame)
    } else {
        false
    };

    display_test_result(
        console,
        "Data frame released separately",
        data_frame_released,
    );


    // --------------------------------------------------------
    // 14. Destroy first address space
    // --------------------------------------------------------

    let first_destroyed = unsafe {
        first.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "First address space destroyed",
        first_destroyed,
    );

    display_test_result(
        console,
        "First address space marked destroyed",
        first.is_destroyed(),
    );

    // --------------------------------------------------------
    // 15. Verify allocator integrity
    // --------------------------------------------------------

    display_test_result(
        console,
        "Frame allocator integrity",
        allocator.check_integrity(),
    );

    // --------------------------------------------------------
// 15. Test cyclic page-table reference
// --------------------------------------------------------

test_address_space_cycle_detection(
    console,
    allocator,
    mapping,
);

test_address_space_duplicate_table_detection(
    console,
    allocator,
    mapping,
);

test_address_space_invalid_frame_detection(
    console,
    allocator,
    mapping,
);

test_address_space_invalid_lower_level_frames(
    console,
    allocator,
    mapping,
);

test_address_space_zero_frame_detection(
    console,
    allocator,
    mapping,
);

test_address_space_huge_page_detection(
    console,
    allocator,
    mapping,
);

test_address_space_valid_huge_page_leaves(
    console,
    allocator,
    mapping,
);

test_address_space_non_present_entries(
    console,
    allocator,
    mapping,
);

test_address_space_transactional_destruction(
    console,
    allocator,
    mapping,
);
}


fn test_address_space_cycle_detection(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use core::ptr::{
        read_volatile,
        write_volatile,
    };

    use x86_64::PhysAddr;
    use x86_64::structures::paging::page_table::PageTableEntry;
    use x86_64::structures::paging::PageTableFlags;

    use memory::address_space::{
        AddressSpace,
        AddressSpaceError,
    };

    console.write(
        "\nAddress space cycle detection test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Create isolated address space
    // --------------------------------------------------------

    let mut space = match AddressSpace::new(
        mapping,
        allocator,
    ) {
        Some(space) => space,
        None => {
            display_test_result(
                console,
                "Cycle test address space creation",
                false,
            );
            return;
        }
    };

    display_test_result(
        console,
        "Cycle test address space creation",
        true,
    );

    let root = space.pml4_physical_address();

    // --------------------------------------------------------
    // 2. Translate private PML4 root
    // --------------------------------------------------------

    let root_virtual = match mapping.translate_frame(root) {
        Some(address) => address,
        None => {
            display_test_result(
                console,
                "Cycle test root translation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let root_pointer =
        root_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 3. Save original entry and create a self-reference
    // --------------------------------------------------------

    let original_entry = unsafe {
        read_volatile(root_pointer)
    };

    let mut cyclic_entry = original_entry.clone();

    cyclic_entry.set_addr(
        PhysAddr::new(root),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            root_pointer,
            cyclic_entry,
        );
    }

    // --------------------------------------------------------
    // 4. Attempt destruction
    // --------------------------------------------------------

    let destroy_result = unsafe {
        space.destroy(mapping, allocator)
    };

    let cycle_detected = matches!(
        destroy_result,
        Err(AddressSpaceError::CycleDetected)
    );

    display_test_result(
        console,
        "Cycle correctly detected",
        cycle_detected,
    );

    // --------------------------------------------------------
    // 5. Verify root frame was not released
    // --------------------------------------------------------

    let root_preserved = allocator.owns_frame(root);

    display_test_result(
        console,
        "Root frame preserved after failure",
        root_preserved,
    );

    // Do not access the root if destruction unexpectedly
    // succeeded or the frame is no longer owned.
    if space.is_destroyed() || !root_preserved {
        display_test_result(
            console,
            "Cycle test cleanup",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 6. Restore original PML4 entry
    // --------------------------------------------------------

    unsafe {
        write_volatile(
            root_pointer,
            original_entry,
        );
    }

    // --------------------------------------------------------
    // 7. Destroy the repaired address space
    // --------------------------------------------------------

    let cleanup_success = unsafe {
        space.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "Cycle test cleanup",
        cleanup_success,
    );
}


fn test_address_space_duplicate_table_detection(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use core::ptr::{
        read_volatile,
        write_bytes,
        write_volatile,
    };

    use x86_64::PhysAddr;
    use x86_64::structures::paging::page_table::PageTableEntry;
    use x86_64::structures::paging::PageTableFlags;

    use memory::address_space::{
        AddressSpace,
        AddressSpaceError,
    };

    console.write(
        "\nAddress space duplicate table detection test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Create isolated address space
    // --------------------------------------------------------

    let mut space = match AddressSpace::new(
        mapping,
        allocator,
    ) {
        Some(space) => space,
        None => {
            display_test_result(
                console,
                "Duplicate test address space creation",
                false,
            );
            return;
        }
    };

    display_test_result(
        console,
        "Duplicate test address space creation",
        true,
    );

    let root = space.pml4_physical_address();

    // --------------------------------------------------------
    // 2. Translate PML4 root
    // --------------------------------------------------------

    let root_virtual = match mapping.translate_frame(root) {
        Some(address) => address,
        None => {
            display_test_result(
                console,
                "Duplicate test root translation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let root_pointer =
        root_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 3. Allocate one shared child table
    // --------------------------------------------------------

    let child_frame = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            display_test_result(
                console,
                "Duplicate test child allocation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let child_virtual = match mapping.translate_frame(child_frame) {
        Some(address) => address,
        None => {
            let _ = allocator.deallocate_frame(child_frame);

            display_test_result(
                console,
                "Duplicate test child translation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    // The allocated frame may contain old data.
    // Initialize it as an empty page table.
    unsafe {
        write_bytes(
            child_virtual.as_u64() as *mut u8,
            0,
            memory::FRAME_SIZE as usize,
        );
    }

    // --------------------------------------------------------
    // 4. Find two free private PML4 entries
    // --------------------------------------------------------

    let mut found_indices: [Option<usize>; 2] = [None, None];
    let mut found_count = 0usize;
    for idx in 0..memory::vmm::KERNEL_PML4_START {
        if memory::vmm::is_shared_kernel_entry(idx) {
            continue;
        }
        let candidate = unsafe { read_volatile(root_pointer.add(idx)) };
        if !candidate.flags().contains(PageTableFlags::PRESENT) {
            found_indices[found_count] = Some(idx);
            found_count += 1;
            if found_count == 2 {
                break;
            }
        }
    }

    let entries_are_empty = found_indices[0].is_some() && found_indices[1].is_some();

    display_test_result(
        console,
        "Private PML4 entries are available",
        entries_are_empty,
    );

    if !entries_are_empty {
        let _ = allocator.deallocate_frame(child_frame);

        unsafe {
            let _ = space.destroy(mapping, allocator);
        }

        return;
    }

    let first_pointer = unsafe {
        root_pointer.add(found_indices[0].unwrap())
    };

    let second_pointer = unsafe {
        root_pointer.add(found_indices[1].unwrap())
    };

    let original_first = unsafe {
        read_volatile(first_pointer)
    };

    let original_second = unsafe {
        read_volatile(second_pointer)
    };

    // --------------------------------------------------------
    // 5. Point both entries to the same physical frame
    // --------------------------------------------------------

    let flags = PageTableFlags::PRESENT
        | PageTableFlags::WRITABLE
        | PageTableFlags::USER_ACCESSIBLE;

    let mut first_entry = original_first.clone();
    first_entry.set_addr(
        PhysAddr::new(child_frame),
        flags,
    );

    let mut second_entry = original_second.clone();
    second_entry.set_addr(
        PhysAddr::new(child_frame),
        flags,
    );

    unsafe {
        write_volatile(
            first_pointer,
            first_entry,
        );

        write_volatile(
            second_pointer,
            second_entry,
        );
    }

    // --------------------------------------------------------
    // 6. Attempt destruction
    // --------------------------------------------------------

    let destroy_result = unsafe {
        space.destroy(mapping, allocator)
    };

    let duplicate_detected = matches!(
        destroy_result,
        Err(AddressSpaceError::DuplicateTableFrame)
    );

    display_test_result(
        console,
        "Duplicate table correctly detected",
        duplicate_detected,
    );

    // --------------------------------------------------------
    // 7. Verify that neither frame was released
    // --------------------------------------------------------

    let root_preserved = allocator.owns_frame(root);
    let child_preserved = allocator.owns_frame(child_frame);

    display_test_result(
        console,
        "Root frame preserved after failure",
        root_preserved,
    );

    display_test_result(
        console,
        "Duplicate child frame preserved after failure",
        child_preserved,
    );

    if space.is_destroyed()
        || !root_preserved
        || !child_preserved
    {
        display_test_result(
            console,
            "Duplicate test cleanup",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 8. Restore original PML4 entries
    // --------------------------------------------------------

    unsafe {
        write_volatile(
            first_pointer,
            original_first,
        );

        write_volatile(
            second_pointer,
            original_second,
        );
    }

    // --------------------------------------------------------
    // 9. Destroy repaired address space
    // --------------------------------------------------------

    let destroy_success = unsafe {
        space.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "Repaired address space destruction",
        destroy_success,
    );

    // The child table is no longer referenced by the address
    // space. It was allocated separately by this test.
    let child_deallocation_success =
        allocator.deallocate_frame(child_frame);

    display_test_result(
        console,
        "Duplicate test child frame cleanup",
        child_deallocation_success,
    );
}


fn test_address_space_invalid_frame_detection(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use core::ptr::{
        read_volatile,
        write_volatile,
    };

    use x86_64::PhysAddr;
    use x86_64::structures::paging::page_table::PageTableEntry;
    use x86_64::structures::paging::PageTableFlags;

    use memory::address_space::{
        AddressSpace,
        AddressSpaceError,
    };

    console.write(
        "\nAddress space invalid frame detection test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Create isolated address space
    // --------------------------------------------------------

    let mut space = match AddressSpace::new(
        mapping,
        allocator,
    ) {
        Some(space) => space,
        None => {
            display_test_result(
                console,
                "Invalid frame test address space creation",
                false,
            );
            return;
        }
    };

    display_test_result(
        console,
        "Invalid frame test address space creation",
        true,
    );

    let root = space.pml4_physical_address();

    // --------------------------------------------------------
    // 2. Translate root
    // --------------------------------------------------------

    let root_virtual = match mapping.translate_frame(root) {
        Some(address) => address,
        None => {
            display_test_result(
                console,
                "Invalid frame test root translation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let root_pointer =
        root_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 3. Find a physical address not owned by PMM
    // --------------------------------------------------------

    let mut invalid_frame = 0x000F_FFFF_FFFF_F000u64;

    while allocator.owns_frame(invalid_frame) {
        invalid_frame -= memory::FRAME_SIZE;
    }

    display_test_result(
        console,
        "Invalid frame is not owned by PMM",
        !allocator.owns_frame(invalid_frame),
    );

    // --------------------------------------------------------
    // 4. Save original PML4 entry
    // --------------------------------------------------------

    let entry_pointer = root_pointer;

    let original_entry = unsafe {
        read_volatile(entry_pointer)
    };

    if original_entry.flags().contains(PageTableFlags::PRESENT) {
        display_test_result(
            console,
            "Private PML4 entry is available",
            false,
        );

        unsafe {
            let _ = space.destroy(mapping, allocator);
        }

        return;
    }

    display_test_result(
        console,
        "Private PML4 entry is available",
        true,
    );

    // --------------------------------------------------------
    // 5. Inject an unowned physical frame
    // --------------------------------------------------------

    let mut invalid_entry = original_entry.clone();

    invalid_entry.set_addr(
        PhysAddr::new(invalid_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            entry_pointer,
            invalid_entry,
        );
    }

    // --------------------------------------------------------
    // 6. Attempt destruction
    // --------------------------------------------------------

    let destroy_result = unsafe {
        space.destroy(mapping, allocator)
    };

    let invalid_frame_detected = matches!(
        destroy_result,
        Err(AddressSpaceError::FrameNotOwned)
    );

    display_test_result(
        console,
        "Unowned table frame correctly rejected",
        invalid_frame_detected,
    );

    // --------------------------------------------------------
    // 7. Verify root was preserved
    // --------------------------------------------------------

    let root_preserved = allocator.owns_frame(root);

    display_test_result(
        console,
        "Root frame preserved after failure",
        root_preserved,
    );

    if space.is_destroyed() || !root_preserved {
        display_test_result(
            console,
            "Invalid frame test cleanup",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 8. Restore original PML4 entry
    // --------------------------------------------------------

    unsafe {
        write_volatile(
            entry_pointer,
            original_entry,
        );
    }

    // --------------------------------------------------------
    // 9. Destroy repaired address space
    // --------------------------------------------------------

    let cleanup_success = unsafe {
        space.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "Invalid frame test cleanup",
        cleanup_success,
    );
}


fn test_address_space_invalid_lower_level_frames(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use core::ptr::{
        read_volatile,
        write_bytes,
        write_volatile,
    };

    use x86_64::PhysAddr;
    use x86_64::structures::paging::page_table::PageTableEntry;
    use x86_64::structures::paging::PageTableFlags;

    use memory::address_space::{
        AddressSpace,
        AddressSpaceError,
    };

    console.write(
        "\nAddress space invalid lower-level frames test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Create isolated address space
    // --------------------------------------------------------

    let mut space = match AddressSpace::new(mapping, allocator) {
        Some(space) => space,
        None => {
            display_test_result(
                console,
                "Lower-level frame test address space creation",
                false,
            );
            return;
        }
    };

    let root = space.pml4_physical_address();

    let root_virtual = match mapping.translate_frame(root) {
        Some(address) => address,
        None => {
            display_test_result(
                console,
                "Lower-level frame test root translation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let root_pointer =
        root_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 2. Verify private PML4 entry
    // --------------------------------------------------------

    let pml4_entry_pointer = root_pointer;

    let original_pml4_entry = unsafe {
        read_volatile(pml4_entry_pointer)
    };

    if original_pml4_entry.flags().contains(
        PageTableFlags::PRESENT,
    ) {
        display_test_result(
            console,
            "Lower-level private PML4 entry available",
            false,
        );

        unsafe {
            let _ = space.destroy(mapping, allocator);
        }

        return;
    }

    // --------------------------------------------------------
    // 3. Find an unowned physical frame
    // --------------------------------------------------------

    let mut invalid_frame = 0x000F_FFFF_FFFF_F000u64;

    while allocator.owns_frame(invalid_frame) {
        invalid_frame -= memory::FRAME_SIZE;
    }

    display_test_result(
        console,
        "Invalid lower-level frame is unowned",
        !allocator.owns_frame(invalid_frame),
    );

    // --------------------------------------------------------
    // 4. Allocate PDPT
    // --------------------------------------------------------

    let pdpt_frame = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            display_test_result(
                console,
                "Lower-level PDPT allocation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    // --------------------------------------------------------
    // 5. Allocate PD
    // --------------------------------------------------------

    let pd_frame = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            let _ = allocator.deallocate_frame(pdpt_frame);

            display_test_result(
                console,
                "Lower-level PD allocation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    // --------------------------------------------------------
    // 6. Translate allocated tables
    // --------------------------------------------------------

    let pdpt_virtual = mapping.translate_frame(pdpt_frame);
    let pd_virtual = mapping.translate_frame(pd_frame);

    let (pdpt_virtual, pd_virtual) =
        match (pdpt_virtual, pd_virtual) {
            (Some(pdpt), Some(pd)) => (pdpt, pd),

            _ => {
                let _ = allocator.deallocate_frame(pd_frame);
                let _ = allocator.deallocate_frame(pdpt_frame);

                display_test_result(
                    console,
                    "Lower-level table translation",
                    false,
                );

                unsafe {
                    let _ = space.destroy(mapping, allocator);
                }

                return;
            }
        };

    let pdpt_pointer =
        pdpt_virtual.as_u64() as *mut PageTableEntry;

    let pd_pointer =
        pd_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 7. Initialize tables
    // --------------------------------------------------------

    unsafe {
        write_bytes(
            pdpt_pointer as *mut u8,
            0,
            memory::FRAME_SIZE as usize,
        );

        write_bytes(
            pd_pointer as *mut u8,
            0,
            memory::FRAME_SIZE as usize,
        );
    }

    // --------------------------------------------------------
    // 8. Save original entries
    // --------------------------------------------------------

    let original_pdpt_entry_0 = unsafe {
        read_volatile(pdpt_pointer)
    };

    let original_pdpt_entry_1 = unsafe {
        read_volatile(pdpt_pointer.add(1))
    };

    let original_pd_entry_0 = unsafe {
        read_volatile(pd_pointer)
    };

    let tables_are_empty =
        !original_pdpt_entry_0.flags().contains(PageTableFlags::PRESENT)
        && !original_pdpt_entry_1.flags().contains(PageTableFlags::PRESENT)
        && !original_pd_entry_0.flags().contains(PageTableFlags::PRESENT);

    if !tables_are_empty {
        let _ = allocator.deallocate_frame(pd_frame);
        let _ = allocator.deallocate_frame(pdpt_frame);

        display_test_result(
            console,
            "Lower-level tables initialized empty",
            false,
        );

        unsafe {
            let _ = space.destroy(mapping, allocator);
        }

        return;
    }

    // --------------------------------------------------------
    // 9. Connect PDPT[1] -> PD
    // --------------------------------------------------------

    let mut pd_link = original_pdpt_entry_1.clone();

    pd_link.set_addr(
        PhysAddr::new(pd_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pdpt_pointer.add(1),
            pd_link,
        );
    }

    // --------------------------------------------------------
    // 10. Inject invalid frame into PD[0]
    // --------------------------------------------------------

    let mut invalid_pd_entry =
        original_pd_entry_0.clone();

    invalid_pd_entry.set_addr(
        PhysAddr::new(invalid_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pd_pointer,
            invalid_pd_entry,
        );
    }

    // --------------------------------------------------------
    // 11. Inject invalid frame into PDPT[0]
    // --------------------------------------------------------

    let mut invalid_pdpt_entry =
        original_pdpt_entry_0.clone();

    invalid_pdpt_entry.set_addr(
        PhysAddr::new(invalid_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pdpt_pointer,
            invalid_pdpt_entry,
        );
    }

    // --------------------------------------------------------
    // 12. Connect PDPT to PML4
    // --------------------------------------------------------

    let mut pml4_entry =
        original_pml4_entry.clone();

    pml4_entry.set_addr(
        PhysAddr::new(pdpt_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pml4_entry_pointer,
            pml4_entry,
        );
    }

    // --------------------------------------------------------
    // 13. Test invalid PDPT entry
    // --------------------------------------------------------

    let pdpt_result = unsafe {
        space.destroy(mapping, allocator)
    };

    let pdpt_rejected = matches!(
        pdpt_result,
        Err(AddressSpaceError::FrameNotOwned)
    );

    display_test_result(
        console,
        "Invalid PDPT table frame rejected",
        pdpt_rejected,
    );

    if space.is_destroyed() {
        display_test_result(
            console,
            "Lower-level PDPT test cleanup",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 14. Verify frames survived PDPT failure
    // --------------------------------------------------------

    let frames_preserved_after_pdpt_failure =
        allocator.owns_frame(root)
            && allocator.owns_frame(pdpt_frame)
            && allocator.owns_frame(pd_frame);

    display_test_result(
        console,
        "All table frames preserved after PDPT failure",
        frames_preserved_after_pdpt_failure,
    );

    if !frames_preserved_after_pdpt_failure {
        display_test_result(
            console,
            "Lower-level PDPT test cleanup",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 15. Repair PDPT[0]
    // --------------------------------------------------------

    unsafe {
        write_volatile(
            pdpt_pointer,
            original_pdpt_entry_0,
        );
    }

    // --------------------------------------------------------
    // 16. Test invalid PD entry
    // --------------------------------------------------------

    let pd_result = unsafe {
        space.destroy(mapping, allocator)
    };

    let pd_rejected = matches!(
        pd_result,
        Err(AddressSpaceError::FrameNotOwned)
    );

    display_test_result(
        console,
        "Invalid PD table frame rejected",
        pd_rejected,
    );

    if space.is_destroyed() {
        display_test_result(
            console,
            "Lower-level PD test cleanup",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 17. Verify frames survived PD failure
    // --------------------------------------------------------

    let frames_preserved_after_pd_failure =
        allocator.owns_frame(root)
            && allocator.owns_frame(pdpt_frame)
            && allocator.owns_frame(pd_frame);

    display_test_result(
        console,
        "All table frames preserved after PD failure",
        frames_preserved_after_pd_failure,
    );

    if !frames_preserved_after_pd_failure {
        display_test_result(
            console,
            "Lower-level PD test cleanup",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 18. Repair PD[0]
    // --------------------------------------------------------

    unsafe {
        write_volatile(
            pd_pointer,
            original_pd_entry_0,
        );
    }

    // --------------------------------------------------------
    // 19. Destroy repaired address space
    // --------------------------------------------------------

    let cleanup_success = unsafe {
        space.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "Lower-level address space cleanup",
        cleanup_success,
    );

    // --------------------------------------------------------
    // 20. Verify all table frames released
    // --------------------------------------------------------

    let all_frames_released =
        cleanup_success
            && space.is_destroyed()
            && !allocator.owns_frame(root)
            && !allocator.owns_frame(pdpt_frame)
            && !allocator.owns_frame(pd_frame);

    display_test_result(
        console,
        "Root, PDPT and PD frames released",
        all_frames_released,
    );
}




fn test_address_space_zero_frame_detection(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use core::ptr::{
        read_volatile,
        write_volatile,
    };

    use x86_64::PhysAddr;
    use x86_64::structures::paging::page_table::PageTableEntry;
    use x86_64::structures::paging::PageTableFlags;

    use memory::address_space::{
        AddressSpace,
        AddressSpaceError,
    };

    console.write(
        "\nAddress space zero frame detection test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Create isolated address space
    // --------------------------------------------------------

    let mut space = match AddressSpace::new(
        mapping,
        allocator,
    ) {
        Some(space) => space,
        None => {
            display_test_result(
                console,
                "Zero frame test address space creation",
                false,
            );
            return;
        }
    };

    display_test_result(
        console,
        "Zero frame test address space creation",
        true,
    );

    let root = space.pml4_physical_address();

    // --------------------------------------------------------
    // 2. Translate root
    // --------------------------------------------------------

    let root_virtual = match mapping.translate_frame(root) {
        Some(address) => address,
        None => {
            display_test_result(
                console,
                "Zero frame test root translation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let root_pointer =
        root_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 3. Save original PML4 entry
    // --------------------------------------------------------

    let entry_pointer = root_pointer;

    let original_entry = unsafe {
        read_volatile(entry_pointer)
    };

    if original_entry.flags().contains(PageTableFlags::PRESENT) {
        display_test_result(
            console,
            "Private PML4 entry is available",
            false,
        );

        unsafe {
            let _ = space.destroy(mapping, allocator);
        }

        return;
    }

    display_test_result(
        console,
        "Private PML4 entry is available",
        true,
    );

    // --------------------------------------------------------
    // 4. Inject a PRESENT entry with physical address zero
    // --------------------------------------------------------

    let mut invalid_entry = original_entry.clone();

    invalid_entry.set_addr(
        PhysAddr::new(0),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            entry_pointer,
            invalid_entry,
        );
    }

    // --------------------------------------------------------
    // 5. Attempt destruction
    // --------------------------------------------------------

    let destroy_result = unsafe {
        space.destroy(mapping, allocator)
    };

    let zero_frame_detected = matches!(
        destroy_result,
        Err(AddressSpaceError::InvalidTableAddress)
    );

    display_test_result(
        console,
        "Zero table address correctly rejected",
        zero_frame_detected,
    );

    // --------------------------------------------------------
    // 6. Verify root frame was preserved
    // --------------------------------------------------------

    let root_preserved = allocator.owns_frame(root);

    display_test_result(
        console,
        "Root frame preserved after failure",
        root_preserved,
    );

    if space.is_destroyed() || !root_preserved {
        display_test_result(
            console,
            "Zero frame test cleanup",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 7. Restore original PML4 entry
    // --------------------------------------------------------

    unsafe {
        write_volatile(
            entry_pointer,
            original_entry,
        );
    }

    // --------------------------------------------------------
    // 8. Destroy repaired address space
    // --------------------------------------------------------

    let cleanup_success = unsafe {
        space.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "Zero frame test cleanup",
        cleanup_success,
    );
}


fn test_address_space_huge_page_detection(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use core::ptr::{
        read_volatile,
        write_volatile,
    };

    use x86_64::PhysAddr;
    use x86_64::structures::paging::page_table::PageTableEntry;
    use x86_64::structures::paging::PageTableFlags;

    use memory::address_space::{
        AddressSpace,
        AddressSpaceError,
    };

    console.write(
        "\nAddress space huge page detection test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Create isolated address space
    // --------------------------------------------------------

    let mut space = match AddressSpace::new(
        mapping,
        allocator,
    ) {
        Some(space) => space,
        None => {
            display_test_result(
                console,
                "Huge page test address space creation",
                false,
            );
            return;
        }
    };

    display_test_result(
        console,
        "Huge page test address space creation",
        true,
    );

    let root = space.pml4_physical_address();

    // --------------------------------------------------------
    // 2. Translate root
    // --------------------------------------------------------

    let root_virtual = match mapping.translate_frame(root) {
        Some(address) => address,
        None => {
            display_test_result(
                console,
                "Huge page test root translation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let root_pointer =
        root_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 3. Save original PML4 entry
    // --------------------------------------------------------

    let entry_pointer = root_pointer;

    let original_entry = unsafe {
        read_volatile(entry_pointer)
    };

    if original_entry.flags().contains(PageTableFlags::PRESENT) {
        display_test_result(
            console,
            "Private PML4 entry is available",
            false,
        );

        unsafe {
            let _ = space.destroy(mapping, allocator);
        }

        return;
    }

    display_test_result(
        console,
        "Private PML4 entry is available",
        true,
    );

    // --------------------------------------------------------
    // 4. Inject an invalid HUGE_PAGE PML4 entry
    // --------------------------------------------------------

    let mut huge_entry = original_entry.clone();

    huge_entry.set_addr(
        PhysAddr::new(0x1000),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::HUGE_PAGE,
    );

    unsafe {
        write_volatile(
            entry_pointer,
            huge_entry,
        );
    }


    let injected_entry = unsafe {
    read_volatile(entry_pointer)
};

let injected_flags = injected_entry.flags();

let huge_flag_written =
    injected_flags.contains(PageTableFlags::HUGE_PAGE)
    && injected_flags.contains(PageTableFlags::PRESENT);

display_test_result(
    console,
    "HUGE_PAGE flag stored in PML4",
    huge_flag_written,
);

    // --------------------------------------------------------
    // 5. Attempt destruction
    // --------------------------------------------------------

    let destroy_result = unsafe {
        space.destroy(mapping, allocator)
    };

    let huge_page_detected = matches!(
        destroy_result,
        Err(AddressSpaceError::InvalidTableAddress)
    );

    display_test_result(
        console,
        "Huge page flag correctly rejected at PML4",
        huge_page_detected,
    );

    // --------------------------------------------------------
    // 6. Verify root frame was preserved
    // --------------------------------------------------------

    let root_preserved = allocator.owns_frame(root);

    display_test_result(
        console,
        "Root frame preserved after failure",
        root_preserved,
    );

    if space.is_destroyed() || !root_preserved {
        display_test_result(
            console,
            "Huge page test cleanup",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 7. Restore original PML4 entry
    // --------------------------------------------------------

    unsafe {
        write_volatile(
            entry_pointer,
            original_entry,
        );
    }

    // --------------------------------------------------------
    // 8. Destroy repaired address space
    // --------------------------------------------------------

    let cleanup_success = unsafe {
        space.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "Huge page test cleanup",
        cleanup_success,
    );
}


fn test_address_space_valid_huge_page_leaves(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use core::ptr::{
        read_volatile,
        write_bytes,
        write_volatile,
    };

    use x86_64::PhysAddr;
    use x86_64::structures::paging::page_table::PageTableEntry;
    use x86_64::structures::paging::PageTableFlags;

    use memory::address_space::AddressSpace;

    console.write(
        "\nAddress space valid huge page leaves test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Create isolated address space
    // --------------------------------------------------------

    let mut space = match AddressSpace::new(mapping, allocator) {
        Some(space) => space,
        None => {
            display_test_result(
                console,
                "Huge-page leaves address space creation",
                false,
            );
            return;
        }
    };

    let root = space.pml4_physical_address();

    let root_virtual = match mapping.translate_frame(root) {
        Some(address) => address,
        None => {
            display_test_result(
                console,
                "Huge-page leaves root translation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let root_pointer =
        root_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 2. Verify private PML4 entry
    // --------------------------------------------------------

    let pml4_entry_pointer = root_pointer;

    let original_pml4_entry = unsafe {
        read_volatile(pml4_entry_pointer)
    };

    if original_pml4_entry.flags().contains(
        PageTableFlags::PRESENT,
    ) {
        display_test_result(
            console,
            "Huge-page leaves PML4 entry available",
            false,
        );

        unsafe {
            let _ = space.destroy(mapping, allocator);
        }

        return;
    }

    // --------------------------------------------------------
    // 3. Allocate PDPT and PD
    // --------------------------------------------------------

    let pdpt_frame = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            display_test_result(
                console,
                "Huge-page leaves PDPT allocation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let pd_frame = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            let _ = allocator.deallocate_frame(pdpt_frame);

            display_test_result(
                console,
                "Huge-page leaves PD allocation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    // --------------------------------------------------------
    // 4. Translate allocated tables
    // --------------------------------------------------------

    let pdpt_virtual = mapping.translate_frame(pdpt_frame);
    let pd_virtual = mapping.translate_frame(pd_frame);

    let (pdpt_virtual, pd_virtual) =
        match (pdpt_virtual, pd_virtual) {
            (Some(pdpt), Some(pd)) => (pdpt, pd),

            _ => {
                let _ = allocator.deallocate_frame(pd_frame);
                let _ = allocator.deallocate_frame(pdpt_frame);

                display_test_result(
                    console,
                    "Huge-page leaves table translation",
                    false,
                );

                unsafe {
                    let _ = space.destroy(mapping, allocator);
                }

                return;
            }
        };

    let pdpt_pointer =
        pdpt_virtual.as_u64() as *mut PageTableEntry;

    let pd_pointer =
        pd_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 5. Initialize page tables
    // --------------------------------------------------------

    unsafe {
        write_bytes(
            pdpt_pointer as *mut u8,
            0,
            memory::FRAME_SIZE as usize,
        );

        write_bytes(
            pd_pointer as *mut u8,
            0,
            memory::FRAME_SIZE as usize,
        );
    }

    // --------------------------------------------------------
    // 6. Construct valid 1 GiB huge-page leaf
    //
    // PDPT[0] -> 1 GiB mapping
    // --------------------------------------------------------

    let mut one_gib_entry = unsafe {
        read_volatile(pdpt_pointer)
    };

    one_gib_entry.set_addr(
        PhysAddr::new(0),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::HUGE_PAGE,
    );

    unsafe {
        write_volatile(
            pdpt_pointer,
            one_gib_entry,
        );
    }

    // --------------------------------------------------------
    // 7. Construct PDPT[1] -> PD
    // --------------------------------------------------------

    let mut pdpt_child_entry = unsafe {
        read_volatile(pdpt_pointer.add(1))
    };

    pdpt_child_entry.set_addr(
        PhysAddr::new(pd_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pdpt_pointer.add(1),
            pdpt_child_entry,
        );
    }

    // --------------------------------------------------------
    // 8. Construct valid 2 MiB huge-page leaf
    //
    // PD[0] -> 2 MiB mapping
    // --------------------------------------------------------

    let mut two_mib_entry = unsafe {
        read_volatile(pd_pointer)
    };

    two_mib_entry.set_addr(
        PhysAddr::new(0),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE
            | PageTableFlags::HUGE_PAGE,
    );

    unsafe {
        write_volatile(
            pd_pointer,
            two_mib_entry,
        );
    }

    // --------------------------------------------------------
    // 9. Connect PDPT to PML4
    // --------------------------------------------------------

    let mut pml4_entry = original_pml4_entry.clone();

    pml4_entry.set_addr(
        PhysAddr::new(pdpt_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pml4_entry_pointer,
            pml4_entry,
        );
    }

    display_test_result(
        console,
        "1 GiB huge-page leaf constructed",
        true,
    );

    display_test_result(
        console,
        "2 MiB huge-page leaf constructed",
        true,
    );

    // --------------------------------------------------------
    // 10. Destroy address space
    // --------------------------------------------------------

    let destroy_success = unsafe {
        space.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "Valid huge-page leaves accepted",
        destroy_success,
    );

    // --------------------------------------------------------
    // 11. Verify destruction and table release
    // --------------------------------------------------------

    let address_space_destroyed =
        destroy_success && space.is_destroyed();

    let pd_frame_released =
        !allocator.owns_frame(pd_frame);

    let pdpt_frame_released =
        !allocator.owns_frame(pdpt_frame);

    let root_frame_released =
        !allocator.owns_frame(root);

    display_test_result(
        console,
        "Address space destroyed",
        address_space_destroyed,
    );

    display_test_result(
        console,
        "PD frame released",
        pd_frame_released,
    );

    display_test_result(
        console,
        "PDPT frame released",
        pdpt_frame_released,
    );

    display_test_result(
        console,
        "Root frame released",
        root_frame_released,
    );

    display_test_result(
        console,
        "Huge-page leaves cleanup",
        address_space_destroyed
            && pd_frame_released
            && pdpt_frame_released
            && root_frame_released,
    );
}


fn test_address_space_non_present_entries(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use core::ptr::{
        read_volatile,
        write_bytes,
        write_volatile,
    };

    use x86_64::PhysAddr;
    use x86_64::structures::paging::page_table::PageTableEntry;
    use x86_64::structures::paging::PageTableFlags;

    use memory::address_space::AddressSpace;

    console.write(
        "\nAddress space non-present entries test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Create address space
    // --------------------------------------------------------

    let mut space = match AddressSpace::new(mapping, allocator) {
        Some(space) => space,
        None => {
            display_test_result(
                console,
                "Non-present test address space creation",
                false,
            );
            return;
        }
    };

    let root = space.pml4_physical_address();

    let root_virtual = match mapping.translate_frame(root) {
        Some(address) => address,
        None => {
            display_test_result(
                console,
                "Non-present test root translation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let root_pointer =
        root_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 2. Check private PML4 entry
    // --------------------------------------------------------

    let pml4_entry_pointer = root_pointer;

    let original_pml4_entry = unsafe {
        read_volatile(pml4_entry_pointer)
    };

    if original_pml4_entry.flags().contains(
        PageTableFlags::PRESENT,
    ) {
        display_test_result(
            console,
            "Non-present test PML4 entry available",
            false,
        );

        unsafe {
            let _ = space.destroy(mapping, allocator);
        }

        return;
    }

    // --------------------------------------------------------
    // 3. Find unowned physical frame
    // --------------------------------------------------------

    let mut invalid_frame = 0x000F_FFFF_FFFF_F000u64;

    while allocator.owns_frame(invalid_frame) {
        invalid_frame -= memory::FRAME_SIZE;
    }

    display_test_result(
        console,
        "Non-present test frame is unowned",
        !allocator.owns_frame(invalid_frame),
    );

    // --------------------------------------------------------
    // 4. Allocate PDPT and PD
    // --------------------------------------------------------

    let pdpt_frame = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            display_test_result(
                console,
                "Non-present test PDPT allocation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    let pd_frame = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            let _ = allocator.deallocate_frame(pdpt_frame);

            display_test_result(
                console,
                "Non-present test PD allocation",
                false,
            );

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            return;
        }
    };

    // --------------------------------------------------------
    // 5. Translate tables
    // --------------------------------------------------------

    let pdpt_virtual = mapping.translate_frame(pdpt_frame);
    let pd_virtual = mapping.translate_frame(pd_frame);

    let (pdpt_virtual, pd_virtual) =
        match (pdpt_virtual, pd_virtual) {
            (Some(pdpt), Some(pd)) => (pdpt, pd),

            _ => {
                let _ = allocator.deallocate_frame(pd_frame);
                let _ = allocator.deallocate_frame(pdpt_frame);

                display_test_result(
                    console,
                    "Non-present test table translation",
                    false,
                );

                unsafe {
                    let _ = space.destroy(mapping, allocator);
                }

                return;
            }
        };

    let pdpt_pointer =
        pdpt_virtual.as_u64() as *mut PageTableEntry;

    let pd_pointer =
        pd_virtual.as_u64() as *mut PageTableEntry;

    // --------------------------------------------------------
    // 6. Initialize tables
    // --------------------------------------------------------

    unsafe {
        write_bytes(
            pdpt_pointer as *mut u8,
            0,
            memory::FRAME_SIZE as usize,
        );

        write_bytes(
            pd_pointer as *mut u8,
            0,
            memory::FRAME_SIZE as usize,
        );
    }

    // --------------------------------------------------------
    // 7. Create PDPT[1] -> PD
    // --------------------------------------------------------

    let mut pd_link = unsafe {
        read_volatile(pdpt_pointer.add(1))
    };

    pd_link.set_addr(
        PhysAddr::new(pd_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pdpt_pointer.add(1),
            pd_link,
        );
    }

    // --------------------------------------------------------
    // 8. Create non-present PDPT entry
    // --------------------------------------------------------

    let mut non_present_pdpt_entry = unsafe {
        read_volatile(pdpt_pointer)
    };

    non_present_pdpt_entry.set_addr(
        PhysAddr::new(invalid_frame),
        PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pdpt_pointer,
            non_present_pdpt_entry,
        );
    }

    // --------------------------------------------------------
    // 9. Create non-present PD entry
    // --------------------------------------------------------

    let mut non_present_pd_entry = unsafe {
        read_volatile(pd_pointer)
    };

    non_present_pd_entry.set_addr(
        PhysAddr::new(invalid_frame),
        PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pd_pointer,
            non_present_pd_entry,
        );
    }

    // --------------------------------------------------------
    // 10. Connect PDPT to PML4
    // --------------------------------------------------------

    let mut pml4_entry =
        original_pml4_entry.clone();

    pml4_entry.set_addr(
        PhysAddr::new(pdpt_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            pml4_entry_pointer,
            pml4_entry,
        );
    }

    // --------------------------------------------------------
    // 11. Verify PRESENT is disabled
    // --------------------------------------------------------

    let pdpt_present_disabled = !unsafe {
        read_volatile(pdpt_pointer)
    }
    .flags()
    .contains(PageTableFlags::PRESENT);

    let pd_present_disabled = !unsafe {
        read_volatile(pd_pointer)
    }
    .flags()
    .contains(PageTableFlags::PRESENT);

    display_test_result(
        console,
        "PDPT entry PRESENT flag disabled",
        pdpt_present_disabled,
    );

    display_test_result(
        console,
        "PD entry PRESENT flag disabled",
        pd_present_disabled,
    );

    // --------------------------------------------------------
    // 12. Destroy address space
    // --------------------------------------------------------

    let destroy_result = unsafe {
        space.destroy(mapping, allocator)
    };

    let destroy_success = destroy_result.is_ok();

    display_test_result(
        console,
        "Non-present entries ignored",
        destroy_success,
    );

    // --------------------------------------------------------
    // 13. Verify all frames released
    // --------------------------------------------------------

    let all_frames_released =
        destroy_success
            && space.is_destroyed()
            && !allocator.owns_frame(root)
            && !allocator.owns_frame(pdpt_frame)
            && !allocator.owns_frame(pd_frame);

    display_test_result(
        console,
        "Root, PDPT and PD frames released",
        all_frames_released,
    );

    display_test_result(
        console,
        "Non-present entries test cleanup",
        all_frames_released,
    );
}


fn test_address_space_transactional_destruction(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use core::ptr::{
        read_volatile,
        write_bytes,
        write_volatile,
    };

    use x86_64::PhysAddr;
    use x86_64::structures::paging::page_table::PageTableEntry;
    use x86_64::structures::paging::PageTableFlags;

    use memory::address_space::{
        AddressSpace,
        AddressSpaceError,
    };

    console.write(
        "\nAddress space transactional destruction test:\n",
        GREEN,
    );

    // 1. Create isolated address space.

    let mut space = match AddressSpace::new(mapping, allocator) {
        Some(space) => space,
        None => {
            display_test_result(
                console,
                "Transactional test address space creation",
                false,
            );
            return;
        }
    };

    let root = space.pml4_physical_address();

    let root_virtual = match mapping.translate_frame(root) {
        Some(address) => address,
        None => {
            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            display_test_result(
                console,
                "Transactional test root translation",
                false,
            );
            return;
        }
    };

    let root_pointer =
        root_virtual.as_u64() as *mut PageTableEntry;

    // 2. Allocate a valid first branch.

    let first_pdpt = match allocator.allocate_frame() {
        Some(frame) => frame,
        None => {
            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            display_test_result(
                console,
                "Transactional first PDPT allocation",
                false,
            );
            return;
        }
    };

 let first_pdpt_virtual =
    match mapping.translate_frame(first_pdpt) {
        Some(address) => address,
        None => {
            allocator.deallocate_frame(first_pdpt);

            unsafe {
                let _ = space.destroy(mapping, allocator);
            }

            display_test_result(
                console,
                "Transactional first PDPT translation",
                false,
            );
            return;
        }
    };

// Initialize the newly allocated page table.
unsafe {
    write_bytes(
        first_pdpt_virtual.as_u64() as *mut u8,
        0,
        memory::vmm::PAGE_SIZE as usize,
    );
}
    // 3. Verify both PML4 entries are unused.

    let first_entry = unsafe {
        read_volatile(root_pointer)
    };

    let second_entry = unsafe {
        read_volatile(root_pointer.add(1))
    };

    if !first_entry.is_unused() || !second_entry.is_unused() {
        allocator.deallocate_frame(first_pdpt);

        unsafe {
            let _ = space.destroy(mapping, allocator);
        }

        display_test_result(
            console,
            "Transactional PML4 entries available",
            false,
        );
        return;
    }

    // 4. Build the valid first branch.

    let first_pdpt_entry =
        PageTableEntry::new();

    let mut first_pdpt_entry =
        first_pdpt_entry;

    first_pdpt_entry.set_addr(
        PhysAddr::new(first_pdpt),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            root_pointer,
            first_pdpt_entry,
        );
    }

    let _ = first_pdpt_virtual;

    // 5. Prepare an unowned second branch.

    let mut invalid_frame = 0x000F_FFFF_FFFF_F000u64;

    while allocator.owns_frame(invalid_frame) {
        invalid_frame -= memory::FRAME_SIZE;
    }

    let mut invalid_entry = second_entry.clone();

    invalid_entry.set_addr(
        PhysAddr::new(invalid_frame),
        PageTableFlags::PRESENT
            | PageTableFlags::WRITABLE
            | PageTableFlags::USER_ACCESSIBLE,
    );

    unsafe {
        write_volatile(
            root_pointer.add(1),
            invalid_entry,
        );
    }

    // 6. Snapshot ownership before destruction.

    let root_owned_before =
        allocator.owns_frame(root);

    let first_pdpt_owned_before =
        allocator.owns_frame(first_pdpt);

    display_test_result(
        console,
        "Valid first branch allocated",
        first_pdpt_owned_before,
    );

    display_test_result(
        console,
        "Second branch is unowned",
        !allocator.owns_frame(invalid_frame),
    );

    // 7. Attempt destruction.

    let destroy_result = unsafe {
        space.destroy(mapping, allocator)
    };

    let expected_error = matches!(
        destroy_result,
        Err(AddressSpaceError::FrameNotOwned)
    );

    display_test_result(
        console,
        "Invalid second branch rejected",
        expected_error,
    );

    // 8. Verify transactional behavior.

    let root_preserved =
        allocator.owns_frame(root) == root_owned_before;

    let first_branch_preserved =
        allocator.owns_frame(first_pdpt)
            == first_pdpt_owned_before;

    let address_space_preserved =
        !space.is_destroyed()
            && space.pml4_physical_address() == root;

    display_test_result(
        console,
        "Root preserved after validation failure",
        root_preserved,
    );

    display_test_result(
        console,
        "First branch preserved after validation failure",
        first_branch_preserved,
    );

    display_test_result(
        console,
        "Address space remains valid",
        address_space_preserved,
    );

    // 9. Restore the corrupted entry.

    unsafe {
        write_volatile(
            root_pointer.add(1),
            second_entry,
        );
    }

    // 10. Destroy the repaired address space.

    let cleanup_success = unsafe {
        space.destroy(mapping, allocator).is_ok()
    };

    display_test_result(
        console,
        "Transactional test cleanup",
        cleanup_success,
    );
}

// ============================================================
// PROCESS MANAGER TESTS
// ============================================================

#[inline(never)]
fn test_process_manager(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use process::ProcessState;

    console.write(
        "\nProcess manager test:\n",
        GREEN,
    );

    let mut manager = process::PROCESS_MANAGER.lock();

    if !manager.is_empty() {
        display_test_result(
            console,
            "Process manager starts empty",
            false,
        );
        return;
    }

    // --------------------------------------------------------
    // 1. Create first process
    // --------------------------------------------------------

    let first_pid = match manager.create_process(mapping, allocator) {
        Ok(pid) => pid,
        Err(_) => {
            display_test_result(
                console,
                "First process creation",
                false,
            );
            return;
        }
    };

    display_test_result(
        console,
        "First process creation",
        true,
    );

    // --------------------------------------------------------
    // 2. Create second process
    // --------------------------------------------------------

    let second_pid = match manager.create_process(mapping, allocator) {
        Ok(pid) => pid,
        Err(_) => {
            display_test_result(
                console,
                "Second process creation",
                false,
            );

            let terminated = unsafe {
                manager.terminate_process(
                    first_pid,
                    mapping,
                    allocator,
                ).is_ok()
            };

            if terminated {
                let _ = manager.reap_process(first_pid);
            }

            return;
        }
    };

    display_test_result(
        console,
        "Second process creation",
        true,
    );

    // --------------------------------------------------------
    // 3. Verify unique PIDs
    // --------------------------------------------------------

    display_test_result(
        console,
        "Unique process IDs",
        first_pid != second_pid,
    );

    // --------------------------------------------------------
    // 4. Verify independent address spaces
    // --------------------------------------------------------

    let first_root = manager
        .get_process(first_pid)
        .map(|process| {
            process.address_space().pml4_physical_address()
        });

    let second_root = manager
        .get_process(second_pid)
        .map(|process| {
            process.address_space().pml4_physical_address()
        });

    let independent_spaces = matches!(
        (first_root, second_root),
        (Some(first), Some(second)) if first != second
    );

    display_test_result(
        console,
        "Independent process address spaces",
        independent_spaces,
    );

    // --------------------------------------------------------
    // 5. Verify shared kernel mappings
    // --------------------------------------------------------

    let first_kernel_shared = manager
        .get_process(first_pid)
        .map(|process| {
            process.address_space().shares_kernel_mappings(mapping)
        })
        .unwrap_or(false);

    let second_kernel_shared = manager
        .get_process(second_pid)
        .map(|process| {
            process.address_space().shares_kernel_mappings(mapping)
        })
        .unwrap_or(false);

    display_test_result(
        console,
        "First process kernel mappings",
        first_kernel_shared,
    );

    display_test_result(
        console,
        "Second process kernel mappings",
        second_kernel_shared,
    );

    // --------------------------------------------------------
    // 6. Verify initial state and state transition
    // --------------------------------------------------------

    let first_created = manager
        .get_process(first_pid)
        .map(|process| process.state() == ProcessState::Created)
        .unwrap_or(false);

    display_test_result(
        console,
        "Initial process state",
        first_created,
    );

    let state_changed = manager.set_process_state(
        first_pid,
        ProcessState::Ready,
    );

    let ready_state = manager
        .get_process(first_pid)
        .map(|process| process.state() == ProcessState::Ready)
        .unwrap_or(false);

    display_test_result(
        console,
        "Process state transition",
        state_changed && ready_state,
    );

    // --------------------------------------------------------
    // 7. Terminate first process
    // --------------------------------------------------------

    let first_terminated = unsafe {
        manager.terminate_process(
            first_pid,
            mapping,
            allocator,
        ).is_ok()
    };

    display_test_result(
        console,
        "First process termination",
        first_terminated,
    );

    let first_state_terminated = manager
        .get_process(first_pid)
        .map(|process| process.state() == ProcessState::Terminated)
        .unwrap_or(false);

    display_test_result(
        console,
        "First process terminated state",
        first_state_terminated,
    );

    // --------------------------------------------------------
    // 8. Reap first process
    // --------------------------------------------------------

    let first_reaped = if first_terminated {
        manager.reap_process(first_pid).is_ok()
    } else {
        false
    };

    display_test_result(
        console,
        "First process reaped",
        first_reaped,
    );

    let first_removed = manager.get_process(first_pid).is_none();

    display_test_result(
        console,
        "First process removed from table",
        first_removed,
    );

    // --------------------------------------------------------
    // 9. Terminate and reap second process
    // --------------------------------------------------------

    let second_terminated = unsafe {
        manager.terminate_process(
            second_pid,
            mapping,
            allocator,
        ).is_ok()
    };

    display_test_result(
        console,
        "Second process termination",
        second_terminated,
    );

    let second_reaped = if second_terminated {
        manager.reap_process(second_pid).is_ok()
    } else {
        false
    };

    display_test_result(
        console,
        "Second process reaped",
        second_reaped,
    );

    display_test_result(
        console,
        "Process table empty",
        manager.is_empty(),
    );

    display_test_result(
        console,
        "Physical frame allocator integrity",
        allocator.check_integrity(),
    );
}
fn test_cr3_switch(
    console: &mut Console<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
    target_pml4: u64,
) {
    use x86_64::VirtAddr;

    console.write(
        "\nCR3 address space switching test:\n",
        GREEN,
    );

    let original_pml4 = memory::cr3::current_pml4();

    console.write("  Original PML4: ", GRAY);
    write_number(console, original_pml4);
    console.write("\n", WHITE);

    console.write("  Target PML4:   ", GRAY);
    write_number(console, target_pml4);
    console.write("\n", WHITE);

    // --------------------------------------------------------
    // 1. Check physical accessibility of target PML4
    // --------------------------------------------------------

    let target_accessible =
        mapping.translate_frame(target_pml4).is_some();

    display_test_result(
        console,
        "Target PML4 physical mapping",
        target_accessible,
    );

    if !target_accessible {
        return;
    }

    // --------------------------------------------------------
    // 2. Capture current stack pointer
    // --------------------------------------------------------

    let stack_pointer: u64;

    unsafe {
        asm!(
            "mov {}, rsp",
            out(reg) stack_pointer,
            options(nomem, nostack, preserves_flags),
        );
    }

    // --------------------------------------------------------
    // 3. Prepare required addresses
    // --------------------------------------------------------

    let raw_addresses = [
        test_cr3_switch as *const () as u64,
        memory::cr3::switch_to as *const () as u64,
        memory::cr3::restore as *const () as u64,
        memory::cr3::current_pml4 as *const () as u64,
        stack_pointer,
    ];

    let mut required = [VirtAddr::zero(); 5];

    let labels = [
        "CR3 test function mapping",
        "CR3 switch function mapping",
        "CR3 restore function mapping",
        "CR3 read function mapping",
        "Kernel stack mapping",
    ];

    for index in 0..raw_addresses.len() {
        required[index] =
            match VirtAddr::try_new(raw_addresses[index]) {
                Ok(address) => address,
                Err(_) => {
                    display_test_result(
                        console,
                        labels[index],
                        false,
                    );
                    return;
                }
            };
    }

    // --------------------------------------------------------
    // 4. Check every required address in target PML4
    // --------------------------------------------------------

    let mut all_mapped = true;

    for index in 0..required.len() {
        let mapped = mapping
            .translate_virtual_address_in(
                target_pml4,
                required[index],
            )
            .is_some();

        display_test_result(
            console,
            labels[index],
            mapped,
        );

        if !mapped {
            all_mapped = false;

            console.write(
                "    Missing virtual address: ",
                YELLOW,
            );

            write_number(
                console,
                required[index].as_u64(),
            );

            console.write("\n", WHITE);
        }
    }

    if !all_mapped {
        console.write(
            "  CR3 switch cancelled: missing mappings\n",
            YELLOW,
        );
        return;
    }

    // --------------------------------------------------------
    // 5. Switch address space
    // --------------------------------------------------------

    display_test_result(
        console,
        "All required mappings validated",
        true,
    );

    unsafe {
        asm!("cli", options(nomem, nostack));
    }

    let switch_result = unsafe {
        memory::cr3::switch_to(
            mapping,
            target_pml4,
            &required,
        )
    };

    let previous_state = match switch_result {
        Ok(state) => state,
        Err(_) => {
            unsafe {
                asm!("sti", options(nomem, nostack));
            }

            display_test_result(
                console,
                "CR3 switch operation",
                false,
            );

            return;
        }
    };

    let switched_pml4 = memory::cr3::current_pml4();

    let switched =
        switched_pml4 == target_pml4;

    // --------------------------------------------------------
    // 6. Restore original address space
    // --------------------------------------------------------

    let restore_result = unsafe {
        memory::cr3::restore(previous_state)
    };

    if restore_result.is_err() {
        halt_forever();
    }

    let restored_pml4 = memory::cr3::current_pml4();

    unsafe {
        asm!("sti", options(nomem, nostack));
    }

    display_test_result(
        console,
        "CR3 switched to target PML4",
        switched,
    );

    display_test_result(
        console,
        "Original CR3 restored",
        restored_pml4 == original_pml4,
    );

    console.write("  Restored PML4: ", GRAY);
    write_number(console, restored_pml4);
    console.write("\n", WHITE);
}

// ============================================================
// VMM ALLOCATION ROLLBACK TEST
// ============================================================

#[inline(never)]
fn test_vmm_allocation_rollback(
    console: &mut Console<'_>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
    mapping: &memory::vmm::PhysicalMemoryMapping,
) {
    use x86_64::VirtAddr;
    use x86_64::structures::paging::PageTableFlags;

    console.write(
        "\nVMM allocation rollback test:\n",
        GREEN,
    );

    let initial_allocated =
        allocator.allocated_frames();

    let initial_slots =
        allocator.available_slots();

    if initial_slots < 2 {
        console.write(
            "  Not enough allocator slots for test\n",
            YELLOW,
        );
        return;
    }

    let virtual_address =
        match VirtAddr::try_new(0x0000_6000_0000_0000) {
            Ok(address) => address,
            Err(_) => {
                console.write(
                    "  Invalid test address\n",
                    YELLOW,
                );
                return;
            }
        };

    if mapping
        .translate_virtual_address(virtual_address)
        .is_some()
    {
        console.write(
            "  Test address is already mapped\n",
            YELLOW,
        );
        return;
    }

    // Reserve frames until one allocator slot remains.
    let mut reserved_frames = [0u64; 1024];
    let mut reserved_count = 0usize;

    while allocator.available_slots() > 1 {
        match allocator.allocate_frame() {
            Some(frame) => {
                reserved_frames[reserved_count] = frame;
                reserved_count += 1;
            }

            None => break,
        }
    }

    let prepared =
        allocator.available_slots() == 1
        && reserved_count > 0;

    display_test_result(
        console,
        "One allocator slot remaining",
        prepared,
    );

    let mut target_frame_mapped = false;
    let mut target_frame_safe_to_release = true;

    let result = if prepared {
        let target_frame = reserved_frames[0];

        mapping.map_page(
            allocator,
            virtual_address,
            target_frame,
            PageTableFlags::PRESENT | PageTableFlags::WRITABLE,
        )
    } else {
        Err(memory::vmm::MapError::AllocationFailed)
    };

    let allocation_failed =
        matches!(
            result,
            Err(memory::vmm::MapError::AllocationFailed)
        );

    display_test_result(
        console,
        "Allocation failure detected",
        allocation_failed,
    );

    // If mapping unexpectedly succeeded, remove it before
    // releasing the physical frame used by the mapping.
    if result.is_ok() {
        target_frame_mapped = true;

        let target_frame = reserved_frames[0];

let cleanup_result =
    mapping.unmap_page_with_allocator(
        allocator,
        virtual_address,
    );

        match cleanup_result {
            Ok(unmapped_frame)
                if unmapped_frame == target_frame =>
            {
                target_frame_mapped = false;
            }

            _ => {
                target_frame_safe_to_release = false;

                console.write(
                    "  WARNING: Could not safely remove unexpected mapping\n",
                    YELLOW,
                );
            }
        }
    }

    let mapping_absent =
        mapping
            .translate_virtual_address(virtual_address)
            .is_none();

    display_test_result(
        console,
        "No partial mapping remains",
        mapping_absent,
    );

    let rollback_success =
        allocator.allocated_frames()
            == initial_allocated + reserved_count;

    display_test_result(
        console,
        "Intermediate table rollback",
        rollback_success,
    );

    // Release temporary frames in reverse order.
    let mut release_success = true;

    for index in (0..reserved_count).rev() {
        let frame = reserved_frames[index];

        // Never release the target frame if it may still be mapped.
        if index == 0
            && target_frame_mapped
            && !target_frame_safe_to_release
        {
            release_success = false;
            continue;
        }

        if !allocator.deallocate_frame(frame) {
            release_success = false;
        }
    }

    display_test_result(
        console,
        "Temporary frames released",
        release_success,
    );

    display_test_result(
        console,
        "Allocator restored",
        allocator.allocated_frames() == initial_allocated,
    );

    display_test_result(
        console,
        "Allocator integrity",
        allocator.check_integrity(),
    );
}

// ============================================================
// HEAP ALLOCATOR TESTS
// ============================================================

// ============================================================
// GLOBAL HEAP ALLOCATOR TESTS
// ============================================================

#[inline(never)]
fn test_heap_allocator(
    console: &mut Console<'_>,
) {
    use memory::heap::GLOBAL_ALLOCATOR;

    console.write(
        "\nGlobal heap allocator test:\n",
        GREEN,
    );

    let initial_free = GLOBAL_ALLOCATOR.free_bytes();

    console.write("  Initial free memory: ", GRAY);
    write_number(console, initial_free as u64);
    console.write(" bytes\n", WHITE);

    // --------------------------------------------------------
    // 1. Box allocation
    // --------------------------------------------------------

    let boxed = Box::new(42u64);

    display_test_result(
        console,
        "Box allocation",
        *boxed == 42,
    );

    // --------------------------------------------------------
    // 2. Vec allocation
    // --------------------------------------------------------

    let mut vector = Vec::new();

    vector.push(10u64);
    vector.push(20u64);
    vector.push(30u64);
    vector.push(40u64);

    let vector_valid =
        vector.len() == 4
        && vector[0] == 10
        && vector[1] == 20
        && vector[2] == 30
        && vector[3] == 40;

    display_test_result(
        console,
        "Vec allocation",
        vector_valid,
    );

    // --------------------------------------------------------
    // 3. String allocation
    // --------------------------------------------------------

    let mut string = String::new();

    string.push_str("AmaryllisOS alloc test");

    display_test_result(
        console,
        "String allocation",
        string.as_str() == "AmaryllisOS alloc test",
    );

    // --------------------------------------------------------
    // 4. Check heap usage
    // --------------------------------------------------------

    let current_free = GLOBAL_ALLOCATOR.free_bytes();

    display_test_result(
        console,
        "Heap usage detected",
        current_free < initial_free,
    );

    // --------------------------------------------------------
    // 5. Release allocations
    // --------------------------------------------------------

    drop(boxed);
    drop(vector);
    drop(string);

    let restored_free = GLOBAL_ALLOCATOR.free_bytes();

    display_test_result(
        console,
        "Memory deallocation",
        restored_free == initial_free,
    );

    // --------------------------------------------------------
    // 6. Check block coalescing
    // --------------------------------------------------------

    display_test_result(
        console,
        "Free block coalescing",
        GLOBAL_ALLOCATOR.free_blocks() == 1,
    );

    // --------------------------------------------------------
    // 7. Check heap integrity
    // --------------------------------------------------------

    display_test_result(
        console,
        "Heap integrity",
        GLOBAL_ALLOCATOR.check_integrity(),
    );

    // --------------------------------------------------------
    // 8. Display statistics
    // --------------------------------------------------------

    console.write("\n  Free blocks: ", GRAY);

    write_number(
        console,
        GLOBAL_ALLOCATOR.free_blocks() as u64,
    );

    console.write("\n", WHITE);

    console.write("  Free memory: ", GRAY);

    write_number(
        console,
        GLOBAL_ALLOCATOR.free_bytes() as u64,
    );

    console.write(" bytes\n", WHITE);
}



#[inline(never)]
fn test_memory_api(
    console: &mut Console<'_>,
) {
    console.write(
        "\nMemory API test:\n",
        GREEN,
    );

    // --------------------------------------------------------
    // 1. Get initial heap statistics
    // --------------------------------------------------------

    let initial_stats = memory::heap_stats();

    display_test_result(
        console,
        "Heap initialized",
        initial_stats.initialized,
    );

    if !initial_stats.initialized {
        return;
    }

    let initial_free = initial_stats.free_bytes;

    // --------------------------------------------------------
    // 2. Allocate memory
    // --------------------------------------------------------

    let layout = match Layout::from_size_align(256, 64) {
        Ok(layout) => layout,
        Err(_) => {
            display_test_result(
                console,
                "Create allocation layout",
                false,
            );
            return;
        }
    };

    let pointer = match memory::allocate(layout) {
        Some(pointer) => pointer,
        None => {
            display_test_result(
                console,
                "Memory allocation",
                false,
            );
            return;
        }
    };

    display_test_result(
        console,
        "Memory allocation",
        true,
    );

    // --------------------------------------------------------
    // 3. Check alignment
    // --------------------------------------------------------

    let aligned =
        pointer.as_ptr() as usize % 64 == 0;

    display_test_result(
        console,
        "64-byte memory alignment",
        aligned,
    );

    // --------------------------------------------------------
    // 4. Check heap statistics
    // --------------------------------------------------------

    let allocated_stats = memory::heap_stats();

    display_test_result(
        console,
        "Heap usage detected",
        allocated_stats.free_bytes < initial_free,
    );

    display_test_result(
        console,
        "Used memory detected",
        allocated_stats.used_bytes > initial_stats.used_bytes,
    );

    // --------------------------------------------------------
    // 5. Deallocate memory
    // --------------------------------------------------------

    let released = unsafe {
        memory::deallocate(pointer)
    };

    display_test_result(
        console,
        "Memory deallocation",
        released,
    );

    // --------------------------------------------------------
    // 6. Check restored statistics
    // --------------------------------------------------------

    let final_stats = memory::heap_stats();

    display_test_result(
        console,
        "Free memory restored",
        final_stats.free_bytes == initial_free,
    );

    display_test_result(
        console,
        "Heap blocks merged",
        final_stats.free_blocks == initial_stats.free_blocks,
    );

    // --------------------------------------------------------
    // 7. Check heap integrity
    // --------------------------------------------------------

    display_test_result(
        console,
        "Memory API integrity",
        memory::check_heap_integrity(),
    );

    // --------------------------------------------------------
    // 8. Display statistics
    // --------------------------------------------------------

    console.write("  Total heap: ", GRAY);
    write_number(console, final_stats.total_bytes as u64);
    console.write(" bytes\n", WHITE);

    console.write("  Used memory: ", GRAY);
    write_number(console, final_stats.used_bytes as u64);
    console.write(" bytes\n", WHITE);

    console.write("  Free memory: ", GRAY);
    write_number(console, final_stats.free_bytes as u64);
    console.write(" bytes\n", WHITE);

    console.write("  Largest free block: ", GRAY);
    write_number(
        console,
        final_stats.largest_free_block as u64,
    );
    console.write(" bytes\n", WHITE);
}


// ============================================================
// INTERACTIVE SHELL
// ============================================================

#[inline(never)]
fn run_shell(
    console: &mut Console<'_>,
    mapping: Option<&memory::vmm::PhysicalMemoryMapping>,
    allocator: &mut memory::PhysicalFrameAllocator<'_>,
) -> ! {
    let mut buffer = [0u8; BUFFER_SIZE];

    let mut length = 0usize;
    let mut cursor_position = 0usize;

    let mut history =
        alloc::vec![[0u8; BUFFER_SIZE]; HISTORY_SIZE];

    let mut history_lengths =
        [0usize; HISTORY_SIZE];

    let mut history_count = 0usize;
    let mut history_position = 0usize;

    let mut shift = false;

    let mut cursor_visible = false;

    let mut last_blink_tick =
        interrupts::timer::ticks();

    shell::write_prompt(console);

    let (mut input_start_x, mut input_start_y) =
        console.cursor_position();

 loop {
    let current_tick = interrupts::timer::ticks();
    while let Some(delta) = input::read_mouse_wheel() {
        console.scroll_history(delta as isize);
    }

    {
        let mut scheduler = task::SCHEDULER.lock();

        scheduler.wake_sleeping_tasks(current_tick);
    }

        shell::run_scheduler_step();

        // ----------------------------------------------------
        // Cursor blinking
        // ----------------------------------------------------

        if current_tick.wrapping_sub(last_blink_tick)
            >= BLINK_INTERVAL_TICKS
        {
            last_blink_tick = current_tick;

            if cursor_visible {
                console.hide_cursor();
                cursor_visible = false;
            } else {
                console.show_cursor();
                cursor_visible = true;
            }
        }

        // ----------------------------------------------------
        // Keyboard input
        // ----------------------------------------------------

        let _ = input::poll_keyboard_hw();
        if let Some(scancode) =
            input::read_scancode()
        {
            if cursor_visible {
                console.hide_cursor();
                cursor_visible = false;
            }

            last_blink_tick =
                interrupts::timer::ticks();

            if scancode == 0x2A
                || scancode == 0x36
            {
                shift = true;
                continue;
            }

            if scancode == 0xAA
                || scancode == 0xB6
            {
                shift = false;
                continue;
            }

            if scancode == 0xE0
                || scancode & 0x80 != 0
            {
                continue;
            }

            // Arrow Up
            if scancode == 0x48 {
                if history_position > 0 {
                    history_position -= 1;

                    let saved_length =
                        history_lengths[history_position];

                    buffer = [0; BUFFER_SIZE];

                    buffer[..saved_length].copy_from_slice(
                        &history[history_position][..saved_length],
                    );

                    length = saved_length;
                    cursor_position = length;

                    redraw_line(
                        console,
                        input_start_x,
                        input_start_y,
                        &buffer,
                        length,
                        cursor_position,
                    );
                }

                continue;
            }

            // Arrow Down
            if scancode == 0x50 {
                if history_position < history_count {
                    history_position += 1;

                    buffer = [0; BUFFER_SIZE];

                    if history_position < history_count {
                        let saved_length =
                            history_lengths[history_position];

                        buffer[..saved_length].copy_from_slice(
                            &history[history_position][..saved_length],
                        );

                        length = saved_length;
                    } else {
                        length = 0;
                    }

                    cursor_position = length;

                    redraw_line(
                        console,
                        input_start_x,
                        input_start_y,
                        &buffer,
                        length,
                        cursor_position,
                    );
                }

                continue;
            }

            // Arrow Left
            if scancode == 0x4B {
                cursor_position =
                    cursor_position.saturating_sub(1);

                redraw_line(
                    console,
                    input_start_x,
                    input_start_y,
                    &buffer,
                    length,
                    cursor_position,
                );

                continue;
            }

            // Arrow Right
            if scancode == 0x4D {
                if cursor_position < length {
                    cursor_position += 1;
                }

                redraw_line(
                    console,
                    input_start_x,
                    input_start_y,
                    &buffer,
                    length,
                    cursor_position,
                );

                continue;
            }

            // Home
            if scancode == 0x47 {
                cursor_position = 0;

                redraw_line(
                    console,
                    input_start_x,
                    input_start_y,
                    &buffer,
                    length,
                    cursor_position,
                );

                continue;
            }

            // End
            if scancode == 0x4F {
                cursor_position = length;

                redraw_line(
                    console,
                    input_start_x,
                    input_start_y,
                    &buffer,
                    length,
                    cursor_position,
                );

                continue;
            }

            // Backspace
            if scancode == 0x0E {
                if cursor_position > 0 {
                    for i in cursor_position..length {
                        buffer[i - 1] = buffer[i];
                    }

                    length -= 1;
                    cursor_position -= 1;

                    buffer[length] = 0;

                    history_position = history_count;

                    redraw_line(
                        console,
                        input_start_x,
                        input_start_y,
                        &buffer,
                        length,
                        cursor_position,
                    );
                }

                continue;
            }

            // Delete
            if scancode == 0x53 {
                if cursor_position < length {
                    for i in cursor_position + 1..length {
                        buffer[i - 1] = buffer[i];
                    }

                    length -= 1;
                    buffer[length] = 0;

                    history_position = history_count;

                    redraw_line(
                        console,
                        input_start_x,
                        input_start_y,
                        &buffer,
                        length,
                        cursor_position,
                    );
                }

                continue;
            }

            // Enter
            if scancode == 0x1C {
                console.write("\n", WHITE);

                let command =
                    core::str::from_utf8(
                        &buffer[..length],
                    )
                    .unwrap_or("");

                save_history(
                    &buffer,
                    length,
                    &mut history,
                    &mut history_lengths,
                    &mut history_count,
                );

                history_position = history_count;

                shell::execute(
                    console,
                    command,
                    mapping,
                    allocator,
                );

                buffer = [0; BUFFER_SIZE];

                length = 0;
                cursor_position = 0;

                shell::write_prompt(console);

                let position =
                    console.cursor_position();

                input_start_x = position.0;
                input_start_y = position.1;

                continue;
            }

            // Character insertion
            if let Some(character) =
                input::scancode_to_char(
                    scancode,
                    shift,
                )
            {
                if character.is_ascii()
                    && !character.is_ascii_control()
                    && length < BUFFER_SIZE - 1
                {
                    for i in
                        (cursor_position..length).rev()
                    {
                        buffer[i + 1] = buffer[i];
                    }

                    buffer[cursor_position] =
                        character as u8;

                    length += 1;
                    cursor_position += 1;

                    history_position = history_count;

                    redraw_line(
                        console,
                        input_start_x,
                        input_start_y,
                        &buffer,
                        length,
                        cursor_position,
                    );
                }
            }
        }

        // ----------------------------------------------------
        // Halt CPU until next interrupt
        // ----------------------------------------------------

        unsafe {
            asm!(
                "sti",
                "hlt",
                options(nomem, nostack)
            );
        }
    }
}

// ============================================================
// COMMAND HISTORY
// ============================================================

fn save_history(
    buffer: &[u8; BUFFER_SIZE],
    length: usize,
    history: &mut [[u8; BUFFER_SIZE]],
    history_lengths: &mut [usize; HISTORY_SIZE],
    history_count: &mut usize,
) {
    if length == 0 {
        return;
    }

    let is_duplicate =
        *history_count > 0
        && history_lengths[*history_count - 1] == length
        && history[*history_count - 1][..length]
            == buffer[..length];

    if is_duplicate {
        return;
    }

    if *history_count == HISTORY_SIZE {
        for i in 1..HISTORY_SIZE {
            history[i - 1] = history[i];
            history_lengths[i - 1] =
                history_lengths[i];
        }

        *history_count -= 1;
    }

    history[*history_count] = [0; BUFFER_SIZE];

    history[*history_count][..length]
        .copy_from_slice(&buffer[..length]);

    history_lengths[*history_count] = length;

    *history_count += 1;
}

// ============================================================
// CONSOLE UTILITIES
// ============================================================

fn write_number(
    console: &mut Console<'_>,
    mut number: u64,
) {
    if number == 0 {
        console.write("0", WHITE);
        return;
    }

    let mut buffer = [0u8; 20];
    let mut index = buffer.len();

    while number > 0 {
        index -= 1;

        buffer[index] =
            b'0' + (number % 10) as u8;

        number /= 10;
    }

    let text =
        core::str::from_utf8(
            &buffer[index..],
        )
        .unwrap_or("0");

    console.write(text, WHITE);
}

fn redraw_line(
    console: &mut Console<'_>,
    start_x: usize,
    start_y: usize,
    buffer: &[u8; BUFFER_SIZE],
    length: usize,
    cursor_position: usize,
) {
    let text =
        core::str::from_utf8(
            &buffer[..length],
        )
        .unwrap_or("");

    console.redraw_input(
        start_x,
        start_y,
        text,
        cursor_position,
        WHITE,
    );
}

// ============================================================
// CPU HALT
// ============================================================

fn halt_forever() -> ! {
    loop {
        unsafe {
            asm!(
                "hlt",
                options(nomem, nostack)
            );
        }
    }
}

// ============================================================
// PANIC HANDLER
// ============================================================

#[panic_handler]
fn panic(info: &PanicInfo) -> ! {
    use core::fmt::Write;
    let mut l1 = serial::FixedBuf::<96>::new();
    let mut l2 = serial::FixedBuf::<96>::new();
    let _ = write!(l1, "{}", info.message());
    if let Some(loc) = info.location() {
        let _ = write!(l2, "{}:{}:{}", loc.file(), loc.line(), loc.column());
    } else {
        let _ = write!(l2, "unknown location");
    }
    serial::panic_banner("KERNEL PANIC", l1.as_str(), l2.as_str());
    halt_forever();
}

// ============================================================
// ALLOCATION ERROR HANDLER
// ============================================================

#[alloc_error_handler]
fn alloc_error(layout: Layout) -> ! {
    use core::fmt::Write;
    let mut l1 = serial::FixedBuf::<96>::new();
    let mut l2 = serial::FixedBuf::<96>::new();
    let _ = write!(l1, "size={} align={}", layout.size(), layout.align());
    let _ = write!(l2, "cpu={}", arch::percpu::current_cpu_id());
    serial::panic_banner("KERNEL ALLOC ERROR", l1.as_str(), l2.as_str());
    halt_forever();
}