//! The wrela machine contract, as Rust types and constants.
//!
//! Normative source: docs/language/06-machine.md. This crate is shared by
//! the compiler (which emits images for the machine) and the VMM (which
//! implements it). If a value here disagrees with the doc, the doc wins and
//! this crate is wrong.

/// Machine contract revision. The compiler seals this into the build
/// identity; the VMM refuses an image built for another revision.
pub const MACHINE_REVISION: u32 = 1;

/// Always four vCPUs. Hosts with more cores run VMM threads on the surplus.
pub const VCPUS: usize = 4;

/// Guest-physical memory layout (06-machine.md §2). Flagship profile.
pub mod layout {
    /// Total guest DRAM for the flagship profile.
    pub const DRAM_SIZE: u64 = 1 << 30; // 1 GiB
    /// Guest-physical DRAM base.
    pub const DRAM_BASE: u64 = 0x4000_0000;
    /// The machine-info page the VMM fills and `x0` points at during boot.
    pub const MACHINE_INFO_BASE: u64 = 0x4000_0000;
    /// Sealed image load base; vCPU 0 starts at the image entry here.
    pub const IMAGE_BASE: u64 = 0x4010_0000;
    /// Base of the per-core shared-memory pages for paravirtual vectors
    /// and doorbells (06-machine.md §4–§5).
    pub const PV_PAGES_BASE: u64 = 0x4001_0000;
}

/// The closed device set of machine v1 (06-machine.md §6). There is no
/// hotplug; a new device is a machine revision.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Device {
    Blk,
    Net,
    Input,
    Console,
    Entropy,
    Sound,
    Display,
    Clock,
}

impl Device {
    pub const ALL: [Device; 8] = [
        Device::Blk,
        Device::Net,
        Device::Input,
        Device::Console,
        Device::Entropy,
        Device::Sound,
        Device::Display,
        Device::Clock,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Device::Blk => "blk",
            Device::Net => "net",
            Device::Input => "input",
            Device::Console => "console",
            Device::Entropy => "entropy",
            Device::Sound => "sound",
            Device::Display => "display",
            Device::Clock => "clock",
        }
    }
}

/// Flagship display mode (06-machine.md §7). 4K is a stretch profile.
pub mod display {
    pub const WIDTH: u32 = 1920;
    pub const HEIGHT: u32 = 1080;
    pub const REFRESH_HZ: u32 = 60;
    pub const BYTES_PER_PIXEL: u32 = 4;
}
