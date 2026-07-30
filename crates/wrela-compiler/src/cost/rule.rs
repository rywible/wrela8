//! Closed ISA op-class tags attached at `FnCtx::push` (plans/M18.md item C).
//! Every variant in `ALL` must be priced by exactly one row of
//! `bench/a76-pi5.toml` — a `[latency.<group>]` sub-table whose key is
//! `as_str()`, or a `[crosscore]` term naming it (plans/M20.md item D).
//!
//! **The rule set is the SOG instruction-group set wrela actually emits**
//! (freeze 1630), measured from codegen's own encoder call sites, not the
//! ISA's group list. Deliberately *not* variants, each because no site
//! emits it: extend-and-shift arithmetic, LSR/ASR/ROR-shifted arithmetic,
//! flagset logical (`ANDS`/`BICS`), `LDP`/`STP`, `BFM` insert, and the
//! W-form (32-bit) multiply-accumulate and divide groups — every
//! `enc_mul` / `enc_msub` / `enc_sdiv` / `enc_udiv` site in `codegen.rs`
//! passes `sf = true`, and `SMULH`/`UMULH` have no W-form at all.

/// ISA op-class for proxy-cycle ranking. Never parsed from mnemonics.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CostRule {
    /// Arithmetic basic / logical basic / conditional compare / conditional
    /// select / move register / bitfield-move basic / variable shift: all
    /// 1 cycle, throughput 3, port I, so they share one group.
    Alu,
    /// Load register, immed — latency 4 on an **L1D hit** (SOG §3.9).
    Load,
    /// `LDAR`. T5 by absence: the SOG prices load-acquire nowhere. Not a
    /// load/store-**exclusive** (`enc_ldaxr_w` is genuinely unused).
    LoadAcquire,
    /// Store register — latency 1 to the **store buffer**, split into an
    /// address uop (L) and a data uop on a V pipe (SOG §3.10).
    Store,
    /// `STLR`. T5 by absence, exactly like `LoadAcquire`.
    StoreRelease,
    Branch,
    /// `BL` — branch and link, immed. Lat 1, thru 1, ports I + B. The
    /// callee-side residual is the swept `call_overhead`, not this row.
    Call,
    Abort,
    AbortVal,
    /// `MOVZ` / `MOVK` — 1 cycle, throughput 3, port I. NarrowImm's win is
    /// therefore fetch and footprint, not latency.
    MovWide,
    /// Multiply-accumulate, **X-form** (`MADD`/`MSUB`; `MUL` is `MADD` with
    /// `XZR`). Lat 4 (acc 3), thru 1/3, port M, and stalls pipe M 2 extra
    /// cycles (SOG §3.6 note 4).
    Mul,
    /// `SMULH` / `UMULH`. Lat 5 (acc 3), thru 1/4, port M, and stalls pipe
    /// M 3 extra cycles (SOG §3.6 note 5). Emitted by `narrow_to_width`
    /// and the checked-multiply overflow check.
    MulHigh,
    /// Divide, X-form. 5-20 cycles with data-dependent early termination,
    /// pinned pessimistic and swept; blocks subsequent divides on pipe M.
    Sdiv,
    Udiv,
    Adrp,
    /// `DMB ishst` / `DMB ishld`. T5 by absence — no barrier entry exists
    /// anywhere in the SOG's 46 pages.
    Barrier,
    /// System / trap words (`BRK` today). T5 by absence.
    System,
    /// FP/ASIMD data-processing, kept as one coarse row and not expanded
    /// (dimension inventory row 35, freeze 1630).
    Neon,
}

