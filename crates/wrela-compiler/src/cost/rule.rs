//! Closed ISA op-class tags attached at `FnCtx::push` (plans/M18.md item C).
//! Table keys in `bench/wrela-cost-v1.toml` must match `as_str()` for every
//! variant in `ALL`.

/// ISA op-class for proxy-cycle ranking. Never parsed from mnemonics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CostRule {
    Alu,
    Load,
    Store,
    Branch,
    Call,
    Abort,
    AbortVal,
    MovWide,
    Mul,
    Sdiv,
    Udiv,
    Adrp,
    Barrier,
    System,
    Neon,
}

impl CostRule {
    /// Every rule the v1 table must provide a latency for.
    pub const ALL: &'static [CostRule] = &[
        CostRule::Alu,
        CostRule::Load,
        CostRule::Store,
        CostRule::Branch,
        CostRule::Call,
        CostRule::Abort,
        CostRule::AbortVal,
        CostRule::MovWide,
        CostRule::Mul,
        CostRule::Sdiv,
        CostRule::Udiv,
        CostRule::Adrp,
        CostRule::Barrier,
        CostRule::System,
        CostRule::Neon,
    ];

    /// TOML `[latency]` key / dump Term id.
    pub fn as_str(self) -> &'static str {
        match self {
            CostRule::Alu => "alu",
            CostRule::Load => "load",
            CostRule::Store => "store",
            CostRule::Branch => "branch",
            CostRule::Call => "call",
            CostRule::Abort => "abort",
            CostRule::AbortVal => "abort_val",
            CostRule::MovWide => "mov_wide",
            CostRule::Mul => "mul",
            CostRule::Sdiv => "sdiv",
            CostRule::Udiv => "udiv",
            CostRule::Adrp => "adrp",
            CostRule::Barrier => "barrier",
            CostRule::System => "system",
            CostRule::Neon => "neon",
        }
    }

    pub fn from_str(s: &str) -> Option<CostRule> {
        Some(match s {
            "alu" => CostRule::Alu,
            "load" => CostRule::Load,
            "store" => CostRule::Store,
            "branch" => CostRule::Branch,
            "call" => CostRule::Call,
            "abort" => CostRule::Abort,
            "abort_val" => CostRule::AbortVal,
            "mov_wide" => CostRule::MovWide,
            "mul" => CostRule::Mul,
            "sdiv" => CostRule::Sdiv,
            "udiv" => CostRule::Udiv,
            "adrp" => CostRule::Adrp,
            "barrier" => CostRule::Barrier,
            "system" => CostRule::System,
            "neon" => CostRule::Neon,
            _ => return None,
        })
    }
}

/// One machine word in the final asm stream, tagged at emit time
/// (plans/M18.md freeze 1303). Scoreboard uses `rule` + regs; asm dump
/// prints only `word` + `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmittedWord {
    pub word: u32,
    pub text: String,
    pub rule: CostRule,
    pub dst: Option<u8>,
    pub srcs: [u8; 4],
    pub src_len: u8,
}

impl EmittedWord {
    pub fn new(
        word: u32,
        text: String,
        rule: CostRule,
        dst: Option<u8>,
        srcs: &[u8],
    ) -> EmittedWord {
        let mut arr = [0u8; 4];
        let n = srcs.len().min(4);
        arr[..n].copy_from_slice(&srcs[..n]);
        EmittedWord {
            word,
            text,
            rule,
            dst,
            srcs: arr,
            src_len: n as u8,
        }
    }

    pub fn src_slice(&self) -> &[u8] {
        &self.srcs[..self.src_len as usize]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_as_str_round_trips() {
        for &r in CostRule::ALL {
            assert_eq!(CostRule::from_str(r.as_str()), Some(r));
        }
    }

    #[test]
    fn all_keys_unique() {
        let mut keys: Vec<&str> = CostRule::ALL.iter().map(|r| r.as_str()).collect();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), CostRule::ALL.len());
    }
}
