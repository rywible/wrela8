//! Per-function, per-region hot-path census for the Pixels renderer.
//!
//! `emitted_a64_census.rs` is a static inventory of *emitter sites* — which
//! `codegen.rs` function emits how many words. It answers "did a new emitter
//! appear", not "where does the raster loop spend its modelled cycles", and
//! overloading it with the second question would make one artifact serve two
//! unrelated ratchets. This module is the second question's home.
//!
//! What it reports, per named function and per region inside it:
//!
//! - `[M]` measured facts: opcode counts by cost class, frame size, call
//!   count, and spill/frame memory traffic. These are counted off the emitted
//!   word stream and involve no model.
//! - `[I]` modelled scores: cycle totals derived from those counts through
//!   the A76 cost table.
//!
//! The two are reported in separate sections and never summed together, so a
//! citation of a cycle number is always visibly `[I]`.
//!
//! Regions come from the non-emitting [`crate::mwir::Inst::RegionMarker`]
//! instructions that lowering installs from the sealed `pixels_census_region`
//! source annotation. A region runs from its marker to the next marker (or to
//! the end of the function), and words before the first marker belong to the
//! implicit [`REGION_UNMARKED`] region.

use std::collections::BTreeMap;

use crate::codegen::{CodegenFn, CodegenProgram};
use crate::cost::{CostRule, CostTable, MemClass};
use crate::mwir::{self, Inst, LayoutCtx, MwirFn, MwirProgram, Temp};
use crate::sema::types::Type;

/// Schema version of the census artifact format.
///
/// Bump this whenever a section, column, or ordering changes, so a checked-in
/// baseline cannot be silently compared against a differently-shaped report.
pub const CENSUS_SCHEMA_VERSION: u32 = 4;

/// Region id for words emitted before any marker in a function.
pub const REGION_UNMARKED: u32 = 0;

/// The sealed region vocabulary.
///
/// Ids are stable and are what the sealed `pixels_census_region` annotation
/// names in Wrela source; the names are what the artifact prints. A region
/// that is retired keeps its id reserved rather than reusing it, so an old
/// baseline can still be read.
const REGIONS: &[(u32, &str)] = &[
    (REGION_UNMARKED, "unmarked"),
    (1, "raster.scalar_prefix"),
    (2, "raster.packet_loop"),
    (3, "raster.scalar_suffix"),
    (4, "raster.charge"),
    (5, "coverage.entry"),
    (6, "coverage.cell_walk"),
];

/// The stable name of a region id, or `region.<id>` for an id the compiler
/// does not know. An unknown id is a fail-closed condition at the lane level
/// ([`check_regions`]); this function stays total so dumps never panic.
pub fn region_name(region: u32) -> String {
    REGIONS
        .iter()
        .find(|(id, _)| *id == region)
        .map(|(_, name)| (*name).to_string())
        .unwrap_or_else(|| format!("region.{region}"))
}

/// Every sealed region id, ascending.
pub fn region_ids() -> Vec<u32> {
    REGIONS.iter().map(|(id, _)| *id).collect()
}

/// True when `region` is part of the sealed vocabulary.
pub fn is_sealed_region(region: u32) -> bool {
    REGIONS.iter().any(|(id, _)| *id == region)
}

/// Measured counts for one region of one function. Model-free `[M]`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RegionCounts {
    pub words: usize,
    /// Opcode counts by cost class, ascending by class key.
    pub by_class: BTreeMap<&'static str, usize>,
    pub calls: usize,
    /// Runtime-computed call/jump instructions. Hot renderer paths are
    /// statically dispatched, so this must remain zero.
    pub indirect_calls: usize,
    /// Loads and stores whose address is a stack or flow-frame slot: the
    /// spill and frame traffic this milestone is trying to remove.
    pub frame_loads: usize,
    pub frame_stores: usize,
}

impl RegionCounts {
    fn observe(&mut self, word: &crate::cost::EmittedWord) {
        self.words += 1;
        *self.by_class.entry(word.rule.as_str()).or_default() += 1;
        if word.rule == CostRule::Call {
            self.calls += 1;
        }
        if word.text.starts_with("blr ") || word.text.starts_with("br x") {
            self.indirect_calls += 1;
        }
        let frame = word.mem.is_some_and(|mem| mem.class == MemClass::Stack);
        if frame && word.rule.is_load() {
            self.frame_loads += 1;
        }
        if frame && word.rule.is_store() {
            self.frame_stores += 1;
        }
    }

