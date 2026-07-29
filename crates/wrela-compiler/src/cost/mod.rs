//! Proxy-cycle ranking (plans/M18.md): emit-time CostRule tags + ISA table
//! + scoreboard. Differential rank only — not host wall time, not A76 SOG.

pub mod ab;
pub mod attr;
pub mod compose;
pub mod dump;
pub mod freq;
pub mod ghz;
pub mod owner;
pub mod rule;
pub mod score;
pub mod stage;
pub mod table;
pub mod workload;

pub use ab::{CostOpts, rank_cmp, score_with_opts};
pub use attr::{CoreBucket, CoreCostReport, PlaceableTurn, attribute_cores};
pub use compose::{WorkloadAttach, attach_workloads, method_grain_fxs};
pub use dump::{dump, dump_for_source};
pub use freq::{MethodFreq, sibling_freq_path};
pub use ghz::{DEFAULT_GHZ, fmt_compact, ms_per_turn, parse_ghz, turns_per_sec};
pub use owner::classify_owner;
pub use rule::{CostRule, EmittedWord, FlagEffect, MEM_SP_REG, MemClass, MemRef};
pub use score::{
    CostReport, FnCost, basic_block_ranges, block_schedule_lengths, score_program,
};
pub use stage::{
    CostStageClosure, codegen_cost_stage, load_cost_stage_closure, score_cost_stage_path,
};
pub use table::{
    CostTable, EXPECTED_VERSION, MemCosts, default_table_path, load_default, load_from_path, parse,
};
pub use workload::{FLAT_NAME, WorkloadSet, default_workloads_path};
