#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cond {
    Eq,
    Ne,
    Cs,
    Cc,
    Mi,
    Pl,
    Vs,
    Vc,
    Hi,
    Ls,
    Ge,
    Lt,
    Gt,
    Le,
    Al,
    Nv,
}

impl Cond {
    fn encoding(self) -> u32 {
        match self {
            Cond::Eq => 0b0000,
            Cond::Ne => 0b0001,
            Cond::Cs => 0b0010,
            Cond::Cc => 0b0011,
            Cond::Mi => 0b0100,
            Cond::Pl => 0b0101,
            Cond::Vs => 0b0110,
            Cond::Vc => 0b0111,
            Cond::Hi => 0b1000,
            Cond::Ls => 0b1001,
            Cond::Ge => 0b1010,
            Cond::Lt => 0b1011,
            Cond::Gt => 0b1100,
            Cond::Le => 0b1101,
            Cond::Al => 0b1110,
            Cond::Nv => 0b1111,
        }
    }

    pub(crate) fn invert(self) -> Cond {
        match self {
            Cond::Eq => Cond::Ne,
            Cond::Ne => Cond::Eq,
            Cond::Cs => Cond::Cc,
            Cond::Cc => Cond::Cs,
            Cond::Mi => Cond::Pl,
            Cond::Pl => Cond::Mi,
            Cond::Vs => Cond::Vc,
            Cond::Vc => Cond::Vs,
            Cond::Hi => Cond::Ls,
            Cond::Ls => Cond::Hi,
            Cond::Ge => Cond::Lt,
            Cond::Lt => Cond::Ge,
            Cond::Gt => Cond::Le,
            Cond::Le => Cond::Gt,
            Cond::Al => Cond::Nv,
            Cond::Nv => Cond::Al,
        }
    }
}

fn reg(r: u8) -> u32 {
    assert!(r <= 31, "register out of range: {r}");
    r as u32
}

fn sf_bit(sf: bool) -> u32 {
    if sf { 1 } else { 0 }
}

fn ldr_str_imm(size: u32, opc: u32, imm12: u32, rn: u8, rt: u8) -> u32 {
    assert!(imm12 < 4096, "imm12 out of range: {imm12}");
    (size << 30) | (0b111001 << 24) | (opc << 22) | (imm12 << 10) | (reg(rn) << 5) | reg(rt)
}

fn scaled_offset(byte_offset: u16, scale: u16) -> u32 {
    assert!(
        byte_offset % scale == 0,
        "offset {byte_offset} not a multiple of {scale}"
    );
    (byte_offset / scale) as u32
}

pub fn enc_str_x_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b11, 0b00, scaled_offset(byte_offset, 8), rn, rt)
}

pub fn enc_ldr_x_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b11, 0b01, scaled_offset(byte_offset, 8), rn, rt)
}

pub fn enc_ldr_x_reg(rt: u8, rn: u8, rm: u8) -> u32 {
    0xf860_6800 | (reg(rm) << 16) | (reg(rn) << 5) | reg(rt)
}

pub fn enc_str_x_reg(rt: u8, rn: u8, rm: u8) -> u32 {
    0xf820_6800 | (reg(rm) << 16) | (reg(rn) << 5) | reg(rt)
}

pub fn enc_ldr_x_reg_scaled(rt: u8, rn: u8, rm: u8) -> u32 {
    enc_ldr_x_reg(rt, rn, rm) | (1 << 12)
}

pub fn enc_str_x_reg_scaled(rt: u8, rn: u8, rm: u8) -> u32 {
    enc_str_x_reg(rt, rn, rm) | (1 << 12)
}

pub fn enc_str_w_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b10, 0b00, scaled_offset(byte_offset, 4), rn, rt)
}

pub fn enc_ldr_w_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b10, 0b01, scaled_offset(byte_offset, 4), rn, rt)
}

fn ldr_str_fp_imm(size: u32, load: bool, imm12: u32, rn: u8, rt: u8) -> u32 {
    assert!(imm12 < 4096, "imm12 out of range: {imm12}");
    (size << 30)
        | (0b111101 << 24)
        | (u32::from(load) << 22)
        | (imm12 << 10)
        | (reg(rn) << 5)
        | reg(rt)
}

/// Scalar FP load/store forms. The register number names the FP/SIMD bank,
/// which is why these do not reuse the integer load/store helper.
pub fn enc_ldr_fp_imm(rt: u8, rn: u8, byte_offset: u16, double: bool) -> u32 {
    let scale = if double { 8 } else { 4 };
    ldr_str_fp_imm(
        if double { 0b11 } else { 0b10 },
        true,
        scaled_offset(byte_offset, scale),
        rn,
        rt,
    )
}

pub fn enc_str_fp_imm(rt: u8, rn: u8, byte_offset: u16, double: bool) -> u32 {
    let scale = if double { 8 } else { 4 };
    ldr_str_fp_imm(
        if double { 0b11 } else { 0b10 },
        false,
        scaled_offset(byte_offset, scale),
        rn,
        rt,
    )
}

pub fn enc_strb_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b00, 0b00, scaled_offset(byte_offset, 1), rn, rt)
}

pub fn enc_ldrb_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b00, 0b01, scaled_offset(byte_offset, 1), rn, rt)
}

pub fn enc_strh_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b01, 0b00, scaled_offset(byte_offset, 2), rn, rt)
}

pub fn enc_ldrh_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b01, 0b01, scaled_offset(byte_offset, 2), rn, rt)
}

pub fn enc_ldar_w(rt: u8, rn: u8) -> u32 {
    0x88dffc00 | (reg(rn) << 5) | reg(rt)
}

pub fn enc_stlr_w(rt: u8, rn: u8) -> u32 {
    0x889ffc00 | (reg(rn) << 5) | reg(rt)
}

pub fn enc_ldaxr_w(rt: u8, rn: u8) -> u32 {
    0x885ffc00 | (reg(rn) << 5) | reg(rt)
}

pub fn enc_stlxr_w(rs: u8, rt: u8, rn: u8) -> u32 {
    0x8800fc00 | (reg(rs) << 16) | (reg(rn) << 5) | reg(rt)
}

pub fn enc_ldar_x(rt: u8, rn: u8) -> u32 {
    0xc8dffc00 | (reg(rn) << 5) | reg(rt)
}

pub fn enc_stlr_x(rt: u8, rn: u8) -> u32 {
    0xc89ffc00 | (reg(rn) << 5) | reg(rt)
}

pub fn enc_ldaxr_x(rt: u8, rn: u8) -> u32 {
    0xc85ffc00 | (reg(rn) << 5) | reg(rt)
}

pub fn enc_stlxr_x(rs: u8, rt: u8, rn: u8) -> u32 {
    0xc800fc00 | (reg(rs) << 16) | (reg(rn) << 5) | reg(rt)
}

pub fn access_width_bytes(word: u32) -> Option<u8> {
    // Integer load/store pair, including offset, pre-index, and post-index.
    // The cost metadata records the total bytes transferred by the pair.
    if word & 0x3a00_0000 == 0x2800_0000 {
        return Some(if word >> 30 == 0b10 { 16 } else { 8 });
    }
    if matches!(word & 0xffc0_0000, 0x3dc0_0000 | 0x3d80_0000) {
        return Some(16);
    }
    if word & 0x3f00_0000 == 0x3d00_0000 {
        return Some(if word >> 30 == 0b10 { 4 } else { 8 });
    }
    let width = 1u8 << (word >> 30);
    if matches!(word & 0x3fe0_0c00, 0x3860_0800 | 0x3820_0800) {
        return Some(width);
    }
    if word & 0x3F00_0000 == 0x3900_0000 {
        return Some(width);
    }
    const LDAR: u32 = 0x08df_fc00;
    const STLR: u32 = 0x089f_fc00;
    let fixed = word & 0x3FFF_FC00;
    let ldaxr = fixed == 0x085f_fc00;
    let stlxr = word & 0x3fe0_fc00 == 0x0800_fc00;
    if (fixed == LDAR || fixed == STLR || ldaxr || stlxr) && (word >> 30) >= 0b10 {
        return Some(width);
    }
    None
}

