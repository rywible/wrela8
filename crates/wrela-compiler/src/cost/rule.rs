#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum CostRule {
    Alu,
    Load,
    LoadAcquire,
    Store,
    StoreRelease,
    Branch,
    Call,
    Abort,
    AbortVal,
    MovWide,
    Mul,
    MulW,
    MulHigh,
    Sdiv,
    Udiv,
    Adrp,
    Barrier,
    System,
    /// Scalar FP add/subtract (`FADD`, `FSUB`).
    FpAddSub,
    /// Scalar FP multiply (`FMUL`).
    FpMul,
    /// Scalar fused multiply-add (`FMADD`/`FMSUB`). Reachable only through an
    /// explicitly sealed fused operation — never by contraction (D-P8R-04).
    FpFma,
    /// Scalar FP divide and square root (`FDIV`, `FSQRT`).
    FpDivSqrt,
    /// Scalar FP compare (`FCMP`): FP sources, no register destination, NZCV.
    FpCompare,
    /// Scalar FP/integer conversion (`FCVTZS`, `SCVTF`, `FCVT`). Distinct
    /// from a compare: it writes a register, and it may cross banks.
    FpConvert,
    /// Register moves between the general and FP/SIMD files (`FMOV` general).
    FpMove,
    /// FP load of 32 or 64 bits (`ldr s`/`ldr d`).
    FpLoad,
    /// 128-bit FP/ASIMD load (`ldr q`).
    FpLoadQ,
    /// FP store of 32 or 64 bits (`str s`/`str d`).
    FpStore,
    /// 128-bit FP/ASIMD store (`str q`).
    FpStoreQ,
    /// ASIMD integer arithmetic and logic (`add v`, `sub v`, `and v`, `orr v`,
    /// `sshr v`, `cmgt v`, `bsl v`).
    AsimdInt,
    /// ASIMD permutes and lane moves (`uzp1`, `dup`, `ins`).
    AsimdPermute,
    /// ASIMD FP add/subtract (`fadd v`, `fsub v`).
    AsimdFpAddSub,
    /// ASIMD FP multiply (`fmul v`).
    AsimdFpMul,
    /// ASIMD fused multiply-add (`fmla v`), sealed like [`CostRule::FpFma`].
    AsimdFpFma,
    /// ASIMD FP compare and min/max (`fcmge`, `fcmgt`, `fmin`, `fmax`).
    AsimdFpCmp,
    /// ASIMD FP/integer conversion (`fcvtzs v`, `scvtf v`).
    AsimdFpCvt,
}

