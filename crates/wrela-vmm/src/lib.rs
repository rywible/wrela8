//! The wrela VMM: the product implementation of the wrela machine
//! (docs/language/06-machine.md). Firecracker-class, userspace, two host
//! backends behind one internal seam:
//!
//!   - `kvm` (Linux): the Raspberry Pi 5 flagship host;
//!   - `hvf` (macOS / Hypervisor.framework): development and Mac hosts.
//!
//! The VMM consumes the compiler's image report as its entire
//! configuration: devices, queues, and shared-memory windows are
//! preconfigured before the guest runs — there is no discovery. It is also
//! the recorder/replayer for the determinism oracle (06 §8) and the
//! digest-pinned runner for image tests (`cargo xtask golden`).
//!
//! Everything here fails closed until implemented: no stub pretends to run
//! a guest.

use wrela_machine::MACHINE_REVISION;

#[derive(Debug)]
pub enum VmmError {
    /// The requested capability is not implemented yet. Fail closed.
    Unsupported(&'static str),
    /// Image was built for a different machine revision.
    MachineRevisionMismatch { image: u32, vmm: u32 },
}

/// Parsed image + report pair. Loading validates digests and the machine
/// revision before anything else.
pub struct LoadedImage {
    pub machine_revision: u32,
}

pub fn load_image(_image_bytes: &[u8], _report_bytes: &[u8]) -> Result<LoadedImage, VmmError> {
    Err(VmmError::Unsupported("image loading"))
}

pub fn run(image: &LoadedImage) -> Result<(), VmmError> {
    if image.machine_revision != MACHINE_REVISION {
        return Err(VmmError::MachineRevisionMismatch {
            image: image.machine_revision,
            vmm: MACHINE_REVISION,
        });
    }
    Err(VmmError::Unsupported("guest execution"))
}

#[cfg(target_os = "linux")]
pub mod kvm {
    //! Linux/KVM backend. May build on the rust-vmm crates.
}

#[cfg(target_os = "macos")]
pub mod hvf {
    //! macOS Hypervisor.framework backend. Thinner than KVM: the
    //! paravirtual interrupt design (06 §4) means no userspace GIC model
    //! is ever needed here.
}

pub mod devices {
    //! Device models for the closed machine v1 set (06 §6). Each model is
    //! the VMM half of the corresponding stdlib driver; the conformance
    //! suite exercises both sides against the same contract.
}

pub mod record {
    //! Recorder/replayer for the determinism boundary (06 §8).
}
