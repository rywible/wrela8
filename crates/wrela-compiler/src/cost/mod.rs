//! Proxy-cycle ranking (plans/M18.md): emit-time CostRule tags + ISA table
//! + scoreboard. Differential rank only — not host wall time, not A76 SOG.

pub mod rule;
pub mod table;

pub use rule::{CostRule, EmittedWord};
pub use table::{
    CostTable, DEFAULT_ISSUE_WIDTH, EXPECTED_VERSION, default_table_path, load_default,
    load_from_path, parse,
};