/// True for a word in the "Data Processing -- Scalar Floating-Point and
/// Advanced SIMD" top-level encoding group (`op1` = `x111`).
pub fn is_fp_simd_data_processing(word: u32) -> bool {
    (word >> 25) & 0b111 == 0b111
}

/// True for a word in the "Loads and Stores" top-level group (`op1` = `x1x0`).
fn is_load_store_group(word: u32) -> bool {
    (word >> 27) & 1 == 1 && (word >> 25) & 1 == 0
}

/// True for every word that reads or writes the FP/SIMD register file.
///
/// This is the structural half of the cost taxonomy: an instruction the
/// architecture routes to the V pipes must carry an FP/ASIMD cost class, and
/// an integer instruction must not. The auditor checks both directions, so a
/// new FP emitter cannot inherit an integer price by omission.
pub fn is_fp_simd_word(word: u32) -> bool {
    is_fp_simd_data_processing(word)
        // Load/store with V=1 is the SIMD&FP register form.
        || (is_load_store_group(word) && (word >> 26) & 1 == 1)
}

pub fn reads_sp(word: u32) -> bool {
    word & 0x1F80_0000 == 0x1100_0000 && (word >> 5) & 0x1F == 31
}

pub fn enc_dmb_ishst() -> u32 {
    0xd5033abf
}

pub fn enc_dmb_ishld() -> u32 {
    0xd50339bf
}

fn ldp_stp(opc: u32, l: u32, imm7: u32, rt2: u8, rn: u8, rt: u8) -> u32 {
    (opc << 30)
        | (0b101 << 27)
        | (0b010 << 23)
        | (l << 22)
        | ((imm7 & 0x7F) << 15)
        | (reg(rt2) << 10)
        | (reg(rn) << 5)
        | reg(rt)
}

fn signed_scaled_offset(byte_offset: i16, scale: i16, half_range: i16) -> u32 {
    assert!(
        byte_offset % scale == 0,
        "offset {byte_offset} not a multiple of {scale}"
    );
    assert!(
        (-half_range..half_range).contains(&byte_offset),
        "offset {byte_offset} out of the imm7 range"
    );
    ((byte_offset / scale) as i32 & 0x7F) as u32
}

pub fn enc_stp_x(rt: u8, rt2: u8, rn: u8, byte_offset: i16) -> u32 {
    ldp_stp(
        0b10,
        0,
        signed_scaled_offset(byte_offset, 8, 512),
        rt2,
        rn,
        rt,
    )
}

pub fn enc_ldp_x(rt: u8, rt2: u8, rn: u8, byte_offset: i16) -> u32 {
    ldp_stp(
        0b10,
        1,
        signed_scaled_offset(byte_offset, 8, 512),
        rt2,
        rn,
        rt,
    )
}

pub fn enc_stp_x_pre(rt: u8, rt2: u8, rn: u8, byte_offset: i16) -> u32 {
    0xa980_0000
        | (signed_scaled_offset(byte_offset, 8, 512) << 15)
        | (reg(rt2) << 10)
        | (reg(rn) << 5)
        | reg(rt)
}

pub fn enc_ldp_x_post(rt: u8, rt2: u8, rn: u8, byte_offset: i16) -> u32 {
    0xa8c0_0000
        | (signed_scaled_offset(byte_offset, 8, 512) << 15)
        | (reg(rt2) << 10)
        | (reg(rn) << 5)
        | reg(rt)
}

pub fn enc_stp_w(rt: u8, rt2: u8, rn: u8, byte_offset: i16) -> u32 {
    ldp_stp(
        0b00,
        0,
        signed_scaled_offset(byte_offset, 4, 256),
        rt2,
        rn,
        rt,
    )
}

pub fn enc_ldp_w(rt: u8, rt2: u8, rn: u8, byte_offset: i16) -> u32 {
    ldp_stp(
        0b00,
        1,
        signed_scaled_offset(byte_offset, 4, 256),
        rt2,
        rn,
        rt,
    )
}

fn move_wide(sf: bool, opc: u32, shift: u8, imm16: u16, rd: u8) -> u32 {
    assert!(
        matches!(shift, 0 | 16 | 32 | 48),
        "shift must be 0, 16, 32, or 48: {shift}"
    );
    let hw = (shift / 16) as u32;
    (sf_bit(sf) << 31)
        | (opc << 29)
        | (0b100101 << 23)
        | (hw << 21)
        | ((imm16 as u32) << 5)
        | reg(rd)
}

pub fn enc_movz(rd: u8, imm16: u16, shift: u8, sf: bool) -> u32 {
    move_wide(sf, 0b10, shift, imm16, rd)
}

pub fn enc_movk(rd: u8, imm16: u16, shift: u8, sf: bool) -> u32 {
    move_wide(sf, 0b11, shift, imm16, rd)
}

pub fn enc_movn(rd: u8, imm16: u16, shift: u8, sf: bool) -> u32 {
    move_wide(sf, 0b00, shift, imm16, rd)
}

fn add_sub_imm(sf: bool, op: u32, s: u32, imm12: u16, rn: u8, rd: u8) -> u32 {
    assert!(imm12 < 4096, "imm12 out of range: {imm12}");
    (sf_bit(sf) << 31)
        | (op << 30)
        | (s << 29)
        | (0b10001 << 24)
        | ((imm12 as u32) << 10)
        | (reg(rn) << 5)
        | reg(rd)
}

pub fn enc_add_imm(rd: u8, rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 0, 0, imm12, rn, rd)
}

/// AArch64 ADD (immediate) with the architectural `LSL #12` selector.
///
/// Keeping this separate from [`enc_add_imm`] makes byte-valued call sites
/// explicit: `imm12` denotes 4096-byte pages here, not bytes.
pub fn enc_add_imm_lsl12(rd: u8, rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 0, 0, imm12, rn, rd) | (1 << 22)
}

pub fn enc_adds_imm(rd: u8, rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 0, 1, imm12, rn, rd)
}

pub fn enc_sub_imm(rd: u8, rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 1, 0, imm12, rn, rd)
}

pub fn enc_subs_imm(rd: u8, rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 1, 1, imm12, rn, rd)
}

pub fn enc_cmp_imm(rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 1, 1, imm12, rn, 31)
}

pub fn enc_cmn_imm(rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 0, 1, imm12, rn, 31)
}

fn add_sub_reg(sf: bool, op: u32, s: u32, rm: u8, rn: u8, rd: u8) -> u32 {
    (sf_bit(sf) << 31)
        | (op << 30)
        | (s << 29)
        | (0b01011 << 24)
        | (reg(rm) << 16)
        | (reg(rn) << 5)
        | reg(rd)
}

pub fn enc_add_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 0, 0, rm, rn, rd)
}

pub fn enc_adds_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 0, 1, rm, rn, rd)
}

pub fn enc_sub_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 1, 0, rm, rn, rd)
}

pub fn enc_subs_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 1, 1, rm, rn, rd)
}

pub fn enc_cmp_reg(rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 1, 1, rm, rn, 31)
}

pub fn enc_cmn_reg(rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 0, 1, rm, rn, 31)
}

fn madd_msub(sf: bool, rm: u8, o0: u32, ra: u8, rn: u8, rd: u8) -> u32 {
    (sf_bit(sf) << 31)
        | (0b0011011 << 24)
        | (reg(rm) << 16)
        | (o0 << 15)
        | (reg(ra) << 10)
        | (reg(rn) << 5)
        | reg(rd)
}

