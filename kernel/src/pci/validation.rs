
use super::resources::{self, PciResource};
use super::PciBarKind;

#[derive(Clone, Copy)]
pub struct ValidationStats {
    pub checked_resources: usize,
    pub invalid_resources: usize,
    pub unknown_sizes: usize,
    pub invalid_sizes: usize,
    pub misaligned_addresses: usize,
    pub address_overflows: usize,
    pub out_of_range: usize,
    pub overlapping_pairs: usize,
}

impl ValidationStats {
    pub const EMPTY: Self = Self {
        checked_resources: 0,
        invalid_resources: 0,
        unknown_sizes: 0,
        invalid_sizes: 0,
        misaligned_addresses: 0,
        address_overflows: 0,
        out_of_range: 0,
        overlapping_pairs: 0,
    };
}

// ============================================================
// ADDRESS SPACE
// ============================================================

fn same_address_space(a: &PciResource, b: &PciResource) -> bool {
    let a_io = a.is_io();
    let b_io = b.is_io();

    // I/O and memory resources use different address spaces.
    a_io == b_io
}

// ============================================================
// RESOURCE VALIDATION
// ============================================================

fn validate_resource(
    resource: &PciResource,
    stats: &mut ValidationStats,
) -> bool {
    let mut invalid = false;

    let bar = &resource.bar;

    if !bar.valid {
        return true;
    }

    if !bar.size_known {
        stats.unknown_sizes += 1;
        return false;
    }

    if bar.size == 0 || !bar.size.is_power_of_two() {
        stats.invalid_sizes += 1;
        invalid = true;
    }

    if bar.size != 0 && bar.size.is_power_of_two() {
        if bar.base & (bar.size - 1) != 0 {
            stats.misaligned_addresses += 1;
            invalid = true;
        }
    }

    let end = match resource.end_address() {
        Some(end) => end,
        None => {
            stats.address_overflows += 1;
            return true;
        }
    };

    match bar.kind {
        PciBarKind::Io => {
            // PCI I/O BARs use a 32-bit address space.
            if end > u32::MAX as u64 {
                stats.out_of_range += 1;
                invalid = true;
            }
        }

        PciBarKind::Memory32 => {
            if end > u32::MAX as u64 {
                stats.out_of_range += 1;
                invalid = true;
            }
        }

        PciBarKind::MemoryBelow1M => {
            if end > 0x000F_FFFF {
                stats.out_of_range += 1;
                invalid = true;
            }
        }

        PciBarKind::Memory64 => {
            // The u64 end address has already been checked.
        }

        PciBarKind::Reserved => {
            stats.invalid_sizes += 1;
            invalid = true;
        }
    }

    invalid
}

// ============================================================
// OVERLAP DETECTION
// ============================================================

fn resources_overlap(
    a: &PciResource,
    b: &PciResource,
) -> bool {
    if !a.bar.valid || !b.bar.valid {
        return false;
    }

    if !a.bar.size_known || !b.bar.size_known {
        return false;
    }

    if a.bar.size == 0 || b.bar.size == 0 {
        return false;
    }

    if !same_address_space(a, b) {
        return false;
    }

    let a_end = match a.end_address() {
        Some(value) => value,
        None => return false,
    };

    let b_end = match b.end_address() {
        Some(value) => value,
        None => return false,
    };

    a.bar.base <= b_end && b.bar.base <= a_end
}

// ============================================================
// PUBLIC VALIDATION API
// ============================================================

/// Validates all resources currently stored in the BAR registry.
///
/// This function only reads registry data.
/// It does not modify PCI configuration space or BAR registers.
pub fn validate() -> ValidationStats {
    let mut stats = ValidationStats::EMPTY;

    let total = resources::count();

    // Validate each resource individually.
    for index in 0..total {
        let resource = match resources::get(index) {
            Some(resource) => resource,
            None => continue,
        };

        stats.checked_resources += 1;

        if validate_resource(&resource, &mut stats) {
            stats.invalid_resources += 1;
        }
    }

    // Check every pair of resources for address overlap.
    for first_index in 0..total {
        let first = match resources::get(first_index) {
            Some(resource) => resource,
            None => continue,
        };

        for second_index in (first_index + 1)..total {
            let second = match resources::get(second_index) {
                Some(resource) => resource,
                None => continue,
            };

            if resources_overlap(&first, &second) {
                stats.overlapping_pairs += 1;
            }
        }
    }

    stats
}
