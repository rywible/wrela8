//! A76 encoder: pure functions from typed instruction arguments to a raw
//! `u32` (the machine word, little-endian on the wire — AArch64 is a
//! fixed-width, little-endian instruction set, so `to_le_bytes()` on the
//! returned `u32` is the whole "assembler").
//!
//! No mnemonic parsing, no assembler, no external cross-check dependency
//! (plans/M5.md decision 5): every function here takes typed register/
//! immediate arguments and returns the exact bit pattern the ARM
//! Architecture Reference Manual (ARM ARM) specifies for that instruction
//! class. Codegen (`codegen.rs`, item C) calls these directly; HVF
//! execution and `diff-eval` are the *behavioral* oracle — this module's
//! own oracle is the ARM ARM's bit layout, pinned by the unit tests below
//! and the one encoding table test at the bottom of this file.
//!
//! Registers are plain `u8` in `0..=31`; by AArch64 convention register 31
//! means either `xzr`/`wzr` (the zero register) or `sp` depending on
//! instruction class — this encoder never special-cases 31, callers pick
//! it deliberately (`enc_cset`/`enc_mov_reg` pass it explicitly for the
//! zero-register aliases they implement). `sf: bool` is the ARM ARM's own
//! `sf` bit: `true` selects the 64-bit (`X`) form, `false` the 32-bit
//! (`W`) form, for every instruction that has both.
//!
//! **Every range/alignment guard below is an unconditional `assert!`, not
//! a `debug_assert!`.** These are not redundant sanity checks that a
//! release build can afford to drop: an out-of-range `imm12`, an
//! unaligned branch distance or an over-wide shift does not merely
//! produce a wrong *number* here — the offending bits OR into a
//! neighbouring instruction field and the function returns a valid
//! encoding of a **different instruction**, which then ships inside a
//! sealed, digest-validated image with nothing downstream able to notice.
//! Stripped guards would make the "fails loudly at encode time rather
//! than silently truncating" promise in the doc comments below true only
//! in debug builds. The workspace `[profile.release]` also turns
//! `debug-assertions` back on for the same reason (see `Cargo.toml`); the
//! `assert!`s here are deliberately independent of that setting, because
//! this is the one module where a stray `--config` must not be able to
//! trade correctness for a handful of cycles.
//!
//! Subset covered (plans/M5.md decision 5 + item A): loads/stores
//! (unsigned-immediate LDR/STR/LDRB/STRB, signed-offset LDP/STP),
//! MOVZ/MOVK/MOVN, the MOV-register alias, ADD/SUB immediate and register
//! (plus their flag-setting/CMP/CMN forms), MUL/MSUB/SDIV/UDIV, AND/ORR/EOR
//! register-only (see the module-level note below on why the immediate
//! form is omitted), LSL/LSR/ASR immediate and register, B.cond (all
//! conditions), CBZ/CBNZ, CSEL/CSINC/CSET, ADR/ADRP, B/BL, BR/BLR/RET,
//! BRK.
//!
//! **Omitted on purpose: AND/ORR/EOR *immediate* forms.** Those encode a
//! "bitmask immediate" — a repeating rotated run of ones packed into
//! `N:immr:imms`, decoded by a non-trivial algorithm (ARM ARM
//! `DecodeBitMasks`). Getting that encoder subtly wrong (an immediate that
//! silently encodes a *different* value than the one asked for) is a
//! correctness trap this milestone does not need to take on: codegen's
//! spill-everything convention (plans/M5.md decision 4) always has the
//! immediate available in a register already (loaded via MOVZ/MOVK), so
//! the register forms below are the only ones item C's codegen calls.
//! Implementing the bitmask-immediate encoder correctly is future work,
//! not a gap in what codegen needs today.

/// AArch64 condition codes (the 4-bit `cond` field shared by `B.cond`,
/// `CSEL`, `CSINC`, ...). Encoding is the ARM ARM's own fixed table;
/// `NV` ("never", `0b1111`) exists in the encoding space but has no
/// mnemonic distinct from `AL` and is never emitted by codegen — included
/// here only so the enum is the complete 16-value space, not a partial
/// one a future caller would have to route around.
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

    /// The condition that fires exactly when `self` does not (used by the
    /// `CSET` alias, which is `CSINC Rd, XZR, XZR, invert(cond)` — ARM
    /// ARM's own alias rule, decision-free bit flip of the low bit for
    /// every paired condition except `AL`/`NV`, which `CSET` never uses;
    /// also `codegen.rs`'s own abort-guard polarity flip).
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

// --- Loads/stores: unsigned immediate offset (LDR/STR/LDRB/STRB) -----------
//
// ARM ARM "LDR/STR (immediate, unsigned offset)": one shared shape,
// `size[31:30] 111001 opc[23:22] imm12[21:10] Rn[9:5] Rt[4:0]`. `size`
// picks the transfer width (`11`=64-bit, `10`=32-bit, `00`=byte); `opc`
// picks store (`00`) vs load (`01`). `imm12` is a *scaled* index — the
// actual byte offset is `imm12 * transfer_size_bytes` — so every function
// below takes a plain byte offset and divides it down, asserting both the
// alignment and the 12-bit range so a bad offset fails loudly at encode
// time rather than silently truncating.

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

/// `STR Xt, [Xn, #byte_offset]` — 64-bit store, unsigned offset (0..32760,
/// multiple of 8).
pub fn enc_str_x_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b11, 0b00, scaled_offset(byte_offset, 8), rn, rt)
}

/// `LDR Xt, [Xn, #byte_offset]` — 64-bit load, unsigned offset.
pub fn enc_ldr_x_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b11, 0b01, scaled_offset(byte_offset, 8), rn, rt)
}

/// `STR Wt, [Xn, #byte_offset]` — 32-bit store, unsigned offset (0..16380,
/// multiple of 4).
pub fn enc_str_w_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b10, 0b00, scaled_offset(byte_offset, 4), rn, rt)
}

/// `LDR Wt, [Xn, #byte_offset]` — 32-bit load, unsigned offset.
pub fn enc_ldr_w_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b10, 0b01, scaled_offset(byte_offset, 4), rn, rt)
}

/// `STRB Wt, [Xn, #byte_offset]` — byte store, unsigned offset (0..4095,
/// unscaled: the transfer size is one byte).
pub fn enc_strb_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b00, 0b00, scaled_offset(byte_offset, 1), rn, rt)
}

/// `LDRB Wt, [Xn, #byte_offset]` — byte load, unsigned offset.
pub fn enc_ldrb_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b00, 0b01, scaled_offset(byte_offset, 1), rn, rt)
}