pub fn enc_mul(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    madd_msub(sf, rm, 0, 31, rn, rd)
}

pub fn enc_msub(rd: u8, rn: u8, rm: u8, ra: u8, sf: bool) -> u32 {
    madd_msub(sf, rm, 1, ra, rn, rd)
}

fn data_proc_2(sf: bool, rm: u8, opcode: u32, rn: u8, rd: u8) -> u32 {
    (sf_bit(sf) << 31)
        | (0b11010110 << 21)
        | (reg(rm) << 16)
        | ((opcode & 0x3F) << 10)
        | (reg(rn) << 5)
        | reg(rd)
}

pub fn enc_udiv(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b000010, rn, rd)
}

pub fn enc_sdiv(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b000011, rn, rd)
}

pub fn enc_fmov_from_gpr(fd: u8, rn: u8, double: bool) -> u32 {
    (if double { 0x9e67_0000 } else { 0x1e27_0000 }) | (reg(rn) << 5) | reg(fd)
}

pub fn enc_fmov_to_gpr(rd: u8, fn_: u8, double: bool) -> u32 {
    (if double { 0x9e66_0000 } else { 0x1e26_0000 }) | (reg(fn_) << 5) | reg(rd)
}

fn fp_data_2(base32: u32, fd: u8, fn_: u8, fm: u8, double: bool) -> u32 {
    (if double { base32 | 0x0040_0000 } else { base32 })
        | (reg(fm) << 16)
        | (reg(fn_) << 5)
        | reg(fd)
}

pub fn enc_fadd(fd: u8, fn_: u8, fm: u8, double: bool) -> u32 {
    fp_data_2(0x1e20_2800, fd, fn_, fm, double)
}

pub fn enc_fsub(fd: u8, fn_: u8, fm: u8, double: bool) -> u32 {
    fp_data_2(0x1e20_3800, fd, fn_, fm, double)
}

pub fn enc_fmul(fd: u8, fn_: u8, fm: u8, double: bool) -> u32 {
    fp_data_2(0x1e20_0800, fd, fn_, fm, double)
}

pub fn enc_fdiv(fd: u8, fn_: u8, fm: u8, double: bool) -> u32 {
    fp_data_2(0x1e20_1800, fd, fn_, fm, double)
}

pub fn enc_fneg(fd: u8, fn_: u8, double: bool) -> u32 {
    (if double { 0x1e61_4000 } else { 0x1e21_4000 }) | (reg(fn_) << 5) | reg(fd)
}

pub fn enc_fcmp(fn_: u8, fm: u8, double: bool) -> u32 {
    (if double { 0x1e60_2000 } else { 0x1e20_2000 }) | (reg(fm) << 16) | (reg(fn_) << 5)
}

pub fn enc_fcvt(fd: u8, fn_: u8, destination_double: bool) -> u32 {
    (if destination_double {
        0x1e22_c000
    } else {
        0x1e62_4000
    }) | (reg(fn_) << 5)
        | reg(fd)
}

pub fn enc_int_to_float(fd: u8, rn: u8, signed: bool, double: bool, wide: bool) -> u32 {
    let base = match (signed, double, wide) {
        (true, false, false) => 0x1e22_0000,
        (false, false, false) => 0x1e23_0000,
        (true, true, true) => 0x9e62_0000,
        (false, true, true) => 0x9e63_0000,
        (true, false, true) => 0x9e22_0000,
        (false, false, true) => 0x9e23_0000,
        (true, true, false) => 0x1e62_0000,
        (false, true, false) => 0x1e63_0000,
    };
    base | (reg(rn) << 5) | reg(fd)
}

pub fn enc_float_to_int(rd: u8, fn_: u8, signed: bool, double: bool, wide: bool) -> u32 {
    let base = match (signed, double, wide) {
        (true, false, false) => 0x1e38_0000,
        (false, false, false) => 0x1e39_0000,
        (true, true, true) => 0x9e78_0000,
        (false, true, true) => 0x9e79_0000,
        (true, false, true) => 0x9e38_0000,
        (false, false, true) => 0x9e39_0000,
        (true, true, false) => 0x1e78_0000,
        (false, true, false) => 0x1e79_0000,
    };
    base | (reg(fn_) << 5) | reg(rd)
}

pub fn enc_lsl_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b001000, rn, rd)
}

pub fn enc_lsr_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b001001, rn, rd)
}

pub fn enc_asr_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b001010, rn, rd)
}

fn mulh(rd: u8, rn: u8, rm: u8, unsigned: bool) -> u32 {
    let op31: u32 = if unsigned { 0b110 } else { 0b010 };
    (1 << 31)
        | (0b11011 << 24)
        | (op31 << 21)
        | (reg(rm) << 16)
        | (31 << 10)
        | (reg(rn) << 5)
        | reg(rd)
}

pub fn enc_smulh(rd: u8, rn: u8, rm: u8) -> u32 {
    mulh(rd, rn, rm, false)
}

pub fn enc_umulh(rd: u8, rn: u8, rm: u8) -> u32 {
    mulh(rd, rn, rm, true)
}

fn logical_shifted_reg(sf: bool, opc: u32, n: bool, rm: u8, rn: u8, rd: u8) -> u32 {
    (sf_bit(sf) << 31)
        | (opc << 29)
        | (0b01010 << 24)
        | ((n as u32) << 21)
        | (reg(rm) << 16)
        | (reg(rn) << 5)
        | reg(rd)
}

pub fn enc_and_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b00, false, rm, rn, rd)
}

pub fn enc_bic_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b00, true, rm, rn, rd)
}

pub fn enc_orr_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b01, false, rm, rn, rd)
}

pub fn enc_eor_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b10, false, rm, rn, rd)
}

pub fn enc_mov_reg(rd: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b01, false, rm, 31, rd)
}

fn bitfield(sf: bool, opc: u32, immr: u32, imms: u32, rn: u8, rd: u8) -> u32 {
    let n = sf_bit(sf);
    (sf_bit(sf) << 31)
        | (opc << 29)
        | (0b100110 << 23)
        | (n << 22)
        | ((immr & 0x3F) << 16)
        | ((imms & 0x3F) << 10)
        | (reg(rn) << 5)
        | reg(rd)
}

fn width_of(sf: bool) -> u32 {
    if sf { 64 } else { 32 }
}

pub fn enc_lsl_imm(rd: u8, rn: u8, shift: u8, sf: bool) -> u32 {
    let width = width_of(sf);
    let shift = shift as u32;
    assert!(shift < width, "shift out of range: {shift}");
    let immr = (width - shift) % width;
    let imms = width - 1 - shift;
    bitfield(sf, 0b10, immr, imms, rn, rd)
}

pub fn enc_lsr_imm(rd: u8, rn: u8, shift: u8, sf: bool) -> u32 {
    let width = width_of(sf);
    let shift = shift as u32;
    assert!(shift < width, "shift out of range: {shift}");
    bitfield(sf, 0b10, shift, width - 1, rn, rd)
}

pub fn enc_asr_imm(rd: u8, rn: u8, shift: u8, sf: bool) -> u32 {
    let width = width_of(sf);
    let shift = shift as u32;
    assert!(shift < width, "shift out of range: {shift}");
    bitfield(sf, 0b00, shift, width - 1, rn, rd)
}

pub fn enc_ubfx(rd: u8, rn: u8, lsb: u8, width: u8, sf: bool) -> u32 {
    let (immr, imms) = bfx_fields(lsb, width, sf);
    bitfield(sf, 0b10, immr, imms, rn, rd)
}

