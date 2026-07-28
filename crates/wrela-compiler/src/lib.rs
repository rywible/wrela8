//! The wrela compiler: a batch pipeline of dumpable artifacts.
//!
//! Normative source: docs/language/. The pipeline shape is fixed by
//! 04-compiler.md §8: each stage is a function from one serializable
//! artifact to the next, every artifact has a stable text dump, and the
//! golden suite (tests/golden/) pins dumps per stage. Implementations are
//! disposable; the dumps and the docs are not.
//!
//! Stages (each becomes a module as it lands; stubs stay honest — they
//! fail closed rather than pretend):
//!
//!   source --lex/parse--> syntax tree      [syntax]
//!   syntax --resolve/check--> SemanticWir  [sema]     (typed tree lands
//!                                                      at plans/M3.md
//!                                                      item A; every
//!                                                      other pass per
//!                                                      plans/M2.md)
//!   comptime evaluation                    [eval]     (item B: the
//!                                                      typed-tree
//!                                                      interpreter;
//!                                                      legality
//!                                                      inference is
//!                                                      item C)
//!   SemanticWir --lower--> FlowWir         [flow]     (unimplemented)
//!   FlowWir --backend--> MachineWir/image  [backend]  (unimplemented)
//!
//! The `eval` module doubles as the reference interpreter for the
//! differential oracle (`cargo xtask diff-eval`): compiled code must agree
//! with it on every pure program.

pub mod census;
pub mod codegen;
pub mod cost;
pub mod emitted_a64_census;
pub mod encode;
pub mod eval;
pub mod flowwir;
pub mod flowwir_lower;
pub mod guest_fn_key_census;
pub mod internal_error_census;
pub mod layout;
pub mod loader;
pub mod lower;
pub mod lower_queue;
pub mod lower_shared;
pub mod mwir;
pub mod placed_static_census;
pub mod placement;
pub mod report;
pub mod rtconfig;
pub mod sema;
pub mod syntax;
pub mod virtqueue;