/// `STRH Wt, [Xn, #byte_offset]` — halfword store, unsigned offset
/// (0..8190, multiple of 2). plans/M7.md item H1: a `WriteOnly[u16]`
/// register is the first thing in this machine that needs a 16-bit
/// access, and 03-hardware.md §2 makes the declared width the *whole* of
/// what picks it — a `u16` register written with a 32-bit store would
/// clobber the neighbouring two bytes of the claim.
pub fn enc_strh_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b01, 0b00, scaled_offset(byte_offset, 2), rn, rt)
}

/// `LDRH Wt, [Xn, #byte_offset]` — halfword load, unsigned offset
/// (zero-extending, like `LDRB`).
pub fn enc_ldrh_imm(rt: u8, rn: u8, byte_offset: u16) -> u32 {
    ldr_str_imm(0b01, 0b01, scaled_offset(byte_offset, 2), rn, rt)
}

// --- plans/M7.md item G: load-acquire / store-release / exclusive RMW -----
//
// ARM ARM "Load-Acquire/Store-Release" and "Load-Acquire Exclusive /
// Store-Release Exclusive" (no offset — the address is `[Xn]` only).
// Ground truth is `as -arch arm64` on macOS (same method the module doc
// names for every other encoder): every constant below is the assembled
// word, not re-derived by hand a second time.
//
// `InterruptCell[T]` (03-hardware.md §6) emits LDAR/STLR for every op —
// pure load/store and RMW (`swap_acquire`, `fetch_or_release`). The
// exclusive pair encoders below are hand-checked and kept for the day
// multicore or nested delivery lands; revision 0.1 does not emit them.
// See `hardware.interrupt-cell` for why acquire/release alone suffice
// under checkpoint-only, single-core, no-nesting delivery.

/// `LDAR Wt, [Xn]` — 32-bit load-acquire. `as`: `ldar w0, [x1]` = `0x88dffc20`.
pub fn enc_ldar_w(rt: u8, rn: u8) -> u32 {
    0x88dffc00 | (reg(rn) << 5) | reg(rt)
}

/// `STLR Wt, [Xn]` — 32-bit store-release. `as`: `stlr w0, [x1]` = `0x889ffc20`.
pub fn enc_stlr_w(rt: u8, rn: u8) -> u32 {
    0x889ffc00 | (reg(rn) << 5) | reg(rt)
}

/// `LDAXR Wt, [Xn]` — 32-bit load-acquire exclusive.
/// `as`: `ldaxr w0, [x1]` = `0x885ffc20`.
pub fn enc_ldaxr_w(rt: u8, rn: u8) -> u32 {
    0x885ffc00 | (reg(rn) << 5) | reg(rt)
}

/// `STLXR Ws, Wt, [Xn]` — 32-bit store-release exclusive; `rs` receives 0
/// on success and 1 on failure (ARM ARM). `as`: `stlxr w2, w0, [x1]` =
/// `0x8802fc20`.
pub fn enc_stlxr_w(rs: u8, rt: u8, rn: u8) -> u32 {
    0x8800fc00 | (reg(rs) << 16) | (reg(rn) << 5) | reg(rt)
}

/// `LDAR Xt, [Xn]` — 64-bit load-acquire. `as`: `ldar x0, [x1]` = `0xc8dffc20`.
pub fn enc_ldar_x(rt: u8, rn: u8) -> u32 {
    0xc8dffc00 | (reg(rn) << 5) | reg(rt)
}

/// `STLR Xt, [Xn]` — 64-bit store-release. `as`: `stlr x0, [x1]` = `0xc89ffc20`.
pub fn enc_stlr_x(rt: u8, rn: u8) -> u32 {
    0xc89ffc00 | (reg(rn) << 5) | reg(rt)
}

/// `LDAXR Xt, [Xn]` — 64-bit load-acquire exclusive.
/// `as`: `ldaxr x0, [x1]` = `0xc85ffc20`.
pub fn enc_ldaxr_x(rt: u8, rn: u8) -> u32 {
    0xc85ffc00 | (reg(rn) << 5) | reg(rt)
}

/// `STLXR Ws, Xt, [Xn]` — 64-bit data, 32-bit status in `rs`.
/// `as`: `stlxr w2, x0, [x1]` = `0xc802fc20`.
pub fn enc_stlxr_x(rs: u8, rt: u8, rn: u8) -> u32 {
    0xc800fc00 | (reg(rs) << 16) | (reg(rn) << 5) | reg(rt)
}

// --- Transfer width of an emitted load/store word ------------------------
//
// plans/M20.md item I needs the **access width** of every emitted
// load/store to price SOG §4.5's two boundaries (a load crossing a 64 B
// cache line, a store crossing a 16 B boundary). The width is *not* a
// semantic classification the way `CostRule` and `MemRef` are: it is a
// field of the encoding, written by the functions above, and this module
// owns that encoding. Reading it back here — rather than passing a second
// `width` argument down ~95 emit sites — is what keeps it from becoming a
// second source of truth that can disagree with the word actually
// emitted. That is the defect freeze 1303 exists to prevent, and it is
// why this is a projection of the emitted word and **not** a parse of the
// mnemonic text (which freeze 1303 forbids, and which nothing here
// touches).
//
// Fails closed: any load/store shape not encoded above returns `None`, so
// adding an `LDP`/`STP`/vector emit site means extending this function
// deliberately rather than silently getting a width of zero.

/// Bytes transferred by an emitted load/store word, or `None` when `word`
/// is not one of the load/store shapes this module encodes.
///
/// Both shapes carry the transfer size in `word[31:30]`
/// (`00`=byte, `01`=halfword, `10`=32-bit, `11`=64-bit):
///
/// * LDR/STR, immediate unsigned offset —
///   `size[31:30] 111001 opc[23:22] imm12 Rn Rt` (bits 29:24 = `0b111001`).
/// * LDAR/STLR — the fixed words above with `size` in the same position.
pub fn access_width_bytes(word: u32) -> Option<u8> {
    let width = 1u8 << (word >> 30);
    // LDR/STR (immediate, unsigned offset): bits 29:24 == 0b111001.
    if word & 0x3F00_0000 == 0x3900_0000 {
        return Some(width);
    }
    // LDAR / STLR: everything but `size`, `Rn` and `Rt` is fixed.
    const LDAR: u32 = 0x08df_fc00;
    const STLR: u32 = 0x089f_fc00;
    let fixed = word & 0x3FFF_FC00;
    if (fixed == LDAR || fixed == STLR) && (word >> 30) >= 0b10 {
        return Some(width);
    }
    None
}

