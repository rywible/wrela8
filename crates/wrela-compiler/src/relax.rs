//! Late-link fragments and deterministic relaxation decisions.
//!
//! The wide representation is the default.  Relaxation is expressed as a
//! separate fragment choice so a shortened site never leaves a stale word
//! index in a relocation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::codegen::{CodegenFn, CodegenProgram, Reloc};
use crate::cost::{CostRule, EmittedWord, Reg};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RelaxTarget {
    Immediate(u64),
    Function(String),
    Rodata(usize),
    FixedAddress(u64),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RelaxKind {
    Immediate,
    Address,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Encoding {
    Wide,
    MovZ { shift: u8 },
    MovN { shift: u8 },
    LogicalImmediate,
    MovWide { base_n: bool, lanes: Vec<u8> },
    Adr,
    AdrpAdd,
}

impl Encoding {
    pub fn width(&self) -> usize {
        match self {
            Self::Wide => 4,
            Self::MovZ { .. } | Self::MovN { .. } | Self::LogicalImmediate | Self::Adr => 1,
            Self::MovWide { lanes, .. } => 1 + lanes.len(),
            Self::AdrpAdd => 2,
        }
    }

    pub fn as_str(&self) -> String {
        match self {
            Self::Wide => "wide".to_string(),
            Self::MovZ { shift } => format!("movz@{shift}"),
            Self::MovN { shift } => format!("movn@{shift}"),
            Self::LogicalImmediate => "logical-imm".to_string(),
            Self::MovWide { base_n, lanes } => format!(
                "{}+movk[{}]",
                if *base_n { "movn" } else { "movz" },
                lanes
                    .iter()
                    .map(|l| l.to_string())
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            Self::Adr => "adr".to_string(),
            Self::AdrpAdd => "adrp+add".to_string(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RelaxSite {
    pub ordinal: u32,
    pub kind: RelaxKind,
    pub target: RelaxTarget,
    pub selected: Encoding,
    pub old_width: usize,
    pub frozen_wide: bool,
    pub reason: String,
    pub wide: Vec<EmittedWord>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Fragment {
    Fixed(EmittedWord),
    Relax(RelaxSite),
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct FragmentProgram {
    pub fns: BTreeMap<String, Vec<Fragment>>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct RelaxedProgram {
    pub program: CodegenProgram,
    pub fragments: FragmentProgram,
    pub dump: String,
}

fn is_control_rule(rule: CostRule) -> bool {
    matches!(
        rule,
        CostRule::Branch | CostRule::Call | CostRule::Abort | CostRule::AbortVal
    )
}

fn is_control_word(word: &EmittedWord) -> bool {
    is_control_rule(word.rule)
}

pub(crate) fn reloc_word(reloc: &Reloc) -> usize {
    match reloc {
        Reloc::Rodata { word_adrp, .. } => *word_adrp,
        Reloc::Call { word, .. }
        | Reloc::RodataAdr { word, .. }
        | Reloc::AbortFixed { word }
        | Reloc::AbortVal { word }
        | Reloc::CheckpointService { word }
        | Reloc::TurnFrameAddr { word, .. }
        | Reloc::TurnIdImm { word, .. }
        | Reloc::TurnsBase { word }
        | Reloc::TurnStride { word }
        | Reloc::GroupArenaBase { word }
        | Reloc::IrqVector { word, .. }
        | Reloc::WakePending { word, .. }
        | Reloc::MailboxAddr { word, .. }
        | Reloc::RrCursor { word, .. }
        | Reloc::RingAddr { word, .. }
        | Reloc::DriverState { word, .. }
        | Reloc::DeviceRegsBase { word, .. }
        | Reloc::PoolBase { word, .. }
        | Reloc::PoolSlot { word, .. } => *word,
    }
}

pub(crate) fn remap_reloc(reloc: &Reloc, word: usize) -> Reloc {
    match reloc {
        Reloc::Call { key, .. } => Reloc::Call {
            word,
            key: key.clone(),
        },
        Reloc::Rodata { byte_offset, .. } => Reloc::Rodata {
            word_adrp: word,
            byte_offset: *byte_offset,
        },
        Reloc::RodataAdr { byte_offset, .. } => Reloc::RodataAdr {
            word,
            byte_offset: *byte_offset,
        },
        Reloc::AbortFixed { .. } => Reloc::AbortFixed { word },
        Reloc::AbortVal { .. } => Reloc::AbortVal { word },
        Reloc::CheckpointService { .. } => Reloc::CheckpointService { word },
        Reloc::TurnFrameAddr { key, .. } => Reloc::TurnFrameAddr {
            word,
            key: key.clone(),
        },
        Reloc::TurnIdImm { key, .. } => Reloc::TurnIdImm {
            word,
            key: key.clone(),
        },
        Reloc::TurnsBase { .. } => Reloc::TurnsBase { word },
        Reloc::TurnStride { .. } => Reloc::TurnStride { word },
        Reloc::GroupArenaBase { .. } => Reloc::GroupArenaBase { word },
        Reloc::IrqVector { driver, .. } => Reloc::IrqVector {
            word,
            driver: driver.clone(),
        },
        Reloc::WakePending { driver, .. } => Reloc::WakePending {
            word,
            driver: driver.clone(),
        },
        Reloc::MailboxAddr { actor, field, .. } => Reloc::MailboxAddr {
            word,
            actor: actor.clone(),
            field: *field,
        },
        Reloc::RrCursor { core, .. } => Reloc::RrCursor { word, core: *core },
        Reloc::RingAddr {
            ring_index, field, ..
        } => Reloc::RingAddr {
            word,
            ring_index: *ring_index,
            field: *field,
        },
        Reloc::DriverState { driver, .. } => Reloc::DriverState {
            word,
            driver: driver.clone(),
        },
        Reloc::DeviceRegsBase { device, .. } => Reloc::DeviceRegsBase {
            word,
            device: *device,
        },
        Reloc::PoolBase { pool, .. } => Reloc::PoolBase {
            word,
            pool: pool.clone(),
        },
        Reloc::PoolSlot {
            pool,
            index,
            slot_bytes,
            ..
        } => Reloc::PoolSlot {
            word,
            pool: pool.clone(),
            index: *index,
            slot_bytes: *slot_bytes,
        },
    }
}

fn is_mov_wide(word: &EmittedWord) -> bool {
    word.rule == CostRule::MovWide && word.word & 0x1f80_0000 == 0x1280_0000
}

fn is_movk(word: &EmittedWord) -> bool {
    word.word & 0xff80_0000 == 0xf280_0000
}

fn rd(word: &EmittedWord) -> u8 {
    (word.word & 0x1f) as u8
}

fn imm(word: &EmittedWord) -> u16 {
    ((word.word >> 5) & 0xffff) as u16
}

fn shift(word: &EmittedWord) -> u8 {
    (((word.word >> 21) & 0x3) * 16) as u8
}

fn is_movn(word: &EmittedWord) -> bool {
    word.word & 0xff80_0000 == 0x9280_0000
}

pub fn decode_materialized_value(words: &[EmittedWord]) -> Option<(u8, u64)> {
    let first = words.first()?;
    if !is_mov_wide(first) || is_movk(first) {
        return None;
    }
    let reg = rd(first);
    let mut value = if is_movn(first) { u64::MAX } else { 0 };
    let sh = shift(first) as u32;
    let lane = (imm(first) as u64) << sh;
    if is_movn(first) {
        value &= !(0xffffu64 << sh);
        value |= (!lane) & (0xffffu64 << sh);
    } else {
        value |= lane;
    }
    for word in &words[1..] {
        if !is_movk(word) || rd(word) != reg {
            return None;
        }
        let sh = shift(word) as u32;
        value = (value & !(0xffffu64 << sh)) | ((imm(word) as u64) << sh);
    }
    Some((reg, value))
}

fn lanes(value: u64, base_n: bool) -> Vec<u8> {
    let mut out = Vec::new();
    for lane in 0..4u8 {
        let h = ((value >> (lane * 16)) & 0xffff) as u16;
        let needed = if base_n { h != 0xffff } else { h != 0 };
        if needed {
            out.push(lane);
        }
    }
    out
}

pub fn choose_immediate(reg: u8, value: u64) -> Encoding {
    if crate::encode::enc_mov_bitmask_imm(reg, value).is_some() {
        return Encoding::LogicalImmediate;
    }
    let mut choices = Vec::new();
    for base_n in [false, true] {
        let all = lanes(value, base_n);
        if all.is_empty() {
            choices.push((1usize, base_n, 0u8, Vec::new()));
            continue;
        }
        for &base in &all {
            let rest: Vec<u8> = all.iter().copied().filter(|lane| *lane != base).collect();
            choices.push((1 + rest.len(), base_n, base, rest));
        }
    }
    choices.sort_by_key(|(width, base_n, base, _)| (*width, *base_n, *base));
    let (_, base_n, base, rest) = choices[0].clone();
    if rest.is_empty() {
        if base_n {
            Encoding::MovN { shift: base * 16 }
        } else {
            Encoding::MovZ { shift: base * 16 }
        }
    } else {
        Encoding::MovWide {
            base_n,
            lanes: rest,
        }
    }
}

fn make_word(reg: u8, value: u64, enc: &Encoding, old: &EmittedWord) -> Vec<EmittedWord> {
    let mut out = Vec::new();
    let text = |name: &str| format!("{name} x{reg}, #{value:#x}");
    match enc {
        Encoding::LogicalImmediate => out.push(EmittedWord::gpr(
            crate::encode::enc_mov_bitmask_imm(reg, value).expect("selected logical immediate"),
            text("mov"),
            CostRule::Alu,
            Some(reg),
            &[],
        )),
        Encoding::MovZ { shift } => out.push(EmittedWord::gpr(
            crate::encode::enc_movz(reg, ((value >> *shift) & 0xffff) as u16, *shift, true),
            text("movz"),
            CostRule::MovWide,
            Some(reg),
            &[],
        )),
        Encoding::MovN { shift } => out.push(EmittedWord::gpr(
            crate::encode::enc_movn(reg, ((!value >> *shift) & 0xffff) as u16, *shift, true),
            text("movn"),
            CostRule::MovWide,
            Some(reg),
            &[],
        )),
        Encoding::MovWide {
            base_n,
            lanes: keep_lanes,
        } => {
            let all = lanes(value, *base_n);
            let base = all
                .iter()
                .copied()
                .find(|lane| !keep_lanes.contains(lane))
                .unwrap_or(0);
            if *base_n {
                out.push(EmittedWord::gpr(
                    crate::encode::enc_movn(
                        reg,
                        ((!value >> (base * 16)) & 0xffff) as u16,
                        base * 16,
                        true,
                    ),
                    format!("movn x{reg}"),
                    CostRule::MovWide,
                    Some(reg),
                    &[],
                ));
            } else {
                out.push(EmittedWord::gpr(
                    crate::encode::enc_movz(
                        reg,
                        ((value >> (base * 16)) & 0xffff) as u16,
                        base * 16,
                        true,
                    ),
                    format!("movz x{reg}"),
                    CostRule::MovWide,
                    Some(reg),
                    &[],
                ));
            }
            for lane in keep_lanes {
                out.push(EmittedWord::gpr(
                    crate::encode::enc_movk(
                        reg,
                        ((value >> (*lane * 16)) & 0xffff) as u16,
                        *lane * 16,
                        true,
                    ),
                    format!("movk x{reg}"),
                    CostRule::MovWide,
                    Some(reg),
                    &[reg],
                ));
            }
        }
        Encoding::Wide | Encoding::Adr | Encoding::AdrpAdd => return vec![old.clone()],
    }
    for word in &mut out {
        word.text = if word.rule == CostRule::Alu {
            format!("mov x{reg}, #{value:#x}")
        } else {
            word.text.clone()
        };
    }
    out
}

fn immediate_site_allowed(words: &[EmittedWord]) -> bool {
    words.iter().all(|word| {
        !word.text.contains("rodata")
            && !word.text.contains("addr[")
            && !word.text.contains("state")
            && !word.text.contains("pool")
            && !word.text.contains("turn-")
            && !word.text.contains("frame")
            && !word.text.contains("vector")
            && !word.text.contains("device")
            && !word.text.contains("group-arena")
    })
}

fn fragments_for_fn(key: &str, f: &CodegenFn) -> Vec<Fragment> {
    let mut out = Vec::new();
    let mut ordinal = 0u32;
    let mut i = 0usize;
    while i < f.code.len() {
        if !is_mov_wide(&f.code[i]) {
            out.push(Fragment::Fixed(f.code[i].clone()));
            i += 1;
            continue;
        }
        let start = i;
        let reg = rd(&f.code[i]);
        i += 1;
        while i < f.code.len() && is_movk(&f.code[i]) && rd(&f.code[i]) == reg {
            i += 1;
        }
        let wide = f.code[start..i].to_vec();
        let decoded = decode_materialized_value(&wide);
        let target = decoded
            .map(|(_, value)| RelaxTarget::Immediate(value))
            .unwrap_or(RelaxTarget::FixedAddress(0));
        out.push(Fragment::Relax(RelaxSite {
            ordinal,
            kind: RelaxKind::Immediate,
            target,
            selected: Encoding::Wide,
            old_width: wide.len(),
            frozen_wide: true,
            reason: if decoded.is_some() && immediate_site_allowed(&wide) {
                format!("wide-only site in {key}")
            } else {
                "symbolic or undecodable materialization".to_string()
            },
            wide,
        }));
        ordinal += 1;
    }
    out
}

pub fn make_fragments(program: &CodegenProgram) -> FragmentProgram {
    FragmentProgram {
        fns: program
            .fns
            .iter()
            .map(|(key, f)| (key.clone(), fragments_for_fn(key, f)))
            .collect(),
    }
}

/// Apply value-only immediate relaxation to a copy of a wide program.
fn remap_recorded_block_spans(
    program: &mut CodegenProgram,
    key: &str,
    ranges: &[(usize, usize, usize, usize)],
) -> Result<(), String> {
    let spans = &mut program.origin_spans;
    let old_words = ranges.last().map(|range| range.1).unwrap_or(0);
    if spans
        .iter()
        .filter(|span| span.fn_key == key)
        .any(|span| span.word_start > span.word_end || span.word_end > old_words)
    {
        // Some early width-selecting emitters still report their wide probe
        // offsets.  They are not suitable measured origins; discard this
        // function's spans so the linked representation uses its exact final
        // emitted-basic-block partition instead of retaining a stale index.
        spans.retain(|span| span.fn_key != key);
        return Ok(());
    }
    let map_boundary = |old: usize| -> Result<usize, String> {
        if let Some((_, _, _, new_end)) = ranges.last().filter(|(_, old_end, _, _)| old == *old_end)
        {
            return Ok(*new_end);
        }
        let &(old_start, old_end, new_start, new_end) = ranges
            .iter()
            .find(|(start, end, _, _)| *start <= old && old < *end)
            .ok_or_else(|| {
                format!(
                    "block span boundary {old} fell outside `{key}` (wide_words={})",
                    ranges.last().map(|range| range.1).unwrap_or(0)
                )
            })?;
        if old == old_start {
            return Ok(new_start);
        }
        if old_end - old_start == new_end - new_start {
            return Ok(new_start + (old - old_start));
        }
        Err(format!(
            "block span boundary {old} splits a shortened immediate site in `{key}`"
        ))
    };
    for span in spans.iter_mut().filter(|span| span.fn_key == key) {
        span.word_start = map_boundary(span.word_start)?;
        span.word_end = map_boundary(span.word_end)?;
    }
    Ok(())
}

pub fn relax_immediates(program: &CodegenProgram) -> Result<RelaxedProgram, String> {
    relax_immediates_owned(program.clone())
}

pub(crate) fn relax_immediates_owned(
    mut relaxed: CodegenProgram,
) -> Result<RelaxedProgram, String> {
    let mut fragments = make_fragments(&relaxed);
    let mut dump = String::new();
    for (key, parts) in &mut fragments.fns {
        let (blocked, relocs) = relaxed
            .fns
            .get(key)
            .map_or((false, Vec::new()), |function| {
                (
                    function.code.iter().any(is_control_word),
                    function.relocs.clone(),
                )
            });
        let mut code = Vec::new();
        let mut old_cursor = 0usize;
        let mut ranges: Vec<(usize, usize, usize, usize)> = Vec::new();
        for part in parts {
            let old_start = old_cursor;
            let old_width = match part {
                Fragment::Fixed(_) => 1,
                Fragment::Relax(site) => site.old_width,
            };
            old_cursor += old_width;
            let new_start = code.len();
            let reloc_in_site = relocs
                .iter()
                .any(|reloc| (old_start..old_cursor).contains(&reloc_word(reloc)));
            if blocked || reloc_in_site {
                if let Fragment::Relax(site) = part {
                    site.selected = Encoding::Wide;
                    site.frozen_wide = true;
                    site.reason = if blocked {
                        "control transfer requires relocation-aware layout"
                    } else {
                        "relocation overlaps materialization"
                    }
                    .to_string();
                    let target = match &site.target {
                        RelaxTarget::Immediate(value) => format!(" value={value:#018x}"),
                        _ => String::new(),
                    };
                    let _ = writeln!(
                        dump,
                        "relax fn={key} site={} kind=imm{} encoding=wide width={} old_width={} saved_words=0 reason={} frozen=true",
                        site.ordinal, target, site.old_width, site.old_width, site.reason
                    );
                    code.extend(site.wide.clone());
                    ranges.push((old_start, old_cursor, new_start, code.len()));
                    continue;
                }
            }
            match part {
                Fragment::Fixed(word) => code.push(word.clone()),
                Fragment::Relax(site) => {
                    let RelaxTarget::Immediate(value) = site.target.clone() else {
                        site.reason = "symbolic or undecodable materialization".to_string();
                        site.frozen_wide = true;
                        let _ = writeln!(
                            dump,
                            "relax fn={key} site={} kind=imm encoding=wide width={} old_width={} saved_words=0 reason={} frozen=true",
                            site.ordinal, site.old_width, site.old_width, site.reason,
                        );
                        code.extend(site.wide.clone());
                        ranges.push((old_start, old_cursor, new_start, code.len()));
                        continue;
                    };
                    if !immediate_site_allowed(&site.wide) {
                        site.reason = "symbolic address materialization".to_string();
                        site.frozen_wide = true;
                        let _ = writeln!(
                            dump,
                            "relax fn={key} site={} kind=imm value={value:#018x} encoding=wide width={} old_width={} saved_words=0 reason={} frozen=true",
                            site.ordinal, site.old_width, site.old_width, site.reason,
                        );
                        code.extend(site.wide.clone());
                        ranges.push((old_start, old_cursor, new_start, code.len()));
                        continue;
                    }
                    let (reg, decoded_value) =
                        decode_materialized_value(&site.wide).ok_or_else(|| {
                            format!("cannot decode immediate site {} in `{key}`", site.ordinal)
                        })?;
                    if decoded_value != value {
                        return Err(format!(
                            "immediate site {} changed value while relaxing",
                            site.ordinal
                        ));
                    }
                    let selected = choose_immediate(reg, value);
                    site.selected = selected.clone();
                    site.frozen_wide = false;
                    site.reason = "value-only immediate".to_string();
                    let words = if selected.width() < site.wide.len() {
                        make_word(reg, value, &selected, &site.wide[0])
                    } else {
                        site.selected = Encoding::Wide;
                        site.frozen_wide = true;
                        site.reason = "wide is already shortest".to_string();
                        site.wide.clone()
                    };
                    let _ = writeln!(
                        dump,
                        "relax fn={key} site={} kind=imm value={value:#018x} encoding={} width={} old_width={} saved_words={} reason={} frozen={}",
                        site.ordinal,
                        site.selected.as_str(),
                        words.len(),
                        site.old_width,
                        site.old_width.saturating_sub(words.len()),
                        site.reason,
                        site.frozen_wide
                    );
                    code.extend(words);
                }
            }
            ranges.push((old_start, old_cursor, new_start, code.len()));
        }
        let mut new_relocs = Vec::with_capacity(relocs.len());
        for reloc in &relocs {
            let old = reloc_word(reloc);
            let Some(&(old_start, old_end, new_start, new_end)) = ranges
                .iter()
                .find(|(start, end, _, _)| (*start..*end).contains(&old))
            else {
                return Err(format!("relocation at word {old} fell outside `{key}`"));
            };
            if new_end - new_start != old_end - old_start {
                return Err(format!(
                    "relocation at word {old} lies inside a shortened site in `{key}`"
                ));
            }
            let new_word = new_start + (old - old_start);
            new_relocs.push(remap_reloc(reloc, new_word));
        }
        remap_recorded_block_spans(&mut relaxed, key, &ranges)?;
        let output = relaxed.fns.get_mut(key).expect("fragment function exists");
        output.code = code;
        output.relocs = new_relocs;
    }
    if crate::codegen::block_bridge_enabled() {
        crate::codegen::replace_block_spans(relaxed.origin_spans.clone());
    }
    Ok(RelaxedProgram {
        program: relaxed,
        fragments,
        dump,
    })
}

/// Relax value-only sites in a linked program when the containing function has
/// no PC-relative control transfer.  Such a function has no displacement to
/// repatch after a width change; functions with branches/calls remain wide
/// until the full relocation-aware linker consumes fragments.
pub fn relax_linked_immediates(
    linked: &crate::linked::LinkedProgram,
) -> Result<(crate::linked::LinkedProgram, String), String> {
    let mut out = linked.clone();
    let mut dump = String::new();
    let keys: Vec<String> = out.fns.keys().cloned().collect();
    for key in keys {
        let Some(f) = out.fns.get(&key).cloned() else {
            continue;
        };
        let program = CodegenProgram {
            fns: BTreeMap::from([(
                key.clone(),
                CodegenFn {
                    frame_size: f.frame_size as usize,
                    code: f.code.clone(),
                    relocs: f.relocs.clone(),
                    regions: Vec::new(),
                },
            )]),
            rodata: Vec::new(),
            conventions: BTreeMap::new(),
            origin_spans: f
                .origin_word_ranges
                .iter()
                .map(
                    |&(block_index, word_start, word_end)| crate::codegen::BlockSpan {
                        fn_key: key.clone(),
                        block_index,
                        id: block_index,
                        word_start,
                        word_end,
                    },
                )
                .collect(),
        };
        let relaxed = relax_immediates(&program)?;
        dump.push_str(&relaxed.dump);
        if let Some(new_fn) = relaxed.program.fns.get(&key) {
            if new_fn.code.len() < f.code.len() {
                let output = out.fns.get_mut(&key).expect("linked function exists");
                output.origin_word_ranges = crate::linked::recorded_origin_ranges(
                    &relaxed.program.origin_spans,
                    &key,
                    &new_fn.code,
                );
                output.code = new_fn.code.clone();
                output.relocs = new_fn.relocs.clone();
            }
        }
    }

    for section_id in 0..out.sections.len() {
        let section_address = out.sections[section_id].byte_address;
        let mut functions: Vec<String> = out
            .fns
            .values()
            .filter(|f| f.section == section_id)
            .map(|f| f.key.clone())
            .collect();
        functions.sort_by_key(|key| (out.fns[key].byte_address, key.clone()));
        if functions.is_empty() {
            continue;
        }
        let mut cursor = section_address;
        let mut code = Vec::new();
        for key in functions {
            let f = out.fns.get_mut(&key).expect("function exists");
            f.byte_address = cursor;
            cursor += (f.code.len() as u64) * 4;
            code.extend(f.code.iter().cloned());
        }
        out.sections[section_id].code = code;
    }
    out.image_bytes = out
        .sections
        .iter()
        .map(crate::linked::LinkedSection::end)
        .max()
        .unwrap_or(0)
        .saturating_sub(
            out.sections
                .iter()
                .map(|s| s.byte_address)
                .min()
                .unwrap_or(0),
        );
    refresh_section_padding(&mut out)?;
    out.validate()?;
    Ok((out, dump))
}

fn adr_fits(pc: u64, target: u64) -> bool {
    let delta = target as i128 - pc as i128;
    delta >= -(1i128 << 20) && delta < (1i128 << 20)
}

fn adrp_fits(pc: u64, target: u64) -> bool {
    let page_delta = (target / 4096) as i128 - (pc / 4096) as i128;
    page_delta >= -(1i128 << 20) && page_delta < (1i128 << 20)
}

fn rodata_target(linked: &crate::linked::LinkedProgram, byte_offset: usize) -> Result<u64, String> {
    let section = linked
        .sections
        .iter()
        .find(|section| !section.executable && section.name == "rodata")
        .ok_or_else(|| "address relaxation needs a linked rodata section".to_string())?;
    section
        .byte_address
        .checked_add(byte_offset as u64)
        .ok_or_else(|| "rodata address overflow during relaxation".to_string())
}

fn remap_site_relocs(
    relocs: &[Reloc],
    at: usize,
    old_width: usize,
    new_width: usize,
) -> Result<Vec<Reloc>, String> {
    let mut out = Vec::with_capacity(relocs.len());
    for reloc in relocs {
        let old = reloc_word(reloc);
        if old == at {
            let mapped = match (old_width, new_width, reloc) {
                (2, 1, Reloc::Rodata { byte_offset, .. }) => Reloc::RodataAdr {
                    word: at,
                    byte_offset: *byte_offset,
                },
                (1, 2, Reloc::RodataAdr { byte_offset, .. }) => Reloc::Rodata {
                    word_adrp: at,
                    byte_offset: *byte_offset,
                },
                _ => {
                    return Err(format!(
                        "address relaxation found an incompatible relocation at word {at}"
                    ));
                }
            };
            out.push(mapped);
        } else if old > at && old < at + old_width {
            return Err(format!(
                "relocation at word {old} overlaps address site [{at}, {})",
                at + old_width
            ));
        } else {
            let new = if old >= at + old_width {
                if new_width >= old_width {
                    old + (new_width - old_width)
                } else {
                    old - (old_width - new_width)
                }
            } else {
                old
            };
            out.push(remap_reloc(reloc, new));
        }
    }
    Ok(out)
}

fn sign_extend(value: u32, bits: u32) -> i64 {
    let shift = 32 - bits;
    ((value << shift) as i32 >> shift) as i64
}

fn branch_target_index(word: u32, from: usize) -> Option<usize> {
    let words = if word & 0x8000_0000 == 0 && word & 0x7c00_0000 == 0x1400_0000 {
        sign_extend(word & 0x03ff_ffff, 26)
    } else if word & 0xff00_0010 == 0x5400_0000 {
        sign_extend((word >> 5) & 0x7ffff, 19)
    } else if word & 0x7e00_0000 == 0x3400_0000 {
        sign_extend((word >> 5) & 0x7ffff, 19)
    } else {
        return None;
    };
    usize::try_from((from as i64).checked_add(words)?).ok()
}

fn remap_index(index: usize, at: usize, old_width: usize, new_width: usize) -> usize {
    if index >= at + old_width {
        if new_width >= old_width {
            index + (new_width - old_width)
        } else {
            index - (old_width - new_width)
        }
    } else {
        index
    }
}

fn patch_local_branches(
    code: &mut [EmittedWord],
    branches: &[(usize, usize)],
    at: usize,
    old_width: usize,
    new_width: usize,
) -> Result<(), String> {
    for &(old_from, old_target) in branches {
        let from = remap_index(old_from, at, old_width, new_width);
        let target = remap_index(old_target, at, old_width, new_width);
        let delta = (target as i64 - from as i64) * 4;
        if delta < i32::MIN as i64 || delta > i32::MAX as i64 {
            return Err("local branch displacement overflow during relaxation".to_string());
        }
        let word = code
            .get(from)
            .ok_or_else(|| "local branch moved outside relaxed function".to_string())?
            .word;
        let patched = if word & 0x8000_0000 == 0 && word & 0x7c00_0000 == 0x1400_0000 {
            crate::encode::enc_b(delta as i32)
        } else if word & 0xff00_0010 == 0x5400_0000 {
            let imm = ((delta / 4) as u32) & 0x7ffff;
            (word & !(0x7ffff << 5)) | (imm << 5)
        } else if word & 0x7e00_0000 == 0x3400_0000 {
            let imm = ((delta / 4) as u32) & 0x7ffff;
            (word & !(0x7ffff << 5)) | (imm << 5)
        } else {
            return Err("unsupported local control transfer during relaxation".to_string());
        };
        code[from].word = patched;
        let text = code[from].text.clone();
        if let Some(hash) = text.rfind('#') {
            code[from].text = format!("{}{}", &text[..=hash], delta);
        }
    }
    Ok(())
}

fn patch_local_branches_after_edits(
    code: &mut [EmittedWord],
    branches: &[(usize, usize)],
    edits: &[(usize, usize, usize)],
) -> Result<(), String> {
    for &(old_from, old_target) in branches {
        let mut from = old_from;
        let mut target = old_target;
        for &(at, old_width, new_width) in edits {
            from = remap_index(from, at, old_width, new_width);
            target = remap_index(target, at, old_width, new_width);
        }
        let delta = (target as i64 - from as i64) * 4;
        if delta < i32::MIN as i64 || delta > i32::MAX as i64 {
            return Err("local branch displacement overflow during relaxation".to_string());
        }
        let word = code
            .get(from)
            .ok_or_else(|| "local branch moved outside relaxed function".to_string())?
            .word;
        let patched = if word & 0x8000_0000 == 0 && word & 0x7c00_0000 == 0x1400_0000 {
            crate::encode::enc_b(delta as i32)
        } else if word & 0xff00_0010 == 0x5400_0000 {
            let imm = ((delta / 4) as u32) & 0x7ffff;
            (word & !(0x7ffff << 5)) | (imm << 5)
        } else if word & 0x7e00_0000 == 0x3400_0000 {
            let imm = ((delta / 4) as u32) & 0x7ffff;
            (word & !(0x7ffff << 5)) | (imm << 5)
        } else {
            return Err("unsupported local control transfer during relaxation".to_string());
        };
        code[from].word = patched;
        let text = &mut code[from].text;
        if let Some(hash) = text.rfind('#') {
            text.truncate(hash + 1);
            let _ = write!(text, "{delta}");
        }
    }
    Ok(())
}

/// Apply a descending set of independent ADRP+ADD-to-ADR shrinks to one
/// function. Local branches are decoded before the first edit and repatched
/// once after the last; repatching after every site made address relaxation
/// quadratic in both branch count and formatting allocations.
fn shrink_rodata_sites(
    f: &mut crate::linked::LinkedFn,
    sites: &[(usize, usize)],
) -> Result<(), String> {
    if sites.windows(2).any(|pair| pair[0].0 <= pair[1].0) {
        return Err("batched address-relaxation sites are not strictly descending".to_string());
    }
    let branches: Vec<(usize, usize)> = f
        .code
        .iter()
        .enumerate()
        .filter_map(|(from, ew)| branch_target_index(ew.word, from).map(|target| (from, target)))
        .collect();
    let edits = sites
        .iter()
        .map(|(word, _)| (*word, 2usize, 1usize))
        .collect::<Vec<_>>();
    for (_, start, end) in &mut f.origin_word_ranges {
        for &(word, old_width, new_width) in &edits {
            *start = remap_index(*start, word, old_width, new_width);
            *end = remap_index(*end, word, old_width, new_width);
        }
    }
    for &(word, byte_offset) in sites {
        if word + 1 >= f.code.len() {
            return Err(format!("ADR site at word {word} has no ADRP+ADD pair"));
        }
        let reg = rd(&f.code[word]);
        let mut adr = f.code[word].clone();
        adr.word = crate::encode::enc_adr(reg, 0);
        adr.text = format!("adr x{reg}, rodata+{byte_offset:#x}");
        adr.rule = CostRule::Adrp;
        adr.dst = Some(Reg::gpr(reg));
        adr.clear_srcs();
        f.code[word] = adr;
        f.code.remove(word + 1);
        f.relocs = remap_site_relocs(&f.relocs, word, 2, 1)?;
    }
    patch_local_branches_after_edits(&mut f.code, &branches, &edits)
}

fn change_rodata_site(
    f: &mut crate::linked::LinkedFn,
    word: usize,
    byte_offset: usize,
    shrink: bool,
) -> Result<(), String> {
    let old_width = if shrink { 2 } else { 1 };
    let new_width = if shrink { 1 } else { 2 };
    for (_, start, end) in &mut f.origin_word_ranges {
        *start = remap_index(*start, word, old_width, new_width);
        *end = remap_index(*end, word, old_width, new_width);
    }
    let branches: Vec<(usize, usize)> = f
        .code
        .iter()
        .enumerate()
        .filter_map(|(from, ew)| branch_target_index(ew.word, from).map(|target| (from, target)))
        .collect();
    if shrink {
        if word + 1 >= f.code.len() {
            return Err(format!("ADR site at word {word} has no ADRP+ADD pair"));
        }
        let reg = rd(&f.code[word]);
        let mut adr = f.code[word].clone();
        adr.word = crate::encode::enc_adr(reg, 0);
        adr.text = format!("adr x{reg}, rodata+{byte_offset:#x}");
        adr.rule = CostRule::Adrp;
        adr.dst = Some(Reg::gpr(reg));
        adr.clear_srcs();
        f.code[word] = adr;
        f.code.remove(word + 1);
        patch_local_branches(&mut f.code, &branches, word, old_width, new_width)?;
        f.relocs = remap_site_relocs(&f.relocs, word, old_width, new_width)?;
    } else {
        if word >= f.code.len() {
            return Err(format!("ADRP site at word {word} is outside the function"));
        }
        let reg = rd(&f.code[word]);
        let old = f.code[word].clone();
        let mut adrp = old.clone();
        adrp.word = crate::encode::enc_adrp(reg, 0);
        adrp.text = format!("adrp x{reg}, rodata+{byte_offset:#x}");
        adrp.rule = CostRule::Adrp;
        adrp.dst = Some(Reg::gpr(reg));
        adrp.clear_srcs();
        let add = EmittedWord::gpr(
            crate::encode::enc_add_imm(reg, reg, 0, true),
            format!("add x{reg}, x{reg}, rodata+{byte_offset:#x}"),
            CostRule::Alu,
            Some(reg),
            &[reg],
        );
        f.code[word] = adrp;
        f.code.insert(word + 1, add);
        patch_local_branches(&mut f.code, &branches, word, old_width, new_width)?;
        f.relocs = remap_site_relocs(&f.relocs, word, old_width, new_width)?;
    }
    Ok(())
}

fn section_payload_bytes(
    linked: &crate::linked::LinkedProgram,
    section_id: usize,
) -> Result<u64, String> {
    let section = linked
        .sections
        .get(section_id)
        .ok_or_else(|| format!("missing linked section {section_id}"))?;
    if !section.executable {
        return Ok(section.raw_bytes.len() as u64);
    }
    linked
        .fns
        .values()
        .filter(|f| f.section == section_id)
        .try_fold(0u64, |words, f| {
            words
                .checked_add(f.code.len() as u64)
                .ok_or_else(|| "linked executable size overflow".to_string())
        })?
        .checked_mul(4)
        .ok_or_else(|| "linked executable size overflow".to_string())
}

fn refresh_section_padding(linked: &mut crate::linked::LinkedProgram) -> Result<(), String> {
    let payloads: Vec<u64> = (0..linked.sections.len())
        .map(|section_id| section_payload_bytes(linked, section_id))
        .collect::<Result<_, _>>()?;
    let mut previous_end = None;
    for (section, payload) in linked.sections.iter_mut().zip(payloads) {
        section.padding_before = previous_end
            .map(|end| section.byte_address.saturating_sub(end))
            .unwrap_or(0);
        previous_end = Some(
            section
                .byte_address
                .checked_add(payload)
                .ok_or_else(|| "linked section address overflow".to_string())?,
        );
    }
    Ok(())
}

fn repack_linked_sections(linked: &mut crate::linked::LinkedProgram) -> Result<(), String> {
    let payloads: Vec<u64> = (0..linked.sections.len())
        .map(|section_id| section_payload_bytes(linked, section_id))
        .collect::<Result<_, _>>()?;
    let mut cursor = linked
        .sections
        .first()
        .map(|section| section.byte_address)
        .unwrap_or(0);
    let mut fixed = false;
    for (section, payload) in linked.sections.iter_mut().zip(payloads) {
        if section.name == "rtdata" {
            fixed = true;
            cursor = section
                .byte_address
                .checked_add(payload)
                .ok_or_else(|| "linked section address overflow".to_string())?;
            continue;
        }
        if fixed {
            continue;
        }
        let alignment = if section.executable { 4 } else { 8 };
        section.byte_address = cursor.div_ceil(alignment) * alignment;
        cursor = section
            .byte_address
            .checked_add(payload)
            .ok_or_else(|| "linked section address overflow".to_string())?;
    }
    refresh_section_padding(linked)
}

fn relayout_linked_executable_sections(
    linked: &mut crate::linked::LinkedProgram,
) -> Result<(), String> {
    for section_id in 0..linked.sections.len() {
        if !linked.sections[section_id].executable {
            continue;
        }
        let section_address = linked.sections[section_id].byte_address;
        let mut keys: Vec<String> = linked
            .fns
            .values()
            .filter(|f| f.section == section_id)
            .map(|f| f.key.clone())
            .collect();
        keys.sort_by_key(|key| (linked.fns[key].byte_address, key.clone()));
        if keys.is_empty() {
            continue;
        }
        let mut cursor = section_address;
        for key in keys {
            let f = linked
                .fns
                .get_mut(&key)
                .ok_or_else(|| format!("missing linked function `{key}`"))?;
            f.byte_address = cursor;
            cursor = cursor
                .checked_add((f.code.len() as u64) * 4)
                .ok_or_else(|| "linked executable address overflow".to_string())?;
        }
    }
    Ok(())
}

fn rebuild_linked_executable_sections(
    linked: &mut crate::linked::LinkedProgram,
) -> Result<(), String> {
    relayout_linked_executable_sections(linked)?;
    for section_id in 0..linked.sections.len() {
        if !linked.sections[section_id].executable {
            continue;
        }
        let mut keys: Vec<String> = linked
            .fns
            .values()
            .filter(|f| f.section == section_id)
            .map(|f| f.key.clone())
            .collect();
        keys.sort_by_key(|key| (linked.fns[key].byte_address, key.clone()));
        linked.sections[section_id].code = keys
            .iter()
            .flat_map(|key| linked.fns[key].code.iter().cloned())
            .collect();
    }
    linked.image_bytes = linked
        .sections
        .iter()
        .enumerate()
        .map(|(section_id, section)| {
            section_payload_bytes(linked, section_id).and_then(|payload| {
                section
                    .byte_address
                    .checked_add(payload)
                    .ok_or_else(|| "linked section address overflow".to_string())
            })
        })
        .collect::<Result<Vec<_>, _>>()?
        .into_iter()
        .max()
        .unwrap_or(0)
        .saturating_sub(
            linked
                .sections
                .iter()
                .map(|s| s.byte_address)
                .min()
                .unwrap_or(0),
        );
    refresh_section_padding(linked)
}

fn blocked_address_sites_fit(linked: &crate::linked::LinkedProgram) -> Result<bool, String> {
    for f in linked.fns.values() {
        if !f.code.iter().any(is_control_word) {
            continue;
        }
        for reloc in &f.relocs {
            let (word, byte_offset, adr) = match reloc {
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => (*word_adrp, *byte_offset, false),
                Reloc::RodataAdr { word, byte_offset } => (*word, *byte_offset, true),
                _ => continue,
            };
            let pc = f.byte_address + (word as u64) * 4;
            let target = rodata_target(linked, byte_offset)?;
            if (adr && !adr_fits(pc, target)) || (!adr && !adrp_fits(pc, target)) {
                return Ok(false);
            }
        }
    }
    Ok(true)
}

fn patch_linked_addresses(linked: &mut crate::linked::LinkedProgram) -> Result<(), String> {
    let keys: Vec<String> = linked.fns.keys().cloned().collect();
    for key in keys {
        let (function_address, relocs) = {
            let f = linked
                .fns
                .get(&key)
                .ok_or_else(|| format!("missing linked function `{key}`"))?;
            (f.byte_address, f.relocs.clone())
        };
        for reloc in relocs {
            let word = reloc_word(&reloc);
            let pc = function_address
                .checked_add((word as u64) * 4)
                .ok_or_else(|| "linked relocation PC overflow".to_string())?;
            let patch = match reloc {
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => {
                    let target = rodata_target(linked, byte_offset)?;
                    if !adrp_fits(pc, target) {
                        return Err(format!(
                            "rodata ADRP relocation in `{key}` is out of range: pc={pc:#x} target={target:#x}"
                        ));
                    }
                    let reg = linked
                        .fns
                        .get(&key)
                        .and_then(|f| f.code.get(word_adrp))
                        .map(rd)
                        .ok_or_else(|| format!("invalid ADRP relocation in `{key}`"))?;
                    (
                        word_adrp,
                        crate::encode::enc_adrp(
                            reg,
                            (target as i128 / 4096 - pc as i128 / 4096) as i32,
                        ),
                        Some((
                            word_adrp + 1,
                            crate::encode::enc_add_imm(reg, reg, (target & 0xfff) as u16, true),
                        )),
                    )
                }
                Reloc::RodataAdr { word, byte_offset } => {
                    let target = rodata_target(linked, byte_offset)?;
                    if !adr_fits(pc, target) {
                        return Err(format!(
                            "rodata ADR relocation in `{key}` is out of range after linking: pc={pc:#x} target={target:#x}"
                        ));
                    }
                    let reg = linked
                        .fns
                        .get(&key)
                        .and_then(|f| f.code.get(word))
                        .map(rd)
                        .ok_or_else(|| format!("invalid ADR relocation in `{key}`"))?;
                    (
                        word,
                        crate::encode::enc_adr(reg, (target as i128 - pc as i128) as i32),
                        None,
                    )
                }
                Reloc::Call { word, key: target } => {
                    let target_address = linked
                        .fns
                        .get(&target)
                        .map(|target| target.byte_address)
                        .ok_or_else(|| format!("call relocation target `{target}` is missing"))?;
                    let delta = target_address as i128 - pc as i128;
                    if delta % 4 != 0 || delta < -(1i128 << 27) || delta >= (1i128 << 27) {
                        return Err(format!("call relocation in `{key}` is out of range"));
                    }
                    (word, crate::encode::enc_bl(delta as i32), None)
                }
                Reloc::AbortFixed { word } | Reloc::AbortVal { word } => {
                    let target_key = if matches!(reloc, Reloc::AbortFixed { .. }) {
                        "__image_abort_fixed"
                    } else {
                        "__image_abort_value"
                    };
                    let Some(target_address) =
                        linked.fns.get(target_key).map(|target| target.byte_address)
                    else {
                        // Closure-only diagnostics have no fixed image abort
                        // landing section.  Keep their relocation-backed BL
                        // placeholder; image links always provide and patch
                        // the synthetic target.
                        continue;
                    };
                    let delta = target_address as i128 - pc as i128;
                    if delta % 4 != 0 || delta < -(1i128 << 27) || delta >= (1i128 << 27) {
                        return Err(format!("abort relocation in `{key}` is out of range"));
                    }
                    (word, crate::encode::enc_bl(delta as i32), None)
                }
                _ => continue,
            };
            let f = linked
                .fns
                .get_mut(&key)
                .ok_or_else(|| format!("missing linked function `{key}`"))?;
            f.code[patch.0].word = patch.1;
            if let Some((at, word)) = patch.2 {
                f.code[at].word = word;
            }
        }
    }
    Ok(())
}

fn control_rewrite_allowed(f: &crate::linked::LinkedFn) -> bool {
    f.code.iter().enumerate().all(|(word, ew)| {
        if !is_control_word(ew) {
            return true;
        }
        if branch_target_index(ew.word, word).is_some() || ew.word & 0xffff_fc1f == 0xd65f_0000 {
            return true;
        }
        if ew.word & 0xfc00_0000 == 0x9400_0000 {
            return f.relocs.iter().any(|reloc| {
                reloc_word(reloc) == word
                    && matches!(
                        reloc,
                        Reloc::Call { .. } | Reloc::AbortFixed { .. } | Reloc::AbortVal { .. }
                    )
            });
        }
        false
    })
}

/// Relax final-address rodata references after all image functions and fixed
/// sections have been linked.  Local branches, returns, and relocation-backed
/// calls are remapped and repatched when a preceding site shrinks; unknown
/// control transfers remain wide as the fail-closed choice.
pub fn relax_linked_addresses(
    linked: &crate::linked::LinkedProgram,
) -> Result<(crate::linked::LinkedProgram, String), String> {
    let mut out = linked.clone();
    // Functions own every executable word. During convergence their code is
    // authoritative, so do not duplicate the full instruction stream in each
    // transactional trial. The final rebuild restores section materialization
    // before validation and serialization.
    for section in &mut out.sections {
        if section.executable {
            section.code.clear();
        }
    }
    let mut frozen = BTreeSet::<(String, u32)>::new();
    let mut original_widths = BTreeMap::<(String, u32), usize>::new();
    let mut site_count = 0usize;
    for (key, f) in &out.fns {
        let mut ordinal = 0u32;
        for reloc in &f.relocs {
            if matches!(reloc, Reloc::Rodata { .. } | Reloc::RodataAdr { .. }) {
                original_widths.insert(
                    (key.clone(), ordinal),
                    linked
                        .address_site_widths
                        .get(&(key.clone(), ordinal))
                        .copied()
                        .unwrap_or_else(|| {
                            if matches!(reloc, Reloc::Rodata { .. }) {
                                2
                            } else {
                                1
                            }
                        }),
                );
                ordinal += 1;
                site_count += 1;
            }
        }
    }
    let cap = site_count.saturating_mul(2).saturating_add(1).max(1);
    let mut converged = false;
    for _ in 0..cap {
        relayout_linked_executable_sections(&mut out)?;
        repack_linked_sections(&mut out)?;
        relayout_linked_executable_sections(&mut out)?;
        let mut actions: Vec<(String, usize, usize, bool, u32)> = Vec::new();
        for (key, f) in &out.fns {
            let blocked = !control_rewrite_allowed(f);
            let mut ordinal = 0u32;
            for reloc in &f.relocs {
                let (word, byte_offset, width) = match reloc {
                    Reloc::Rodata {
                        word_adrp,
                        byte_offset,
                    } => (*word_adrp, *byte_offset, 2),
                    Reloc::RodataAdr { word, byte_offset } => (*word, *byte_offset, 1),
                    _ => continue,
                };
                let id = (key.clone(), ordinal);
                ordinal += 1;
                if blocked || frozen.contains(&id) {
                    continue;
                }
                let target = rodata_target(&out, byte_offset)?;
                let pc = f.byte_address + (word as u64) * 4;
                match width {
                    2 if adr_fits(pc, target) => {
                        actions.push((key.clone(), word, byte_offset, true, ordinal - 1))
                    }
                    1 if !adr_fits(pc, target) => {
                        actions.push((key.clone(), word, byte_offset, false, ordinal - 1))
                    }
                    _ => {}
                }
            }
        }
        if actions.is_empty() {
            converged = true;
            break;
        }
        actions.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| b.1.cmp(&a.1)));
        let shrinks: Vec<_> = actions
            .iter()
            .filter(|(_, _, _, shrink, _)| *shrink)
            .cloned()
            .collect();
        if !shrinks.is_empty() {
            // Shrinks are monotone, so try the whole descending-index set in
            // one transaction. If an existing blocked address would stop
            // fitting, fall back to the conservative one-site transactions
            // below and freeze only the responsible sites. Snapshot only the
            // functions the transaction edits plus the addresses relayout can
            // move; cloning the complete linked image here duplicated every
            // emitted word in the ordinary all-shrinks-succeed path.
            let mut by_function: BTreeMap<String, Vec<(usize, usize)>> = BTreeMap::new();
            for (key, word, byte_offset, _, _) in shrinks {
                by_function
                    .entry(key)
                    .or_default()
                    .push((word, byte_offset));
            }
            let saved_sections: Vec<_> = out
                .sections
                .iter()
                .map(|section| (section.byte_address, section.padding_before))
                .collect();
            let saved_addresses: Vec<_> = out
                .fns
                .iter()
                .map(|(key, function)| (key.clone(), function.byte_address))
                .collect();
            let mut saved_functions = Vec::with_capacity(by_function.len());
            for (key, sites) in by_function {
                let f = out
                    .fns
                    .get_mut(&key)
                    .ok_or_else(|| format!("missing linked function `{key}`"))?;
                saved_functions.push((key, f.clone()));
                shrink_rodata_sites(f, &sites)?;
            }
            relayout_linked_executable_sections(&mut out)?;
            repack_linked_sections(&mut out)?;
            relayout_linked_executable_sections(&mut out)?;
            if blocked_address_sites_fit(&out)? {
                patch_linked_addresses(&mut out)?;
                continue;
            }
            for (key, function) in saved_functions {
                out.fns.insert(key, function);
            }
            for (key, address) in saved_addresses {
                out.fns
                    .get_mut(&key)
                    .ok_or_else(|| format!("missing linked function `{key}` during rollback"))?
                    .byte_address = address;
            }
            for (section, (address, padding)) in out.sections.iter_mut().zip(saved_sections) {
                section.byte_address = address;
                section.padding_before = padding;
            }
        }
        for (key, word, byte_offset, shrink, ordinal) in actions {
            let mut trial = out.clone();
            let f = trial
                .fns
                .get_mut(&key)
                .ok_or_else(|| format!("missing linked function `{key}`"))?;
            change_rodata_site(f, word, byte_offset, shrink)?;
            relayout_linked_executable_sections(&mut trial)?;
            repack_linked_sections(&mut trial)?;
            relayout_linked_executable_sections(&mut trial)?;
            if !blocked_address_sites_fit(&trial)? {
                frozen.insert((key, ordinal));
                continue;
            }
            patch_linked_addresses(&mut trial)?;
            out = trial;
            if !shrink {
                frozen.insert((key, ordinal));
            }
        }
        patch_linked_addresses(&mut out)?;
    }
    if !converged {
        return Err(format!(
            "address relaxation exceeded its monotone iteration cap ({cap})"
        ));
    }
    // Payload sizing reads function bodies directly, so no executable section
    // materialization is needed until addresses and relocation words are
    // final. The old sequence rebuilt the complete section stream three times.
    repack_linked_sections(&mut out)?;
    relayout_linked_executable_sections(&mut out)?;
    patch_linked_addresses(&mut out)?;
    rebuild_linked_executable_sections(&mut out)?;
    out.validate()?;

    let mut dump = String::new();
    for (key, f) in &out.fns {
        let blocked = !control_rewrite_allowed(f);
        let mut ordinal = 0u32;
        for reloc in &f.relocs {
            let (word, byte_offset, width) = match reloc {
                Reloc::Rodata {
                    word_adrp,
                    byte_offset,
                } => (*word_adrp, *byte_offset, 2),
                Reloc::RodataAdr { word, byte_offset } => (*word, *byte_offset, 1),
                _ => continue,
            };
            let old_width = original_widths
                .get(&(key.clone(), ordinal))
                .copied()
                .unwrap_or(width);
            let pc = f.byte_address + (word as u64) * 4;
            let target = rodata_target(&out, byte_offset)?;
            let (encoding, reason) = if blocked && width == 2 {
                (
                    "wide",
                    "control transfer requires relocation-aware layout".to_string(),
                )
            } else if width == 1 {
                (
                    "adr",
                    if blocked {
                        "control transfer retained a valid ADR".to_string()
                    } else {
                        "final address fits ADR".to_string()
                    },
                )
            } else {
                ("adrp+add", "ADR out of range".to_string())
            };
            let _ = writeln!(
                dump,
                "relax fn={key} site={ordinal} kind=addr target=rodata+{byte_offset:#x} pc={pc:#x} target_addr={target:#x} encoding={encoding} width={width} old_width={old_width} saved_words={} reason={} frozen={}",
                old_width.saturating_sub(width),
                reason,
                frozen.contains(&(key.clone(), ordinal))
            );
            ordinal += 1;
        }
    }
    Ok((out, dump))
}

pub fn choose_address(pc: u64, target: u64) -> Encoding {
    let delta = target as i128 - pc as i128;
    if delta >= -(1i128 << 20) && delta < (1i128 << 20) {
        return Encoding::Adr;
    }
    let page_delta = (target / 4096) as i128 - (pc / 4096) as i128;
    if page_delta >= -(1i128 << 20) && page_delta < (1i128 << 20) {
        Encoding::AdrpAdd
    } else {
        Encoding::Wide
    }
}

/// Monotone address relaxation.  The callback returns the current PC and
/// target for each ordinal after the caller lays out current widths.
pub fn relax_addresses(
    sites: &mut [RelaxSite],
    mut layout: impl FnMut(&[RelaxSite]) -> Vec<(u64, u64)>,
) -> Result<(), String> {
    let cap = 2usize
        .checked_mul(sites.len())
        .and_then(|n| n.checked_add(1))
        .unwrap_or(usize::MAX);
    for _ in 0..cap {
        let positions = layout(sites);
        if positions.len() != sites.len() {
            return Err("address layout returned the wrong relax-site count".to_string());
        }
        let mut changed = false;
        for (site, &(pc, target)) in sites.iter_mut().zip(positions.iter()) {
            if site.frozen_wide {
                continue;
            }
            let choice = choose_address(pc, target);
            if choice.width() >= site.old_width {
                site.selected = Encoding::Wide;
                site.frozen_wide = true;
                site.reason = "short encoding is not smaller".to_string();
                changed = true;
            } else if site.selected != choice {
                site.selected = choice;
                changed = true;
            }
        }
        if !changed {
            return Ok(());
        }
    }
    Err("address relaxation exceeded its monotone iteration cap".to_string())
}

pub fn dump_fragments(program: &FragmentProgram) -> String {
    let mut out = String::new();
    for (key, parts) in &program.fns {
        for part in parts {
            if let Fragment::Relax(site) = part {
                let target = match site.target {
                    RelaxTarget::Immediate(v) => format!("value={v:#018x}"),
                    RelaxTarget::Function(ref f) => format!("target={f}"),
                    RelaxTarget::Rodata(i) => format!("rodata={i}"),
                    RelaxTarget::FixedAddress(a) => format!("address={a:#x}"),
                };
                let _ = writeln!(
                    out,
                    "relax fn={key} site={} kind={:?} {} encoding={} width={} old_width={} reason={} frozen={}",
                    site.ordinal,
                    site.kind,
                    target,
                    site.selected.as_str(),
                    if matches!(site.selected, Encoding::Wide) {
                        site.old_width
                    } else {
                        site.selected.width()
                    },
                    site.old_width,
                    site.reason,
                    site.frozen_wide
                );
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ew(word: u32) -> EmittedWord {
        EmittedWord::gpr(word, String::new(), CostRule::MovWide, Some(9), &[])
    }

    #[test]
    fn immediate_choices_cover_zero_ones_sparse_and_movn_values() {
        let values = [
            0,
            u64::MAX,
            0x40,
            0x0000_1234_0000_5678,
            0xffff_ffff_ffff_0000,
            0x1234_5678_9abc_def0,
        ];
        for value in values {
            let enc = choose_immediate(9, value);
            let old = ew(crate::encode::enc_movz(9, 0, 0, true));
            let words = make_word(9, value, &enc, &old);
            assert!(!words.is_empty(), "{value:#x} selected no words");
            if enc != Encoding::LogicalImmediate {
                assert_eq!(decode_materialized_value(&words), Some((9, value)));
            } else {
                assert_eq!(words.len(), 1);
            }
            if value == 0x40 {
                assert!(
                    enc.width() <= 1,
                    "small constants must use one word: {enc:?}"
                );
            }
            if value == 0x1234_5678_9abc_def0 {
                assert!(enc.width() <= 4);
            }
        }
    }

    #[test]
    fn immediate_relaxation_preserves_value_and_movk_dependencies() {
        let code = vec![
            ew(crate::encode::enc_movz(9, 0x40, 0, true)),
            EmittedWord::gpr(
                crate::encode::enc_movk(9, 0, 16, true),
                String::new(),
                CostRule::MovWide,
                Some(9),
                &[9],
            ),
            EmittedWord::gpr(
                crate::encode::enc_movk(9, 0, 32, true),
                String::new(),
                CostRule::MovWide,
                Some(9),
                &[9],
            ),
            EmittedWord::gpr(
                crate::encode::enc_movk(9, 0, 48, true),
                String::new(),
                CostRule::MovWide,
                Some(9),
                &[9],
            ),
        ];
        let p = CodegenProgram {
            fns: BTreeMap::from([(
                "f".into(),
                CodegenFn {
                    frame_size: 0,
                    code,
                    relocs: Vec::new(),
                    regions: Vec::new(),
                },
            )]),
            ..CodegenProgram::default()
        };
        let r = relax_immediates(&p).expect("relax");
        let owned = relax_immediates_owned(p).expect("owned relax");
        assert_eq!(owned.program, r.program);
        assert_eq!(owned.dump, r.dump);
        assert_eq!(r.program.fns["f"].code.len(), 1);
        assert!(r.dump.contains("saved_words=3"));
    }

    #[test]
    fn shortening_remaps_relocations_after_the_site() {
        let code = vec![
            ew(crate::encode::enc_movz(9, 0x40, 0, true)),
            EmittedWord::gpr(
                crate::encode::enc_movk(9, 0, 16, true),
                String::new(),
                CostRule::MovWide,
                Some(9),
                &[9],
            ),
            EmittedWord::gpr(
                crate::encode::enc_movk(9, 0, 32, true),
                String::new(),
                CostRule::MovWide,
                Some(9),
                &[9],
            ),
            EmittedWord::gpr(
                crate::encode::enc_movk(9, 0, 48, true),
                String::new(),
                CostRule::MovWide,
                Some(9),
                &[9],
            ),
            ew(crate::encode::enc_movz(10, 1, 0, true)),
            ew(crate::encode::enc_movz(11, 2, 0, true)),
        ];
        let p = CodegenProgram {
            fns: BTreeMap::from([(
                "f".into(),
                CodegenFn {
                    frame_size: 0,
                    code,
                    relocs: vec![Reloc::TurnIdImm {
                        word: 5,
                        key: "turn".into(),
                    }],
                    regions: Vec::new(),
                },
            )]),
            ..CodegenProgram::default()
        };
        let r = relax_immediates(&p).expect("relax");
        assert_eq!(r.program.fns["f"].code.len(), 3);
        assert_eq!(
            r.program.fns["f"].relocs,
            vec![Reloc::TurnIdImm {
                word: 2,
                key: "turn".into(),
            }]
        );
    }

    #[test]
    fn linked_address_relaxation_shrinks_and_remaps_rodata_sites() {
        let code = vec![
            EmittedWord::gpr(
                crate::encode::enc_adrp(9, 0),
                "adrp x9, rodata".to_string(),
                CostRule::Adrp,
                Some(9),
                &[],
            ),
            EmittedWord::gpr(
                crate::encode::enc_add_imm(9, 9, 0, true),
                "add x9, x9, rodata".to_string(),
                CostRule::Alu,
                Some(9),
                &[9],
            ),
        ];
        let linked = crate::linked::LinkedProgram::from_parts(
            vec![
                crate::linked::LinkedSection {
                    id: 0,
                    name: "code".to_string(),
                    byte_address: 0x1000,
                    executable: true,
                    code: code.clone(),
                    raw_bytes: Vec::new(),
                    padding_before: 0,
                },
                crate::linked::LinkedSection {
                    id: 1,
                    name: "rodata".to_string(),
                    byte_address: 0x2000,
                    executable: false,
                    code: Vec::new(),
                    raw_bytes: vec![0; 8],
                    padding_before: 0,
                },
            ],
            BTreeMap::from([(
                "f".to_string(),
                crate::linked::LinkedFn {
                    key: "f".to_string(),
                    section: 0,
                    byte_address: 0x1000,
                    origin_word_ranges: crate::linked::default_origin_ranges(&code),
                    code,
                    relocs: vec![Reloc::Rodata {
                        word_adrp: 0,
                        byte_offset: 0,
                    }],
                    frame_size: 0,
                },
            )]),
            0x1000,
        )
        .expect("linked program");
        let (out, dump) = relax_linked_addresses(&linked).expect("address relax");
        assert_eq!(out.fns["f"].code.len(), 1);
        assert!(matches!(
            out.fns["f"].relocs[0],
            Reloc::RodataAdr { word: 0, .. }
        ));
        assert!(dump.contains("kind=addr") && dump.contains("encoding=adr"));
        out.validate().expect("relaxed linked program");
    }

    #[test]
    fn linked_address_relaxation_batches_multiple_shrinks() {
        let pair = |reg| {
            [
                EmittedWord::gpr(
                    crate::encode::enc_adrp(reg, 0),
                    format!("adrp x{reg}, rodata"),
                    CostRule::Adrp,
                    Some(reg),
                    &[],
                ),
                EmittedWord::gpr(
                    crate::encode::enc_add_imm(reg, reg, 0, true),
                    format!("add x{reg}, x{reg}, rodata"),
                    CostRule::Alu,
                    Some(reg),
                    &[reg],
                ),
            ]
        };
        // The forward branch crosses both two-word sites. Applying the two
        // shrinks as one batch must still retarget it to the same source word.
        let code: Vec<_> = std::iter::once(EmittedWord::gpr(
            crate::encode::enc_b(20),
            "b #20".to_string(),
            CostRule::Branch,
            None,
            &[],
        ))
        .chain(pair(9))
        .chain(pair(10))
        .chain(std::iter::once(raw_word_for_test(7)))
        .collect();
        let linked = crate::linked::LinkedProgram::from_parts(
            vec![
                crate::linked::LinkedSection {
                    id: 0,
                    name: "code".to_string(),
                    byte_address: 0x1000,
                    executable: true,
                    code: code.clone(),
                    raw_bytes: Vec::new(),
                    padding_before: 0,
                },
                crate::linked::LinkedSection {
                    id: 1,
                    name: "rodata".to_string(),
                    byte_address: 0x2000,
                    executable: false,
                    code: Vec::new(),
                    raw_bytes: vec![0; 8],
                    padding_before: 0,
                },
            ],
            BTreeMap::from([(
                "f".to_string(),
                crate::linked::LinkedFn {
                    key: "f".to_string(),
                    section: 0,
                    byte_address: 0x1000,
                    origin_word_ranges: crate::linked::default_origin_ranges(&code),
                    code,
                    relocs: vec![
                        Reloc::Rodata {
                            word_adrp: 1,
                            byte_offset: 0,
                        },
                        Reloc::Rodata {
                            word_adrp: 3,
                            byte_offset: 4,
                        },
                    ],
                    frame_size: 0,
                },
            )]),
            0x1000,
        )
        .expect("linked program");

        let (out, dump) = relax_linked_addresses(&linked).expect("address relax");
        assert_eq!(out.fns["f"].code.len(), 4);
        assert_eq!(out.sections[0].code, out.fns["f"].code);
        assert_eq!(branch_target_index(out.fns["f"].code[0].word, 0), Some(3));
        assert!(matches!(
            out.fns["f"].relocs.as_slice(),
            [
                Reloc::RodataAdr { word: 1, .. },
                Reloc::RodataAdr { word: 2, .. }
            ]
        ));
        assert_eq!(dump.matches("encoding=adr").count(), 2);
        out.validate().expect("relaxed linked program");
    }

    #[test]
    fn address_shrink_patches_local_branch_targets() {
        let code = vec![
            EmittedWord::gpr(
                crate::encode::enc_adrp(9, 0),
                "adrp x9, rodata".to_string(),
                CostRule::Adrp,
                Some(9),
                &[],
            ),
            EmittedWord::gpr(
                crate::encode::enc_add_imm(9, 9, 0, true),
                "add x9, x9, rodata".to_string(),
                CostRule::Alu,
                Some(9),
                &[9],
            ),
            EmittedWord::gpr(
                crate::encode::enc_b(8),
                "b #8".to_string(),
                CostRule::Branch,
                None,
                &[],
            ),
            raw_word_for_test(0),
            raw_word_for_test(1),
        ];
        let mut function = crate::linked::LinkedFn {
            key: "f".to_string(),
            section: 0,
            byte_address: 0x1000,
            origin_word_ranges: crate::linked::default_origin_ranges(&code),
            code,
            relocs: vec![Reloc::Rodata {
                word_adrp: 0,
                byte_offset: 0,
            }],
            frame_size: 0,
        };
        change_rodata_site(&mut function, 0, 0, true).expect("change site");
        assert_eq!(function.code.len(), 4);
        assert_eq!(
            function.relocs,
            vec![Reloc::RodataAdr {
                word: 0,
                byte_offset: 0
            }]
        );
        assert_eq!(branch_target_index(function.code[1].word, 1), Some(3));
    }

    fn raw_word_for_test(value: u32) -> EmittedWord {
        EmittedWord::gpr(value, format!("word {value}"), CostRule::Alu, None, &[])
    }

    #[test]
    fn address_boundaries_are_checked_at_the_architectural_range() {
        assert_eq!(choose_address(0x1000, 0x1000), Encoding::Adr);
        assert_eq!(
            choose_address(0x1000, 0x1000 + (1 << 20)),
            Encoding::AdrpAdd
        );
        assert_eq!(
            choose_address(0x1000, 0x1000 + (1 << 20) * 4096),
            Encoding::Wide
        );
    }

    #[test]
    fn address_freezing_terminates_when_layout_changes() {
        let mut sites = vec![RelaxSite {
            ordinal: 0,
            kind: RelaxKind::Address,
            target: RelaxTarget::FixedAddress(0),
            selected: Encoding::Wide,
            old_width: 4,
            frozen_wide: false,
            reason: String::new(),
            wide: Vec::new(),
        }];
        relax_addresses(&mut sites, |_| vec![(0, 0)]).expect("fixed point");
        assert_eq!(sites[0].selected, Encoding::Adr);
    }
}
