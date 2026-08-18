//! A small metadata auditor for the AArch64 subset emitted by Wrela.
//!
//! This is not a disassembler.  It checks only the architectural fields that
//! the cost scheduler relies on and fails closed when a tagged word cannot be
//! reconciled with its tag.

use crate::codegen::CodegenProgram;
use crate::cost::{CostRule, EmittedWord, FlagEffect, Reg, RegBank};

fn rd(word: u32) -> u8 {
    (word & 0x1f) as u8
}

fn rn(word: u32) -> u8 {
    ((word >> 5) & 0x1f) as u8
}

fn rm(word: u32) -> u8 {
    ((word >> 16) & 0x1f) as u8
}

fn ra(word: u32) -> u8 {
    ((word >> 10) & 0x1f) as u8
}

fn is_load_store(word: u32) -> bool {
    crate::encode::access_width_bytes(word).is_some()
        && (word & 0x3a00_0000 == 0x2800_0000
            || word & 0x3f00_0000 == 0x3900_0000
            || word & 0x3f00_0000 == 0x3d00_0000
            || matches!(word & 0x3fe0_0c00, 0x3860_0800 | 0x3820_0800)
            || matches!(word & 0xffc0_0000, 0x3dc0_0000 | 0x3d80_0000)
            || (word & 0x3fff_fc00 == 0x08df_fc00)
            || (word & 0x3fff_fc00 == 0x089f_fc00)
            || (word & 0x3fff_fc00 == 0x085f_fc00)
            || (word & 0x3fe0_fc00 == 0x0800_fc00))
}

fn is_ldar(word: u32) -> bool {
    let fixed = word & 0x3fff_fc00;
    fixed == 0x08df_fc00 || fixed == 0xc8df_fc00 || fixed == 0x085f_fc00
}

fn is_stlr(word: u32) -> bool {
    let fixed = word & 0x3fff_fc00;
    fixed == 0x089f_fc00 || fixed == 0xc89f_fc00 || word & 0x3fe0_fc00 == 0x0800_fc00
}

fn is_mov_wide(word: u32) -> bool {
    word & 0x1f80_0000 == 0x1280_0000
}

fn is_movk(word: u32) -> bool {
    word & 0xff80_0000 == 0xf280_0000
}

fn is_adrp(word: u32) -> bool {
    word & 0x9f00_0000 == 0x9000_0000
}

fn is_adr(word: u32) -> bool {
    word & 0x9f00_0000 == 0x1000_0000
}

fn is_bl(word: u32) -> bool {
    word & 0xfc00_0000 == 0x9400_0000
}

fn is_control(word: u32) -> bool {
    let b = word & 0x7c00_0000 == 0x1400_0000;
    let bcond = word & 0xff00_0010 == 0x5400_0000;
    let cb = word & 0x7e00_0000 == 0x3400_0000;
    let tb = word & 0x7e00_0000 == 0x3600_0000;
    let br = word & 0xffff_fc1f == 0xd61f_0000;
    let ret = word & 0xffff_fc1f == 0xd65f_0000;
    b || bcond || cb || tb || br || ret || is_bl(word)
}

fn is_barrier(word: u32) -> bool {
    matches!(word, 0xd503_3abf | 0xd503_39bf)
}

fn is_system(word: u32) -> bool {
    word & 0xffe0_001f == 0xd420_0000
}

/// Name a register the way its bank writes it, for diagnostics.
fn reg_text(reg: Reg) -> String {
    match reg.bank {
        RegBank::Gpr => format!("x{}", reg.num),
        RegBank::FpSimd => format!("v{}", reg.num),
    }
}

fn expect_dst(ew: &EmittedWord, required: bool, bank: RegBank) -> Result<(), String> {
    if required {
        let Some(dst) = ew.dst else {
            return Err("encoded destination has no metadata destination".to_string());
        };
        let encoded = Reg {
            bank,
            num: rd(ew.word),
        };
        if dst != encoded {
            return Err(format!(
                "encoded destination {} disagrees with metadata {}",
                reg_text(encoded),
                reg_text(dst)
            ));
        }
    }
    Ok(())
}