pub fn enc_sbfx(rd: u8, rn: u8, lsb: u8, width: u8, sf: bool) -> u32 {
    let (immr, imms) = bfx_fields(lsb, width, sf);
    bitfield(sf, 0b00, immr, imms, rn, rd)
}

fn bfx_fields(lsb: u8, width: u8, sf: bool) -> (u32, u32) {
    let reg_width = width_of(sf);
    let lsb = lsb as u32;
    let width = width as u32;
    assert!(width >= 1, "bitfield extract width must be >= 1");
    assert!(
        lsb + width <= reg_width,
        "bitfield extract [{lsb}, {}) runs off a {reg_width}-bit register",
        lsb + width
    );
    (lsb, lsb + width - 1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BitmaskImm {
    pub n: u32,
    pub immr: u32,
    pub imms: u32,
}

pub fn decode_bitmask_imm(b: BitmaskImm) -> Option<u64> {
    let n = b.n & 1;
    let imms = b.imms & 0x3F;
    let immr = b.immr & 0x3F;
    let field = (n << 6) | ((!imms) & 0x3F);
    if field == 0 {
        return None;
    }
    let len = 31 - field.leading_zeros() as i32;
    if len < 1 {
        return None;
    }
    let esize = 1u32 << len;
    let levels = (1u32 << len) - 1;
    if (imms & levels) == levels {
        return None;
    }
    let s = imms & levels;
    let r = immr & levels;
    let ones = s + 1;
    let welem: u64 = if ones >= 64 {
        u64::MAX
    } else {
        (1u64 << ones) - 1
    };
    let r = r % esize;
    let rotated = if esize == 64 {
        welem.rotate_right(r)
    } else {
        let mask = (1u64 << esize) - 1;
        ((welem >> r) | (welem << (esize - r) % esize)) & mask
    };
    let mut out = 0u64;
    let mut shift = 0u32;
    while shift < 64 {
        out |= rotated << shift;
        shift += esize;
    }
    Some(out)
}

pub fn encode_bitmask_imm(value: u64) -> Option<BitmaskImm> {
    if value == 0 || value == u64::MAX {
        return None;
    }

    let mut size = 64u32;
    loop {
        size /= 2;
        let mask = (1u64 << size) - 1;
        if (value & mask) != ((value >> size) & mask) {
            size *= 2;
            break;
        }
        if size <= 2 {
            break;
        }
    }

    let mask = u64::MAX >> (64 - size);
    let mut elem = value & mask;

    let (rotation, ones) = if is_shifted_mask(elem) {
        let i = elem.trailing_zeros();
        (i, (elem >> i).trailing_ones())
    } else {
        elem |= !mask;
        if !is_shifted_mask(!elem) {
            return None;
        }
        let clo = (!elem).leading_zeros();
        let i = 64 - clo;
        let cto = clo + elem.trailing_ones() - (64 - size);
        (i, cto)
    };
    if ones == 0 || ones >= size {
        return None;
    }

    let immr = (size - rotation) & (size - 1);
    let nimms = (!(size - 1) << 1) | (ones - 1);
    let n = ((nimms >> 6) & 1) ^ 1;

    let b = BitmaskImm {
        n,
        immr: immr & 0x3F,
        imms: nimms & 0x3F,
    };
    if decode_bitmask_imm(b) == Some(value) {
        Some(b)
    } else {
        None
    }
}

fn is_shifted_mask(x: u64) -> bool {
    x != 0 && (x.wrapping_add(x & x.wrapping_neg()) & x) == 0
}

fn logical_imm(sf: bool, opc: u32, b: BitmaskImm, rn: u8, rd: u8) -> u32 {
    (sf_bit(sf) << 31)
        | (opc << 29)
        | (0b100100 << 23)
        | ((b.n & 1) << 22)
        | ((b.immr & 0x3F) << 16)
        | ((b.imms & 0x3F) << 10)
        | (reg(rn) << 5)
        | reg(rd)
}

pub fn enc_tst_imm(rn: u8, imm: u64) -> Option<u32> {
    let b = encode_bitmask_imm(imm)?;
    Some(logical_imm(true, 0b11, b, rn, 31))
}

pub fn enc_mov_bitmask_imm(rd: u8, imm: u64) -> Option<u32> {
    let b = encode_bitmask_imm(imm)?;
    Some(logical_imm(true, 0b01, b, 31, rd))
}

fn word_offset(byte_offset: i32, bits: u32) -> u32 {
    assert!(byte_offset % 4 == 0, "branch offset not 4-byte aligned");
    let half_range = 1i64 << (bits + 1);
    assert!(
        (-half_range..half_range).contains(&(byte_offset as i64)),
        "branch offset out of range"
    );
    let words = byte_offset / 4;
    (words as u32) & ((1u32 << bits) - 1)
}

pub fn enc_b_cond(cond: Cond, byte_offset: i32) -> u32 {
    (0b0101010 << 25) | (word_offset(byte_offset, 19) << 5) | cond.encoding()
}

fn cbz_cbnz(sf: bool, op: u32, rt: u8, byte_offset: i32) -> u32 {
    (sf_bit(sf) << 31)
        | (0b011010 << 25)
        | (op << 24)
        | (word_offset(byte_offset, 19) << 5)
        | reg(rt)
}

pub fn enc_cbz(rt: u8, byte_offset: i32, sf: bool) -> u32 {
    cbz_cbnz(sf, 0, rt, byte_offset)
}

pub fn enc_cbnz(rt: u8, byte_offset: i32, sf: bool) -> u32 {
    cbz_cbnz(sf, 1, rt, byte_offset)
}

fn cond_select(sf: bool, op: u32, rm: u8, cond: Cond, op2: u32, rn: u8, rd: u8) -> u32 {
    (sf_bit(sf) << 31)
        | (op << 30)
        | (0b11010100 << 21)
        | (reg(rm) << 16)
        | (cond.encoding() << 12)
        | ((op2 & 0x3) << 10)
        | (reg(rn) << 5)
        | reg(rd)
}

pub fn enc_csel(rd: u8, rn: u8, rm: u8, cond: Cond, sf: bool) -> u32 {
    cond_select(sf, 0, rm, cond, 0b00, rn, rd)
}

pub fn enc_csinc(rd: u8, rn: u8, rm: u8, cond: Cond, sf: bool) -> u32 {
    cond_select(sf, 0, rm, cond, 0b01, rn, rd)
}

pub fn enc_cset(rd: u8, cond: Cond, sf: bool) -> u32 {
    cond_select(sf, 0, 31, cond.invert(), 0b01, 31, rd)
}

fn adr_adrp(op: u32, rd: u8, imm21: i32) -> u32 {
    let bits = (imm21 as u32) & 0x1F_FFFF;
    let immlo = bits & 0x3;
    let immhi = bits >> 2;
    (op << 31) | (immlo << 29) | (0b10000 << 24) | (immhi << 5) | reg(rd)
}

pub fn enc_adr(rd: u8, byte_offset: i32) -> u32 {
    adr_adrp(0, rd, byte_offset)
}

pub fn enc_adrp(rd: u8, page_offset: i32) -> u32 {
    adr_adrp(1, rd, page_offset)
}

fn b_bl(op: u32, byte_offset: i32) -> u32 {
    (op << 31) | (0b00101 << 26) | word_offset(byte_offset, 26)
}

pub fn enc_b(byte_offset: i32) -> u32 {
    b_bl(0, byte_offset)
}

pub fn enc_bl(byte_offset: i32) -> u32 {
    b_bl(1, byte_offset)
}

fn br_blr_ret(opc: u32, rn: u8) -> u32 {
    (0b1101011 << 25) | (opc << 21) | (0b11111 << 16) | (reg(rn) << 5)
}

pub fn enc_br(rn: u8) -> u32 {
    br_blr_ret(0b00, rn)
}

pub fn enc_blr(rn: u8) -> u32 {
    br_blr_ret(0b01, rn)
}

pub fn enc_ret(rn: u8) -> u32 {
    br_blr_ret(0b10, rn)
}

pub fn enc_brk(imm16: u16) -> u32 {
    (0b11010100001 << 21) | ((imm16 as u32) << 5)
}

pub fn enc_ldr_q_imm(rt: u8, rn: u8, imm: u16) -> u32 {
    assert!(rt < 32 && rn < 32 && imm % 16 == 0 && imm <= 65_520);
    0x3dc0_0000 | (u32::from(imm / 16) << 10) | (u32::from(rn) << 5) | u32::from(rt)
}

pub fn enc_str_q_imm(rt: u8, rn: u8, imm: u16) -> u32 {
    assert!(rt < 32 && rn < 32 && imm % 16 == 0 && imm <= 65_520);
    0x3d80_0000 | (u32::from(imm / 16) << 10) | (u32::from(rn) << 5) | u32::from(rt)
}

pub fn enc_add_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    assert!(vd < 32 && vn < 32 && vm < 32);
    0x4ea0_8400 | (u32::from(vm) << 16) | (u32::from(vn) << 5) | u32::from(vd)
}

fn asimd_three(base: u32, vd: u8, vn: u8, vm: u8) -> u32 {
    assert!(vd < 32 && vn < 32 && vm < 32);
    base | (u32::from(vm) << 16) | (u32::from(vn) << 5) | u32::from(vd)
}

pub fn enc_sub_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x6ea0_8400, vd, vn, vm)
}

