//! The comptime evaluator (plans/M3.md item B) and legality inference
//! (item C). `legal.rs` lands with item C; `value.rs`/`interp.rs`/
//! `quota.rs` land alongside it from a parallel item B session — this
//! file only ever owns module wiring, per crate convention
//! (`sema/mod.rs`'s own doc comment).

pub mod legal;