fn contains(srcs: &[Reg], reg: Reg) -> bool {
    srcs.contains(&reg)
}

fn require_src(fn_key: &str, index: usize, ew: &EmittedWord, reg: Reg) -> Result<(), String> {
    // x31 is `sp`/`xzr` depending on the form and is never a metadata source;
    // v31 is an ordinary vector register and is.
    if reg.is_gpr() && reg.num == 31 {
        return Ok(());
    }
    if !contains(ew.src_slice(), reg) {
        return Err(format!(
            "{fn_key}[{index}] encoded source {} is absent from srcs {:?} ({})",
            reg_text(reg),
            ew.src_slice(),
            ew.text
        ));
    }
    Ok(())
}

fn audit_three_reg(
    fn_key: &str,
    index: usize,
    ew: &EmittedWord,
    include_accumulator: bool,
) -> Result<(), String> {
    expect_dst(ew, true, RegBank::Gpr)?;
    require_src(fn_key, index, ew, Reg::gpr(rn(ew.word)))?;
    require_src(fn_key, index, ew, Reg::gpr(rm(ew.word)))?;
    if include_accumulator {
        require_src(fn_key, index, ew, Reg::gpr(ra(ew.word)))?;
    }
    Ok(())
}

/// Every 128-bit FP/ASIMD data-processing word this backend emits, by the
/// exact encoding the class promises. A word outside this set carrying an
/// ASIMD class is a metadata bug, not an unmodelled instruction.
fn asimd_data_processing_matches(rule: CostRule, word: u32) -> bool {
    match rule {
        // ADD/SUB, logic/BSL, signed compare, and immediate arithmetic shift.
        CostRule::AsimdInt => {
            word & 0xbf20_fc00 == 0x0e20_8400
                || word & 0xbf20_fc00 == 0x2e20_8400
                || word & 0xbf20_fc00 == 0x0e20_1c00
                || word & 0xbf20_fc00 == 0x2e20_1c00
                || word & 0xff20_fc00 == 0x4e20_3400
                || word & 0xff80_fc00 == 0x4f00_0400
        }
        // UZP1 and scalar-element DUP.
        CostRule::AsimdPermute => {
            word & 0xbf20_fc00 == 0x0e00_1800 || word & 0xffff_fc00 == 0x4e04_0400
        }
        // FADD/FSUB vector, single precision.
        CostRule::AsimdFpAddSub => matches!(word & 0xbfa0_fc00, 0x0e20_d400 | 0x0ea0_d400),
        // FMUL vector.
        CostRule::AsimdFpMul => word & 0xbfa0_fc00 == 0x2e20_dc00,
        // FMLA vector.
        CostRule::AsimdFpFma => word & 0xbfa0_fc00 == 0x0e20_cc00,
        // FCMGE/FCMGT vector, and FMIN/FMAX vector.
        CostRule::AsimdFpCmp => {
            word & 0xbfa0_fc00 == 0x2e20_e400
                || word & 0xbfa0_fc00 == 0x2ea0_e400
                || word & 0xbfa0_fc00 == 0x0e20_f400
                || word & 0xbfa0_fc00 == 0x0ea0_f400
        }
        // FCVTZS/SCVTF vector.
        CostRule::AsimdFpCvt => {
            word & 0xbfbf_fc00 == 0x0ea1_b800 || word & 0xbfbf_fc00 == 0x0e21_d800
        }
        _ => false,
    }
}