/// Does this encoded word read **SP** at register number 31, rather than
/// XZR?
///
/// Register number 31 is two different things depending on the instruction
/// group, and the cost model has to tell them apart: in the *add/subtract
/// (immediate)* group it names **SP** (that is how `add x0, sp, #0` and
/// `sub sp, sp, #N` are written), and almost everywhere else it names
/// **XZR**, a constant with no producer. Reading it back from the word the
/// encoder already wrote keeps one source of truth — the same reasoning
/// `access_width_bytes` above records, and the reason freeze 1303 forbids
/// inferring this from mnemonic text.
///
/// Add/subtract (immediate) is `sf op S 100010 sh imm12 Rn Rd`, so bits
/// 28:23 are `0b100010`; `Rn` is bits 9:5. This deliberately does **not**
/// cover load/store base registers — a load or store with an SP base is
/// already identified by its `MemRef` carrying `MemClass::Stack`, which is
/// a *proven* SP-relative address rather than an encoding guess.
pub fn reads_sp(word: u32) -> bool {
    word & 0x1F80_0000 == 0x1100_0000 && (word >> 5) & 0x1F == 31
}

// --- Barriers: DMB (plans/M15.md item H, decisions 1080–1085) ------------
//
// ARM ARM "DMB": `1101 0101 0000 0011 0011 CRm[3:0] 1 01 11111`
// (`0xD50330BF` with CRm in bits [11:8]). Option encodings (CRm):
// ISHST=0b1010, ISHLD=0b1001. Ground truth is `as -arch arm64` on macOS
// (same method as the InterruptCell block above). Runtime-only
// `@dmb(ishst)` / `@dmb(ishld)` emit these; no `DMB SY` by default.

/// `DMB ISHST` — inner-shareable store barrier.
/// `as`: `dmb ishst` = `0xd5033abf`.
pub fn enc_dmb_ishst() -> u32 {
    0xd5033abf
}

/// `DMB ISHLD` — inner-shareable load barrier.
/// `as`: `dmb ishld` = `0xd50339bf`.
pub fn enc_dmb_ishld() -> u32 {
    0xd50339bf
}

// --- Loads/stores: register pair, signed offset (LDP/STP) -----------------
//
// ARM ARM "LDP/STP (signed offset)": `opc[31:30] 101 0 010 L[22]
// imm7[21:15] Rt2[14:10] Rn[9:5] Rt[4:0]`. `opc` is `10` for the 64-bit
// (`X`) pair form, `00` for the 32-bit (`W`) form (there is no byte-pair
// form). `imm7` is signed and scaled by the transfer size (8 for `X`, 4
// for `W`); the caller passes a plain signed byte offset and this module
// divides and range-checks it, same shape as the unsigned-offset loads
// above.

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

/// `STP Xt, Xt2, [Xn, #byte_offset]` — 64-bit pair store, signed offset
/// (multiple of 8, -512..504).
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

/// `LDP Xt, Xt2, [Xn, #byte_offset]` — 64-bit pair load, signed offset.
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

/// `STP Wt, Wt2, [Xn, #byte_offset]` — 32-bit pair store, signed offset
/// (multiple of 4, -256..252).
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

/// `LDP Wt, Wt2, [Xn, #byte_offset]` — 32-bit pair load, signed offset.
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

// --- Move wide immediate: MOVZ/MOVK/MOVN -----------------------------------
//
// ARM ARM "Move wide (immediate)": `sf[31] opc[30:29] 100101 hw[22:21]
// imm16[20:5] Rd[4:0]`. `hw` selects which 16-bit lane of the register
// `imm16` loads into, as a *shift amount* (0/16/32/48) rather than a lane
// index — this module keeps that same unit so the argument reads the way
// the mnemonic's own `, lsl #N` suffix does.

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

/// `MOVZ Rd, #imm16, LSL #shift` — sets the selected halfword, zeroes the
/// rest of the register.
pub fn enc_movz(rd: u8, imm16: u16, shift: u8, sf: bool) -> u32 {
    move_wide(sf, 0b10, shift, imm16, rd)
}

/// `MOVK Rd, #imm16, LSL #shift` — sets the selected halfword, leaves
/// every other bit of the register unchanged.
pub fn enc_movk(rd: u8, imm16: u16, shift: u8, sf: bool) -> u32 {
    move_wide(sf, 0b11, shift, imm16, rd)
}

/// `MOVN Rd, #imm16, LSL #shift` — sets the selected halfword, then
/// bitwise-inverts the whole register (used to materialize immediates
/// whose complement is cheaper to express in one halfword).
pub fn enc_movn(rd: u8, imm16: u16, shift: u8, sf: bool) -> u32 {
    move_wide(sf, 0b00, shift, imm16, rd)
}

// --- ADD/SUB (immediate and register), and their flag-setting/CMP/CMN -----
//
// ARM ARM "Add/subtract (immediate)": `sf[31] op[30] S[29] 10001
// shift[23:22] imm12[21:10] Rn[9:5] Rd[4:0]`. `op` picks add (`0`) vs
// subtract (`1`); `S` sets flags (`ADDS`/`SUBS`). This module always
// leaves `shift` (the `imm12, LSL #12` form) at `0` — codegen never needs
// a shifted immediate since MOVZ/MOVK can already materialize any 64-bit
// constant a register would hold.
//
// "Add/subtract (shifted register)" is the identical field layout with
// `01011` in place of `10001` and an `Rm`/no-immediate in place of
// `imm12`; this module always leaves that form's own shift at `LSL #0`
// for the same reason.
//
// `CMP`/`CMN` are the standard ARM ARM aliases: `SUBS`/`ADDS` with `Rd`
// forced to the zero register (`31`) and the result discarded.

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

/// `ADD Rd, Rn, #imm12`.
pub fn enc_add_imm(rd: u8, rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 0, 0, imm12, rn, rd)
}

/// `ADDS Rd, Rn, #imm12` — flag-setting add.
pub fn enc_adds_imm(rd: u8, rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 0, 1, imm12, rn, rd)
}

/// `SUB Rd, Rn, #imm12`.
pub fn enc_sub_imm(rd: u8, rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 1, 0, imm12, rn, rd)
}

/// `SUBS Rd, Rn, #imm12` — flag-setting subtract.
pub fn enc_subs_imm(rd: u8, rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 1, 1, imm12, rn, rd)
}

/// `CMP Rn, #imm12` — alias of `SUBS XZR/WZR, Rn, #imm12`.
pub fn enc_cmp_imm(rn: u8, imm12: u16, sf: bool) -> u32 {
    add_sub_imm(sf, 1, 1, imm12, rn, 31)
}

/// `CMN Rn, #imm12` — alias of `ADDS XZR/WZR, Rn, #imm12`.
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

/// `ADD Rd, Rn, Rm`.
pub fn enc_add_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 0, 0, rm, rn, rd)
}

/// `ADDS Rd, Rn, Rm` — flag-setting add.
pub fn enc_adds_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 0, 1, rm, rn, rd)
}

/// `SUB Rd, Rn, Rm`.
pub fn enc_sub_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 1, 0, rm, rn, rd)
}

/// `SUBS Rd, Rn, Rm` — flag-setting subtract.
pub fn enc_subs_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 1, 1, rm, rn, rd)
}

