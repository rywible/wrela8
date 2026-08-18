//! Public boot entry points. Host selection is compile-time and lives in `host`.

use std::path::Path;

use crate::{BootOutcome, VmmError, record};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranslationMode {
    SealedStage1,
    DiagnosticMmuOff,
}

pub fn boot_image(report_path: &Path, image_path: &Path) -> Result<BootOutcome, VmmError> {
    boot_image_with_display(
        report_path,
        image_path,
        crate::display::DisplayBackendSelection::Headless,
    )
}

pub fn boot_image_with_display(
    report_path: &Path,
    image_path: &Path,
    display: crate::display::DisplayBackendSelection,
) -> Result<BootOutcome, VmmError> {
    crate::engine::boot_image::<crate::host::SelectedBackend>(
        report_path,
        image_path,
        None,
        display,
        None,
        TranslationMode::SealedStage1,
        false,
    )
    .map(|(outcome, _)| outcome)
}

pub fn boot_image_diagnostic_mmu_off(
    report_path: &Path,
    image_path: &Path,
) -> Result<BootOutcome, VmmError> {
    crate::engine::boot_image::<crate::host::SelectedBackend>(
        report_path,
        image_path,
        None,
        crate::display::DisplayBackendSelection::Headless,
        None,
        TranslationMode::DiagnosticMmuOff,
        false,
    )
    .map(|(outcome, _)| outcome)
}

pub fn boot_image_with_guest_counters(
    report_path: &Path,
    image_path: &Path,
    display: crate::display::DisplayBackendSelection,
) -> Result<BootOutcome, VmmError> {
    crate::engine::boot_image::<crate::host::SelectedBackend>(
        report_path,
        image_path,
        None,
        display,
        None,
        TranslationMode::SealedStage1,
        true,
    )
    .map(|(outcome, _)| outcome)
}

pub(crate) fn boot_image_core(
    report_path: &Path,
    image_path: &Path,
    replay: Option<Vec<record::ChoiceEntry>>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    crate::engine::boot_image::<crate::host::SelectedBackend>(
        report_path,
        image_path,
        replay,
        crate::display::DisplayBackendSelection::Headless,
        None,
        TranslationMode::SealedStage1,
        false,
    )
}

#[cfg(test)]
pub(crate) fn boot_image_core_with_delayed_raise(
    report_path: &Path,
    image_path: &Path,
    replay: Option<Vec<record::ChoiceEntry>>,
    delayed_raise: Option<(std::time::Duration, u64)>,
) -> Result<(BootOutcome, Vec<record::Divergence>), VmmError> {
    crate::engine::boot_image::<crate::host::SelectedBackend>(
        report_path,
        image_path,
        replay,
        crate::display::DisplayBackendSelection::Headless,
        delayed_raise,
        TranslationMode::SealedStage1,
        false,
    )
}