    fn latency_weighted_cycles(&self, table: &CostTable) -> Result<u64, String> {
        let mut cycles = 0u64;
        for (class, count) in &self.by_class {
            let rule = CostRule::from_str(class).ok_or_else(|| {
                format!("hot census: measured class `{class}` is not a sealed cost rule")
            })?;
            cycles = cycles.saturating_add(
                table
                    .latency(rule)
                    .saturating_mul(u64::try_from(*count).unwrap_or(u64::MAX)),
            );
        }
        Ok(cycles)
    }
}

/// One function's census: its regions in emission order, plus its frame size.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FunctionCensus {
    pub key: String,
    pub frame_size: usize,
    /// `(region id, counts)` in ascending region order.
    pub regions: Vec<(u32, RegionCounts)>,
}

fn check_static_dispatch(census: &FunctionCensus) -> Result<(), String> {
    if census.total().indirect_calls != 0 {
        return Err(format!(
            "hot census: `{}` contains a runtime indirect call/jump; Pixels handlers must be statically dispatched",
            census.key
        ));
    }
    Ok(())
}

impl FunctionCensus {
    pub fn total(&self) -> RegionCounts {
        let mut total = RegionCounts::default();
        for (_, counts) in &self.regions {
            total.words += counts.words;
            total.calls += counts.calls;
            total.indirect_calls += counts.indirect_calls;
            total.frame_loads += counts.frame_loads;
            total.frame_stores += counts.frame_stores;
            for (class, n) in &counts.by_class {
                *total.by_class.entry(class).or_default() += n;
            }
        }
        total
    }
}

/// Split one function's emitted words into its census regions.
///
/// Markers are recorded as `(word index, region id)` in emission order; a
/// region runs from its marker to the next one. Two markers at the same word
/// index mean an empty region, which is reported as empty rather than
/// dropped.
pub fn function_census(key: &str, f: &CodegenFn) -> FunctionCensus {
    let mut bounds: Vec<(usize, u32)> = f.regions.clone();
    bounds.sort_by_key(|(at, _)| *at);
    let mut per_region: BTreeMap<u32, RegionCounts> = BTreeMap::new();
    // Seed every region the function names, so an empty region is visible.
    per_region.entry(REGION_UNMARKED).or_default();
    for (_, region) in &bounds {
        per_region.entry(*region).or_default();
    }
    let mut current = REGION_UNMARKED;
    let mut next = 0usize;
    for (index, word) in f.code.iter().enumerate() {
        while next < bounds.len() && bounds[next].0 <= index {
            current = bounds[next].1;
            next += 1;
        }
        per_region.entry(current).or_default().observe(word);
    }
    FunctionCensus {
        key: key.to_string(),
        frame_size: f.frame_size,
        regions: per_region.into_iter().collect(),
    }
}

/// Fail closed on a region id no sealed vocabulary entry names.
pub fn check_regions(program: &CodegenProgram) -> Result<(), String> {
    for (key, f) in &program.fns {
        for (at, region) in &f.regions {
            if !is_sealed_region(*region) {
                return Err(format!(
                    "hot census: `{key}` word {at} names region {region}, which is not in the \
                     sealed vocabulary"
                ));
            }
        }
    }
    Ok(())
}

/// A generated helper family the census reports "as emitted".
///
/// Some census targets are families rather than single symbols: the
/// `__wrela_pixels_p8_write*` writers, and the sealed numeric helper
/// sequences. A family reports every member the basis actually emits, and
/// fails closed in two directions — an empty family means the hot path this
/// census exists to measure is gone, and a `prefix` member that no
/// `members` entry names means the family grew without the census noticing.
#[derive(Debug, Clone)]
pub struct CensusFamily {
    pub label: String,
    pub members: Vec<String>,
    /// When set, every emitted function with this prefix must be listed in
    /// `members`.
    pub prefix: Option<String>,
}

/// The functions a census report covers, in report order.
///
/// The lane fails when a named function is absent, so a renamed or deleted
/// hot path is a visible failure rather than a silently shorter report.
#[derive(Debug, Clone, Default)]
pub struct CensusTargets {
    pub functions: Vec<String>,
    pub families: Vec<CensusFamily>,
    /// Regions each function must actually contain, by id.
    pub required_regions: BTreeMap<String, Vec<u32>>,
}