pub fn enc_sshr_v4s(vd: u8, vn: u8, immediate: u8) -> u32 {
    assert!((1..32).contains(&immediate), "sshr immediate out of range");
    0x4f00_0400 | (u32::from(64 - immediate) << 16) | (u32::from(vn) << 5) | u32::from(vd)
}

pub fn enc_and_v16b(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x4e20_1c00, vd, vn, vm)
}

pub fn enc_orr_v16b(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x4ea0_1c00, vd, vn, vm)
}

pub fn enc_bsl_v16b(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x6e60_1c00, vd, vn, vm)
}

pub fn enc_cmgt_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x4ea0_3400, vd, vn, vm)
}

pub fn enc_dup_v4s_element0(vd: u8, vn: u8) -> u32 {
    assert!(vd < 32 && vn < 32);
    0x4e04_0400 | (u32::from(vn) << 5) | u32::from(vd)
}

pub fn enc_fadd_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x4e20_d400, vd, vn, vm)
}

pub fn enc_fsub_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x4ea0_d400, vd, vn, vm)
}

pub fn enc_fmul_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x6e20_dc00, vd, vn, vm)
}

pub fn enc_fmax_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x4e20_f400, vd, vn, vm)
}

pub fn enc_fmin_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x4ea0_f400, vd, vn, vm)
}

pub fn enc_fmla_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x4e20_cc00, vd, vn, vm)
}

pub fn enc_fcmge_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x6e20_e400, vd, vn, vm)
}

pub fn enc_fcmgt_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    asimd_three(0x6ea0_e400, vd, vn, vm)
}

pub fn enc_scvtf_v4s(vd: u8, vn: u8) -> u32 {
    assert!(vd < 32 && vn < 32);
    0x4e21_d800 | (u32::from(vn) << 5) | u32::from(vd)
}

pub fn enc_fcvtzs_v4s(vd: u8, vn: u8) -> u32 {
    assert!(vd < 32 && vn < 32);
    0x4ea1_b800 | (u32::from(vn) << 5) | u32::from(vd)
}

