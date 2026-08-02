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
    Neon,
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
        CostRule::Neon,
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
            "mul_w" => CostRule::MulW,
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
        matches!(self, CostRule::Load | CostRule::LoadAcquire)
    }

    pub fn is_store(self) -> bool {
        matches!(self, CostRule::Store | CostRule::StoreRelease)
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
        MemRef {
            class: MemClass::Stack,
            key: offset,
            target: MemTarget::Stack { function, offset },
            base: Some(MEM_SP_REG),
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
    pub dst: Option<u8>,
    pub srcs: [u8; 4],
    pub src_len: u8,
    pub mem: Option<MemRef>,
    pub flags: FlagEffect,
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
                &[
                    CostRule::StoreRelease,
                    CostRule::StoreRelease,
                    CostRule::StoreRelease,
                ],
            ),
            (
                "enc_stlr_x",
                &[
                    CostRule::StoreRelease,
                    CostRule::StoreRelease,
                    CostRule::StoreRelease,
                ],
            ),
            (
                "enc_ldar_w",
                &[CostRule::LoadAcquire, CostRule::LoadAcquire],
            ),
            (
                "enc_ldar_x",
                &[CostRule::LoadAcquire, CostRule::LoadAcquire],
            ),
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
        assert!(MemRef::cold_unique(3).require_base_in_srcs(&[]).is_ok());
    }
}