/// Resolve one family against the emitted program.
///
/// Returns `(present members, absent members)`. Every named member is required:
/// the census target list is literal, so a missing numeric sequence or writer
/// is a failed basis rather than a shorter measurement.
fn resolve_family(
    program: &CodegenProgram,
    family: &CensusFamily,
) -> Result<(Vec<String>, Vec<String>), String> {
    if let Some(prefix) = &family.prefix {
        let unlisted: Vec<&String> = program
            .fns
            .keys()
            .filter(|key| key.starts_with(prefix.as_str()) && !family.members.contains(key))
            .collect();
        if !unlisted.is_empty() {
            return Err(format!(
                "hot census: family `{}` grew members the census does not name: {unlisted:?}",
                family.label
            ));
        }
    }
    let (present, absent): (Vec<String>, Vec<String>) = family
        .members
        .iter()
        .cloned()
        .partition(|key| program.fns.contains_key(key));
    if !absent.is_empty() {
        return Err(format!(
            "hot census: family `{}` is missing required members {absent:?}",
            family.label,
        ));
    }
    Ok((present, absent))
}

/// Build one census report over `targets`.
///
/// `[M]` sections are counted off `program`; `[I]` sections are scored
/// through `table`. `digests` are opaque identity lines (image and dump
/// digests) recorded verbatim in the header so a report names the exact basis
/// it measured.
pub fn report(
    program: &CodegenProgram,
    table: &CostTable,
    placement: &crate::placement::PlacementTable,
    targets: &CensusTargets,
    digests: &[(String, String)],
) -> Result<String, String> {
    check_regions(program)?;
    let missing: Vec<&String> = targets
        .functions
        .iter()
        .filter(|key| !program.fns.contains_key(*key))
        .collect();
    if !missing.is_empty() {
        return Err(format!(
            "hot census: named function(s) absent from the emitted program: {missing:?}"
        ));
    }

    let mut keys: Vec<String> = targets.functions.clone();
    let mut family_notes: Vec<String> = Vec::new();
    for family in &targets.families {
        let (present, absent) = resolve_family(program, family)?;
        family_notes.push(format!(
            "family.{} present={present:?} absent={absent:?}",
            family.label
        ));
        for key in present {
            if !keys.contains(&key) {
                keys.push(key);
            }
        }
    }

    let mut census = Vec::new();
    for key in &keys {
        let one = function_census(key, &program.fns[key]);
        check_static_dispatch(&one)?;
        if let Some(required) = targets.required_regions.get(key) {
            for region in required {
                if !one.regions.iter().any(|(id, _)| id == region) {
                    return Err(format!(
                        "hot census: `{key}` is missing region marker `{}` ({region})",
                        region_name(*region)
                    ));
                }
            }
        }
        census.push(one);
    }

    let scores = crate::cost::score_program(program, table, placement)
        .map_err(|error| format!("hot census: scoring failed: {error}"))?;

    let mut out = String::new();
    out.push_str(&format!(
        "# pixels hot-path census\nschema = {CENSUS_SCHEMA_VERSION}\nprofile = {}\ntable_digest = {}\n",
        table.profile_name(),
        table.table_digest(),
    ));
    for (name, value) in digests {
        out.push_str(&format!("{name} = {value}\n"));
    }
    for note in &family_notes {
        out.push_str(&format!("{note}\n"));
    }

    out.push_str("\n## [M] measured counts\n");
    for one in &census {
        let mut encoded_words = Vec::new();
        for word in &program.fns[&one.key].code {
            encoded_words.extend_from_slice(&word.word.to_le_bytes());
        }
        out.push_str(&format!(
            "\n[fn.{}]\nframe_size = {}\nword_sha256 = {}\n",
            one.key,
            one.frame_size,
            crate::report::sha256_hex(&encoded_words),
        ));
        for (region, counts) in &one.regions {
            out.push_str(&format!(
                "region.{} words={} calls={} indirect_calls={} frame_loads={} frame_stores={}\n",
                region_name(*region),
                counts.words,
                counts.calls,
                counts.indirect_calls,
                counts.frame_loads,
                counts.frame_stores,
            ));
            for (class, n) in &counts.by_class {
                out.push_str(&format!(
                    "region.{}.class.{class} = {n}\n",
                    region_name(*region)
                ));
            }
        }
    }

    out.push_str("\n## [I] modelled cycles\n");
    out.push_str(
        "# Derived from the [M] counts above through the A76 cost table.\n\
         # Never summed with an [M] row; anything citing a cycle cites [I].\n",
    );
    for one in &census {
        let cycles = scores
            .fns
            .iter()
            .find(|scored| scored.key == one.key)
            .map(|scored| scored.proxy_cycles)
            .ok_or_else(|| format!("hot census: `{}` has no modelled score", one.key))?;
        out.push_str(&format!("fn.{}.proxy_cycles = {cycles}\n", one.key));
        for (region, counts) in &one.regions {
            let region_cycles = counts.latency_weighted_cycles(table)?;
            out.push_str(&format!(
                "fn.{}.region.{}.latency_weighted_cycles = {region_cycles}\n",
                one.key,
                region_name(*region),
            ));
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::{EmittedWord, MemRef, Reg};

    fn alu() -> EmittedWord {
        EmittedWord::gpr(0, String::new(), CostRule::Alu, Some(1), &[2, 3])
    }

    fn frame_load() -> EmittedWord {
        EmittedWord::gpr(0, String::new(), CostRule::Load, Some(1), &[31])
            .with_mem(MemRef::stack(8))
    }

    fn packet_add() -> EmittedWord {
        EmittedWord::banked(
            0,
            String::new(),
            CostRule::AsimdInt,
            Some(Reg::fp(2)),
            &[Reg::fp(0), Reg::fp(1)],
        )
    }

    fn cfn(code: Vec<EmittedWord>, regions: Vec<(usize, u32)>) -> CodegenFn {
        CodegenFn {
            frame_size: 64,
            code,
            relocs: Vec::new(),
            regions,
        }
    }

    #[test]
    fn words_split_at_their_markers_and_before_the_first_marker() {
        let f = cfn(
            vec![alu(), frame_load(), packet_add(), packet_add(), alu()],
            vec![(1, 1), (2, 2), (4, 3)],
        );
        let census = function_census("raster", &f);
        let by_id: BTreeMap<u32, RegionCounts> = census.regions.iter().cloned().collect();
        assert_eq!(by_id[&REGION_UNMARKED].words, 1, "the word before marker 1");
        assert_eq!(by_id[&1].words, 1);
        assert_eq!(by_id[&1].frame_loads, 1);
        assert_eq!(by_id[&2].words, 2, "the packet loop");
        assert_eq!(by_id[&2].by_class["asimd_int"], 2);
        assert_eq!(by_id[&3].words, 1);
        assert_eq!(census.total().words, 5);
    }

    #[test]
    fn an_empty_region_is_reported_rather_than_dropped() {
        // Two markers at the same word index: region 2 opens and closes
        // without owning a word, and region 3 owns the rest.
        let f = cfn(vec![alu(), alu()], vec![(1, 2), (1, 3)]);
        let census = function_census("raster", &f);
        let by_id: BTreeMap<u32, RegionCounts> = census.regions.iter().cloned().collect();
        assert_eq!(by_id[&REGION_UNMARKED].words, 1);
        assert_eq!(by_id[&2].words, 0);
        assert_eq!(by_id[&3].words, 1);
        assert_eq!(census.total().words, 2);
    }

    #[test]
    fn an_unsealed_region_id_fails_closed() {
        let mut program = CodegenProgram::default();
        program
            .fns
            .insert("f".to_string(), cfn(vec![alu()], vec![(0, 4242)]));
        let error = check_regions(&program).expect_err("an unknown region must fail");
        assert!(error.contains("sealed vocabulary"), "{error}");
    }

    #[test]
    fn a_runtime_indirect_handler_call_fails_the_census_closed() {
        let indirect = EmittedWord::gpr(0, "blr x9".to_string(), CostRule::Branch, None, &[9]);
        let census = function_census("arrangement", &cfn(vec![indirect], Vec::new()));
        let error = check_static_dispatch(&census).expect_err("indirect handler call must fail");
        assert!(error.contains("statically dispatched"), "{error}");
    }

    #[test]
    fn a_partially_emitted_named_family_fails_closed() {
        let mut program = CodegenProgram::default();
        program
            .fns
            .insert("sqrt_scalar".to_string(), cfn(vec![alu()], Vec::new()));
        let family = CensusFamily {
            label: "sealed_numeric".to_string(),
            members: vec!["sqrt_scalar".to_string(), "rsqrt_scalar".to_string()],
            prefix: None,
        };
        let error = resolve_family(&program, &family)
            .expect_err("one present member must not hide another named member's absence");
        assert!(error.contains("rsqrt_scalar"), "{error}");
    }

    #[test]
    fn region_names_are_unique_and_ids_are_dense_from_zero() {
        let mut ids = region_ids();
        ids.sort_unstable();
        ids.dedup();
        assert_eq!(ids.len(), REGIONS.len(), "region ids must be unique");
        assert_eq!(ids, (0..REGIONS.len() as u32).collect::<Vec<_>>());
        let mut names: Vec<String> = REGIONS.iter().map(|(_, n)| (*n).to_string()).collect();
        names.sort();
        names.dedup();
        assert_eq!(names.len(), REGIONS.len(), "region names must be unique");
    }
}

/// The P8-basis census targets, by literal symbol.
///
/// This list is the one the P8R tightening plan names; changing it changes
/// what every later P8R diff is a diff against, so it moves only with the
/// baseline artifact in the same commit.
pub fn p8_baseline_targets() -> CensusTargets {
    let mut required_regions = BTreeMap::new();
    required_regions.insert(
        "__wrela_pixels_p8_raster_regular".to_string(),
        vec![1, 2, 3, 4],
    );
    required_regions.insert(
        "__wrela_pixels_p7_union_silhouette_coverage_at_slack".to_string(),
        vec![5, 6],
    );
    CensusTargets {
        functions: vec![
            "__wrela_pixels_p8_raster_regular".to_string(),
            "__wrela_pixels_p8_geometry_lane_valid".to_string(),
            "__wrela_pixels_p8_geometry_packet_valid".to_string(),
            "__wrela_pixels_p7_union_silhouette_coverage_at_slack".to_string(),
            "__wrela_pixels_p7_isolate_smooth_object".to_string(),
            "__wrela_pixels_p7_collect_roots_box".to_string(),
        ],
        families: vec![
            CensusFamily {
                label: "p8_write".to_string(),
                members: vec![
                    "__wrela_pixels_p8_write".to_string(),
                    "__wrela_pixels_p8_write4".to_string(),
                ],
                prefix: Some("__wrela_pixels_p8_write".to_string()),
            },
            CensusFamily {
                label: "sealed_numeric".to_string(),
                // The sealed numeric helper sequences. The census fixture
                // deliberately reaches every member so deletion, renaming,
                // or reachability drift fails closed.
                members: vec![
                    "sqrt_scalar".to_string(),
                    "rsqrt_scalar".to_string(),
                    "raster_rsqrt".to_string(),
                ],
                prefix: None,
            },
        ],
        required_regions,
    }
}

/// Emit one synthetic, sealed MWIR kernel containing every P8R.5 packet
/// operation. This is a census basis, not guest product code: the semantic
/// Wrela fixture separately pins lane results and special values, while this
/// kernel makes the emitted opcode/cost-class closure measurable even before
/// a P9 renderer consumer uses every operation in one hot function.
pub fn p8r5_packet_census_program() -> Result<CodegenProgram, String> {
    let f32x4 = Type::Named("F32x4".to_string(), vec![]);
    let i32x4 = Type::Named("I32x4".to_string(), vec![]);
    let temp_types = vec![
        Type::F32,
        Type::I32,
        f32x4.clone(),
        f32x4.clone(),
        f32x4.clone(),
        f32x4.clone(),
        f32x4.clone(),
        f32x4.clone(),
        f32x4.clone(),
        i32x4.clone(),
        i32x4.clone(),
        i32x4.clone(),
        i32x4.clone(),
        i32x4.clone(),
        i32x4.clone(),
        f32x4.clone(),
        f32x4.clone(),
        i32x4.clone(),
        f32x4.clone(),
        i32x4.clone(),
        f32x4.clone(),
    ];
    let body = vec![
        Inst::ConstFloat {
            dst: Temp(0),
            ty: Type::F32,
            bits: 1.25_f32.to_bits().into(),
        },
        Inst::ConstInt {
            dst: Temp(1),
            ty: Type::I32,
            value: 7,
        },
        Inst::PacketSplat {
            kind: mwir::PacketKind::F32x4,
            dst: Temp(2),
            scalar: Temp(0),
        },
        Inst::PacketSplat {
            kind: mwir::PacketKind::F32x4,
            dst: Temp(3),
            scalar: Temp(0),
        },
        Inst::PacketSplat {
            kind: mwir::PacketKind::I32x4,
            dst: Temp(9),
            scalar: Temp(1),
        },
        Inst::PacketBinary {
            kind: mwir::PacketKind::F32x4,
            op: mwir::PacketBinaryOp::Add,
            dst: Temp(4),
            lhs: Temp(2),
            rhs: Temp(3),
        },
        Inst::PacketBinary {
            kind: mwir::PacketKind::F32x4,
            op: mwir::PacketBinaryOp::Sub,
            dst: Temp(5),
            lhs: Temp(2),
            rhs: Temp(3),
        },
        Inst::PacketBinary {
            kind: mwir::PacketKind::F32x4,
            op: mwir::PacketBinaryOp::Mul,
            dst: Temp(6),
            lhs: Temp(2),
            rhs: Temp(3),
        },
        Inst::PacketBinary {
            kind: mwir::PacketKind::F32x4,
            op: mwir::PacketBinaryOp::Min,
            dst: Temp(7),
            lhs: Temp(2),
            rhs: Temp(3),
        },
        Inst::PacketBinary {
            kind: mwir::PacketKind::F32x4,
            op: mwir::PacketBinaryOp::Max,
            dst: Temp(8),
            lhs: Temp(2),
            rhs: Temp(3),
        },
        Inst::PacketBinary {
            kind: mwir::PacketKind::I32x4,
            op: mwir::PacketBinaryOp::Add,
            dst: Temp(10),
            lhs: Temp(9),
            rhs: Temp(9),
        },
        Inst::PacketBinary {
            kind: mwir::PacketKind::I32x4,
            op: mwir::PacketBinaryOp::Sub,
            dst: Temp(11),
            lhs: Temp(9),
            rhs: Temp(9),
        },
        Inst::PacketBinary {
            kind: mwir::PacketKind::I32x4,
            op: mwir::PacketBinaryOp::And,
            dst: Temp(12),
            lhs: Temp(9),
            rhs: Temp(9),
        },
        Inst::PacketBinary {
            kind: mwir::PacketKind::I32x4,
            op: mwir::PacketBinaryOp::Or,
            dst: Temp(13),
            lhs: Temp(9),
            rhs: Temp(9),
        },
        Inst::PacketShiftRightArithmetic {
            dst: Temp(14),
            src: Temp(9),
            immediate: 7,
        },
        Inst::PacketSelect {
            kind: mwir::PacketKind::F32x4,
            op: mwir::PacketSelectOp::Ge,
            dst: Temp(15),
            lhs: Temp(2),
            rhs: Temp(3),
            if_true: Temp(4),
            if_false: Temp(2),
        },
        Inst::PacketSelect {
            kind: mwir::PacketKind::F32x4,
            op: mwir::PacketSelectOp::Gt,
            dst: Temp(16),
            lhs: Temp(2),
            rhs: Temp(3),
            if_true: Temp(4),
            if_false: Temp(2),
        },
        Inst::PacketSelect {
            kind: mwir::PacketKind::I32x4,
            op: mwir::PacketSelectOp::Gt,
            dst: Temp(17),
            lhs: Temp(9),
            rhs: Temp(10),
            if_true: Temp(11),
            if_false: Temp(12),
        },
        Inst::PacketFma {
            dst: Temp(18),
            lhs: Temp(2),
            rhs: Temp(3),
            addend: Temp(4),
        },
        Inst::PacketConvert {
            from: mwir::PacketKind::F32x4,
            to: mwir::PacketKind::I32x4,
            dst: Temp(19),
            src: Temp(2),
        },
        Inst::PacketConvert {
            from: mwir::PacketKind::I32x4,
            to: mwir::PacketKind::F32x4,
            dst: Temp(20),
            src: Temp(9),
        },
        Inst::Return { value: None },
    ];
    let mut mwir = MwirProgram::default();
    mwir.fns.insert(
        "pixels_packet_census".to_string(),
        MwirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::Unit,
            temp_types,
            body,
        },
    );
    let mut layout = LayoutCtx::default();
    layout
        .structs
        .insert("F32x4".to_string(), vec![Type::I64, Type::I64]);
    layout
        .structs
        .insert("I32x4".to_string(), vec![Type::I64, Type::I64]);
    // The packet substrate deliberately has fixed-register lowering and no
    // packet register allocator. Keep this diagnostic kernel in the same
    // non-resident mode as its decoded-word obligation test; release scalar
    // residency is orthogonal and can otherwise assign the scalar splat slot
    // that the fixed-register sequence must load.
    let previous_opts = crate::opts::active_opts();
    crate::opts::apply_mode(crate::opts::CompileMode::Dev);
    let generated = crate::codegen::codegen_program(&mwir, &layout).map_err(|error| error.message);
    crate::opts::apply_opts(&previous_opts);
    generated
}