impl CostRule {
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
        CostRule::MulW,
        CostRule::MulHigh,
        CostRule::Sdiv,
        CostRule::Udiv,
        CostRule::Adrp,
        CostRule::Barrier,
        CostRule::System,
        CostRule::FpAddSub,
        CostRule::FpMul,
        CostRule::FpFma,
        CostRule::FpDivSqrt,
        CostRule::FpCompare,
        CostRule::FpConvert,
        CostRule::FpMove,
        CostRule::FpLoad,
        CostRule::FpLoadQ,
        CostRule::FpStore,
        CostRule::FpStoreQ,
        CostRule::AsimdInt,
        CostRule::AsimdPermute,
        CostRule::AsimdFpAddSub,
        CostRule::AsimdFpMul,
        CostRule::AsimdFpFma,
        CostRule::AsimdFpCmp,
        CostRule::AsimdFpCvt,
    ];

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
            CostRule::MulW => "mul_w",
            CostRule::MulHigh => "mul_high",
            CostRule::Sdiv => "sdiv",
            CostRule::Udiv => "udiv",
            CostRule::Adrp => "adrp",
            CostRule::Barrier => "barrier",
            CostRule::System => "system",
            CostRule::FpAddSub => "fp_add_sub",
            CostRule::FpMul => "fp_mul",
            CostRule::FpFma => "fp_fma",
            CostRule::FpDivSqrt => "fp_div_sqrt",
            CostRule::FpCompare => "fp_compare",
            CostRule::FpConvert => "fp_convert",
            CostRule::FpMove => "fp_move",
            CostRule::FpLoad => "fp_load",
            CostRule::FpLoadQ => "fp_load_q",
            CostRule::FpStore => "fp_store",
            CostRule::FpStoreQ => "fp_store_q",
            CostRule::AsimdInt => "asimd_int",
            CostRule::AsimdPermute => "asimd_permute",
            CostRule::AsimdFpAddSub => "asimd_fp_add_sub",
            CostRule::AsimdFpMul => "asimd_fp_mul",
            CostRule::AsimdFpFma => "asimd_fp_fma",
            CostRule::AsimdFpCmp => "asimd_fp_cmp",
            CostRule::AsimdFpCvt => "asimd_fp_cvt",
        }
    }

    pub fn from_str(s: &str) -> Option<CostRule> {
        CostRule::ALL.iter().copied().find(|r| r.as_str() == s)
    }

    pub fn from_str_variant(s: &str) -> Option<CostRule> {
        CostRule::ALL
            .iter()
            .copied()
            .find(|r| format!("{r:?}") == s)
    }

    pub fn is_crosscore(self) -> bool {
        matches!(
            self,
            CostRule::Barrier | CostRule::System | CostRule::LoadAcquire | CostRule::StoreRelease
        )
    }

    pub fn is_load(self) -> bool {
        matches!(
            self,
            CostRule::Load | CostRule::LoadAcquire | CostRule::FpLoad | CostRule::FpLoadQ
        )
    }

    pub fn is_store(self) -> bool {
        matches!(
            self,
            CostRule::Store | CostRule::StoreRelease | CostRule::FpStore | CostRule::FpStoreQ
        )
    }

    /// True for every class that occupies the A76's FP/ASIMD pipes.
    ///
    /// Store *data* micro-ops occupy a V pipe too (the `[latency.store]`
    /// `ports = "L,D"` split), but an integer store is not itself an FP
    /// class; that contention is modelled by its port string, not here.
    pub fn is_fp_simd(self) -> bool {
        !matches!(self.bank_shape(), BankShape::AllGpr)
    }

    /// The operand banks this class is allowed to name.
    pub fn bank_shape(self) -> BankShape {
        match self {
            CostRule::Alu
            | CostRule::Load
            | CostRule::LoadAcquire
            | CostRule::Store
            | CostRule::StoreRelease
            | CostRule::Branch
            | CostRule::Call
            | CostRule::Abort
            | CostRule::AbortVal
            | CostRule::MovWide
            | CostRule::Mul
            | CostRule::MulW
            | CostRule::MulHigh
            | CostRule::Sdiv
            | CostRule::Udiv
            | CostRule::Adrp
            | CostRule::Barrier
            | CostRule::System => BankShape::AllGpr,
            CostRule::FpAddSub
            | CostRule::FpMul
            | CostRule::FpFma
            | CostRule::FpDivSqrt
            | CostRule::FpCompare
            | CostRule::AsimdInt
            | CostRule::AsimdPermute
            | CostRule::AsimdFpAddSub
            | CostRule::AsimdFpMul
            | CostRule::AsimdFpFma
            | CostRule::AsimdFpCmp => BankShape::FpData,
            // A convert may be FP→FP (`fcvt`), FP→GPR (`fcvtzs w, s`) or
            // GPR→FP (`scvtf s, w`); all three are the same SOG group.
            CostRule::FpConvert | CostRule::AsimdFpCvt => BankShape::FpMixed,
            CostRule::FpMove => BankShape::GprFpTransfer,
            CostRule::FpLoad | CostRule::FpLoadQ => BankShape::FpLoad,
            CostRule::FpStore | CostRule::FpStoreQ => BankShape::FpStore,
        }
    }
}