/// Exact scalar FP opcode family promised by each scalar cost class.
/// Register fields and the S/D type bit are ignored; operation bits are not.
fn scalar_fp_data_processing_matches(rule: CostRule, word: u32) -> bool {
    let two_source = word & 0xffa0_fc00;
    let one_source = word & 0xffbf_fc00;
    let transfer = word & 0xffff_fc00;
    match rule {
        CostRule::FpAddSub => {
            matches!(two_source, 0x1e20_2800 | 0x1e20_3800) || one_source == 0x1e21_4000
        }
        CostRule::FpMul => two_source == 0x1e20_0800,
        // FMADD. No scalar FMA emitter exists yet, but the sealed class must
        // still refuse an unrelated scalar opcode if one is mislabelled.
        CostRule::FpFma => word & 0xff20_8000 == 0x1f00_0000,
        CostRule::FpDivSqrt => two_source == 0x1e20_1800 || one_source == 0x1e21_c000,
        CostRule::FpCompare => word & 0xffa0_fc1f == 0x1e20_2000,
        CostRule::FpConvert => matches!(
            transfer,
            0x1e22_c000
                | 0x1e62_4000
                | 0x1e22_0000
                | 0x1e23_0000
                | 0x9e62_0000
                | 0x9e63_0000
                | 0x9e22_0000
                | 0x9e23_0000
                | 0x1e62_0000
                | 0x1e63_0000
                | 0x1e38_0000
                | 0x1e39_0000
                | 0x9e78_0000
                | 0x9e79_0000
                | 0x9e38_0000
                | 0x9e39_0000
                | 0x1e78_0000
                | 0x1e79_0000
        ),
        CostRule::FpMove => matches!(
            transfer,
            0x1e27_0000 | 0x9e67_0000 | 0x1e26_0000 | 0x9e66_0000
        ),
        _ => false,
    }
}

