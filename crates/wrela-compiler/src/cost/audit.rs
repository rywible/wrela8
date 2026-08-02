//! A small metadata auditor for the AArch64 subset emitted by Wrela.
//!
//! This is not a disassembler.  It checks only the architectural fields that
//! the cost scheduler relies on and fails closed when a tagged word cannot be
//! reconciled with its tag.

use crate::codegen::CodegenProgram;
use crate::cost::{CostRule, EmittedWord, FlagEffect};

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
        && (word & 0x3f00_0000 == 0x3900_0000
            || (word & 0x3fff_fc00 == 0x08df_fc00)
            || (word & 0x3fff_fc00 == 0x089f_fc00))
}

fn is_ldar(word: u32) -> bool {
    let fixed = word & 0x3fff_fc00;
    fixed == 0x08df_fc00 || fixed == 0xc8df_fc00
}

fn is_stlr(word: u32) -> bool {
    let fixed = word & 0x3fff_fc00;
    fixed == 0x089f_fc00 || fixed == 0xc89f_fc00
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

fn expect_dst(ew: &EmittedWord, required: bool) -> Result<(), String> {
    if required {
        let Some(dst) = ew.dst else {
            return Err("encoded destination has no metadata destination".to_string());
        };
        if dst != rd(ew.word) {
            return Err(format!(
                "encoded destination x{} disagrees with metadata x{}",
                rd(ew.word),
                dst
            ));
        }
    }
    Ok(())
}

fn contains(srcs: &[u8], reg: u8) -> bool {
    srcs.iter().any(|&r| r == reg)
}

fn require_src(fn_key: &str, index: usize, ew: &EmittedWord, reg: u8) -> Result<(), String> {
    if reg != 31 && !contains(ew.src_slice(), reg) {
        return Err(format!(
            "{fn_key}[{index}] encoded source x{reg} is absent from srcs {:?} ({})",
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
    expect_dst(ew, true)?;
    require_src(fn_key, index, ew, rn(ew.word))?;
    require_src(fn_key, index, ew, rm(ew.word))?;
    if include_accumulator {
        require_src(fn_key, index, ew, ra(ew.word))?;
    }
    Ok(())
}

pub fn audit_word(fn_key: &str, index: usize, ew: &EmittedWord) -> Result<(), String> {
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
                expect_dst(ew, true)?;
                if !contains(srcs, rn(ew.word)) {
                    return Err(format!(
                        "{fn_key}[{index}] load base x{} is absent from srcs {srcs:?}",
                        rn(ew.word)
                    ));
                }
            } else {
                if ew.dst.is_some() {
                    return Err(format!("{fn_key}[{index}] store has a destination"));
                }
                if !contains(srcs, rn(ew.word)) || !contains(srcs, rd(ew.word)) {
                    return Err(format!(
                        "{fn_key}[{index}] store base/data are absent from srcs {srcs:?}"
                    ));
                }
            }
        }
        CostRule::MovWide => {
            if !is_mov_wide(ew.word) {
                return Err(format!(
                    "{fn_key}[{index}] mov-wide rule does not match opcode"
                ));
            }
            expect_dst(ew, true)?;
            if is_movk(ew.word) {
                if !contains(srcs, rd(ew.word)) {
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
            expect_dst(ew, true)?;
        }
        CostRule::Call => {
            if !is_bl(ew.word) {
                return Err(format!("{fn_key}[{index}] call is not BL"));
            }
            if ew.dst != Some(0) {
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
                require_src(fn_key, index, ew, rd(ew.word))?;
            }
            if register_branch {
                require_src(fn_key, index, ew, rn(ew.word))?;
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
