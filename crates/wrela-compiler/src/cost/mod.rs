//! Proxy-cycle ranking (plans/M18.md): emit-time CostRule tags + ISA table
//! + scoreboard. Differential rank only — not host wall time, not A76 SOG.

pub mod ab;
pub mod attr;
pub mod dump;
pub mod ghz;
pub mod owner;
pub mod rule;
pub mod score;
pub mod stage;
pub mod table;

pub use ab::{CostOpts, rank_cmp, score_with_opts};
pub use attr::{CoreBucket, CoreCostReport, PlaceableTurn, attribute_cores};
pub use dump::dump;
pub use ghz::{DEFAULT_GHZ, fmt_compact, ms_per_turn, parse_ghz, turns_per_sec};
pub use owner::classify_owner;
pub use rule::{CostRule, EmittedWord};
pub use score::{CostReport, FnCost, score_program};
pub use stage::{
    CostStageClosure, codegen_cost_stage, load_cost_stage_closure, score_cost_stage_path,
};
pub use table::{
    CostTable, DEFAULT_ISSUE_WIDTH, EXPECTED_VERSION, default_table_path, load_default,
    load_from_path, parse,
};
