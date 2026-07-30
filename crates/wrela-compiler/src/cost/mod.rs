//! Proxy-cycle ranking (plans/M18.md; plans/M20.md item A reversed its
//! scope): emit-time CostRule tags + the A76 table + scoreboard. The
//! documented Cortex-A76 port map **is** the model, pinned to one profile
//! (`a76-pi5` — A76 / BCM2712 / Pi 5), with fidelity claimed to the
//! published record and never to silicon this project measured. Rank
//! direction only: still not host wall time (04 §5).

use std::path::{Path, PathBuf};

/// `crates/wrela-compiler` -> repo root (same shape as xtask's own `root()`).
/// Shared by this module's file-backed tables (`table.rs`, `workload.rs`).
pub(crate) fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("repo root from crates/wrela-compiler")
        .to_path_buf()
}

/// FNV-1a 64-bit — deterministic, std-only, fine for the table/workload
/// digests this module prints.
pub(crate) struct Fnv64(u64);

impl Fnv64 {
    pub(crate) fn new() -> Self {
        Fnv64(0xcbf29ce484222325)
    }

    pub(crate) fn write(&mut self, bytes: &[u8]) {
        let mut h = self.0;
        for &b in bytes {
            h ^= u64::from(b);
            h = h.wrapping_mul(0x100000001b3);
        }
        self.0 = h;
    }

    pub(crate) fn finish(self) -> u64 {
        self.0
    }
}

pub mod ab;
pub mod attr;
pub mod branch;
pub mod bridge;
pub mod compose;
pub mod crosscore;
pub mod dump;
pub mod footprint;
pub mod freq;
pub mod ghz;
pub mod mem;
pub mod owner;
pub mod rule;
pub mod score;
pub mod stage;
pub mod sweep;
pub mod table;
pub mod workload;

pub use ab::{CostOpts, rank_cmp, score_with_opts};
pub use attr::{CoreBucket, CoreCostReport, PlaceableTurn, attribute_cores};
pub use branch::{BlockCounts, BlockObs, BranchSummary, BranchTerms, FRONTEND_SWEEP_DIM};
pub use bridge::{BlockBridge, BridgedBlock, Resolved, make_key, split_key};
pub use compose::{
    BlockGrainMeasure, WorkloadAttach, attach_workloads, block_grain_fxs, method_grain_fxs,
    uncovered_charge,
};
pub use dump::{dump, dump_for_source};
pub use footprint::{CoreBudget, HotBlocks, PAGE_BYTES};
pub use freq::{BlockFreq, MethodFreq, sibling_block_freq_path, sibling_freq_path};
pub use ghz::{DEFAULT_GHZ, fmt_compact, ms_per_turn, parse_ghz, turns_per_sec};
pub use mem::{LineId, MemLevel, MemState, MemVerdict};
pub use owner::classify_owner;
pub use rule::{CostRule, EmittedWord, FlagEffect, MEM_SP_REG, MemClass, MemRef};
pub use score::{
    BranchBias, CostReport, CrossExtra, FnCost, basic_block_ranges, block_schedule_lengths,
    block_schedule_lengths_with_counts, score_program, score_program_at, score_program_at_with_hot,
};
pub use stage::{
    CostStageClosure, codegen_cost_stage, codegen_cost_stage_with_placement,
    load_cost_stage_closure, report_cost_stage_path, score_cost_stage_path,
};
pub use sweep::{SweepPoint, endpoint_corners, record_reads};
pub use table::{
    CostTable, CrossRow, EXPECTED_VERSION, End, LatRow, PROFILE_NAME, Row, SweepRow, Tier,
    default_table_path, load_default, load_from_path, parse, profile_ghz,
};
pub use workload::{FLAT_NAME, WorkloadSet, default_workloads_path};
