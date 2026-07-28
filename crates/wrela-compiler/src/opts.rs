//! Compile modes and the fixed in-code release opt list
//! (plans/M19.md item B / decisions 1420–1423).
//!
//! `apply_mode` is the single front door for all opt TLS knobs. The
//! release order is a dumb `const` slice — not a plugin registry
//! (freeze 1402 / 1406). Edit + re-rank here; nowhere else.

/// Compile mode: `Dev` leaves opts off; `Release` runs `RELEASE_OPTS`
/// in fixed order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompileMode {
    Dev,
    Release,
}

/// Named opts that `apply_mode(Release)` may enable. Two ids today
/// (decision 1421): lower-side bounds elision and codegen narrow
/// immediates.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OptId {
    BoundsElide,
    NarrowImm,
}

/// Fixed release order. Add opts here — nowhere else.
/// Order is part of the product (decision 1423): BoundsElide then
/// NarrowImm.
pub const RELEASE_OPTS: &[OptId] = &[OptId::BoundsElide, OptId::NarrowImm];

/// Single front door for all opt TLS knobs (decision 1422).
pub fn apply_mode(mode: CompileMode) {
    let on = matches!(mode, CompileMode::Release);
    crate::lower::set_bounds_elide(on && RELEASE_OPTS.contains(&OptId::BoundsElide));
    crate::codegen::set_narrow_imm(on && RELEASE_OPTS.contains(&OptId::NarrowImm));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::codegen::narrow_imm;
    use crate::lower::bounds_elide;

    #[test]
    fn dev_disables_both_opts() {
        apply_mode(CompileMode::Release);
        assert!(bounds_elide());
        assert!(narrow_imm());

        apply_mode(CompileMode::Dev);
        assert!(!bounds_elide());
        assert!(!narrow_imm());
    }

    #[test]
    fn release_enables_both_opts() {
        apply_mode(CompileMode::Dev);
        assert!(!bounds_elide());
        assert!(!narrow_imm());

        apply_mode(CompileMode::Release);
        assert!(bounds_elide());
        assert!(narrow_imm());
    }

    #[test]
    fn release_opts_order_is_bounds_elide_then_narrow_imm() {
        assert_eq!(
            RELEASE_OPTS,
            &[OptId::BoundsElide, OptId::NarrowImm]
        );
    }
}