pub fn audit_word(fn_key: &str, index: usize, ew: &EmittedWord) -> Result<(), String> {
    // Structural class check, both directions. An instruction the
    // architecture routes to the FP/SIMD register file must carry an FP/ASIMD
    // cost class, and an integer instruction must not — otherwise a new FP
    // emitter silently inherits the integer ALU price, which is exactly how
    // the scalar float path was mis-priced before P8R.1.
    let encoded_fp = crate::encode::is_fp_simd_word(ew.word);
    if encoded_fp != ew.rule.is_fp_simd() {
        return Err(format!(
            "{fn_key}[{index}] `{}` is {} an FP/ASIMD class but word {:#010x} ({}) is {} an \
             FP/SIMD instruction",
            ew.rule.as_str(),
            if ew.rule.is_fp_simd() { "" } else { "not" },
            ew.word,
            ew.text,
            if encoded_fp { "" } else { "not" },
        ));
    }
    let srcs = ew.src_slice();
    match ew.rule {
        CostRule::Load | CostRule::LoadAcquire | CostRule::Store | CostRule::StoreRelease => {
            if !is_load_store(ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] memory rule does not match opcode"
                ));
            }
            let width = crate::encode::access_width_bytes(ew.word).unwrap_or(0);
            if ew.access_bytes != width {
                return Err(format!(
                    "{fn_key}[{index}] access width metadata={} encoded={width}",
                    ew.access_bytes
                ));
            }
            if ew.rule == CostRule::LoadAcquire && !is_ldar(ew.word) {
                return Err(format!("{fn_key}[{index}] load-acquire is not LDAR/LDAXR"));
            }
            if ew.rule == CostRule::StoreRelease && !is_stlr(ew.word) {
                return Err(format!("{fn_key}[{index}] store-release is not STLR/STLXR"));
            }
            if ew.rule == CostRule::Load && is_ldar(ew.word) {
                return Err(format!("{fn_key}[{index}] LDAR tagged as ordinary load"));
            }
            if ew.rule == CostRule::Store && is_stlr(ew.word) {
                return Err(format!("{fn_key}[{index}] STLR tagged as ordinary store"));
            }
            if ew.rule.is_load() {
                expect_dst(ew, true, RegBank::Gpr)?;
                if !contains(srcs, Reg::gpr(rn(ew.word))) {
                    return Err(format!(
                        "{fn_key}[{index}] load base x{} is absent from srcs {srcs:?}",
                        rn(ew.word)
                    ));
                }
            } else {
                let store_exclusive = ew.word & 0x3fe0_fc00 == 0x0800_fc00;
                let expected_dst =
                    store_exclusive.then(|| Reg::gpr(((ew.word >> 16) & 0x1f) as u8));
                if ew.dst != expected_dst {
                    return Err(format!(
                        "{fn_key}[{index}] store destination {:?} does not match the encoded \
                         exclusive-status destination {expected_dst:?}",
                        ew.dst
                    ));
                }
                if !contains(srcs, Reg::gpr(rn(ew.word))) || !contains(srcs, Reg::gpr(rd(ew.word)))
                {
                    return Err(format!(
                        "{fn_key}[{index}] store base/data are absent from srcs {srcs:?}"
                    ));
                }
            }
        }
        CostRule::FpLoad | CostRule::FpLoadQ | CostRule::FpStore | CostRule::FpStoreQ => {
            if !is_load_store(ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] FP memory rule does not match opcode"
                ));
            }
            let width = crate::encode::access_width_bytes(ew.word).unwrap_or(0);
            if ew.access_bytes != width {
                return Err(format!(
                    "{fn_key}[{index}] access width metadata={} encoded={width}",
                    ew.access_bytes
                ));
            }
            let quad = matches!(ew.rule, CostRule::FpLoadQ | CostRule::FpStoreQ);
            if quad != (width == 16) {
                return Err(format!(
                    "{fn_key}[{index}] `{}` disagrees with the encoded {width}-byte access",
                    ew.rule.as_str()
                ));
            }
            if ew.rule.is_load() {
                expect_dst(ew, true, RegBank::FpSimd)?;
                require_src(fn_key, index, ew, Reg::gpr(rn(ew.word)))?;
            } else {
                if ew.dst.is_some() {
                    return Err(format!("{fn_key}[{index}] FP store has a destination"));
                }
                require_src(fn_key, index, ew, Reg::gpr(rn(ew.word)))?;
                require_src(fn_key, index, ew, Reg::fp(rd(ew.word)))?;
            }
        }
        CostRule::AsimdInt
        | CostRule::AsimdPermute
        | CostRule::AsimdFpAddSub
        | CostRule::AsimdFpMul
        | CostRule::AsimdFpFma
        | CostRule::AsimdFpCmp
        | CostRule::AsimdFpCvt => {
            if !asimd_data_processing_matches(ew.rule, ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] `{}` does not match its declared ASIMD encoding \
                     word={:#010x} text={}",
                    ew.rule.as_str(),
                    ew.word,
                    ew.text
                ));
            }
            expect_dst(ew, true, RegBank::FpSimd)?;
            require_src(fn_key, index, ew, Reg::fp(rn(ew.word)))?;
            // One-source forms encode Rm as part of the opcode/immediate.
            let one_source = matches!(ew.rule, CostRule::AsimdFpCvt)
                || ew.word & 0xffff_fc00 == 0x4e04_0400
                || ew.word & 0xff80_fc00 == 0x4f00_0400;
            if !one_source {
                require_src(fn_key, index, ew, Reg::fp(rm(ew.word)))?;
            }
            let bsl = ew.word & 0xbf20_fc00 == 0x2e20_1c00;
            if matches!(ew.rule, CostRule::AsimdFpFma) || bsl {
                // FMLA's accumulator and BSL's mask are read as well as written.
                require_src(fn_key, index, ew, Reg::fp(rd(ew.word)))?;
            }
        }
        CostRule::FpAddSub | CostRule::FpMul | CostRule::FpFma | CostRule::FpDivSqrt => {
            if !scalar_fp_data_processing_matches(ew.rule, ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] `{}` does not match its declared scalar FP encoding \
                     word={:#010x} text={}",
                    ew.rule.as_str(),
                    ew.word,
                    ew.text
                ));
            }
            expect_dst(ew, true, RegBank::FpSimd)?;
            require_src(fn_key, index, ew, Reg::fp(rn(ew.word)))?;
            let one_source = ew.rule == CostRule::FpAddSub && ew.word & 0xffbf_fc00 == 0x1e21_4000
                || ew.rule == CostRule::FpDivSqrt && ew.word & 0xffbf_fc00 == 0x1e21_c000;
            if !one_source {
                require_src(fn_key, index, ew, Reg::fp(rm(ew.word)))?;
            }
            if ew.rule == CostRule::FpFma {
                require_src(fn_key, index, ew, Reg::fp(ra(ew.word)))?;
            }
        }
        CostRule::FpCompare => {
            if !scalar_fp_data_processing_matches(ew.rule, ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] `fp_compare` does not match FCMP word={:#010x}",
                    ew.word
                ));
            }
            if ew.dst.is_some() || !ew.flags.writes() {
                return Err(format!(
                    "{fn_key}[{index}] FCMP must have no destination and must write NZCV"
                ));
            }
            require_src(fn_key, index, ew, Reg::fp(rn(ew.word)))?;
            require_src(fn_key, index, ew, Reg::fp(rm(ew.word)))?;
        }
        CostRule::FpMove => {
            if !scalar_fp_data_processing_matches(ew.rule, ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] `fp_move` does not match FMOV word={:#010x}",
                    ew.word
                ));
            }
            let to_gpr = matches!(ew.word & 0xffff_fc00, 0x1e26_0000 | 0x9e66_0000);
            let dst_bank = if to_gpr {
                RegBank::Gpr
            } else {
                RegBank::FpSimd
            };
            let src_bank = if to_gpr {
                RegBank::FpSimd
            } else {
                RegBank::Gpr
            };
            expect_dst(ew, true, dst_bank)?;
            require_src(
                fn_key,
                index,
                ew,
                Reg {
                    bank: src_bank,
                    num: rn(ew.word),
                },
            )?;
        }
        CostRule::FpConvert => {
            if !scalar_fp_data_processing_matches(ew.rule, ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] `fp_convert` does not match a sealed conversion \
                     word={:#010x}",
                    ew.word
                ));
            }
            let fixed = ew.word & 0xffff_fc00;
            let fp_to_fp = matches!(fixed, 0x1e22_c000 | 0x1e62_4000);
            let fp_to_int = matches!(
                fixed,
                0x1e38_0000
                    | 0x1e39_0000
                    | 0x9e78_0000
                    | 0x9e79_0000
                    | 0x9e38_0000
                    | 0x9e39_0000
                    | 0x1e78_0000
                    | 0x1e79_0000
            );
            let dst_bank = if fp_to_int {
                RegBank::Gpr
            } else {
                RegBank::FpSimd
            };
            let src_bank = if fp_to_fp || fp_to_int {
                RegBank::FpSimd
            } else {
                RegBank::Gpr
            };
            expect_dst(ew, true, dst_bank)?;
            require_src(
                fn_key,
                index,
                ew,
                Reg {
                    bank: src_bank,
                    num: rn(ew.word),
                },
            )?;
        }
        CostRule::MovWide => {
            if !is_mov_wide(ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] mov-wide rule does not match opcode"
                ));
            }
            expect_dst(ew, true, RegBank::Gpr)?;
            if is_movk(ew.word) {
                if !contains(srcs, Reg::gpr(rd(ew.word))) {
                    return Err(format!(
                        "{fn_key}[{index}] MOVK must read its destination x{}",
                        rd(ew.word)
                    ));
                }
            } else if !srcs.is_empty() {
                return Err(format!(
                    "{fn_key}[{index}] MOVZ/MOVN unexpectedly reads {:?}",
                    srcs
                ));
            }
        }
        CostRule::Adrp => {
            if !is_adrp(ew.word) && !is_adr(ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] address rule does not match ADR/ADRP"
                ));
            }
            expect_dst(ew, true, RegBank::Gpr)?;
        }
        CostRule::Call => {
            if !is_bl(ew.word) {
                return Err(format!("{fn_key}[{index}] call is not BL"));
            }
            if ew.dst != Some(Reg::gpr(0)) {
                return Err(format!("{fn_key}[{index}] returning call must declare x0"));
            }
        }
        CostRule::Branch | CostRule::Abort | CostRule::AbortVal => {
            if !is_control(ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] control rule does not transfer control word={:#010x} text={}",
                    ew.word, ew.text
                ));
            }
            let conditional = ew.word & 0xff00_0010 == 0x5400_0000;
            let conditional_reads_flags = conditional && (ew.word & 0xf) < 14;
            let compare_branch =
                ew.word & 0x7e00_0000 == 0x3400_0000 || ew.word & 0x7e00_0000 == 0x3600_0000;
            let register_branch =
                ew.word & 0xffff_fc1f == 0xd61f_0000 || ew.word & 0xffff_fc1f == 0xd65f_0000;
            if conditional_reads_flags && ew.flags != FlagEffect::Read {
                return Err(format!(
                    "{fn_key}[{index}] conditional branch does not read NZCV metadata"
                ));
            }
            if !conditional_reads_flags && ew.flags == FlagEffect::Read {
                return Err(format!(
                    "{fn_key}[{index}] non-conditional branch reads NZCV metadata"
                ));
            }
            if compare_branch {
                require_src(fn_key, index, ew, Reg::gpr(rd(ew.word)))?;
            }
            if register_branch {
                require_src(fn_key, index, ew, Reg::gpr(rn(ew.word)))?;
            }
        }
        CostRule::Mul | CostRule::MulW => {
            if ew.word & 0x1f00_0000 != 0x1b00_0000 {
                return Err(format!(
                    "{fn_key}[{index}] multiply rule does not match opcode"
                ));
            }
            audit_three_reg(fn_key, index, ew, ra(ew.word) != 31)?;
        }
        CostRule::MulHigh => {
            if ew.word & 0x1f00_0000 != 0x1b00_0000 || ra(ew.word) != 31 {
                return Err(format!(
                    "{fn_key}[{index}] mul-high rule does not match opcode"
                ));
            }
            audit_three_reg(fn_key, index, ew, false)?;
        }
        CostRule::Sdiv | CostRule::Udiv => {
            if ew.word & 0x1fe0_0000 != 0x1ac0_0000 {
                return Err(format!(
                    "{fn_key}[{index}] divide rule does not match opcode"
                ));
            }
            audit_three_reg(fn_key, index, ew, false)?;
        }
        CostRule::Barrier => {
            if !is_barrier(ew.word) {
                return Err(format!("{fn_key}[{index}] barrier rule does not match DMB"));
            }
        }
        CostRule::System => {
            if !is_system(ew.word) {
                return Err(format!("{fn_key}[{index}] system rule does not match BRK"));
            }
        }
        _ => {}
    }

    if ew.flags == FlagEffect::Read {
        let conditional = ew.word & 0xff00_0010 == 0x5400_0000;
        if !conditional && ew.rule == CostRule::Branch {
            return Err(format!(
                "{fn_key}[{index}] branch reads NZCV without a conditional opcode"
            ));
        }
    }
    if ew.flags == FlagEffect::Write && is_control(ew.word) {
        return Err(format!(
            "{fn_key}[{index}] control transfer writes NZCV metadata"
        ));
    }
    if ew.rule.is_load() || ew.rule.is_store() {
        if ew.mem.is_none() {
            return Err(format!(
                "{fn_key}[{index}] memory instruction has no MemRef"
            ));
        }
    }
    Ok(())
}

