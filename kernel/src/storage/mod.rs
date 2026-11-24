pub mod ata;
pub mod block;
pub mod cache;
pub mod cli;
pub mod fat16;
pub mod fat32;
pub mod ide;
pub mod partition;
pub mod ramfs;
pub mod vfs;

use crate::console::Console;
use crate::hardware::IdeControllerDiscoveryList;

/// Initializes the storage subsystem using hardware controller descriptors discovered by PCI.
pub fn init(console: &mut Console<'_>, discovered: &IdeControllerDiscoveryList) {
    ide::register_discovered_controllers(console, discovered);
}