/// `CMP Rn, Rm` — alias of `SUBS XZR/WZR, Rn, Rm`.
pub fn enc_cmp_reg(rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 1, 1, rm, rn, 31)
}

/// `CMN Rn, Rm` — alias of `ADDS XZR/WZR, Rn, Rm`.
pub fn enc_cmn_reg(rn: u8, rm: u8, sf: bool) -> u32 {
    add_sub_reg(sf, 0, 1, rm, rn, 31)
}

// --- MUL/MSUB/SDIV/UDIV ----------------------------------------------------
//
// ARM ARM "Data-processing (3 source)", `MADD`/`MSUB` shape: `sf[31] 00
// 11011 000[23:21] Rm[20:16] o0[15] Ra[14:10] Rn[9:5] Rd[4:0]`. `MUL` is
// the standard alias `MADD Rd, Rn, Rm, XZR` (`o0=0`, `Ra` forced to `31`);
// `MSUB` (`o0=1`) takes its accumulator explicitly since codegen's checked
// `%`/remainder-style lowering needs the real subtract-from-accumulator
// form, not just the multiply alias.
//
// ARM ARM "Data-processing (2 source)" shape (also LSLV/LSRV/ASRV below):
// `sf[31] 0 0 11010110[28:21] Rm[20:16] opcode[15:10] Rn[9:5] Rd[4:0]`.
// `SDIV`/`UDIV` are `opcode` `0b000011`/`0b000010`.

fn madd_msub(sf: bool, rm: u8, o0: u32, ra: u8, rn: u8, rd: u8) -> u32 {
    (sf_bit(sf) << 31)
        | (0b0011011 << 24)
        | (reg(rm) << 16)
        | (o0 << 15)
        | (reg(ra) << 10)
        | (reg(rn) << 5)
        | reg(rd)
}

/// `MUL Rd, Rn, Rm` — alias of `MADD Rd, Rn, Rm, XZR`.
pub fn enc_mul(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    madd_msub(sf, rm, 0, 31, rn, rd)
}

/// `MSUB Rd, Rn, Rm, Ra` — `Rd = Ra - Rn*Rm`.
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

/// `UDIV Rd, Rn, Rm` — unsigned divide, truncating toward zero.
pub fn enc_udiv(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b000010, rn, rd)
}

/// `SDIV Rd, Rn, Rm` — signed divide, truncating toward zero.
pub fn enc_sdiv(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b000011, rn, rd)
}

/// `LSL Rd, Rn, Rm` — register-controlled left shift (`LSLV`).
pub fn enc_lsl_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b001000, rn, rd)
}

/// `LSR Rd, Rn, Rm` — register-controlled logical right shift (`LSRV`).
pub fn enc_lsr_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b001001, rn, rd)
}

/// `ASR Rd, Rn, Rm` — register-controlled arithmetic right shift (`ASRV`).
pub fn enc_asr_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    data_proc_2(sf, rm, 0b001010, rn, rd)
}

// --- SMULH/UMULH ------------------------------------------------------------
//
// ARM ARM "Data-processing (3 source)", the same shape `MADD`/`MSUB`
// use above, one class over: `sf op54[30:29] 11011[28:24] op31[23:21]
// Rm[20:16] o0[15] Ra[14:10] Rn[9:5] Rd[4:0]`. `SMULH`/`UMULH` compute
// the *high* 64 bits of a full 128-bit product (`sf` is always `1` —
// there is no 32-bit form); `op54` is `0b00` for both (same as plain
// `MUL`/`MADD`/`MSUB` — it is *not* where signed/unsigned lives, a bug
// this module shipped with until real hardware execution caught it, see
// below), `o0` is `0` (unused by this shape), and `Ra` is fixed to `31`
// (unused by this shape, same convention `MUL`'s own `Ra=31` alias
// already follows). The signed/unsigned distinction is encoded in
// `op31` itself: `0b010` for `SMULH`, `0b110` for `UMULH` (i.e. `op31`'s
// own top bit is the unsigned flag, its low two bits `0b10` fixed for
// both — confirmed against `clang -target arm64-apple-macos`'s own
// assembler output for `smulh`/`umulh x0, x1, x2` and a second,
// independent register triple, not merely re-derived by the same
// reasoning that produced the original, wrong version of this function).
// Item C (codegen) is the reason this pair exists at all: 64-bit
// (`U64`/`I64`/`Usize`/`Isize`) checked multiplication has no other way
// to detect overflow without a 128-bit register (plans/M5.md decision
// 4's own "smulh compare").
//
// **Bug history (plans/M5.md item E, found by real HVF execution, never
// by the dump-only goldens that existed before a VMM could run
// anything):** the original version of this function put the signed/
// unsigned distinction in `op54` (bits 30:29, `0b00`/`0b10`) instead of
// in `op31` — self-consistent with its own hand-derived unit tests
// (which made the identical reasoning error), so it passed `cargo test`
// while being wrong on real silicon: `UMULH` decoded as an entirely
// different, unrelated instruction encoding. `emit_arith_checked`'s own
// 64-bit unsigned multiply overflow check (`codegen.rs`) was consequently
// silently broken for every unsigned 64-bit checked `*` — undetectable
// by any golden that only ever dumped text, never booted an image, which
// is exactly why 06-machine.md's own "HVF execution ... is the
// behavioral oracle" framing (decision 5) matters: this is the bug it
// exists to catch.

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

/// `SMULH Rd, Rn, Rm` — the high 64 bits of the signed 128-bit product
/// `Rn * Rm`. Always 64-bit (no `sf` parameter: there is no 32-bit
/// form).
pub fn enc_smulh(rd: u8, rn: u8, rm: u8) -> u32 {
    mulh(rd, rn, rm, false)
}

/// `UMULH Rd, Rn, Rm` — the high 64 bits of the unsigned 128-bit
/// product `Rn * Rm`.
pub fn enc_umulh(rd: u8, rn: u8, rm: u8) -> u32 {
    mulh(rd, rn, rm, true)
}

// --- AND/ORR/EOR (register), MOV (register alias) --------------------------
//
// ARM ARM "Logical (shifted register)": `sf[31] opc[30:29] 01010
// shift[23:22] N[21] Rm[20:16] imm6[15:10] Rn[9:5] Rd[4:0]`. `opc` picks
// `AND`(`00`)/`ORR`(`01`)/`EOR`(`10`); this module always leaves `shift`
// (the shift *kind*, `LSL`/`LSR`/`ASR`/`ROR`) at `LSL` and `imm6` (the
// shift *amount*) at `0`, and `N` (which would flip `AND`/`ORR`/`EOR` to
// their `BIC`/`ORN`/`EON` negated-operand cousins) at `0` — plain
// two-register logical ops are all codegen needs (see the module-level
// note on why the bitmask-immediate forms are omitted entirely).
//
// `MOV Rd, Rm` is the standard alias `ORR Rd, XZR/WZR, Rm`.

