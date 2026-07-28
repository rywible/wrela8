//! Proxy-cycle ranking (plans/M18.md): emit-time CostRule tags + ISA table
//! + scoreboard. Differential rank only — not host wall time, not A76 SOG.

pub mod ab;
pub mod dump;
pub mod owner;
pub mod rule;
pub mod score;
pub mod table;

pub use ab::{CostOpts, rank_cmp, score_with_opts};
pub use dump::dump;
pub use owner::classify_owner;
pub use rule::{CostRule, EmittedWord};
pub use score::{CostReport, FnCost, score_program};
pub use table::{
    CostTable, DEFAULT_ISSUE_WIDTH, EXPECTED_VERSION, default_table_path, load_default,
    load_from_path, parse,
};