pub fn audit_program(program: &CodegenProgram) -> Result<(), String> {
    for (key, f) in &program.fns {
        for (i, ew) in f.code.iter().enumerate() {
            audit_word(key, i, ew)?;
        }
    }
    Ok(())
}

pub fn audit_linked(linked: &crate::linked::LinkedProgram) -> Result<(), String> {
    for (key, f) in &linked.fns {
        for (i, ew) in f.code.iter().enumerate() {
            audit_word(key, i, ew)?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::MemRef;
    use crate::encode;

    fn vector_load(vector: u8, base: u8) -> EmittedWord {
        EmittedWord::banked(
            encode::enc_ldr_q_imm(vector, base, 0),
            format!("ldr q{vector}, [x{base}, #0]"),
            CostRule::FpLoadQ,
            Some(Reg::fp(vector)),
            &[Reg::gpr(base)],
        )
        .with_mem(MemRef::flow_frame(0, 0, base))
    }

    #[test]
    fn an_fp_instruction_tagged_with_an_integer_class_fails_closed() {
        let mut word = vector_load(0, 9);
        word.rule = CostRule::Load;
        let error = audit_word("f", 0, &word).expect_err("a mis-classed FP load must fail");
        assert!(error.contains("FP/SIMD instruction"), "{error}");

        // And the converse: an integer word carrying an FP class.
        let mut integer = EmittedWord::banked(
            encode::enc_ldr_x_imm(0, 31, 0),
            "ldr x0, [sp, #0]".to_string(),
            CostRule::FpLoad,
            Some(Reg::fp(0)),
            &[Reg::gpr(31)],
        )
        .with_mem(MemRef::stack(0));
        let error = audit_word("f", 0, &integer).expect_err("an integer load is not FP");
        assert!(error.contains("FP/ASIMD class"), "{error}");
        integer.rule = CostRule::Load;
        integer.dst = Some(Reg::gpr(0));
        audit_word("f", 0, &integer).expect("the integer form audits");
    }

    #[test]
    fn a_quad_class_must_match_its_encoded_access_width() {
        let mut word = vector_load(1, 9);
        word.rule = CostRule::FpLoad;
        let error = audit_word("f", 0, &word).expect_err("s-form class over a q-form word");
        assert!(error.contains("16-byte access"), "{error}");
    }

    #[test]
    fn an_asimd_class_must_match_its_declared_encoding() {
        let permute = EmittedWord::banked(
            encode::enc_uzp1_v4s(0, 0, 1),
            "uzp1 v0.4s, v0.4s, v1.4s".to_string(),
            CostRule::AsimdPermute,
            Some(Reg::fp(0)),
            &[Reg::fp(0), Reg::fp(1)],
        );
        audit_word("f", 0, &permute).expect("uzp1 audits as a permute");

        let mut mislabelled = permute.clone();
        mislabelled.rule = CostRule::AsimdInt;
        let error = audit_word("f", 0, &mislabelled).expect_err("uzp1 is not ASIMD arithmetic");
        assert!(error.contains("declared ASIMD encoding"), "{error}");
    }

    #[test]
    fn scalar_fp_classes_reject_operation_relabelling() {
        let add = EmittedWord::banked(
            encode::enc_fadd(2, 0, 1, false),
            "fadd s2, s0, s1".to_string(),
            CostRule::FpAddSub,
            Some(Reg::fp(2)),
            &[Reg::fp(0), Reg::fp(1)],
        );
        audit_word("f", 0, &add).expect("the scalar add audits");

        for wrong in [
            CostRule::FpMul,
            CostRule::FpFma,
            CostRule::FpDivSqrt,
            CostRule::FpCompare,
            CostRule::FpConvert,
            CostRule::FpMove,
        ] {
            let mut relabelled = add.clone();
            relabelled.rule = wrong;
            assert!(
                audit_word("f", 0, &relabelled).is_err(),
                "fadd must not audit as {}",
                wrong.as_str()
            );
        }

        let compare = EmittedWord::banked(
            encode::enc_fcmp(0, 1, false),
            "fcmp s0, s1".to_string(),
            CostRule::FpCompare,
            None,
            &[Reg::fp(0), Reg::fp(1)],
        )
        .with_flags(FlagEffect::Write);
        audit_word("f", 1, &compare).expect("the scalar compare audits");
    }

    #[test]
    fn an_fp_store_must_name_its_base_and_its_data_register() {
        let store = EmittedWord::banked(
            encode::enc_str_q_imm(2, 11, 0),
            "str q2, [x11, #0]".to_string(),
            CostRule::FpStoreQ,
            None,
            &[Reg::gpr(11), Reg::fp(2)],
        )
        .with_mem(MemRef::flow_frame(0, 0, 11));
        audit_word("f", 0, &store).expect("the canonical packet store audits");

        // The general register of the same number is a different register.
        let mut aliased = store.clone();
        aliased.set_srcs(&[Reg::gpr(11), Reg::gpr(2)]);
        let error = audit_word("f", 0, &aliased).expect_err("x2 does not satisfy v2");
        assert!(error.contains("v2"), "{error}");
    }
}