fn logical_shifted_reg(sf: bool, opc: u32, n: bool, rm: u8, rn: u8, rd: u8) -> u32 {
    (sf_bit(sf) << 31)
        | (opc << 29)
        | (0b01010 << 24)
        | ((n as u32) << 21)
        | (reg(rm) << 16)
        | (reg(rn) << 5)
        | reg(rd)
}

/// `AND Rd, Rn, Rm`.
pub fn enc_and_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b00, false, rm, rn, rd)
}

/// `BIC Rd, Rn, Rm` — `Rd = Rn & ~Rm`. plans/M7.md item G: multi-vector
/// checkpoint clear (AND-clear only the bit(s) just serviced).
pub fn enc_bic_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b00, true, rm, rn, rd)
}

/// `ORR Rd, Rn, Rm`.
pub fn enc_orr_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b01, false, rm, rn, rd)
}

/// `EOR Rd, Rn, Rm`.
pub fn enc_eor_reg(rd: u8, rn: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b10, false, rm, rn, rd)
}

/// `MOV Rd, Rm` — alias of `ORR Rd, XZR/WZR, Rm`.
pub fn enc_mov_reg(rd: u8, rm: u8, sf: bool) -> u32 {
    logical_shifted_reg(sf, 0b01, false, rm, 31, rd)
}

// --- LSL/LSR/ASR (immediate) -----------------------------------------------
//
// All three are ARM ARM aliases of the "Bitfield" instructions `UBFM`
// (`LSL`/`LSR`) / `SBFM` (`ASR`): `sf[31] opc[30:29] 100110 N[22]
// immr[21:16] imms[15:10] Rn[9:5] Rd[4:0]`, `N` tied to `sf` (both `1` for
// the 64-bit form). `LSR Rd, Rn, #s` is `UBFM Rd, Rn, #s, #(width-1)`;
// `ASR` is the same `immr`/`imms` pair through `SBFM`; `LSL Rd, Rn, #s` is
// `UBFM Rd, Rn, #(-s mod width), #(width-1-s)` — the one alias whose
// `immr`/`imms` are not simply the shift amount itself, because a left
// shift is expressed as a bitfield move of the low `width-s` bits up to
// the top, per the ARM ARM's own `LSL` alias definition.

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

/// `LSL Rd, Rn, #shift` — alias of `UBFM`.
pub fn enc_lsl_imm(rd: u8, rn: u8, shift: u8, sf: bool) -> u32 {
    let width = width_of(sf);
    let shift = shift as u32;
    assert!(shift < width, "shift out of range: {shift}");
    let immr = (width - shift) % width;
    let imms = width - 1 - shift;
    bitfield(sf, 0b10, immr, imms, rn, rd)
}

/// `LSR Rd, Rn, #shift` — alias of `UBFM`.
pub fn enc_lsr_imm(rd: u8, rn: u8, shift: u8, sf: bool) -> u32 {
    let width = width_of(sf);
    let shift = shift as u32;
    assert!(shift < width, "shift out of range: {shift}");
    bitfield(sf, 0b10, shift, width - 1, rn, rd)
}

/// `ASR Rd, Rn, #shift` — alias of `SBFM`.
pub fn enc_asr_imm(rd: u8, rn: u8, shift: u8, sf: bool) -> u32 {
    let width = width_of(sf);
    let shift = shift as u32;
    assert!(shift < width, "shift out of range: {shift}");
    bitfield(sf, 0b00, shift, width - 1, rn, rd)
}

// --- Conditional branch, compare-and-branch --------------------------------
//
// ARM ARM "Conditional branch (immediate)": `0101010 0 imm19[23:5] 0
// cond[3:0]`. `imm19` is a signed, word-aligned (4-byte) offset from the
// instruction's own address; the caller passes a plain signed byte
// distance and this module divides by 4, asserting alignment.
//
// "Compare and branch (immediate)": `sf[31] 011010 op[24] imm19[23:5]
// Rt[4:0]` — `op` picks `CBZ`(`0`)/`CBNZ`(`1`).

fn word_offset(byte_offset: i32, bits: u32) -> u32 {
    assert!(byte_offset % 4 == 0, "branch offset not 4-byte aligned");
    let half_range = 1i64 << (bits + 1); // imm field counts words; +-2^(bits+1) bytes
    assert!(
        (-half_range..half_range).contains(&(byte_offset as i64)),
        "branch offset out of range"
    );
    let words = byte_offset / 4;
    (words as u32) & ((1u32 << bits) - 1)
}

/// `B.cond #byte_offset` — conditional branch, PC-relative signed byte
/// offset (multiple of 4, ±1 MiB).
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

/// `CBZ Rt, #byte_offset` — branch if `Rt == 0`.
pub fn enc_cbz(rt: u8, byte_offset: i32, sf: bool) -> u32 {
    cbz_cbnz(sf, 0, rt, byte_offset)
}

/// `CBNZ Rt, #byte_offset` — branch if `Rt != 0`.
pub fn enc_cbnz(rt: u8, byte_offset: i32, sf: bool) -> u32 {
    cbz_cbnz(sf, 1, rt, byte_offset)
}

// --- Conditional select: CSEL/CSINC/CSET -----------------------------------
//
// ARM ARM "Conditional select": `sf[31] op[30] S[29] 11010100[28:21]
// Rm[20:16] cond[15:12] op2[11:10] Rn[9:5] Rd[4:0]`. `CSEL` is `op=0,
// op2=00`; `CSINC` is `op=0, op2=01` (`Rd = cond ? Rn : Rm+1`). `CSET Rd,
// cond` is the standard alias `CSINC Rd, XZR, XZR, invert(cond)`.

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

/// `CSEL Rd, Rn, Rm, cond` — `Rd = cond ? Rn : Rm`.
pub fn enc_csel(rd: u8, rn: u8, rm: u8, cond: Cond, sf: bool) -> u32 {
    cond_select(sf, 0, rm, cond, 0b00, rn, rd)
}

/// `CSINC Rd, Rn, Rm, cond` — `Rd = cond ? Rn : Rm + 1`.
pub fn enc_csinc(rd: u8, rn: u8, rm: u8, cond: Cond, sf: bool) -> u32 {
    cond_select(sf, 0, rm, cond, 0b01, rn, rd)
}

/// `CSET Rd, cond` — alias of `CSINC Rd, XZR, XZR, invert(cond)`; sets
/// `Rd` to 1 if `cond` holds, else 0.
pub fn enc_cset(rd: u8, cond: Cond, sf: bool) -> u32 {
    cond_select(sf, 0, 31, cond.invert(), 0b01, 31, rd)
}