pub fn enc_uzp1_v4s(vd: u8, vn: u8, vm: u8) -> u32 {
    assert!(vd < 32 && vn < 32 && vm < 32);
    0x4e80_1800 | (u32::from(vm) << 16) | (u32::from(vn) << 5) | u32::from(vd)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ret_defaults_to_x30() {
        assert_eq!(enc_ret(30), 0xd65f03c0);
        assert_eq!(enc_ret(0), 0xd65f0000);
    }

    #[test]
    fn pixels_i32x4_words_match_arm_architecture_encodings() {
        assert_eq!(enc_ldr_q_imm(0, 9, 0), 0x3dc0_0120);
        assert_eq!(enc_ldr_q_imm(1, 10, 16), 0x3dc0_0541);
        assert_eq!(enc_uzp1_v4s(0, 0, 3), 0x4e83_1800);
        assert_eq!(enc_add_v4s(2, 0, 1), 0x4ea1_8402);
        assert_eq!(enc_str_q_imm(2, 11, 32), 0x3d80_0962);
    }

    #[test]
    fn p8r_packet_words_match_arm_architecture_encodings() {
        assert_eq!(enc_fadd_v4s(0, 1, 2), 0x4e22_d420);
        assert_eq!(enc_fsub_v4s(3, 4, 5), 0x4ea5_d483);
        assert_eq!(enc_fmul_v4s(6, 7, 8), 0x6e28_dce6);
        assert_eq!(enc_fmax_v4s(9, 10, 11), 0x4e2b_f549);
        assert_eq!(enc_fmin_v4s(12, 13, 14), 0x4eae_f5ac);
        assert_eq!(enc_fmla_v4s(15, 16, 17), 0x4e31_ce0f);
        assert_eq!(enc_fcmge_v4s(18, 19, 20), 0x6e34_e672);
        assert_eq!(enc_fcmgt_v4s(21, 22, 23), 0x6eb7_e6d5);
        assert_eq!(enc_bsl_v16b(18, 0, 1), 0x6e61_1c12);
        assert_eq!(enc_sub_v4s(2, 3, 4), 0x6ea4_8462);
        assert_eq!(enc_sshr_v4s(5, 6, 7), 0x4f39_04c5);
        assert_eq!(enc_and_v16b(7, 8, 9), 0x4e29_1d07);
        assert_eq!(enc_orr_v16b(10, 11, 12), 0x4eac_1d6a);
        assert_eq!(enc_cmgt_v4s(13, 14, 15), 0x4eaf_35cd);
        assert_eq!(enc_scvtf_v4s(16, 17), 0x4e21_da30);
        assert_eq!(enc_fcvtzs_v4s(18, 19), 0x4ea1_ba72);
        assert_eq!(enc_dup_v4s_element0(1, 0), 0x4e04_0401);
    }

    #[test]
    fn add_imm_matches_hand_verified_bits() {
        assert_eq!(enc_add_imm(0, 1, 1, true), 0x91000420);
        assert_eq!(enc_add_imm(2, 3, 5, false), 0x11001462);
        assert_eq!(enc_add_imm_lsl12(0, 1, 1, true), 0x91400420);
    }

    #[test]
    fn sub_and_flags_variants_share_add_imms_bit_layout() {
        assert_eq!(enc_sub_imm(0, 1, 1, true), 0xd1000420);
        assert_eq!(enc_adds_imm(0, 1, 1, true), 0xb1000420);
        assert_eq!(enc_subs_imm(0, 1, 1, true), 0xf1000420);
        assert_eq!(enc_cmp_imm(1, 1, true), 0xf100043f);
        assert_eq!(enc_cmn_imm(1, 1, true), 0xb100043f);
    }

    #[test]
    fn add_sub_reg_and_mov_alias() {
        assert_eq!(enc_add_reg(0, 1, 2, true), 0x8b020020);
        assert_eq!(enc_sub_reg(0, 1, 2, true), 0xcb020020);
        assert_eq!(enc_mov_reg(0, 1, true), 0xaa0103e0);
        assert_eq!(enc_mov_reg(0, 1, false), 0x2a0103e0);
    }

    #[test]
    fn scalar_floating_arithmetic_and_conversions_match_assembler_bits() {
        assert_eq!(enc_fmov_from_gpr(0, 0, false), 0x1e27_0000);
        assert_eq!(enc_fmov_to_gpr(1, 1, false), 0x1e26_0021);
        assert_eq!(enc_fmov_from_gpr(2, 2, true), 0x9e67_0042);
        assert_eq!(enc_fmov_to_gpr(3, 3, true), 0x9e66_0063);
        assert_eq!(enc_fadd(4, 5, 6, false), 0x1e26_28a4);
        assert_eq!(enc_fsub(7, 8, 9, false), 0x1e29_3907);
        assert_eq!(enc_fmul(10, 11, 12, true), 0x1e6c_096a);
        assert_eq!(enc_fdiv(13, 14, 15, true), 0x1e6f_19cd);
        assert_eq!(enc_fneg(16, 17, false), 0x1e21_4230);
        assert_eq!(enc_fneg(18, 19, true), 0x1e61_4272);
        assert_eq!(enc_fcmp(0, 1, false), 0x1e21_2000);
        assert_eq!(enc_fcmp(2, 3, true), 0x1e63_2040);
        assert_eq!(enc_fcvt(4, 5, false), 0x1e62_40a4);
        assert_eq!(enc_fcvt(6, 7, true), 0x1e22_c0e6);
        assert_eq!(enc_int_to_float(0, 0, true, false, false), 0x1e22_0000);
        assert_eq!(enc_int_to_float(3, 3, false, true, true), 0x9e63_0063);
        assert_eq!(enc_float_to_int(4, 4, true, false, false), 0x1e38_0084);
        assert_eq!(enc_float_to_int(7, 7, false, true, true), 0x9e79_00e7);
    }

    #[test]
    fn ldr_str_x_unsigned_offset() {
        assert_eq!(enc_str_x_imm(0, 1, 0), 0xf9000020);
        assert_eq!(enc_ldr_x_imm(0, 1, 0), 0xf9400020);
        assert_eq!(enc_str_x_imm(3, 2, 16), 0xf9000843);
        assert_eq!(enc_ldr_x_imm(3, 2, 16), 0xf9400843);
    }

    #[test]
    fn ldr_str_x_register_offset() {
        assert_eq!(enc_ldr_x_reg(0, 17, 18), 0xf872_6a20);
        assert_eq!(enc_str_x_reg(0, 17, 18), 0xf832_6a20);
        assert_eq!(enc_ldr_x_reg_scaled(0, 18, 17), 0xf871_7a40);
        assert_eq!(enc_str_x_reg_scaled(0, 18, 17), 0xf831_7a40);
    }

    #[test]
    fn ldr_str_w_and_byte_forms() {
        assert_eq!(enc_str_w_imm(0, 1, 0), 0xb9000020);
        assert_eq!(enc_ldr_w_imm(0, 1, 0), 0xb9400020);
        assert_eq!(enc_strb_imm(0, 1, 0), 0x39000020);
        assert_eq!(enc_ldrb_imm(0, 1, 0), 0x39400020);
    }

    #[test]
    fn ldr_str_scalar_fp_unsigned_offset() {
        assert_eq!(enc_str_fp_imm(0, 1, 0, false), 0xbd00_0020);
        assert_eq!(enc_ldr_fp_imm(0, 1, 0, false), 0xbd40_0020);
        assert_eq!(enc_str_fp_imm(2, 3, 16, true), 0xfd00_0862);
        assert_eq!(enc_ldr_fp_imm(2, 3, 16, true), 0xfd40_0862);
    }

    #[test]
    fn reads_sp_distinguishes_sp_from_xzr_at_register_31() {
        assert!(reads_sp(enc_sub_imm(31, 31, 64, true)));
        assert!(reads_sp(enc_add_imm(31, 31, 64, true)));
        assert!(reads_sp(enc_add_imm(0, 31, 0, true)));
        assert!(!reads_sp(enc_add_imm(0, 1, 8, true)));
        assert!(!reads_sp(enc_sub_imm(2, 3, 8, true)));
        assert!(!reads_sp(enc_orr_reg(0, 31, 1, true)));
        assert!(!reads_sp(enc_and_reg(0, 31, 1, true)));
        assert!(!reads_sp(enc_cmp_reg(31, 1, true)));
        assert!(!reads_sp(enc_mul(0, 31, 1, true)));
        assert!(!reads_sp(enc_ldr_x_imm(0, 31, 8)));
        assert!(!reads_sp(enc_str_x_imm(0, 31, 8)));
    }

    #[test]
    fn access_width_round_trips_every_load_store_encoder() {
        let cases: &[(u32, u8, &str)] = &[
            (enc_ldr_x_imm(0, 1, 8), 8, "ldr x"),
            (enc_str_x_imm(0, 1, 8), 8, "str x"),
            (enc_ldr_w_imm(0, 1, 4), 4, "ldr w"),
            (enc_str_w_imm(0, 1, 4), 4, "str w"),
            (enc_ldr_fp_imm(0, 1, 4, false), 4, "ldr s"),
            (enc_str_fp_imm(0, 1, 4, false), 4, "str s"),
            (enc_ldr_fp_imm(0, 1, 8, true), 8, "ldr d"),
            (enc_str_fp_imm(0, 1, 8, true), 8, "str d"),
            (enc_ldrh_imm(0, 1, 2), 2, "ldrh"),
            (enc_strh_imm(0, 1, 2), 2, "strh"),
            (enc_ldrb_imm(0, 1, 1), 1, "ldrb"),
            (enc_strb_imm(0, 1, 1), 1, "strb"),
            (enc_ldar_x(0, 1), 8, "ldar x"),
            (enc_stlr_x(0, 1), 8, "stlr x"),
            (enc_ldar_w(0, 1), 4, "ldar w"),
            (enc_stlr_w(0, 1), 4, "stlr w"),
            (enc_ldaxr_w(0, 1), 4, "ldaxr w"),
            (enc_stlxr_w(2, 0, 1), 4, "stlxr w"),
            (enc_ldaxr_x(0, 1), 8, "ldaxr x"),
            (enc_stlxr_x(2, 0, 1), 8, "stlxr x"),
            (enc_ldp_x(0, 1, 2, 0), 16, "ldp x"),
            (enc_stp_x(0, 1, 2, 0), 16, "stp x"),
            (enc_ldp_w(0, 1, 2, 0), 8, "ldp w"),
            (enc_stp_w(0, 1, 2, 0), 8, "stp w"),
            (enc_stp_x_pre(29, 30, 31, -16), 16, "stp x pre"),
            (enc_ldp_x_post(29, 30, 31, 16), 16, "ldp x post"),
        ];
        for &(word, want, name) in cases {
            assert_eq!(
                access_width_bytes(word),
                Some(want),
                "{name} ({word:#010x}) width"
            );
        }
        for (word, name) in [
            (enc_add_imm(0, 1, 8, true), "add imm"),
            (enc_movz(0, 1, 0, true), "movz"),
            (enc_dmb_ishst(), "dmb ishst"),
            (enc_ret(30), "ret"),
            (0, "zero word"),
        ] {
            assert_eq!(
                access_width_bytes(word),
                None,
                "{name} ({word:#010x}) must not report a transfer width"
            );
        }
    }

    #[test]
    fn dmb_ishst_and_ishld_match_arm_arm() {
        assert_eq!(enc_dmb_ishst(), 0xd5033abf);
        assert_eq!(enc_dmb_ishld(), 0xd50339bf);
    }

    #[test]
    fn interrupt_cell_acquire_release_and_exclusive_forms() {
        assert_eq!(enc_ldar_w(0, 1), 0x88dffc20);
        assert_eq!(enc_stlr_w(0, 1), 0x889ffc20);
        assert_eq!(enc_ldaxr_w(0, 1), 0x885ffc20);
        assert_eq!(enc_stlxr_w(2, 0, 1), 0x8802fc20);
        assert_eq!(enc_ldar_w(3, 4), 0x88dffc83);
        assert_eq!(enc_stlr_w(3, 4), 0x889ffc83);
        assert_eq!(enc_ldaxr_w(3, 4), 0x885ffc83);
        assert_eq!(enc_stlxr_w(5, 3, 4), 0x8805fc83);
        assert_eq!(enc_ldar_x(0, 1), 0xc8dffc20);
        assert_eq!(enc_stlr_x(0, 1), 0xc89ffc20);
        assert_eq!(enc_ldaxr_x(0, 1), 0xc85ffc20);
        assert_eq!(enc_stlxr_x(2, 0, 1), 0xc802fc20);
    }

    #[test]
    fn ldr_str_halfword_forms() {
        assert_eq!(enc_strh_imm(0, 1, 0), 0x79000020);
        assert_eq!(enc_ldrh_imm(0, 1, 0), 0x79400020);
        assert_eq!(enc_strh_imm(2, 9, 0x102), 0x79020522);
        assert_eq!(enc_ldrh_imm(2, 9, 0x102), 0x79420522);
    }

    #[test]
    fn ldp_stp_x_signed_offset() {
        assert_eq!(enc_stp_x(0, 1, 2, 0), 0xa9000440);
        assert_eq!(enc_ldp_x(0, 1, 2, 0), 0xa9400440);
        assert_eq!(enc_stp_x(0, 1, 2, -16), 0xa93f0440);
        assert_eq!(enc_stp_x_pre(29, 30, 31, -16), 0xa9bf7bfd);
        assert_eq!(enc_ldp_x_post(29, 30, 31, 16), 0xa8c17bfd);
    }

    #[test]
    fn ldp_stp_w_signed_offset() {
        assert_eq!(enc_stp_w(0, 1, 2, 0), 0x29000440);
        assert_eq!(enc_ldp_w(0, 1, 2, 0), 0x29400440);
    }

    #[test]
    fn movz_movk_movn_all_shifts() {
        assert_eq!(enc_movz(0, 1, 0, true), 0xd2800020);
        assert_eq!(enc_movk(0, 1, 16, true), 0xf2a00020);
        assert_eq!(enc_movn(0, 1, 0, true), 0x92800020);
        assert_eq!(enc_movz(0, 0xffff, 48, true), 0xd2ffffe0);
    }

    #[test]
    fn mul_msub_div() {
        assert_eq!(enc_mul(0, 1, 2, true), 0x9b027c20);
        assert_eq!(enc_msub(0, 1, 2, 3, true), 0x9b028c20);
        assert_eq!(enc_udiv(0, 1, 2, true), 0x9ac20820);
        assert_eq!(enc_sdiv(0, 1, 2, true), 0x9ac20c20);
    }

    #[test]
    fn smulh_umulh_forms() {
        assert_eq!(enc_smulh(0, 1, 2), 0x9b427c20);
        assert_eq!(enc_umulh(0, 1, 2), 0x9bc27c20);
        assert_eq!(enc_smulh(3, 4, 5), 0x9b457c83);
        assert_eq!(enc_umulh(3, 4, 5), 0x9bc57c83);
    }

    #[test]
    fn logical_reg_forms() {
        assert_eq!(enc_and_reg(0, 1, 2, true), 0x8a020020);
        assert_eq!(enc_orr_reg(0, 1, 2, true), 0xaa020020);
        assert_eq!(enc_eor_reg(0, 1, 2, true), 0xca020020);
        assert_eq!(enc_bic_reg(0, 1, 2, true), 0x8a220020);
    }

    #[test]
    fn shift_reg_forms() {
        assert_eq!(enc_lsl_reg(0, 1, 2, true), 0x9ac22020);
        assert_eq!(enc_lsr_reg(0, 1, 2, true), 0x9ac22420);
        assert_eq!(enc_asr_reg(0, 1, 2, true), 0x9ac22820);
    }

    #[test]
    fn shift_imm_forms() {
        assert_eq!(enc_lsl_imm(0, 1, 3, true), 0xd37df020);
        assert_eq!(enc_lsr_imm(0, 1, 3, true), 0xd343fc20);
        assert_eq!(enc_asr_imm(0, 1, 3, true), 0x9343fc20);
        assert_eq!(enc_lsr_imm(0, 1, 0, true), 0xd340fc20);
    }

    #[test]
    fn b_cond_all_conditions_share_one_shape() {
        assert_eq!(enc_b_cond(Cond::Eq, 0), 0x54000000);
        assert_eq!(enc_b_cond(Cond::Ne, 8), 0x54000041);
        assert_eq!(enc_b_cond(Cond::Al, -4), 0x54ffffee);
    }

    #[test]
    fn cbz_cbnz() {
        assert_eq!(enc_cbz(0, 0, true), 0xb4000000);
        assert_eq!(enc_cbnz(0, 0, true), 0xb5000000);
        assert_eq!(enc_cbz(1, 0, false), 0x34000001);
    }

    #[test]
    fn csel_csinc_cset() {
        assert_eq!(enc_csel(0, 0, 0, Cond::Eq, true), 0x9a800000);
        assert_eq!(enc_csinc(0, 1, 2, Cond::Ne, true), 0x9a821420);
        assert_eq!(enc_cset(0, Cond::Eq, true), 0x9a9f17e0);
    }

    #[test]
    fn adr_adrp() {
        assert_eq!(enc_adr(0, 0), 0x10000000);
        assert_eq!(enc_adrp(0, 0), 0x90000000);
        assert_eq!(enc_adr(1, 4), 0x10000021);
    }

    #[test]
    fn b_bl_unconditional() {
        assert_eq!(enc_b(0), 0x14000000);
        assert_eq!(enc_bl(0), 0x94000000);
        assert_eq!(enc_b(4), 0x14000001);
    }

    #[test]
    fn br_blr_ret_forms() {
        assert_eq!(enc_br(0), 0xd61f0000);
        assert_eq!(enc_blr(0), 0xd63f0000);
        assert_eq!(enc_ret(30), 0xd65f03c0);
    }

    #[test]
    fn brk_forms() {
        assert_eq!(enc_brk(0), 0xd4200000);
        assert_eq!(enc_brk(1), 0xd4200020);
    }

    #[test]
    fn encoding_table_golden() {
        let words: Vec<u32> = vec![
            enc_str_x_imm(0, 31, 0),
            enc_ldr_x_imm(0, 31, 8),
            enc_str_w_imm(1, 31, 0),
            enc_ldr_w_imm(1, 31, 4),
            enc_strb_imm(2, 31, 0),
            enc_ldrb_imm(2, 31, 1),
            enc_stp_x(0, 1, 31, 0),
            enc_ldp_x(0, 1, 31, 0),
            enc_stp_w(0, 1, 31, 0),
            enc_ldp_w(0, 1, 31, 0),
            enc_movz(9, 0x1234, 0, true),
            enc_movk(9, 0x5678, 16, true),
            enc_movn(9, 0, 0, true),
            enc_mov_reg(0, 1, true),
            enc_add_imm(0, 1, 1, true),
            enc_adds_imm(0, 1, 1, true),
            enc_sub_imm(0, 1, 1, true),
            enc_subs_imm(0, 1, 1, true),
            enc_cmp_imm(1, 1, true),
            enc_cmn_imm(1, 1, true),
            enc_add_reg(0, 1, 2, true),
            enc_sub_reg(0, 1, 2, true),
            enc_cmp_reg(1, 2, true),
            enc_cmn_reg(1, 2, true),
            enc_mul(0, 1, 2, true),
            enc_msub(0, 1, 2, 3, true),
            enc_sdiv(0, 1, 2, true),
            enc_udiv(0, 1, 2, true),
            enc_and_reg(0, 1, 2, true),
            enc_orr_reg(0, 1, 2, true),
            enc_eor_reg(0, 1, 2, true),
            enc_lsl_imm(0, 1, 3, true),
            enc_lsr_imm(0, 1, 3, true),
            enc_asr_imm(0, 1, 3, true),
            enc_lsl_reg(0, 1, 2, true),
            enc_lsr_reg(0, 1, 2, true),
            enc_asr_reg(0, 1, 2, true),
            enc_b_cond(Cond::Eq, 0),
            enc_cbz(0, 0, true),
            enc_cbnz(0, 0, true),
            enc_csel(0, 1, 2, Cond::Ne, true),
            enc_csinc(0, 1, 2, Cond::Ne, true),
            enc_cset(0, Cond::Eq, true),
            enc_adr(0, 0),
            enc_adrp(0, 0),
            enc_b(0),
            enc_bl(0),
            enc_br(0),
            enc_blr(0),
            enc_ret(30),
            enc_brk(0),
        ];

        let expected: Vec<u32> = vec![
            0xf90003e0, 0xf94007e0, 0xb90003e1, 0xb94007e1, 0x390003e2, 0x394007e2, 0xa90007e0,
            0xa94007e0, 0x290007e0, 0x294007e0, 0xd2824689, 0xf2aacf09, 0x92800009, 0xaa0103e0,
            0x91000420, 0xb1000420, 0xd1000420, 0xf1000420, 0xf100043f, 0xb100043f, 0x8b020020,
            0xcb020020, 0xeb02003f, 0xab02003f, 0x9b027c20, 0x9b028c20, 0x9ac20c20, 0x9ac20820,
            0x8a020020, 0xaa020020, 0xca020020, 0xd37df020, 0xd343fc20, 0x9343fc20, 0x9ac22020,
            0x9ac22420, 0x9ac22820, 0x54000000, 0xb4000000, 0xb5000000, 0x9a821020, 0x9a821420,
            0x9a9f17e0, 0x10000000, 0x90000000, 0x14000000, 0x94000000, 0xd61f0000, 0xd63f0000,
            0xd65f03c0, 0xd4200000,
        ];

        assert_eq!(words.len(), expected.len());
        for (i, (w, e)) in words.iter().zip(expected.iter()).enumerate() {
            assert_eq!(w, e, "instruction #{i}: got {w:#010x}, expected {e:#010x}");
        }

        let mut bytes = Vec::new();
        for w in &words {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        assert_eq!(bytes.len(), words.len() * 4);
        assert_eq!(&bytes[0..4], &0xf90003e0u32.to_le_bytes());
    }

    #[test]
    fn every_guard_in_this_module_is_unconditional() {
        let src = include_str!("encode.rs");
        let offenders: Vec<&str> = src
            .lines()
            .map(str::trim)
            .filter(|l| l.starts_with("debug_assert!") || l.starts_with("debug_assert_eq!"))
            .collect();
        assert!(
            offenders.is_empty(),
            "a stripped guard here does not produce a wrong number, it produces a valid \
             encoding of a different instruction (module doc). Offending line(s): {offenders:?}"
        );
        assert!(
            src.matches("assert!(").count() >= 12,
            "the range/alignment guards are gone, not merely weakened"
        );
    }

    #[test]
    fn ubfx_sbfx_encode_and_alias_the_shift_pair() {
        assert_eq!(enc_ubfx(0, 1, 0, 8, true), 0xd3401c20);
        assert_eq!(enc_sbfx(0, 1, 0, 8, true), 0x93401c20);
        assert_eq!(enc_ubfx(2, 2, 0, 32, true), 0xd3407c42);
        assert_eq!(enc_sbfx(2, 2, 0, 32, true), 0x93407c42);
    }

    #[test]
    #[should_panic(expected = "runs off a 64-bit register")]
    fn bfx_refuses_a_field_past_the_register() {
        enc_ubfx(0, 1, 40, 32, true);
    }

    #[test]
    fn every_valid_bitmask_immediate_round_trips() {
        let mut seen = std::collections::BTreeSet::new();
        for n in 0..2u32 {
            for immr in 0..64u32 {
                for imms in 0..64u32 {
                    let b = BitmaskImm { n, immr, imms };
                    let Some(v) = decode_bitmask_imm(b) else {
                        continue;
                    };
                    assert_ne!(v, 0, "a decoded bitmask immediate is never 0");
                    assert_ne!(v, u64::MAX, "a decoded bitmask immediate is never ~0");
                    let got = encode_bitmask_imm(v).unwrap_or_else(|| {
                        panic!("{v:#018x} decodes from N={n} immr={immr} imms={imms} but does not encode")
                    });
                    assert_eq!(
                        decode_bitmask_imm(got),
                        Some(v),
                        "encoder produced {got:?} for {v:#018x}, which decodes elsewhere"
                    );
                    seen.insert(v);
                }
            }
        }
        assert_eq!(seen.len(), 5334, "distinct 64-bit bitmask immediates");
    }

    #[test]
    fn every_narrow_high_mask_is_a_bitmask_immediate() {
        for w in [8u32, 16, 32] {
            let mask = !((1u64 << w) - 1);
            let word = enc_tst_imm(2, mask)
                .unwrap_or_else(|| panic!("high mask for {w} bits is not encodable"));
            assert_eq!(word & 0x1F, 31, "TST writes XZR");
            assert_eq!(word >> 29 & 0b11, 0b11, "TST is the ANDS opc");
        }
        assert_eq!(enc_tst_imm(2, 0xFFFF_FFFF_0000_0000), Some(0xf260_7c5f));
        assert_eq!(
            decode_bitmask_imm(BitmaskImm {
                n: 1,
                immr: 32,
                imms: 31
            }),
            Some(0xFFFF_FFFF_0000_0000)
        );
    }

    #[test]
    fn non_bitmask_values_are_refused() {
        for v in [
            0u64,
            u64::MAX,
            5,
            0x1234_5678_9abc_def0,
            0b1011,
            1 << 63 | 1 << 5,
        ] {
            if let Some(b) = encode_bitmask_imm(v) {
                assert_eq!(
                    decode_bitmask_imm(b),
                    Some(v),
                    "{v:#x} encoded to a triple that decodes elsewhere"
                );
            }
        }
        assert_eq!(encode_bitmask_imm(0), None);
        assert_eq!(encode_bitmask_imm(u64::MAX), None);
        assert_eq!(encode_bitmask_imm(0x1234_5678_9abc_def0), None);
        assert_eq!(enc_tst_imm(0, 0x1234_5678_9abc_def0), None);
    }

    #[test]
    fn mov_bitmask_imm_is_orr_from_xzr() {
        let word = enc_mov_bitmask_imm(3, 0xFFFF_FFFF_0000_0000).expect("encodable");
        assert_eq!(word & 0x1F, 3, "Rd");
        assert_eq!(word >> 5 & 0x1F, 31, "Rn = XZR");
        assert_eq!(word >> 29 & 0b11, 0b01, "ORR opc");
        assert_eq!(enc_mov_bitmask_imm(0, 0x1234_5678_9abc_def0), None);
    }

    #[test]
    fn the_release_profile_keeps_assertions_live() {
        let manifest = include_str!("../../../Cargo.toml");
        let release = manifest
            .split("[profile.release]")
            .nth(1)
            .expect("workspace Cargo.toml declares [profile.release]");
        for setting in ["debug-assertions = true", "overflow-checks = true"] {
            assert!(
                release.contains(setting),
                "[profile.release] must keep `{setting}` (Cargo.toml's own comment has the why)"
            );
        }
    }
}
