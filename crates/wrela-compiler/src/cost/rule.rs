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

/// Memory class for load/store MemRef tags (cost hard-cut item B).
/// Stack = proven SP-relative; Cold = everything else that was tagged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum MemClass {
    Stack,
    Cold,
}

/// AArch64 load/store encoding of SP (not XZR) — the only Stack base.
pub const MEM_SP_REG: u8 = 31;

/// Proven or unique memory identity for scoreboard reuse (item C).
/// Missing `EmittedWord::mem` is scored as a cold miss later; Adrp never
/// carries a MemRef.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MemRef {
    pub class: MemClass,
    /// Stack: frame byte offset from SP. Cold stable: packed base+imm.
    /// Cold unique: high bit set + per-push sequence.
    pub key: u64,
}

impl MemRef {
    pub fn stack(offset: u64) -> MemRef {
        MemRef {
            class: MemClass::Stack,
            key: offset,
        }
    }

    /// Stable Cold key for a proven `[base_reg, #imm]` (base ≠ SP).
    pub fn cold_stable(base_reg: u8, imm: u64) -> MemRef {
        MemRef {
            class: MemClass::Cold,
            key: ((base_reg as u64) << 48) | (imm & 0x0000_FFFF_FFFF_FFFF),
        }
    }

    /// Unique Cold key when the address is not a proven base+imm.
    pub fn cold_unique(seq: u64) -> MemRef {
        MemRef {
            class: MemClass::Cold,
            key: (1u64 << 63) | (seq & !(1u64 << 63)),
        }
    }

    /// Classify a proven `[base_reg, #imm]`: SP → Stack; else Cold stable.
    pub fn for_base_imm(base_reg: u8, imm: u64) -> MemRef {
        if base_reg == MEM_SP_REG {
            MemRef::stack(imm)
        } else {
            MemRef::cold_stable(base_reg, imm)
        }
    }
}

/// NZCV flag side-effect declared at emit (integrity item B). Never
/// inferred from mnemonics (freeze 1303).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlagEffect {
    #[default]
    None,
    /// Writes NZCV (cmp / adds / subs / …).
    Write,
    /// Reads NZCV (b.cond / cset / …).
    Read,
}

impl FlagEffect {
    pub fn writes(self) -> bool {
        matches!(self, FlagEffect::Write)
    }

    pub fn reads(self) -> bool {
        matches!(self, FlagEffect::Read)
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
    /// Load/Store memory identity when tagged at emit; `None` for Adrp
    /// and untagged sites (scorer treats missing as cold miss).
    pub mem: Option<MemRef>,
    /// NZCV read/write at emit (integrity item B).
    pub flags: FlagEffect,
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
            mem: None,
            flags: FlagEffect::None,
        }
    }

    pub fn with_mem(mut self, mem: MemRef) -> EmittedWord {
        self.mem = Some(mem);
        self
    }

    pub fn with_flags(mut self, flags: FlagEffect) -> EmittedWord {
        self.flags = flags;
        self
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

    #[test]
    fn sp_base_imm_is_stack() {
        let m = MemRef::for_base_imm(MEM_SP_REG, 24);
        assert_eq!(m.class, MemClass::Stack);
        assert_eq!(m.key, 24);
    }

    #[test]
    fn non_sp_base_imm_is_cold_stable() {
        let m = MemRef::for_base_imm(28, 16);
        assert_eq!(m.class, MemClass::Cold);
        assert_eq!(m, MemRef::cold_stable(28, 16));
        assert_eq!(m.key & (1u64 << 63), 0);
    }

    #[test]
    fn unknown_cold_unique_sets_high_bit_and_differs() {
        let a = MemRef::cold_unique(0);
        let b = MemRef::cold_unique(1);
        assert_eq!(a.class, MemClass::Cold);
        assert_eq!(b.class, MemClass::Cold);
        assert_ne!(a.key, b.key);
        assert_ne!(a.key & (1u64 << 63), 0);
        assert_ne!(b.key & (1u64 << 63), 0);
    }

    #[test]
    fn emitted_word_new_has_no_mem() {
        let ew = EmittedWord::new(0, String::new(), CostRule::Adrp, None, &[]);
        assert_eq!(ew.mem, None);
        assert_eq!(ew.flags, FlagEffect::None);
    }

    #[test]
    fn emitted_word_with_mem_sets_tag() {
        let ew = EmittedWord::new(0, String::new(), CostRule::Load, Some(0), &[31])
            .with_mem(MemRef::stack(8));
        assert_eq!(ew.mem, Some(MemRef::stack(8)));
    }

    #[test]
    fn emitted_word_with_flags_sets_nzcv() {
        let ew = EmittedWord::new(0, String::new(), CostRule::Alu, None, &[0, 1])
            .with_flags(FlagEffect::Write);
        assert!(ew.flags.writes());
        assert!(!ew.flags.reads());
    }
}
