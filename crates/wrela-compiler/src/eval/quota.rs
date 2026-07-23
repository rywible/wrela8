//! Comptime evaluation quotas (plans/M3.md decision 6): finite step,
//! memory, and call-depth budgets. Fixed constants for revision 0.1 —
//! build profiles (and per-profile quotas) arrive at M4; there is no
//! wall clock anywhere in this file or its callers.
//!
//! Every evaluator loop iteration and function/method/init call costs
//! one step (`tick_step`); every `Value` a construction site
//! materializes charges its own weight against the memory budget
//! (`charge_memory`, `value::Value::weight`) — a plain running counter,
//! not a real allocator hook (decision 6: "a simple running counter is
//! fine — dumb and correct"). Exceeding either is a build error naming
//! the hottest stack (04-compiler.md §8); `MAX_CALL_DEPTH` is this
//! module's own addition, not in the plan's decision text verbatim, but
//! required for the same reason `bodies::MAX_GENERIC_DEPTH` exists: an
//! unbounded-recursion program must fail closed with a diagnostic
//! *before* it overflows this process's own native stack, which the
//! step quota alone cannot promise (a single step can itself recurse
//! arbitrarily deep, e.g. one `fn` calling itself with no loop in
//! sight).

/// One evaluation's total step budget — revision-0.1 constant. Kept
/// modest (rather than the "1,000,000-ish" figure a first cut might
/// reach for) so that a single quota-exhausting program stays cheap
/// enough to sit in the golden/bench corpus alongside everything else:
/// `xtask bench compiler`'s check lane locks lex+parse+`sema::check`
/// (now including const evaluation) over the *whole* golden corpus
/// against one combined threshold (`bench/thresholds.toml`), so a
/// worst-case quota-exceeding `const` initializer's own runtime has to
/// stay a small fraction of that budget, not dominate it. Real build
/// profiles (and their own, more generous quotas) arrive at M4.
pub const MAX_STEPS: u64 = 20_000;

/// One evaluation's total `Value`-weight budget (`value::Value::weight`
/// units) — revision-0.1 constant.
pub const MAX_MEMORY: u64 = 10_000_000;

/// The deepest live call chain a single evaluation may build — revision-
/// 0.1 constant, chosen to stay well inside this process's own native
/// stack while comfortably covering any reasonable recursive comptime
/// program (mirrors `sema::bodies::MAX_GENERIC_DEPTH`'s own role one
/// layer up the pipeline).
pub const MAX_CALL_DEPTH: usize = 1_000;

#[derive(Debug, Clone, Default)]
pub struct Quota {
    steps: u64,
    memory: u64,
}

impl Quota {
    pub fn new() -> Quota {
        Quota::default()
    }

    /// Charges one step (a loop iteration or a call); `Err` names the
    /// budget once it is exceeded.
    pub fn tick_step(&mut self) -> Result<(), String> {
        self.steps += 1;
        if self.steps > MAX_STEPS {
            Err(format!("step quota exceeded ({MAX_STEPS} steps)"))
        } else {
            Ok(())
        }
    }

    /// Charges `n` `Value`-weight units.
    pub fn charge_memory(&mut self, n: u64) -> Result<(), String> {
        self.memory += n;
        if self.memory > MAX_MEMORY {
            Err(format!("memory quota exceeded ({MAX_MEMORY} elements)"))
        } else {
            Ok(())
        }
    }
}
