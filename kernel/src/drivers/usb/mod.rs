pub mod hid;
pub mod umass;
pub mod xhci;

use crate::hardware::PciXhciDescriptor;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;

pub use umass::UsbMassBlockDevice;
pub use xhci::XhciSnapshot;

pub fn init(
    desc: &PciXhciDescriptor,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) -> Result<XhciSnapshot, &'static str> {
    xhci::init(desc, mapping, allocator)
}

pub fn is_ready() -> bool {
    xhci::is_ready()
}

pub fn snapshot() -> XhciSnapshot {
    xhci::snapshot()
}

pub fn poll_hid() -> usize {
    xhci::poll_hid()
}

pub fn irq_top_half() -> bool {
    xhci::irq_top_half()
}

pub fn umass_snapshot() -> umass::UsbMassSnapshot {
    umass::snapshot()
}

pub fn has_mass_storage() -> bool {
    umass::is_ready()
}