/// Which register banks a cost class's operands may name.
///
/// This is the fail-closed half of bank-aware operands: a class states its
/// shape once, and every emitted word is checked against it, so an FP
/// emitter that reaches for general-register operands is a hard error rather
/// than a scheduler that silently invents dependencies.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BankShape {
    /// Every operand is a general register.
    AllGpr,
    /// Every operand is an FP/SIMD register (the destination may be absent
    /// when the class writes NZCV instead).
    FpData,
    /// At least one operand is FP/SIMD; the rest may be either.
    FpMixed,
    /// Exactly one FP/SIMD operand and exactly one general operand.
    GprFpTransfer,
    /// FP/SIMD destination, general address sources.
    FpLoad,
    /// No destination; general address sources plus exactly one FP/SIMD data
    /// source.
    FpStore,
}

/// Check an emitted word's operand banks against its class's [`BankShape`].
pub fn check_bank_shape(rule: CostRule, dst: Option<Reg>, srcs: &[Reg]) -> Result<(), String> {
    let operands: Vec<Reg> = dst.into_iter().chain(srcs.iter().copied()).collect();
    let fp = operands.iter().filter(|reg| reg.is_fp()).count();
    let gpr = operands.len() - fp;
    let fail = |why: &str| {
        Err(format!(
            "cost class `{}` requires {why}, got dst={dst:?} srcs={srcs:?}",
            rule.as_str()
        ))
    };
    match rule.bank_shape() {
        BankShape::AllGpr if fp != 0 => fail("general-register operands only"),
        BankShape::FpData if gpr != 0 => fail("FP/SIMD operands only"),
        BankShape::FpMixed if fp == 0 => fail("at least one FP/SIMD operand"),
        BankShape::GprFpTransfer if fp != 1 || gpr != 1 => {
            fail("exactly one FP/SIMD and one general operand")
        }
        BankShape::FpLoad if !dst.is_some_and(Reg::is_fp) || srcs.iter().any(|reg| reg.is_fp()) => {
            fail("an FP/SIMD destination and general address sources")
        }
        BankShape::FpStore if dst.is_some() || fp != 1 => {
            fail("no destination and exactly one FP/SIMD data source")
        }
        _ => Ok(()),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemClass {
    Stack,
    Cold,
}

pub const MEM_SP_REG: u8 = 31;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MemTarget {
    Stack { function: u64, offset: u64 },
    FlowFrame { function: u64, offset: u64 },
    Static { symbol: u64, offset: u64 },
    Mmio { device: u64, offset: u64 },
    Unknown { site: u64 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct MemRef {
    pub class: MemClass,
    /// Legacy encoded offset retained for stable dumps and table consumers.
    pub key: u64,
    /// Compiler provenance.  The base GPR is deliberately stored separately
    /// and is used only for dependency validation.
    pub target: MemTarget,
    pub base: Option<u8>,
}

impl MemRef {
    pub fn stack(offset: u64) -> MemRef {
        MemRef::stack_in(0, offset)
    }

    pub fn stack_in(function: u64, offset: u64) -> MemRef {
        Self::stack_at_base(function, offset, MEM_SP_REG)
    }

    pub fn stack_at_base(function: u64, offset: u64, base: u8) -> MemRef {
        MemRef {
            class: MemClass::Stack,
            key: offset,
            target: MemTarget::Stack { function, offset },
            base: Some(base),
        }
    }

    pub fn flow_frame(function: u64, offset: u64, base: u8) -> MemRef {
        MemRef {
            class: MemClass::Stack,
            key: offset,
            target: MemTarget::FlowFrame { function, offset },
            base: Some(base),
        }
    }

    pub fn static_ref(symbol: u64, offset: u64, base: u8) -> MemRef {
        MemRef {
            class: MemClass::Cold,
            key: offset & 0x0000_FFFF_FFFF_FFFF,
            target: MemTarget::Static { symbol, offset },
            base: Some(base),
        }
    }

    pub fn mmio(device: u64, offset: u64, base: u8) -> MemRef {
        MemRef {
            class: MemClass::Cold,
            key: offset & 0x0000_FFFF_FFFF_FFFF,
            target: MemTarget::Mmio { device, offset },
            base: Some(base),
        }
    }

    pub fn unknown(site: u64, base: Option<u8>, _offset: u64) -> MemRef {
        MemRef {
            class: MemClass::Cold,
            key: (1u64 << 63) | (site & !(1u64 << 63)),
            target: MemTarget::Unknown { site },
            base,
        }
    }

    pub fn cold_stable(base_reg: u8, imm: u64) -> MemRef {
        // This constructor is the explicit stable-target escape hatch used by
        // cost tests and by lowering once it has a symbolic target.  Generic
        // emitter sites convert this to an Unknown site in `push_mem` unless
        // lowering supplied stronger provenance.
        MemRef {
            class: MemClass::Cold,
            key: ((base_reg as u64) << 48) | (imm & 0x0000_FFFF_FFFF_FFFF),
            target: MemTarget::Static {
                symbol: base_reg as u64,
                offset: imm,
            },
            base: Some(base_reg),
        }
    }

    pub fn cold_unique(seq: u64) -> MemRef {
        // A named synthetic cold line is useful for deterministic model tests;
        // actual emitted unknown addresses use `unknown` through CodegenState.
        MemRef {
            class: MemClass::Cold,
            key: (1u64 << 63) | (seq & !(1u64 << 63)),
            target: MemTarget::Static {
                symbol: u64::MAX,
                offset: seq.saturating_mul(64),
            },
            base: None,
        }
    }

    pub fn for_base_imm(base_reg: u8, imm: u64) -> MemRef {
        if base_reg == MEM_SP_REG {
            MemRef::stack(imm)
        } else {
            MemRef::unknown(
                ((base_reg as u64) << 48) | (imm & 0x0000_FFFF_FFFF_FFFF),
                Some(base_reg),
                imm,
            )
        }
    }

    pub fn base_reg(self) -> Option<u8> {
        self.base
    }

    pub fn require_base_in_srcs(self, srcs: &[Reg]) -> Result<(), String> {
        let Some(base) = self.base_reg() else {
            return Ok(());
        };
        // An address base is always a general register; an FP/SIMD operand of
        // the same number is a different register and does not satisfy it.
        if srcs.contains(&Reg::gpr(base)) {
            Ok(())
        } else {
            Err(format!("MemRef base register x{base} not in srcs {srcs:?}"))
        }
    }
}

/// The A76 register file an operand names.
///
/// AArch64's general and FP/SIMD register files are numbered independently:
/// `x0` and `v0` are both "register 0" and share no state. A single `u8`
/// operand therefore made `ldr q0, [x9]` look like it defined the same
/// register `add x0, x1, x2` defines, so the scheduler invented dependencies
/// between unrelated values and hid real ones. The bank is part of the
/// operand's identity.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum RegBank {
    /// `x`/`w` general registers, plus `sp` at number [`MEM_SP_REG`].
    Gpr,
    /// `b`/`h`/`s`/`d`/`q` (`v`) FP and ASIMD registers.
    FpSimd,
}

impl RegBank {
    pub const ALL: &'static [RegBank] = &[RegBank::Gpr, RegBank::FpSimd];

    pub fn as_str(self) -> &'static str {
        match self {
            RegBank::Gpr => "gpr",
            RegBank::FpSimd => "fpsimd",
        }
    }

    /// Dense index into per-bank scheduler state.
    pub fn index(self) -> usize {
        match self {
            RegBank::Gpr => 0,
            RegBank::FpSimd => 1,
        }
    }
}

pub const REG_BANK_COUNT: usize = 2;

/// One banked register operand of an emitted word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct Reg {
    pub bank: RegBank,
    pub num: u8,
}

impl Reg {
    pub const fn gpr(num: u8) -> Reg {
        Reg {
            bank: RegBank::Gpr,
            num,
        }
    }

    pub const fn fp(num: u8) -> Reg {
        Reg {
            bank: RegBank::FpSimd,
            num,
        }
    }

    pub fn is_gpr(self) -> bool {
        self.bank == RegBank::Gpr
    }

    pub fn is_fp(self) -> bool {
        self.bank == RegBank::FpSimd
    }

    /// The general register number, or `None` for an FP/SIMD operand.
    ///
    /// Consumers that reason about addresses, the stack pointer, or the
    /// procedure-call ABI want this rather than a bare number: an FP operand
    /// must not silently answer a question about `x`.
    pub fn as_gpr(self) -> Option<u8> {
        self.is_gpr().then_some(self.num)
    }

    pub fn is_sp(self) -> bool {
        self.is_gpr() && self.num == MEM_SP_REG
    }
}

/// Convert a slice of general register numbers into banked operands.
pub fn gpr_operands(regs: &[u8]) -> Vec<Reg> {
    regs.iter().copied().map(Reg::gpr).collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FlagEffect {
    #[default]
    None,
    Write,
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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EmittedWord {
    pub word: u32,
    pub text: String,
    pub rule: CostRule,
    pub dst: Option<Reg>,
    pub srcs: [Reg; 4],
    pub src_len: u8,
    pub mem: Option<MemRef>,
    pub flags: FlagEffect,
    pub access_bytes: u8,
}

/// Filler for the unused tail of [`EmittedWord::srcs`]. Only `..src_len` is
/// ever read; this exists because the array is fixed-size.
const SRC_FILLER: Reg = Reg::gpr(0);

impl EmittedWord {
    /// Construct a word whose operands are all general registers.
    ///
    /// Named for its bank on purpose: an FP/SIMD emitter that reaches for the
    /// obvious constructor gets a compile error rather than a silently
    /// mis-banked operand. Use [`EmittedWord::banked`] for anything that
    /// touches `v` registers.
    pub fn gpr(
        word: u32,
        text: String,
        rule: CostRule,
        dst: Option<u8>,
        srcs: &[u8],
    ) -> EmittedWord {
        let banked: Vec<Reg> = srcs.iter().copied().map(Reg::gpr).collect();
        EmittedWord::banked(word, text, rule, dst.map(Reg::gpr), &banked)
    }

    pub fn banked(
        word: u32,
        text: String,
        rule: CostRule,
        dst: Option<Reg>,
        srcs: &[Reg],
    ) -> EmittedWord {
        let mut arr = [SRC_FILLER; 4];
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

    pub fn src_slice(&self) -> &[Reg] {
        &self.srcs[..self.src_len as usize]
    }

    /// The general register numbers among the sources, dropping FP/SIMD
    /// operands. Address and ABI reasoning wants exactly this.
    pub fn gpr_srcs(&self) -> impl Iterator<Item = u8> + '_ {
        self.src_slice().iter().filter_map(|reg| reg.as_gpr())
    }

    pub fn clear_srcs(&mut self) {
        self.srcs = [SRC_FILLER; 4];
        self.src_len = 0;
    }

    pub fn set_srcs(&mut self, srcs: &[Reg]) {
        let mut arr = [SRC_FILLER; 4];
        let n = srcs.len().min(4);
        arr[..n].copy_from_slice(&srcs[..n]);
        self.srcs = arr;
        self.src_len = n as u8;
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
    fn emit_sites_carry_their_sog_group() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/codegen.rs"),
        )
        .expect("read codegen.rs");
        let cut = src
            .find("#[cfg(test)]\nmod tests {")
            .expect("codegen.rs test module marker");
        let prod = &src[..cut];

        let expected: &[(&str, &[CostRule])] = &[
            ("enc_sdiv", &[CostRule::Sdiv]),
            ("enc_udiv", &[CostRule::Udiv, CostRule::Udiv]),
            ("enc_smulh", &[CostRule::MulHigh]),
            ("enc_umulh", &[CostRule::MulHigh]),
            (
                "enc_mul(",
                &[CostRule::Mul, CostRule::Mul, CostRule::MulW, CostRule::Mul],
            ),
            ("enc_msub", &[CostRule::Mul, CostRule::Mul]),
            (
                "enc_stlr_w",
                &[CostRule::StoreRelease, CostRule::StoreRelease],
            ),
            (
                "enc_stlr_x",
                &[CostRule::StoreRelease, CostRule::StoreRelease],
            ),
            (
                "enc_ldar_w",
                &[CostRule::LoadAcquire, CostRule::LoadAcquire],
            ),
            (
                "enc_ldar_x",
                &[
                    CostRule::LoadAcquire,
                    CostRule::LoadAcquire,
                    CostRule::LoadAcquire,
                ],
            ),
            ("enc_ldaxr_w", &[CostRule::LoadAcquire]),
            ("enc_ldaxr_x", &[CostRule::LoadAcquire]),
            ("enc_stlxr_w", &[CostRule::StoreRelease]),
            ("enc_stlxr_x", &[CostRule::StoreRelease]),
            ("enc_dmb_ishst", &[CostRule::Barrier]),
            ("enc_dmb_ishld", &[CostRule::Barrier]),
            ("enc_brk", &[CostRule::System]),
        ];

        for &(enc, want) in expected {
            let needle = format!("encode::{enc}");
            let mut tags: Vec<Option<CostRule>> = Vec::new();
            let mut at = 0usize;
            while let Some(off) = prod[at..].find(&needle) {
                let start = at + off;
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
                tags.push(CostRule::from_str_variant(&tag));
                at = start + 1;
            }
            let want_tags: Vec<Option<CostRule>> = want.iter().copied().map(Some).collect();
            assert_eq!(
                tags, want_tags,
                "{enc} emit-site tags moved (plans/M20.md item D: the tag is the \
                 SOG instruction group). Classify each new site deliberately and \
                 move this list in the same commit"
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
    fn non_sp_base_imm_is_unknown_until_lowering_provides_provenance() {
        let m = MemRef::for_base_imm(28, 16);
        assert_eq!(m.class, MemClass::Cold);
        assert!(matches!(m.target, MemTarget::Unknown { .. }));
        assert_eq!(m.base_reg(), Some(28));
        assert_ne!(m, MemRef::cold_stable(28, 16));
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
        let ew = EmittedWord::gpr(0, String::new(), CostRule::Adrp, None, &[]);
        assert_eq!(ew.mem, None);
        assert_eq!(ew.flags, FlagEffect::None);
    }

    #[test]
    fn emitted_word_with_mem_sets_tag() {
        let ew = EmittedWord::gpr(0, String::new(), CostRule::Load, Some(0), &[31])
            .with_mem(MemRef::stack(8));
        assert_eq!(ew.mem, Some(MemRef::stack(8)));
    }

    #[test]
    fn emitted_word_with_flags_sets_nzcv() {
        let ew = EmittedWord::gpr(0, String::new(), CostRule::Alu, None, &[0, 1])
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
        assert!(
            stack
                .require_base_in_srcs(&[Reg::gpr(MEM_SP_REG), Reg::gpr(0)])
                .is_ok()
        );
        assert!(
            stack
                .require_base_in_srcs(&[Reg::gpr(0), Reg::gpr(1)])
                .is_err()
        );
        let cold = MemRef::cold_stable(28, 16);
        assert!(cold.require_base_in_srcs(&[Reg::gpr(28)]).is_ok());
        assert!(cold.require_base_in_srcs(&[Reg::gpr(0)]).is_err());
        // An FP operand of the same number is a different register.
        assert!(cold.require_base_in_srcs(&[Reg::fp(28)]).is_err());
        assert!(MemRef::cold_unique(3).require_base_in_srcs(&[]).is_ok());
    }
}
