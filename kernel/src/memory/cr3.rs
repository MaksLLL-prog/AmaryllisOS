
use x86_64::registers::control::{Cr3, Cr3Flags};
use x86_64::structures::paging::{PhysFrame, Size4KiB};
use x86_64::{PhysAddr, VirtAddr};

use super::vmm::PhysicalMemoryMapping;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cr3Error {
    UnalignedAddress,
    InvalidPhysicalAddress,
    InvalidTableAddress,
    MissingRequiredMapping,
}

#[derive(Debug, Clone, Copy)]
pub struct Cr3State {
    pml4_physical_address: u64,
    flags: Cr3Flags,
}

impl Cr3State {
    pub fn pml4_physical_address(&self) -> u64 {
        self.pml4_physical_address
    }
}

// ============================================================
// CURRENT CR3
// ============================================================

pub fn current_pml4() -> u64 {
    let (frame, _) = Cr3::read();
    frame.start_address().as_u64()
}

// ============================================================
// VALIDATE TARGET ADDRESS SPACE
// ============================================================

pub fn validate_address_space(
    mapping: &PhysicalMemoryMapping,
    pml4: u64,
    required_mappings: &[VirtAddr],
) -> Result<(), Cr3Error> {
    if pml4 % 4096 != 0 {
        return Err(Cr3Error::UnalignedAddress);
    }

    if mapping.translate_frame(pml4).is_none() {
        return Err(Cr3Error::InvalidTableAddress);
    }

    for address in required_mappings {
        if mapping
            .translate_virtual_address_in(pml4, *address)
            .is_none()
        {
            return Err(Cr3Error::MissingRequiredMapping);
        }
    }

    Ok(())
}

// ============================================================
// SWITCH ADDRESS SPACE
// ============================================================

/// Switches CR3 to another PML4.
///
/// # Safety
///
/// The caller must ensure that all memory required immediately
/// after the switch is mapped in the target address space.
/// This includes code, stack, global data, interrupt handlers,
/// descriptor tables and device mappings.
pub unsafe fn switch_to(
    mapping: &PhysicalMemoryMapping,
    target_pml4: u64,
    required_mappings: &[VirtAddr],
) -> Result<Cr3State, Cr3Error> {
    validate_address_space(
        mapping,
        target_pml4,
        required_mappings,
    )?;

    let physical_address =
        PhysAddr::try_new(target_pml4)
            .map_err(|_| Cr3Error::InvalidPhysicalAddress)?;

    let target_frame =
        PhysFrame::<Size4KiB>::from_start_address(
            physical_address,
        )
        .map_err(|_| Cr3Error::UnalignedAddress)?;

    let (previous_frame, previous_flags) = Cr3::read();

    let previous_state = Cr3State {
        pml4_physical_address: previous_frame
            .start_address()
            .as_u64(),
        flags: previous_flags,
    };

    unsafe {
        Cr3::write(
            target_frame,
            previous_flags,
        );
    }

    Ok(previous_state)
}

// ============================================================
// RESTORE PREVIOUS ADDRESS SPACE
// ============================================================

/// Restores a previously saved CR3 state.
///
/// # Safety
///
/// The saved PML4 must still exist and remain valid.
pub unsafe fn restore(
    state: Cr3State,
) -> Result<(), Cr3Error> {
    let physical_address =
        PhysAddr::try_new(state.pml4_physical_address)
            .map_err(|_| Cr3Error::InvalidPhysicalAddress)?;

    let frame =
        PhysFrame::<Size4KiB>::from_start_address(
            physical_address,
        )
        .map_err(|_| Cr3Error::UnalignedAddress)?;

    unsafe {
        Cr3::write(frame, state.flags);
    }

    Ok(())
}
