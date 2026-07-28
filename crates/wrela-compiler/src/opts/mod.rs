//! Compile modes and the fixed in-code release opt list
//! (plans/M19.md item B / decisions 1420–1423).
//!
//! `apply_mode` is the single front door for product modes. `apply_opts`
//! sets the same TLS knobs from an explicit list so item E can A/B a
//! candidate order offline (decision 1452). The release order is a dumb
//! `const` slice — not a plugin registry (freeze 1402 / 1406). Edit +
//! re-rank here; nowhere else.

pub mod win;

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

/// Enable exactly the named opts (decision 1452). Product modes go
/// through [`apply_mode`]; tests and candidate A/B use this directly.
pub fn apply_opts(opts: &[OptId]) {
    crate::lower::set_bounds_elide(opts.contains(&OptId::BoundsElide));
    crate::codegen::set_narrow_imm(opts.contains(&OptId::NarrowImm));
}

/// Single front door for product-mode TLS knobs (decision 1422).
pub fn apply_mode(mode: CompileMode) {
    match mode {
        CompileMode::Dev => apply_opts(&[]),
        CompileMode::Release => apply_opts(RELEASE_OPTS),
    }
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

    #[test]
    fn apply_opts_enables_only_named() {
        apply_opts(&[OptId::NarrowImm]);
        assert!(!bounds_elide());
        assert!(narrow_imm());

        apply_opts(&[OptId::BoundsElide]);
        assert!(bounds_elide());
        assert!(!narrow_imm());

        apply_mode(CompileMode::Release);
    }
}
