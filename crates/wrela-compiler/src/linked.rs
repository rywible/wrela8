//! Final-address, wide instruction representation shared by reporting and
//! costing.  The first linker mode is deliberately wide-only; relaxable
//! fragments build on this representation later.

use std::collections::BTreeMap;

use crate::codegen::{CodegenFn, CodegenProgram};
use crate::cost::{CostRule, EmittedWord};

pub type SectionId = usize;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedSection {
    pub id: SectionId,
    pub name: String,
    pub byte_address: u64,
    pub executable: bool,
    pub code: Vec<EmittedWord>,
    pub raw_bytes: Vec<u8>,
    pub padding_before: u64,
}

impl LinkedSection {
    pub fn payload_bytes(&self) -> u64 {
        if self.executable {
            (self.code.len() as u64) * 4
        } else {
            self.raw_bytes.len() as u64
        }
    }

    pub fn end(&self) -> u64 {
        self.byte_address + self.payload_bytes()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedFn {
    pub key: String,
    pub section: SectionId,
    pub byte_address: u64,
    pub code: Vec<EmittedWord>,
    pub relocs: Vec<crate::codegen::Reloc>,
    pub frame_size: u64,
    /// Stable source/emitter origin partitions.  These ranges tile `code` and
    /// survive section movement and relaxation.
    pub origin_word_ranges: Vec<(u32, usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedOriginBlock {
    pub ordinal: u32,
    pub byte_start: u64,
    pub byte_end: u64,
}

impl LinkedFn {
    /// Return a deterministic origin partition for measured attribution.  The
    /// ordinal is tied to source-order leaders and therefore remains stable
    /// when sections or functions move; synthetic functions naturally get the
    /// same stable source-order ordinals.
    pub fn origin_blocks(&self) -> Vec<LinkedOriginBlock> {
        self.origin_word_ranges
            .iter()
            .map(|&(ordinal, start, end)| LinkedOriginBlock {
                ordinal,
                byte_start: self.byte_address + (start as u64) * 4,
                byte_end: self.byte_address + (end as u64) * 4,
            })
            .collect()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinkedProgram {
    pub sections: Vec<LinkedSection>,
    pub fns: BTreeMap<String, LinkedFn>,
    /// Original widths of address sites, retained for stable relaxation dumps.
    pub address_site_widths: BTreeMap<(String, u32), usize>,
    pub image_bytes: u64,
    pub sync_frame_max_bytes: u64,
    pub async_frame_total_bytes: u64,
}

impl LinkedProgram {
    pub fn from_parts(
        mut sections: Vec<LinkedSection>,
        mut fns: BTreeMap<String, LinkedFn>,
        image_base: u64,
    ) -> Result<LinkedProgram, String> {
        sections.sort_by_key(|s| (s.byte_address, s.id));
        let mut remap = BTreeMap::new();
        let mut previous_end = None;
        for (new, section) in sections.iter_mut().enumerate() {
            remap.insert(section.id, new);
            section.id = new;
            section.padding_before = previous_end
                .map(|end| section.byte_address.saturating_sub(end))
                .unwrap_or(0);
            previous_end = Some(section.end());
        }
        for function in fns.values_mut() {
            function.section = *remap
                .get(&function.section)
                .ok_or_else(|| format!("function `{}` names a missing section", function.key))?;
        }
        let image_end = sections
            .iter()
            .map(LinkedSection::end)
            .max()
            .unwrap_or(image_base);
        let sync_frame_max_bytes = fns.values().map(|f| f.frame_size).max().unwrap_or(0);
        let mut address_site_widths = BTreeMap::new();
        for (key, function) in &fns {
            let mut ordinal = 0u32;
            for reloc in &function.relocs {
                let width = match reloc {
                    crate::codegen::Reloc::Rodata { .. } => Some(2),
                    crate::codegen::Reloc::RodataAdr { .. } => Some(1),
                    _ => None,
                };
                if let Some(width) = width {
                    address_site_widths.insert((key.clone(), ordinal), width);
                    ordinal += 1;
                }
            }
        }
        let linked = LinkedProgram {
            sections,
            fns,
            address_site_widths,
            image_bytes: image_end.saturating_sub(image_base),
            sync_frame_max_bytes,
            async_frame_total_bytes: 0,
        };
        linked.validate()?;
        Ok(linked)
    }

    pub fn executable_words(&self) -> u64 {
        self.sections
            .iter()
            .filter(|s| s.executable)
            .map(|s| s.code.len() as u64)
            .sum()
    }

    pub fn executable_code_bytes(&self) -> u64 {
        self.executable_words() * 4
    }

    pub fn rodata_bytes(&self) -> u64 {
        self.sections
            .iter()
            .filter(|s| !s.executable && s.name == "rodata")
            .map(LinkedSection::payload_bytes)
            .sum()
    }

    pub fn section(&self, id: SectionId) -> Option<&LinkedSection> {
        self.sections.get(id)
    }

    pub fn fn_at(&self, key: &str) -> Option<&LinkedFn> {
        self.fns.get(key)
    }

    pub fn validate(&self) -> Result<(), String> {
        let mut executable_payload = 0u64;
        let mut previous_end = None;
        let mut previous_name = "";
        for (index, section) in self.sections.iter().enumerate() {
            if section.id != index {
                return Err(format!(
                    "linked section `{}` has id {}, expected {index}",
                    section.name, section.id
                ));
            }
            if let Some(end) = previous_end {
                if section.byte_address < end {
                    return Err(format!(
                        "linked sections `{previous_name}` and `{}` overlap",
                        section.name
                    ));
                }
                let actual_padding = section.byte_address - end;
                if section.padding_before != actual_padding {
                    return Err(format!(
                        "linked section `{}` records padding_before={} but its address requires {actual_padding}",
                        section.name, section.padding_before
                    ));
                }
            } else if section.padding_before != 0 {
                return Err(format!(
                    "first linked section `{}` has nonzero padding_before={}",
                    section.name, section.padding_before
                ));
            }
            previous_end = Some(section.end());
            previous_name = &section.name;
            if section.executable && section.byte_address % 4 != 0 {
                return Err(format!(
                    "executable section `{}` is not instruction aligned",
                    section.name
                ));
            }
            if !section.executable && !section.code.is_empty() {
                return Err(format!(
                    "non-executable section `{}` carries instruction metadata",
                    section.name
                ));
            }
            if section.executable {
                executable_payload = executable_payload
                    .checked_add((section.code.len() as u64) * 4)
                    .ok_or_else(|| "linked executable byte count overflow".to_string())?;
                if !section.raw_bytes.is_empty() {
                    return Err(format!(
                        "executable section `{}` carries both words and raw bytes",
                        section.name
                    ));
                }
            }
        }
        if executable_payload != self.executable_words() * 4 {
            return Err("linked executable payload does not reconcile with word count".to_string());
        }
        let first = self.sections.first().map(|section| section.byte_address);
        let last = self.sections.last().map(LinkedSection::end);
        let expected_image_bytes = first
            .zip(last)
            .map(|(first, last)| last.saturating_sub(first))
            .unwrap_or(0);
        if self.image_bytes != expected_image_bytes {
            return Err(format!(
                "linked image byte count {} does not match section span {expected_image_bytes}",
                self.image_bytes
            ));
        }
        let mut by_section: BTreeMap<SectionId, Vec<&LinkedFn>> = BTreeMap::new();
        for (key, f) in &self.fns {
            let Some(section) = self.sections.get(f.section) else {
                return Err(format!("function `{key}` names a missing section"));
            };
            if !section.executable {
                return Err(format!("function `{key}` is in a non-executable section"));
            }
            if f.byte_address < section.byte_address
                || f.byte_address + (f.code.len() as u64) * 4 > section.end()
            {
                return Err(format!(
                    "function `{key}` lies outside its executable section"
                ));
            }
            let mut origin_cursor = 0usize;
            for (ordinal, &(origin, start, end)) in f.origin_word_ranges.iter().enumerate() {
                if origin as usize != ordinal
                    || start != origin_cursor
                    || end < start
                    || end > f.code.len()
                {
                    return Err(format!(
                        "function `{key}` has an invalid origin block {origin} range [{start},{end})"
                    ));
                }
                origin_cursor = end;
            }
            if origin_cursor != f.code.len() {
                return Err(format!(
                    "function `{key}` origin blocks cover 0..{origin_cursor} of {} words",
                    f.code.len()
                ));
            }
            for reloc in &f.relocs {
                let word = crate::relax::reloc_word(reloc);
                if word >= f.code.len() {
                    return Err(format!(
                        "function `{key}` has relocation at word {word} outside its code"
                    ));
                }
                if matches!(reloc, crate::codegen::Reloc::Rodata { .. }) && word + 1 >= f.code.len()
                {
                    return Err(format!(
                        "function `{key}` has a two-word relocation at its final word"
                    ));
                }
            }
            by_section.entry(f.section).or_default().push(f);
        }
        let owned_words: u64 = self.fns.values().map(|f| f.code.len() as u64).sum();
        if owned_words != self.executable_words() {
            return Err(format!(
                "linked executable words are not fully owned by functions: functions={owned_words} sections={}",
                self.executable_words()
            ));
        }
        for section in &self.sections {
            if section.executable
                && !section.code.is_empty()
                && !by_section.contains_key(&section.id)
            {
                return Err(format!(
                    "executable section `{}` has no synthetic or compiled function owner",
                    section.name
                ));
            }
        }
        for (section_id, mut functions) in by_section {
            let section = &self.sections[section_id];
            functions.sort_by_key(|f| (f.byte_address, f.key.as_str()));
            let mut cursor = section.byte_address;
            for f in functions {
                if f.byte_address != cursor {
                    return Err(format!(
                        "executable section `{}` has a gap or overlap before `{}`",
                        section.name, f.key
                    ));
                }
                let start = ((cursor - section.byte_address) / 4) as usize;
                let end = start + f.code.len();
                if section.code.get(start..end) != Some(f.code.as_slice()) {
                    return Err(format!(
                        "function `{}` metadata/code differs from section `{}`",
                        f.key, section.name
                    ));
                }
                cursor += (f.code.len() as u64) * 4;
            }
            if cursor != section.end() {
                return Err(format!(
                    "executable section `{}` has unowned words",
                    section.name
                ));
            }
        }
        Ok(())
    }

    /// Serialize only after metadata validation.  The output is the existing
    /// little-endian wide instruction stream followed by each non-executable
    /// payload at its recorded address; holes are zero fill, never words.
    pub fn serialize(&self, image_base: u64) -> Result<Vec<u8>, String> {
        self.validate()?;
        let mut out = Vec::new();
        for section in &self.sections {
            let want = section
                .byte_address
                .checked_sub(image_base)
                .ok_or_else(|| format!("section `{}` precedes image base", section.name))?
                as usize;
            if out.len() > want {
                return Err(format!("linked sections overlap at `{}`", section.name));
            }
            out.resize(want, 0);
            if section.executable {
                for word in &section.code {
                    out.extend_from_slice(&word.word.to_le_bytes());
                }
            } else {
                out.extend_from_slice(&section.raw_bytes);
            }
        }
        Ok(out)
    }
}

#[cfg(test)]
fn raw_word(word: u32, text: &str, rule: CostRule) -> EmittedWord {
    EmittedWord::gpr(word, text.to_string(), rule, None, &[])
}

pub(crate) fn default_origin_ranges(code: &[EmittedWord]) -> Vec<(u32, usize, usize)> {
    crate::cost::score::basic_block_ranges(code)
        .into_iter()
        .enumerate()
        .map(|(ordinal, (start, end))| (ordinal as u32, start, end))
        .collect()
}

pub(crate) fn recorded_origin_ranges(
    spans: &[crate::codegen::BlockSpan],
    key: &str,
    code: &[EmittedWord],
) -> Vec<(u32, usize, usize)> {
    let mut spans: Vec<_> = spans
        .iter()
        .filter(|span| span.fn_key == key)
        .cloned()
        .collect();
    spans.sort_by_key(|span| span.block_index);
    let valid = !spans.is_empty()
        && spans.iter().enumerate().all(|(ordinal, span)| {
            span.block_index as usize == ordinal
                && span.word_start <= span.word_end
                && span.word_end <= code.len()
                && (ordinal == 0 || spans[ordinal - 1].word_end == span.word_start)
        })
        && spans.first().is_some_and(|span| span.word_start == 0)
        && spans.last().is_some_and(|span| span.word_end == code.len());
    if valid {
        spans
            .into_iter()
            .map(|span| (span.block_index, span.word_start, span.word_end))
            .collect()
    } else {
        default_origin_ranges(code)
    }
}

fn is_control(word: u32) -> bool {
    word & 0x7c00_0000 == 0x1400_0000
        || word & 0xff00_0010 == 0x5400_0000
        || word & 0x7e00_0000 == 0x3400_0000
        || word & 0xfc00_0000 == 0x9400_0000
        || word & 0xffff_fc1f == 0xd65f_0000
}

/// Complete metadata for layout-injected helper functions whose small emitters
/// historically used the raw `push` helper for a memory opcode.  The address
/// identity is deliberately Unknown unless the encoded base is SP, where the
/// immediate identifies a compiler-owned stack line.
pub fn complete_memory_metadata(key: &str, code: &mut [EmittedWord]) {
    let mut site = 0xcbf2_9ce4_8422_2325u64;
    for byte in key.as_bytes() {
        site = site.rotate_left(5) ^ u64::from(*byte);
    }
    for (i, ew) in code.iter_mut().enumerate() {
        if !(ew.rule.is_load() || ew.rule.is_store()) || ew.mem.is_some() {
            continue;
        }
        let base = ((ew.word >> 5) & 0x1f) as u8;
        let bytes = u64::from(ew.access_bytes.max(1));
        let imm = ((ew.word >> 10) & 0xfff) as u64 * bytes;
        let mem = if base == crate::cost::MEM_SP_REG {
            crate::cost::MemRef::stack(imm)
        } else {
            crate::cost::MemRef::unknown(
                site ^ (i as u64).wrapping_mul(0x9e37_79b9),
                Some(base),
                imm,
            )
        };
        ew.mem = Some(mem);
    }
}

/// Attach conservative metadata to fixed-section words produced by the
/// layout harness.  These words are synthetic functions, not raw u32s.
pub fn synthetic_words(words: &[u32], section: &str) -> Vec<EmittedWord> {
    words
        .iter()
        .enumerate()
        .map(|(i, &word)| {
            let rule = if crate::encode::access_width_bytes(word).is_some() {
                if word & 0x0040_0000 != 0 {
                    CostRule::Load
                } else {
                    CostRule::Store
                }
            } else if word & 0x1f80_0000 == 0x1280_0000 {
                CostRule::MovWide
            } else if word & 0x9f00_0000 == 0x9000_0000 {
                CostRule::Adrp
            } else if word == crate::encode::enc_dmb_ishst()
                || word == crate::encode::enc_dmb_ishld()
            {
                CostRule::Barrier
            } else if word & 0xffe0_001f == 0xd420_0000 {
                CostRule::System
            } else if is_control(word) {
                if word & 0xfc00_0000 == 0x9400_0000 {
                    CostRule::Call
                } else {
                    CostRule::Branch
                }
            } else {
                CostRule::Alu
            };
            let dst = if matches!(rule, CostRule::MovWide | CostRule::Adrp | CostRule::Load) {
                Some((word & 0x1f) as u8)
            } else if rule == CostRule::Call {
                Some(0)
            } else {
                None
            };
            let srcs = if rule.is_load() {
                vec![((word >> 5) & 0x1f) as u8]
            } else if rule.is_store() {
                vec![(word & 0x1f) as u8, ((word >> 5) & 0x1f) as u8]
            } else if rule == CostRule::MovWide && word & 0xff80_0000 == 0xf280_0000 {
                vec![(word & 0x1f) as u8]
            } else if rule == CostRule::Branch && word & 0x7e00_0000 == 0x3400_0000 {
                vec![(word & 0x1f) as u8]
            } else if rule == CostRule::Branch
                && (word & 0xffff_fc1f == 0xd61f_0000 || word & 0xffff_fc1f == 0xd65f_0000)
            {
                vec![((word >> 5) & 0x1f) as u8]
            } else {
                Vec::new()
            };
            let mut ew = EmittedWord::gpr(word, format!("{section}[{i}]"), rule, dst, &srcs);
            if rule == CostRule::Branch && word & 0xff00_0010 == 0x5400_0000 && (word & 0xf) < 14 {
                ew.flags = crate::cost::FlagEffect::Read;
            }
            if rule.is_load() || rule.is_store() {
                ew.mem = Some(crate::cost::MemRef::cold_unique(i as u64));
            }
            ew
        })
        .collect()
}

/// Link a CodegenProgram in deterministic wide-only mode.  Layout-time image
/// injection can use `link_sections` with the same representation.
pub fn link_wide(program: &CodegenProgram, image_base: u64) -> Result<LinkedProgram, String> {
    let mut code = Vec::new();
    let mut fns = BTreeMap::new();
    let code_base = image_base;
    for (key, f) in &program.fns {
        let address = code_base + (code.len() as u64) * 4;
        fns.insert(
            key.clone(),
            LinkedFn {
                key: key.clone(),
                section: 0,
                byte_address: address,
                code: f.code.clone(),
                relocs: f.relocs.clone(),
                frame_size: f.frame_size as u64,
                origin_word_ranges: recorded_origin_ranges(&program.origin_spans, key, &f.code),
            },
        );
        code.extend(f.code.iter().cloned());
    }
    let mut sections = vec![LinkedSection {
        id: 0,
        name: "code".to_string(),
        byte_address: code_base,
        executable: true,
        code,
        raw_bytes: Vec::new(),
        padding_before: 0,
    }];
    if !program.rodata.is_empty() {
        let code_end = sections[0].end();
        let rodata_address = code_end.div_ceil(8) * 8;
        let raw_bytes: Vec<u8> = program
            .rodata
            .iter()
            .flat_map(|bytes| bytes.iter().copied())
            .collect();
        sections.push(LinkedSection {
            id: 1,
            name: "rodata".to_string(),
            byte_address: rodata_address,
            executable: false,
            code: Vec::new(),
            raw_bytes,
            padding_before: rodata_address - code_end,
        });
    }
    LinkedProgram::from_parts(sections, fns, image_base)
}

/// Build a linked program from already placed function code and fixed
/// executable sections.  `fixed` entries use stable synthetic keys.
pub fn link_sections(
    functions: &BTreeMap<String, CodegenFn>,
    fixed: &[(String, u64, Vec<EmittedWord>)],
    rodata: &[Vec<u8>],
    image_base: u64,
) -> Result<LinkedProgram, String> {
    let mut sections = Vec::new();
    let mut fns = BTreeMap::new();
    let mut cursor = image_base;
    let mut code = Vec::new();
    let code_base = cursor;
    for (key, f) in functions {
        let address = code_base + (code.len() as u64) * 4;
        fns.insert(
            key.clone(),
            LinkedFn {
                key: key.clone(),
                section: 0,
                byte_address: address,
                code: f.code.clone(),
                relocs: f.relocs.clone(),
                frame_size: f.frame_size as u64,
                origin_word_ranges: default_origin_ranges(&f.code),
            },
        );
        code.extend(f.code.iter().cloned());
    }
    cursor += (code.len() as u64) * 4;
    sections.push(LinkedSection {
        id: 0,
        name: "code".to_string(),
        byte_address: code_base,
        executable: true,
        code,
        raw_bytes: Vec::new(),
        padding_before: 0,
    });
    for (key, address, words) in fixed {
        let id = sections.len();
        sections.push(LinkedSection {
            id,
            name: key.clone(),
            byte_address: *address,
            executable: true,
            code: words.clone(),
            raw_bytes: Vec::new(),
            padding_before: address.saturating_sub(cursor),
        });
        fns.insert(
            key.clone(),
            LinkedFn {
                key: key.clone(),
                section: id,
                byte_address: *address,
                code: words.clone(),
                relocs: Vec::new(),
                frame_size: 0,
                origin_word_ranges: default_origin_ranges(words),
            },
        );
        cursor = cursor.max(address + (words.len() as u64) * 4);
    }
    if !rodata.is_empty() {
        let address = cursor.div_ceil(8) * 8;
        let raw_bytes: Vec<u8> = rodata.iter().flat_map(|x| x.iter().copied()).collect();
        let id = sections.len();
        sections.push(LinkedSection {
            id,
            name: "rodata".to_string(),
            byte_address: address,
            executable: false,
            code: Vec::new(),
            raw_bytes,
            padding_before: address.saturating_sub(cursor),
        });
    }
    LinkedProgram::from_parts(sections, fns, image_base)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cost::CostRule;

    fn f(words: usize) -> CodegenFn {
        CodegenFn {
            frame_size: 8,
            code: (0..words)
                .map(|i| raw_word(i as u32, "nop", CostRule::Alu))
                .collect(),
            relocs: Vec::new(),
            regions: Vec::new(),
        }
    }

    #[test]
    fn functions_are_adjacent_without_hypothetical_cache_padding() {
        let p = CodegenProgram {
            fns: BTreeMap::from([("a".into(), f(2)), ("b".into(), f(2))]),
            ..CodegenProgram::default()
        };
        let linked = link_wide(&p, 0x1000).expect("link");
        assert_eq!(linked.fns["b"].byte_address, 0x1008);
        assert_eq!(linked.executable_words(), 4);
        assert_eq!(linked.executable_code_bytes(), 16);
        assert_eq!(linked.serialize(0x1000).expect("serialize").len(), 16);
        assert_eq!(linked.fns["a"].origin_blocks()[0].byte_start, 0x1000);
    }

    #[test]
    fn linking_uses_program_owned_origins_after_another_compilation_replaces_the_legacy_view() {
        let mut p = CodegenProgram {
            fns: BTreeMap::from([("a".into(), f(3))]),
            ..CodegenProgram::default()
        };
        p.origin_spans = vec![
            crate::codegen::BlockSpan {
                fn_key: "a".into(),
                block_index: 0,
                id: 10,
                word_start: 0,
                word_end: 1,
            },
            crate::codegen::BlockSpan {
                fn_key: "a".into(),
                block_index: 1,
                id: 11,
                word_start: 1,
                word_end: 3,
            },
        ];
        crate::codegen::replace_block_spans(vec![crate::codegen::BlockSpan {
            fn_key: "unrelated".into(),
            block_index: 0,
            id: 99,
            word_start: 0,
            word_end: 999,
        }]);

        let linked = link_wide(&p, 0x1000).expect("link");
        let blocks = linked.fns["a"].origin_blocks();
        assert_eq!(
            blocks
                .iter()
                .map(|block| (block.ordinal, block.byte_start, block.byte_end))
                .collect::<Vec<_>>(),
            vec![(0, 0x1000, 0x1004), (1, 0x1004, 0x100c)]
        );
    }

    #[test]
    fn fixed_sections_are_synthetic_functions_and_not_rodata_words() {
        let p = CodegenProgram {
            fns: BTreeMap::from([("a".into(), f(1))]),
            rodata: vec![vec![1, 2, 3]],
            ..CodegenProgram::default()
        };
        let fixed = vec![(
            "__image_entry".into(),
            0x2004,
            vec![raw_word(0, "b", CostRule::Branch)],
        )];
        let linked = link_sections(&p.fns, &fixed, &p.rodata, 0x2000).expect("link");
        assert!(linked.fns.contains_key("__image_entry"));
        assert_eq!(linked.executable_words(), 2);
        assert_eq!(linked.rodata_bytes(), 3);
    }
}