impl CostRule {
    /// Every rule the profile must price exactly once.
    pub const ALL: &'static [CostRule] = &[
        CostRule::Alu,
        CostRule::Load,
        CostRule::LoadAcquire,
        CostRule::Store,
        CostRule::StoreRelease,
        CostRule::Branch,
        CostRule::Call,
        CostRule::Abort,
        CostRule::AbortVal,
        CostRule::MovWide,
        CostRule::Mul,
        CostRule::MulHigh,
        CostRule::Sdiv,
        CostRule::Udiv,
        CostRule::Adrp,
        CostRule::Barrier,
        CostRule::System,
        CostRule::Neon,
    ];

    /// TOML `[latency]` / `[crosscore].rule` key, and the dump Term id.
    pub fn as_str(self) -> &'static str {
        match self {
            CostRule::Alu => "alu",
            CostRule::Load => "load",
            CostRule::LoadAcquire => "load_acquire",
            CostRule::Store => "store",
            CostRule::StoreRelease => "store_release",
            CostRule::Branch => "branch",
            CostRule::Call => "call",
            CostRule::Abort => "abort",
            CostRule::AbortVal => "abort_val",
            CostRule::MovWide => "mov_wide",
            CostRule::Mul => "mul",
            CostRule::MulHigh => "mul_high",
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
            "load_acquire" => CostRule::LoadAcquire,
            "store" => CostRule::Store,
            "store_release" => CostRule::StoreRelease,
            "branch" => CostRule::Branch,
            "call" => CostRule::Call,
            "abort" => CostRule::Abort,
            "abort_val" => CostRule::AbortVal,
            "mov_wide" => CostRule::MovWide,
            "mul" => CostRule::Mul,
            "mul_high" => CostRule::MulHigh,
            "sdiv" => CostRule::Sdiv,
            "udiv" => CostRule::Udiv,
            "adrp" => CostRule::Adrp,
            "barrier" => CostRule::Barrier,
            "system" => CostRule::System,
            "neon" => CostRule::Neon,
            _ => return None,
        })
    }

    /// Rust variant name (`"MulHigh"`) -> the variant. Only the emit-site
    /// classifier scan uses this; the TOML key is `as_str` / `from_str`.
    pub fn from_str_variant(s: &str) -> Option<CostRule> {
        CostRule::ALL
            .iter()
            .copied()
            .find(|r| format!("{r:?}") == s)
    }

    /// True for the rules whose cost is a **swept** cross-core term rather
    /// than a pinned latency row (`[crosscore]`, decision 1602).
    pub fn is_crosscore(self) -> bool {
        matches!(
            self,
            CostRule::Barrier | CostRule::System | CostRule::LoadAcquire | CostRule::StoreRelease
        )
    }

    /// True for the rules that read or write memory — the ordered accesses
    /// take the same memory path as their plain twins.
    pub fn is_load(self) -> bool {
        matches!(self, CostRule::Load | CostRule::LoadAcquire)
    }

    pub fn is_store(self) -> bool {
        matches!(self, CostRule::Store | CostRule::StoreRelease)
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

    /// Base register for a non-unique MemRef (Stack → SP; Cold stable →
    /// packed base). Cold unique has no reusable base — `None`.
    pub fn base_reg(self) -> Option<u8> {
        match self.class {
            MemClass::Stack => Some(MEM_SP_REG),
            MemClass::Cold => {
                if self.key & (1u64 << 63) != 0 {
                    None
                } else {
                    Some((self.key >> 48) as u8)
                }
            }
        }
    }

    /// Fail closed when a non-unique MemRef's base is absent from `srcs`
    /// (integrity item C). Unique Cold keys skip the check.
    pub fn require_base_in_srcs(self, srcs: &[u8]) -> Result<(), String> {
        let Some(base) = self.base_reg() else {
            return Ok(());
        };
        if srcs.contains(&base) {
            Ok(())
        } else {
            Err(format!("MemRef base register {base} not in srcs {srcs:?}"))
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
    /// Bytes this word transfers, for a load/store; `0` when the word is
    /// not a load/store shape `encode.rs` knows (plans/M20.md item I).
    ///
    /// Assigned **at construction**, from the encoded word, by
    /// `encode::access_width_bytes` — the module that wrote the `size`
    /// field in the first place. Unlike `rule` and `mem`, the width is not
    /// a semantic classification that has to be declared: it is *in* the
    /// encoding, so threading a second `width` argument through every emit
    /// site would create a source of truth that can disagree with the word
    /// actually emitted, which is the defect freeze 1303 exists to
    /// prevent. Nothing here reads the mnemonic text.
    ///
    /// `0` means "no width fact", and SOG §4.5's alignment terms treat it
    /// as undecidable rather than as an aligned access (`score.rs`).
    pub access_bytes: u8,
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
            access_bytes: crate::encode::access_width_bytes(word).unwrap_or(0),
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

    /// plans/M20.md item D's classifier oracle: every emit site of an
    /// encoder whose SOG group is **not** the coarse integer-ALU group must
    /// carry that group's `CostRule`.
    ///
    /// This is a source scan rather than a scoring assertion because the
    /// defect class it catches is a **mistagged emit site**, which no
    /// schedule number can see: before this item, `sdiv`/`udiv`, the
    /// wrapping `MUL`, `SMULH`/`UMULH`, `STLR` and `LDAR` were all tagged
    /// as `Alu` / `Load` / `Store`, so the coarse table priced a 20-cycle
    /// divide at 1 cycle and no test could tell. Adding a site with the
    /// wrong tag fails here; adding one at all moves the pinned count, so
    /// the classification is a deliberate act.
    #[test]
    fn emit_sites_carry_their_sog_group() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/codegen.rs"),
        )
        .expect("read codegen.rs");
        // The unit-test module quotes some of these encoders in asserts
        // rather than pushing words, so only production code is scanned.
        let cut = src
            .find("#[cfg(test)]\nmod tests {")
            .expect("codegen.rs test module marker");
        let prod = &src[..cut];

        // (encoder, expected rule, expected site count)
        let expected: &[(&str, CostRule, usize)] = &[
            ("enc_sdiv", CostRule::Sdiv, 1),
            ("enc_udiv", CostRule::Udiv, 2),
            ("enc_smulh", CostRule::MulHigh, 1),
            ("enc_umulh", CostRule::MulHigh, 1),
            ("enc_mul(", CostRule::Mul, 3),
            ("enc_msub", CostRule::Mul, 2),
            ("enc_stlr_w", CostRule::StoreRelease, 3),
            ("enc_stlr_x", CostRule::StoreRelease, 3),
            ("enc_ldar_w", CostRule::LoadAcquire, 2),
            ("enc_ldar_x", CostRule::LoadAcquire, 2),
            ("enc_dmb_ishst", CostRule::Barrier, 1),
            ("enc_dmb_ishld", CostRule::Barrier, 1),
            ("enc_brk", CostRule::System, 1),
        ];

        for &(enc, want, count) in expected {
            let needle = format!("encode::{enc}");
            let mut sites = 0usize;
            let mut at = 0usize;
            while let Some(off) = prod[at..].find(&needle) {
                let start = at + off;
                // The tag is the third argument of the `push` this site
                // feeds; it always lands within the same statement.
                let window = &prod[start..(start + 1500).min(prod.len())];
                let tag = window
                    .find("CostRule::")
                    .map(|i| {
                        window[i + "CostRule::".len()..]
                            .chars()
                            .take_while(|c| c.is_alphanumeric())
                            .collect::<String>()
                    })
                    .unwrap_or_default();
                assert_eq!(
                    CostRule::from_str_variant(&tag),
                    Some(want),
                    "{enc} site #{} is tagged `CostRule::{tag}`, expected `{:?}` \
                     (plans/M20.md item D: the tag is the SOG instruction group)",
                    sites + 1,
                    want
                );
                sites += 1;
                at = start + 1;
            }
            assert_eq!(
                sites, count,
                "{enc} site count moved ({sites} live, {count} pinned) — classify the \
                 new site deliberately and move the count in the same commit"
            );
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

    #[test]
    fn memref_base_reg_stack_and_cold_stable() {
        assert_eq!(MemRef::stack(24).base_reg(), Some(MEM_SP_REG));
        assert_eq!(MemRef::cold_stable(28, 16).base_reg(), Some(28));
        assert_eq!(MemRef::cold_unique(0).base_reg(), None);
    }

    #[test]
    fn memref_require_base_in_srcs_fail_closed() {
        let stack = MemRef::stack(8);
        assert!(stack.require_base_in_srcs(&[MEM_SP_REG, 0]).is_ok());
        assert!(stack.require_base_in_srcs(&[0, 1]).is_err());
        let cold = MemRef::cold_stable(28, 16);
        assert!(cold.require_base_in_srcs(&[28]).is_ok());
        assert!(cold.require_base_in_srcs(&[0]).is_err());
        // Unique: no base check.
        assert!(MemRef::cold_unique(3).require_base_in_srcs(&[]).is_ok());
    }
}
