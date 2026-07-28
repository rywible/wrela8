//! Proxy-cycle ranking (plans/M18.md): emit-time CostRule tags + ISA table
//! + scoreboard. Differential rank only — not host wall time, not A76 SOG.

pub mod rule;

pub use rule::{CostRule, EmittedWord};
