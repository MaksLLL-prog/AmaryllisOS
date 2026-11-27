use alloc::boxed::Box;
use alloc::vec::Vec;
use core::ptr::addr_of;
use lazy_static::lazy_static;
use spin::Once;
use x86_64::instructions::segmentation::{Segment, CS, DS, ES, FS, GS, SS};
use x86_64::instructions::tables::load_tss;
use x86_64::structures::gdt::{
    Descriptor,
    GlobalDescriptorTable,
    SegmentSelector,
};
use x86_64::structures::tss::TaskStateSegment;
use x86_64::VirtAddr;

use super::percpu::MAX_SMP_CPUS;

pub const DOUBLE_FAULT_IST_INDEX: u16 = 0;

const DOUBLE_FAULT_STACK_SIZE: usize = 64 * 1024;
const RING0_TRAP_STACK_SIZE: usize = 256 * 1024;
const AP_DF_STACK_SIZE: usize = 32 * 1024;
const AP_RING0_STACK_SIZE: usize = 128 * 1024;

#[repr(align(16))]
struct AlignedStack<const N: usize>([u8; N]);

static mut DOUBLE_FAULT_STACK: AlignedStack<DOUBLE_FAULT_STACK_SIZE> =
    AlignedStack([0; DOUBLE_FAULT_STACK_SIZE]);

static mut RING0_TRAP_STACK: AlignedStack<RING0_TRAP_STACK_SIZE> =
    AlignedStack([0; RING0_TRAP_STACK_SIZE]);

#[derive(Debug, Clone, Copy)]
pub struct Selectors {
    pub kernel_code: SegmentSelector,
    pub kernel_data: SegmentSelector,
    pub user_data: SegmentSelector,
    pub user_code: SegmentSelector,
    pub tss: SegmentSelector,
}

lazy_static! {
    static ref TSS: TaskStateSegment = {
        let mut tss = TaskStateSegment::new();

        // Privilege level 0 stack (used by hardware when transitioning Ring 3 -> Ring 0 on interrupts/exceptions)
        let ring0_top = VirtAddr::from_ptr(addr_of!(RING0_TRAP_STACK))
            + RING0_TRAP_STACK_SIZE as u64;
        tss.privilege_stack_table[0] = ring0_top;

        // IST stack 0 dedicated to Double Fault (#DF)
        let df_top = VirtAddr::from_ptr(addr_of!(DOUBLE_FAULT_STACK))
            + DOUBLE_FAULT_STACK_SIZE as u64;
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = df_top;

        tss
    };

    static ref GDT: (GlobalDescriptorTable, Selectors) = {
        let mut gdt = GlobalDescriptorTable::new();

        // Selector ordering MUST satisfy x86_64 SYSCALL / SYSRET MSR (IA32_STAR) rules:
        // - Kernel Code (0x08), immediately followed by Kernel Data (0x10)
        // - User Data   (0x1B), immediately followed by User Code   (0x23)
        // Because SYSRET loads SS = STAR[63:48] + 8 and CS = STAR[63:48] + 16!
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let tss = gdt.append(Descriptor::tss_segment(&TSS));

        (
            gdt,
            Selectors {
                kernel_code,
                kernel_data,
                user_data,
                user_code,
                tss,
            },
        )
    };
}

struct ApGdtSlot {
    gdt: &'static GlobalDescriptorTable,
    selectors: Selectors,
}

unsafe impl Sync for ApGdtSlot {}
unsafe impl Send for ApGdtSlot {}

static AP_GDT_SLOTS: Once<Vec<ApGdtSlot>> = Once::new();

fn build_ap_tables() -> Vec<ApGdtSlot> {
    let ap_count = MAX_SMP_CPUS.saturating_sub(1);
    let mut slots = Vec::with_capacity(ap_count);

    for _ in 0..ap_count {
        let df_stack: &'static [u8] = Box::leak(alloc::vec![0u8; AP_DF_STACK_SIZE].into_boxed_slice());
        let r0_stack: &'static [u8] = Box::leak(alloc::vec![0u8; AP_RING0_STACK_SIZE].into_boxed_slice());

        let mut tss = TaskStateSegment::new();
        let r0_top = ((r0_stack.as_ptr() as u64) + AP_RING0_STACK_SIZE as u64) & !0xF;
        let df_top = ((df_stack.as_ptr() as u64) + AP_DF_STACK_SIZE as u64) & !0xF;
        tss.privilege_stack_table[0] = VirtAddr::new(r0_top);
        tss.interrupt_stack_table[DOUBLE_FAULT_IST_INDEX as usize] = VirtAddr::new(df_top);

        let tss_static: &'static TaskStateSegment = Box::leak(Box::new(tss));

        let mut gdt = GlobalDescriptorTable::new();
        let kernel_code = gdt.append(Descriptor::kernel_code_segment());
        let kernel_data = gdt.append(Descriptor::kernel_data_segment());
        let user_data = gdt.append(Descriptor::user_data_segment());
        let user_code = gdt.append(Descriptor::user_code_segment());
        let tss_sel = gdt.append(Descriptor::tss_segment(tss_static));
        let gdt_static: &'static GlobalDescriptorTable = Box::leak(Box::new(gdt));

        slots.push(ApGdtSlot {
            gdt: gdt_static,
            selectors: Selectors {
                kernel_code,
                kernel_data,
                user_data,
                user_code,
                tss: tss_sel,
            },
        });
    }

    slots
}

/// Allocates dedicated per-CPU GDT, TSS, Ring 0 trap stack, and #DF IST stack for every AP
/// on the BSP before SIPI brings secondary CPUs online.
pub fn prepare_ap_descriptors() {
    let _ = AP_GDT_SLOTS.call_once(build_ap_tables);
}

/// Loads the unique GDT and non-busy TSS assigned to secondary core `cpu_id` (`1..MAX_SMP_CPUS`).
pub unsafe fn init_ap(cpu_id: usize) {
    let Some(slots) = AP_GDT_SLOTS.get() else {
        return;
    };
    let Some(idx) = cpu_id.checked_sub(1) else {
        return;
    };
    let Some(slot) = slots.get(idx) else {
        return;
    };

    slot.gdt.load();
    unsafe {
        CS::set_reg(slot.selectors.kernel_code);
        SS::set_reg(slot.selectors.kernel_data);
        DS::set_reg(slot.selectors.kernel_data);
        ES::set_reg(slot.selectors.kernel_data);
        FS::set_reg(SegmentSelector(0));
        GS::set_reg(SegmentSelector(0));
        load_tss(slot.selectors.tss);
    }
}

pub fn init() {
    GDT.0.load();

    unsafe {
        CS::set_reg(GDT.1.kernel_code);
        SS::set_reg(GDT.1.kernel_data);
        DS::set_reg(GDT.1.kernel_data);
        ES::set_reg(GDT.1.kernel_data);
        FS::set_reg(SegmentSelector(0));
        GS::set_reg(SegmentSelector(0));
        load_tss(GDT.1.tss);
    }
}

pub fn selectors() -> Selectors {
    GDT.1
}

pub fn ring0_trap_stack_top() -> u64 {
    TSS.privilege_stack_table[0].as_u64()
}