// --- ADR/ADRP ---------------------------------------------------------------
//
// ARM ARM "PC-relative addressing": `op[31] immlo[30:29] 10000[28:24]
// immhi[23:5] Rd[4:0]`. `op` picks `ADR`(`0`)/`ADRP`(`1`). The 21-bit
// signed immediate is split as `immhi:immlo` (a `<<2`/mask-off-low-2-bits
// split, not a magnitude split) — both functions take that already-
// assembled 21-bit signed value and do the split themselves so the
// caller never has to know the split exists. For `ADR` the value is a
// plain signed byte offset from the instruction's own address; for
// `ADRP` it is a signed *page* count (the instruction computes
// `PC_page_aligned + imm * 4096` — scaling by 4096 is the caller's job,
// matching how codegen will compute it from the image layout, item D).

fn adr_adrp(op: u32, rd: u8, imm21: i32) -> u32 {
    let bits = (imm21 as u32) & 0x1F_FFFF;
    let immlo = bits & 0x3;
    let immhi = bits >> 2;
    (op << 31) | (immlo << 29) | (0b10000 << 24) | (immhi << 5) | reg(rd)
}

/// `ADR Rd, #byte_offset` — PC-relative address, byte-granular.
pub fn enc_adr(rd: u8, byte_offset: i32) -> u32 {
    adr_adrp(0, rd, byte_offset)
}

/// `ADRP Rd, #page_offset` — PC-page-relative address; `page_offset` is a
/// signed count of 4 KiB pages, not bytes.
pub fn enc_adrp(rd: u8, page_offset: i32) -> u32 {
    adr_adrp(1, rd, page_offset)
}

// --- Unconditional branch (immediate): B/BL --------------------------------
//
// ARM ARM "Unconditional branch (immediate)": `op[31] 00101[30:26]
// imm26[25:0]`. `op` picks `B`(`0`)/`BL`(`1`); `imm26` is a signed,
// word-aligned (4-byte) offset from the instruction's own address (±128
// MiB).

fn b_bl(op: u32, byte_offset: i32) -> u32 {
    (op << 31) | (0b00101 << 26) | word_offset(byte_offset, 26)
}

/// `B #byte_offset`.
pub fn enc_b(byte_offset: i32) -> u32 {
    b_bl(0, byte_offset)
}

/// `BL #byte_offset`.
pub fn enc_bl(byte_offset: i32) -> u32 {
    b_bl(1, byte_offset)
}

// --- Unconditional branch (register): BR/BLR/RET ---------------------------
//
// ARM ARM "Unconditional branch (register)": `1101011 0 opc[22:21] 11111
// 0000 00 Rn[9:5] 00000`. `opc` picks `BR`(`00`)/`BLR`(`01`)/`RET`(`10`);
// the `11111` field and the trailing `00000` are fixed padding this class
// reserves for other (unused-here) variants, always zero/`11111` for the
// plain register-branch forms.

fn br_blr_ret(opc: u32, rn: u8) -> u32 {
    (0b1101011 << 25) | (opc << 21) | (0b11111 << 16) | (reg(rn) << 5)
}

/// `BR Rn`.
pub fn enc_br(rn: u8) -> u32 {
    br_blr_ret(0b00, rn)
}

/// `BLR Rn`.
pub fn enc_blr(rn: u8) -> u32 {
    br_blr_ret(0b01, rn)
}

/// `RET Rn` — defaults to `x30` (the link register) at the call site the
/// same way the bare `ret` mnemonic does, but takes `rn` explicitly since
/// this encoder never hardcodes a register choice codegen did not make.
pub fn enc_ret(rn: u8) -> u32 {
    br_blr_ret(0b10, rn)
}

// --- BRK --------------------------------------------------------------------
//
// ARM ARM "Exception generation", `BRK`: `11010100 001 imm16[20:5] 00000`.

/// `BRK #imm16`.
pub fn enc_brk(imm16: u16) -> u32 {
    (0b11010100001 << 21) | ((imm16 as u32) << 5)
}

#[cfg(test)]
mod tests {
    use super::*;

    // Every form below gets at least two hand-verified encodings. Three
    // are cross-checked against externally known-good values first
    // (plans/M5.md item A's own worked examples): `RET` (defaults to
    // `x30`), `ADD Xd, Xn, #imm`, and (informationally, not part of this
    // encoder's subset) `NOP`'s well-known `0xd503201f`, confirmed by hand
    // against the same "Reserved"/"Hints" ARM ARM table this module does
    // not otherwise touch — recorded here only as a sanity check on the
    // bit-counting method, not as a pinned test of unimplemented code.

    #[test]
    fn ret_defaults_to_x30() {
        assert_eq!(enc_ret(30), 0xd65f03c0);
        assert_eq!(enc_ret(0), 0xd65f0000);
    }

