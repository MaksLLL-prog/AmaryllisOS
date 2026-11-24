
pub mod manager;
pub mod process;
pub mod integration_tests;

pub use manager::{
    ProcessManager,
    ProcessManagerError,
    MAX_PROCESSES,
};

pub use process::{
    Process,
    ProcessState,
};

pub static PROCESS_MANAGER: spin::Mutex<ProcessManager> =
    spin::Mutex::new(ProcessManager::new());
