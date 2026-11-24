use crate::console::Console;
use crate::memory::vmm::PhysicalMemoryMapping;
use crate::memory::PhysicalFrameAllocator;

pub fn cmd_gui(
    console: &mut Console<'_>,
    _arguments: &str,
    mapping: Option<&PhysicalMemoryMapping>,
    allocator: &mut PhysicalFrameAllocator<'_>,
) {
    crate::graphics::wm::run_desktop(console, mapping, allocator);
}