    #[test]
    fn add_imm_matches_hand_verified_bits() {
        // add x0, x1, #1 — sf=1,op=0,S=0,10001,shift=00,imm12=1,Rn=1,Rd=0.
        assert_eq!(enc_add_imm(0, 1, 1, true), 0x91000420);
        // add w2, w3, #5 — 32-bit form, different Rd/Rn/imm12.
        assert_eq!(enc_add_imm(2, 3, 5, false), 0x11001462);
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
    fn ldr_str_x_unsigned_offset() {
        assert_eq!(enc_str_x_imm(0, 1, 0), 0xf9000020);
        assert_eq!(enc_ldr_x_imm(0, 1, 0), 0xf9400020);
        assert_eq!(enc_str_x_imm(3, 2, 16), 0xf9000843);
        assert_eq!(enc_ldr_x_imm(3, 2, 16), 0xf9400843);
    }

    #[test]
    fn ldr_str_w_and_byte_forms() {
        assert_eq!(enc_str_w_imm(0, 1, 0), 0xb9000020);
        assert_eq!(enc_ldr_w_imm(0, 1, 0), 0xb9400020);
        assert_eq!(enc_strb_imm(0, 1, 0), 0x39000020);
        assert_eq!(enc_ldrb_imm(0, 1, 0), 0x39400020);
    }

    /// Register 31 is SP in add/subtract (immediate) and XZR nearly
    /// everywhere else, and the scoreboard's SP dependence turns on telling
    /// them apart. Checked against the real encoders, both directions.
    #[test]
    fn reads_sp_distinguishes_sp_from_xzr_at_register_31() {
        // `sub sp, sp, #64` and `add sp, sp, #64` — prologue and epilogue.
        assert!(reads_sp(enc_sub_imm(31, 31, 64, true)));
        assert!(reads_sp(enc_add_imm(31, 31, 64, true)));
        // `add x0, sp, #0` — materializing a frame address; reads SP even
        // though it does not write it.
        assert!(reads_sp(enc_add_imm(0, 31, 0, true)));
        // An add/sub immediate that does not name 31 as `Rn`.
        assert!(!reads_sp(enc_add_imm(0, 1, 8, true)));
        assert!(!reads_sp(enc_sub_imm(2, 3, 8, true)));
        // Register 31 outside this group is XZR, which has no producer:
        // `orr`/`and`/`mul` and the flag-setting register compares all name
        // it without reading SP.
        assert!(!reads_sp(enc_orr_reg(0, 31, 1, true)));
        assert!(!reads_sp(enc_and_reg(0, 31, 1, true)));
        assert!(!reads_sp(enc_cmp_reg(31, 1, true)));
        assert!(!reads_sp(enc_mul(0, 31, 1, true)));
        // A load/store with an SP base is *not* claimed here — that is the
        // `MemClass::Stack` path, a proven address rather than an encoding
        // guess. Asserted so the split stays deliberate.
        assert!(!reads_sp(enc_ldr_x_imm(0, 31, 8)));
        assert!(!reads_sp(enc_str_x_imm(0, 31, 8)));
    }

    /// plans/M20.md item I: the §4.5 alignment term reads the access width
    /// back off the emitted word, so the projection is checked against
    /// **every** load/store encoder in this module rather than trusted.
    /// A shape this module does not encode (`LDP`/`STP`, the exclusives,
    /// anything else) must report `None` so a new emit site has to extend
    /// `access_width_bytes` deliberately.
    #[test]
    fn access_width_round_trips_every_load_store_encoder() {
        let cases: &[(u32, u8, &str)] = &[
            (enc_ldr_x_imm(0, 1, 8), 8, "ldr x"),
            (enc_str_x_imm(0, 1, 8), 8, "str x"),
            (enc_ldr_w_imm(0, 1, 4), 4, "ldr w"),
            (enc_str_w_imm(0, 1, 4), 4, "str w"),
            (enc_ldrh_imm(0, 1, 2), 2, "ldrh"),
            (enc_strh_imm(0, 1, 2), 2, "strh"),
            (enc_ldrb_imm(0, 1, 1), 1, "ldrb"),
            (enc_strb_imm(0, 1, 1), 1, "strb"),
            (enc_ldar_x(0, 1), 8, "ldar x"),
            (enc_stlr_x(0, 1), 8, "stlr x"),
            (enc_ldar_w(0, 1), 4, "ldar w"),
            (enc_stlr_w(0, 1), 4, "stlr w"),
        ];
        for &(word, want, name) in cases {
            assert_eq!(
                access_width_bytes(word),
                Some(want),
                "{name} ({word:#010x}) width"
            );
        }
        // Not encoded as a plain load/store: fail closed with `None`.
        for (word, name) in [
            (enc_ldp_x(0, 1, 2, 0), "ldp x"),
            (enc_stp_x(0, 1, 2, 0), "stp x"),
            (enc_ldp_w(0, 1, 2, 0), "ldp w"),
            (enc_stp_w(0, 1, 2, 0), "stp w"),
            (enc_ldaxr_w(0, 1), "ldaxr w"),
            (enc_stlxr_w(2, 0, 1), "stlxr w"),
            (enc_ldaxr_x(0, 1), "ldaxr x"),
            (enc_stlxr_x(2, 0, 1), "stlxr x"),
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

    /// plans/M15.md item H: DMB ISHST / ISHLD against `as -arch arm64`.
    #[test]
    fn dmb_ishst_and_ishld_match_arm_arm() {
        assert_eq!(enc_dmb_ishst(), 0xd5033abf);
        assert_eq!(enc_dmb_ishld(), 0xd50339bf);
    }

    /// plans/M7.md item G: every InterruptCell encoder word, pinned against
    /// `as -arch arm64` on macOS (see the module-level block above the
    /// encoders). A second register triple confirms the Rn/Rt/Rs fields.
    #[test]
    fn interrupt_cell_acquire_release_and_exclusive_forms() {
        // W forms — `InterruptCell[u32]`'s path.
        assert_eq!(enc_ldar_w(0, 1), 0x88dffc20);
        assert_eq!(enc_stlr_w(0, 1), 0x889ffc20);
        assert_eq!(enc_ldaxr_w(0, 1), 0x885ffc20);
        assert_eq!(enc_stlxr_w(2, 0, 1), 0x8802fc20);
        assert_eq!(enc_ldar_w(3, 4), 0x88dffc83);
        assert_eq!(enc_stlr_w(3, 4), 0x889ffc83);
        assert_eq!(enc_ldaxr_w(3, 4), 0x885ffc83);
        assert_eq!(enc_stlxr_w(5, 3, 4), 0x8805fc83);
        // X forms — kept for the 64-bit cell path; same assembler oracle.
        assert_eq!(enc_ldar_x(0, 1), 0xc8dffc20);
        assert_eq!(enc_stlr_x(0, 1), 0xc89ffc20);
        assert_eq!(enc_ldaxr_x(0, 1), 0xc85ffc20);
        assert_eq!(enc_stlxr_x(2, 0, 1), 0xc802fc20);
    }

    /// plans/M7.md item H1: the halfword forms a `ReadOnly[u16]`/
    /// `WriteOnly[u16]` register needs. Same `size`/`opc` table as every
    /// form above, `size = 0b01`, scaled by 2 — hand-checked against the
    /// ARM ARM rather than against this encoder.
    #[test]
    fn ldr_str_halfword_forms() {
        assert_eq!(enc_strh_imm(0, 1, 0), 0x79000020);
        assert_eq!(enc_ldrh_imm(0, 1, 0), 0x79400020);
        // The scaled offset field: byte offset 0x102 is imm12 = 0x81.
        assert_eq!(enc_strh_imm(2, 9, 0x102), 0x79020522);
        assert_eq!(enc_ldrh_imm(2, 9, 0x102), 0x79420522);
    }

    #[test]
    fn ldp_stp_x_signed_offset() {
        assert_eq!(enc_stp_x(0, 1, 2, 0), 0xa9000440);
        assert_eq!(enc_ldp_x(0, 1, 2, 0), 0xa9400440);
        // Negative offset exercises the imm7 two's-complement field.
        assert_eq!(enc_stp_x(0, 1, 2, -16), 0xa93f0440);
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
        // Confirmed against `clang -target arm64-apple-macos`'s own
        // assembler (`smulh`/`umulh x0, x1, x2` and a second, independent
        // register triple) — not merely re-derived by hand a second time,
        // which is exactly how the original version of this test passed
        // while `enc_umulh` was actually wrong (module doc's own "bug
        // history" note on `mulh`, above): `smulh x0, x1, x2` is `mul x0,
        // x1, x2`'s own encoding (`0x9b027c20`, Ra=31, op31=`000`) with
        // `op31` changed from `000` to `010` (`+ 0b010 << 21` =
        // `0x00400000`), giving `0x9b427c20`; `umulh` sets `op31`'s own
        // top bit instead (`110` vs `010`, `+ 0b100 << 21` = `0x00800000`
        // on top of `smulh`'s own value), giving `0x9bc27c20` — `op54`
        // (bits 30:29) is `0b00` for both, identical to plain `mul`.
        assert_eq!(enc_smulh(0, 1, 2), 0x9b427c20);
        assert_eq!(enc_umulh(0, 1, 2), 0x9bc27c20);
        // A second, independent pair (distinct Rd/Rn/Rm) confirms the
        // register fields land in the right places, not just Rd=0.
        assert_eq!(enc_smulh(3, 4, 5), 0x9b457c83);
        assert_eq!(enc_umulh(3, 4, 5), 0x9bc57c83);
    }

    #[test]
    fn logical_reg_forms() {
        assert_eq!(enc_and_reg(0, 1, 2, true), 0x8a020020);
        assert_eq!(enc_orr_reg(0, 1, 2, true), 0xaa020020);
        assert_eq!(enc_eor_reg(0, 1, 2, true), 0xca020020);
        // Hand-checked against `as -arch arm64`: `bic x0, x1, x2` →
        // `0x8a220020` (AND's encoding with N=1).
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
        // shift #0 is representable (immr=0, imms=width-1) even though
        // codegen never needs a no-op shift; the encoder does not special
        // case it.
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

    // --- One encoding golden: a fixed instruction list, review-visible as
    // an inline hex table (plans/M5.md item A's own "dumbest reviewable
    // form" call — recorded in the pinned rules under
    // `compiler.encode.a76-pinned`). Every line is one instruction from
    // the subset, chosen to touch every encoder function at least once;
    // the expected column is the literal bit math, not copied from any
    // external tool.
    #[test]
    fn encoding_table_golden() {
        let words: Vec<u32> = vec![
            enc_str_x_imm(0, 31, 0),            // str x0, [sp]
            enc_ldr_x_imm(0, 31, 8),            // ldr x0, [sp, #8]
            enc_str_w_imm(1, 31, 0),            // str w1, [sp]
            enc_ldr_w_imm(1, 31, 4),            // ldr w1, [sp, #4]
            enc_strb_imm(2, 31, 0),             // strb w2, [sp]
            enc_ldrb_imm(2, 31, 1),             // ldrb w2, [sp, #1]
            enc_stp_x(0, 1, 31, 0),             // stp x0, x1, [sp]
            enc_ldp_x(0, 1, 31, 0),             // ldp x0, x1, [sp]
            enc_stp_w(0, 1, 31, 0),             // stp w0, w1, [sp]
            enc_ldp_w(0, 1, 31, 0),             // ldp w0, w1, [sp]
            enc_movz(9, 0x1234, 0, true),       // movz x9, #0x1234
            enc_movk(9, 0x5678, 16, true),      // movk x9, #0x5678, lsl #16
            enc_movn(9, 0, 0, true),            // movn x9, #0
            enc_mov_reg(0, 1, true),            // mov x0, x1
            enc_add_imm(0, 1, 1, true),         // add x0, x1, #1
            enc_adds_imm(0, 1, 1, true),        // adds x0, x1, #1
            enc_sub_imm(0, 1, 1, true),         // sub x0, x1, #1
            enc_subs_imm(0, 1, 1, true),        // subs x0, x1, #1
            enc_cmp_imm(1, 1, true),            // cmp x1, #1
            enc_cmn_imm(1, 1, true),            // cmn x1, #1
            enc_add_reg(0, 1, 2, true),         // add x0, x1, x2
            enc_sub_reg(0, 1, 2, true),         // sub x0, x1, x2
            enc_cmp_reg(1, 2, true),            // cmp x1, x2
            enc_cmn_reg(1, 2, true),            // cmn x1, x2
            enc_mul(0, 1, 2, true),             // mul x0, x1, x2
            enc_msub(0, 1, 2, 3, true),         // msub x0, x1, x2, x3
            enc_sdiv(0, 1, 2, true),            // sdiv x0, x1, x2
            enc_udiv(0, 1, 2, true),            // udiv x0, x1, x2
            enc_and_reg(0, 1, 2, true),         // and x0, x1, x2
            enc_orr_reg(0, 1, 2, true),         // orr x0, x1, x2
            enc_eor_reg(0, 1, 2, true),         // eor x0, x1, x2
            enc_lsl_imm(0, 1, 3, true),         // lsl x0, x1, #3
            enc_lsr_imm(0, 1, 3, true),         // lsr x0, x1, #3
            enc_asr_imm(0, 1, 3, true),         // asr x0, x1, #3
            enc_lsl_reg(0, 1, 2, true),         // lsl x0, x1, x2
            enc_lsr_reg(0, 1, 2, true),         // lsr x0, x1, x2
            enc_asr_reg(0, 1, 2, true),         // asr x0, x1, x2
            enc_b_cond(Cond::Eq, 0),            // b.eq .
            enc_cbz(0, 0, true),                // cbz x0, .
            enc_cbnz(0, 0, true),               // cbnz x0, .
            enc_csel(0, 1, 2, Cond::Ne, true),  // csel x0, x1, x2, ne
            enc_csinc(0, 1, 2, Cond::Ne, true), // csinc x0, x1, x2, ne
            enc_cset(0, Cond::Eq, true),        // cset x0, eq
            enc_adr(0, 0),                      // adr x0, .
            enc_adrp(0, 0),                     // adrp x0, .
            enc_b(0),                           // b .
            enc_bl(0),                          // bl .
            enc_br(0),                          // br x0
            enc_blr(0),                         // blr x0
            enc_ret(30),                        // ret
            enc_brk(0),                         // brk #0
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

        // Bytes are little-endian on the wire (AArch64 is a fixed-width
        // LE instruction set) — this is the "assembler" step codegen and
        // emission (item D) rely on.
        let mut bytes = Vec::new();
        for w in &words {
            bytes.extend_from_slice(&w.to_le_bytes());
        }
        assert_eq!(bytes.len(), words.len() * 4);
        assert_eq!(&bytes[0..4], &0xf90003e0u32.to_le_bytes());
    }

    /// plans/M9.md item RR: the encoder's guards are unconditional
    /// `assert!`s, and nothing in this module may quietly relax them back
    /// to `debug_assert!`. Grepping this file's own source is the same
    /// self-audit shape `sema::intrinsics` uses to keep its surface list
    /// honest — a reviewer editing a guard has to edit this test too.
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
        // The guards exist at all — a future refactor that deletes them
        // outright would otherwise pass the check above vacuously.
        assert!(
            src.matches("assert!(").count() >= 12,
            "the range/alignment guards are gone, not merely weakened"
        );
    }

    /// The workspace keeps `debug-assertions` on in release for the same
    /// reason (`Cargo.toml`'s own comment): `layout.rs` states 51 more
    /// placement invariants as `debug_assert!`, and those *are* profile-
    /// dependent. This locks the profile so a release build cannot go back
    /// to stripping them.
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
