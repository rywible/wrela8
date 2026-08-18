//! Offline cycle-proxy validation schema and standing drift lock.

use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use crate::evidence::{self, Record};

pub(crate) const FORMAT: &str = "wrela-proxy-validation-v1";
pub(crate) const ENVELOPE_FORMAT: &str = "wrela-proxy-envelopes-v1";
pub(crate) const PROXY_REVISION: &str = "a76-pi5-v2";
const REQUIRED_COUNTERS: &[&str] = &[
    "br_mis_pred",
    "cpu_cycles",
    "inst_retired",
    "l1d_cache_refill",
    "l2d_cache_refill",
    "stall_backend",
    "stall_frontend",
];
const GLOBAL_FIELDS: &[&str] = &[
    "build_features",
    "build_profile",
    "build_target",
    "calibration_verdict",
    "cargo_lock_sha256",
    "conservatism_violations",
    "corpus_manifest_sha256",
    "cost_profile_sha256",
    "counter_config",
    "discordance_rate_milli",
    "envelope_provenance",
    "envelopes_sha256",
    "frame_measurement_error_cycles",
    "frame_overprediction_envelope_milli",
    "holdout_manifest_sha256",
    "holdout_verdict",
    "host_identity_sha256",
    "host_profile_sha256",
    "kernel_measurement_error_cycles",
    "kernel_overprediction_envelope_milli",
    "lab_agent_sha256",
    "linker_identity_sha256",
    "max_overprediction_ratio_milli",
    "measurement_error_model",
    "operator",
    "proxy_revision",
    "proxy_rules_sha256",
    "retrieval_method",
    "retrieved_at_utc",
    "run_environment_sha256",
    "rustc_identity_sha256",
    "sequence_measurement_error_cycles",
    "sequence_overprediction_envelope_milli",
    "verdict",
    "vmm_binary_sha256",
    "vmm_source_sha256",
];
const CASE_FIELDS: &[&str] = &[
    "branch_attribution_verdict",
    "cache_state",
    "case",
    "conservatism_verdict",
    "corpus_set",
    "counter_rows",
    "frame_count",
    "frame_digest_sequence",
    "image_sha256",
    "measured_max_cycles",
    "measured_median_cycles",
    "measured_min_cycles",
    "measurement_error_cycles",
    "memory_attribution_verdict",
    "modeled_branch_paths",
    "modeled_memory_accesses",
    "modeled_memory_transitions",
    "overprediction_envelope_verdict",
    "overprediction_ratio_milli",
    "presentation_mode",
    "predicted_cycles_per_core",
    "record_digests",
    "report_sha256",
    "run_environment_after_sha256",
    "run_environment_before_sha256",
    "sample_count",
    "samples_cycles_per_vcpu",
    "samples_frame_cadence_ns",
    "stage1_tables_sha256",
    "stdout_sha256",
    "sustained_duration_ns",
    "sustained_frame_count",
    "sustained_frame_digest_sequence",
    "sustained_launch_count",
    "sustained_record_sha256",
    "sustained_refresh_hz",
    "sustained_stdout_sha256",
    "sustained_vsync_sequence",
    "temperature_after_millic",
    "temperature_before_millic",
    "throttle_after",
    "throttle_before",
    "workload_class",
    "workload_sha256",
];
const PAIR_FIELDS: &[&str] = &[
    "discordant",
    "first_case",
    "measured_order",
    "noise_cycles",
    "pair",
    "predicted_order",
    "second_case",
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ValidationSummary {
    pub cases: usize,
    pub pairs: usize,
    pub violations: u64,
    pub max_ratio_milli: u64,
    pub discordance_rate_milli: u64,
    pub envelope_violations: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CorpusCase {
    pub(crate) set: String,
    pub(crate) workload_class: String,
    pub(crate) case: String,
    pub(crate) workload_kind: String,
    pub(crate) selection_decision: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ClassEnvelope {
    pub(crate) workload_class: String,
    pub(crate) case: String,
    pub(crate) measurement_error_cycles: u64,
    pub(crate) overprediction_envelope_milli: u64,
    pub(crate) predicted_cycles_per_core: Vec<u64>,
    pub(crate) samples_cycles_per_vcpu: Vec<Vec<u64>>,
}

pub(crate) fn sealed_overprediction_envelope_milli(class: &str) -> Option<u64> {
    match class {
        "frame" => Some(3090),
        "kernel" => Some(2779),
        "sequence" => Some(3110),
        _ => None,
    }
}

fn per_core_stats(samples: &[Vec<u64>]) -> Vec<[u64; 3]> {
    (0..samples[0].len())
        .map(|core| {
            let mut values = samples.iter().map(|row| row[core]).collect::<Vec<_>>();
            values.sort_unstable();
            [
                values[0],
                values[(values.len() - 1) / 2],
                *values.last().unwrap(),
            ]
        })
        .collect()
}

pub(crate) fn per_core_measurement_error(samples: &[Vec<u64>]) -> u64 {
    per_core_stats(samples)
        .iter()
        .map(|values| values[2].saturating_sub(values[0]))
        .max()
        .unwrap_or(0)
}

pub(crate) fn per_core_bounds(
    predicted: &[u64],
    samples: &[Vec<u64>],
    measurement_error: u64,
) -> (bool, u64) {
    let stats = per_core_stats(samples);
    let conservative = predicted
        .iter()
        .zip(&stats)
        .all(|(prediction, measured)| measured[2] <= prediction.saturating_add(measurement_error));
    let max_ratio = predicted
        .iter()
        .zip(stats)
        .map(|(prediction, measured)| prediction.saturating_mul(1000) / measured[1].max(1))
        .max()
        .unwrap_or(0);
    (conservative, max_ratio)
}

pub(crate) fn parse_envelopes(text: &str) -> Result<(Record, Vec<ClassEnvelope>), String> {
    let record = evidence::parse(text, ENVELOPE_FORMAT)?;
    let mut globals = BTreeMap::new();
    let mut rows: BTreeMap<usize, BTreeMap<String, String>> = BTreeMap::new();
    for (key, value) in &record.fields {
        if let Some((index, field)) = indexed_for(key, "class", ENVELOPE_FORMAT)? {
            rows.entry(index).or_default().insert(field, value.clone());
        } else {
            globals.insert(key.clone(), value.clone());
        }
    }
    require_keys(
        "proxy envelopes",
        &globals,
        &[
            "calibration_manifest_sha256",
            "counter_config",
            "cost_profile_sha256",
            "host_identity_sha256",
            "host_profile_sha256",
            "lab_agent_sha256",
            "measurement_error_model",
            "proxy_revision",
            "proxy_rules_sha256",
            "sample_count",
            "vmm_binary_sha256",
            "warmup_count",
        ],
    )?;
    if globals["measurement_error_model"] != "per-class-per-core-max-range-v2" {
        return Err(format!(
            "{ENVELOPE_FORMAT}: unsupported measurement error model"
        ));
    }
    for key in [
        "calibration_manifest_sha256",
        "cost_profile_sha256",
        "host_identity_sha256",
        "host_profile_sha256",
        "lab_agent_sha256",
        "proxy_rules_sha256",
        "vmm_binary_sha256",
    ] {
        evidence::require_sha256(key, &globals[key])?;
    }
    let sample_count = evidence::canonical_u64("sample_count", &globals["sample_count"])? as usize;
    if sample_count < 5 || evidence::canonical_u64("warmup_count", &globals["warmup_count"])? == 0 {
        return Err(format!(
            "{ENVELOPE_FORMAT}: requires at least five samples and one warmup"
        ));
    }
    contiguous_for(ENVELOPE_FORMAT, "class", &rows)?;
    if rows.len() != 3 {
        return Err(format!(
            "{ENVELOPE_FORMAT}: requires exactly three workload classes"
        ));
    }
    let mut out = Vec::new();
    let mut prior = "";
    for fields in rows.values() {
        require_keys(
            "proxy envelope class",
            fields,
            &[
                "case",
                "measurement_error_cycles",
                "overprediction_envelope_milli",
                "predicted_cycles_per_core",
                "samples_cycles_per_vcpu",
                "workload_class",
            ],
        )?;
        let class = fields["workload_class"].as_str();
        if !matches!(class, "frame" | "kernel" | "sequence") || class <= prior {
            return Err(format!(
                "{ENVELOPE_FORMAT}: class rows are not sorted and unique"
            ));
        }
        prior = class;
        let predicted = evidence::parse_u64_list(
            "predicted_cycles_per_core",
            &fields["predicted_cycles_per_core"],
        )?;
        let samples = parse_vectors(&fields["samples_cycles_per_vcpu"])?;
        if predicted.is_empty()
            || samples.len() != sample_count
            || samples.iter().any(|row| row.len() != predicted.len())
        {
            return Err(format!(
                "{ENVELOPE_FORMAT}: `{class}` vector dimensions differ"
            ));
        }
        let error = per_core_measurement_error(&samples);
        let (_, observed_ratio) = per_core_bounds(&predicted, &samples, error);
        let envelope = sealed_overprediction_envelope_milli(class)
            .expect("validated workload class has a sealed envelope");
        if evidence::canonical_u64(
            "measurement_error_cycles",
            &fields["measurement_error_cycles"],
        )? != error
            || evidence::canonical_u64(
                "overprediction_envelope_milli",
                &fields["overprediction_envelope_milli"],
            )? != envelope
        {
            return Err(format!(
                "{ENVELOPE_FORMAT}: `{class}` measurement error or sealed V1 envelope differs"
            ));
        }
        if observed_ratio > envelope {
            return Err(format!(
                "{ENVELOPE_FORMAT}: `{class}` calibration breaches its sealed V1 envelope"
            ));
        }
        out.push(ClassEnvelope {
            workload_class: class.to_string(),
            case: fields["case"].clone(),
            measurement_error_cycles: error,
            overprediction_envelope_milli: envelope,
            predicted_cycles_per_core: predicted,
            samples_cycles_per_vcpu: samples,
        });
    }
    Ok((record, out))
}

pub(crate) fn read_manifest(path: &Path, set: &str) -> Result<Vec<CorpusCase>, String> {
    let text = std::fs::read_to_string(path)
        .map_err(|error| format!("proxy corpus: read {}: {error}", path.display()))?;
    parse_manifest(&text, path, set)
}

fn parse_manifest(text: &str, path: &Path, set: &str) -> Result<Vec<CorpusCase>, String> {
    if !text.ends_with('\n') {
        return Err(format!("proxy corpus: {} lacks final LF", path.display()));
    }
    let mut cases = Vec::new();
    let mut prior = "";
    let mut names = BTreeSet::new();
    for line in text.lines() {
        if line.starts_with('#') || line.is_empty() {
            continue;
        }
        let parts = line.split('|').collect::<Vec<_>>();
        if parts.len() != 5
            || parts[0] != set
            || !matches!(parts[1], "kernel" | "frame" | "sequence")
            || !matches!(parts[3], "real" | "differential")
            || parts[2].is_empty()
            || parts[4].is_empty()
        {
            return Err(format!("proxy corpus: malformed row `{line}`"));
        }
        if line <= prior {
            return Err(format!(
                "proxy corpus: rows in {} are not sorted",
                path.display()
            ));
        }
        if !names.insert(parts[2]) {
            return Err(format!("proxy corpus: repeated case `{}`", parts[2]));
        }
        prior = line;
        cases.push(CorpusCase {
            set: parts[0].to_string(),
            workload_class: parts[1].to_string(),
            case: parts[2].to_string(),
            workload_kind: parts[3].to_string(),
            selection_decision: parts[4].to_string(),
        });
    }
    for class in ["kernel", "frame", "sequence"] {
        if !cases.iter().any(|case| case.workload_class == class) {
            return Err(format!("proxy corpus: `{set}` lacks class `{class}`"));
        }
        if !cases
            .iter()
            .any(|case| case.workload_class == class && case.workload_kind == "real")
        {
            return Err(format!(
                "proxy corpus: `{set}` class `{class}` lacks a real workload"
            ));
        }
    }
    if cases.len() != 3 {
        return Err(format!(
            "proxy corpus: `{set}` requires exactly one case in each workload class"
        ));
    }
    Ok(cases)
}

pub(crate) fn validate_candidate_pairs(
    calibration: &[CorpusCase],
    holdout: &[CorpusCase],
) -> Result<(), String> {
    for class in ["frame", "kernel", "sequence"] {
        let first = calibration
            .iter()
            .find(|case| case.workload_class == class)
            .ok_or_else(|| format!("proxy corpus: calibration lacks `{class}`"))?;
        let second = holdout
            .iter()
            .find(|case| case.workload_class == class)
            .ok_or_else(|| format!("proxy corpus: holdout lacks `{class}`"))?;
        if first.selection_decision != second.selection_decision {
            return Err(format!(
                "proxy corpus: `{class}` candidates do not name one compiler selection decision"
            ));
        }
    }
    Ok(())
}

pub(crate) fn parse_and_validate(text: &str) -> Result<(Record, ValidationSummary), String> {
    let record = evidence::parse(text, FORMAT)?;
    let mut globals = BTreeMap::new();
    let mut cases: BTreeMap<usize, BTreeMap<String, String>> = BTreeMap::new();
    let mut pairs: BTreeMap<usize, BTreeMap<String, String>> = BTreeMap::new();
    for (key, value) in &record.fields {
        if let Some((index, field)) = indexed(key, "case")? {
            cases.entry(index).or_default().insert(field, value.clone());
        } else if let Some((index, field)) = indexed(key, "pair")? {
            pairs.entry(index).or_default().insert(field, value.clone());
        } else {
            globals.insert(key.clone(), value.clone());
        }
    }
    require_keys(FORMAT, &globals, GLOBAL_FIELDS)?;
    for key in [
        "cargo_lock_sha256",
        "corpus_manifest_sha256",
        "cost_profile_sha256",
        "envelopes_sha256",
        "holdout_manifest_sha256",
        "host_identity_sha256",
        "host_profile_sha256",
        "lab_agent_sha256",
        "linker_identity_sha256",
        "proxy_rules_sha256",
        "run_environment_sha256",
        "rustc_identity_sha256",
        "vmm_binary_sha256",
        "vmm_source_sha256",
    ] {
        evidence::require_sha256(key, &globals[key])?;
    }
    if globals["build_target"] != "aarch64-unknown-linux-musl"
        || globals["build_profile"] != "release"
        || globals["build_features"] != "native-presentation"
        || globals["retrieval_method"] != "content-addressed-sftp-v1"
        || globals["envelope_provenance"] != "repeated-conforming-calibration-v1"
        || globals["measurement_error_model"] != "per-class-per-core-max-range-v2"
    {
        return Err(format!(
            "{FORMAT}: build, retrieval, or envelope provenance is invalid"
        ));
    }
    if globals["operator"].is_empty() || !is_utc_second(&globals["retrieved_at_utc"]) {
        return Err(format!(
            "{FORMAT}: operator/retrieval timestamp provenance is invalid"
        ));
    }
    let mut class_limits = BTreeMap::new();
    for class in ["kernel", "frame", "sequence"] {
        let error = evidence::canonical_u64(
            &format!("{class} measurement error"),
            &globals[&format!("{class}_measurement_error_cycles")],
        )?;
        let envelope = evidence::canonical_u64(
            &format!("{class} overprediction envelope"),
            &globals[&format!("{class}_overprediction_envelope_milli")],
        )?;
        if envelope < 1000 {
            return Err(format!("{FORMAT}: `{class}` envelope cannot be below 1.0x"));
        }
        class_limits.insert(class, (error, envelope));
    }
    contiguous("case", &cases)?;
    contiguous("pair", &pairs)?;
    if cases.is_empty() {
        return Err(format!("{FORMAT}: at least one case is required"));
    }

    let mut violations = 0_u64;
    let mut max_ratio = 0_u64;
    let mut envelope_violations = 0_u64;
    let mut calibration_ok = true;
    let mut holdout_ok = true;
    let mut prior_case: Option<&str> = None;
    let mut names = BTreeSet::new();
    let mut order_values: BTreeMap<String, (u64, u64)> = BTreeMap::new();
    for fields in cases.values() {
        require_keys("proxy validation case", fields, CASE_FIELDS)?;
        let name = fields["case"].as_str();
        if prior_case.is_some_and(|old| old >= name) || !names.insert(name) {
            return Err(format!(
                "{FORMAT}: case rows are not strictly sorted and unique"
            ));
        }
        prior_case = Some(name);
        if !matches!(
            fields["workload_class"].as_str(),
            "kernel" | "frame" | "sequence"
        ) {
            return Err(format!("{FORMAT}: `{name}` has invalid workload_class"));
        }
        if !matches!(fields["corpus_set"].as_str(), "calibration" | "holdout") {
            return Err(format!("{FORMAT}: `{name}` has invalid corpus_set"));
        }
        if !matches!(fields["cache_state"].as_str(), "cold" | "warm") {
            return Err(format!("{FORMAT}: `{name}` has invalid cache_state"));
        }
        for key in [
            "image_sha256",
            "report_sha256",
            "run_environment_after_sha256",
            "run_environment_before_sha256",
            "stage1_tables_sha256",
            "stdout_sha256",
            "workload_sha256",
        ] {
            evidence::require_sha256(key, &fields[key])?;
        }
        let count = evidence::canonical_u64("sample_count", &fields["sample_count"])? as usize;
        if count == 0 {
            return Err(format!("{FORMAT}: `{name}` has zero samples"));
        }
        let record_digests = fields["record_digests"].split(',').collect::<Vec<_>>();
        if record_digests.len() != count {
            return Err(format!(
                "{FORMAT}: `{name}` record digest count differs from samples"
            ));
        }
        for digest in record_digests {
            evidence::require_sha256("record digest", digest)?;
        }
        let samples = parse_vectors(&fields["samples_cycles_per_vcpu"])?;
        if samples.len() != count || samples.iter().any(Vec::is_empty) {
            return Err(format!(
                "{FORMAT}: `{name}` sample_count/vector width mismatch"
            ));
        }
        let mut maxima: Vec<u64> = samples
            .iter()
            .map(|row| *row.iter().max().expect("nonempty"))
            .collect();
        maxima.sort_unstable();
        let derived = [
            maxima[0],
            maxima[(maxima.len() - 1) / 2],
            *maxima.last().unwrap(),
        ];
        for (key, want) in [
            ("measured_min_cycles", derived[0]),
            ("measured_median_cycles", derived[1]),
            ("measured_max_cycles", derived[2]),
        ] {
            if evidence::canonical_u64(key, &fields[key])? != want {
                return Err(format!(
                    "{FORMAT}: `{name}` `{key}` is not derived from samples"
                ));
            }
        }
        let predicted = evidence::parse_u64_list(
            "predicted_cycles_per_core",
            &fields["predicted_cycles_per_core"],
        )?;
        if predicted.is_empty()
            || predicted.len() != samples[0].len()
            || samples.iter().any(|row| row.len() != predicted.len())
        {
            return Err(format!(
                "{FORMAT}: `{name}` predicted/measured vCPU widths differ"
            ));
        }
        let modeled_branch_paths =
            evidence::canonical_u64("modeled_branch_paths", &fields["modeled_branch_paths"])?;
        let modeled_memory_accesses = evidence::canonical_u64(
            "modeled_memory_accesses",
            &fields["modeled_memory_accesses"],
        )?;
        let modeled_memory_transitions = evidence::canonical_u64(
            "modeled_memory_transitions",
            &fields["modeled_memory_transitions"],
        )?;
        validate_counter_attribution(
            &fields["counter_rows"],
            count,
            predicted.len(),
            modeled_branch_paths,
            modeled_memory_accesses,
            modeled_memory_transitions,
        )?;
        if fields["branch_attribution_verdict"] != "pass"
            || fields["memory_attribution_verdict"] != "pass"
        {
            return Err(format!(
                "{FORMAT}: `{name}` has a non-passing modeled-counter attribution"
            ));
        }
        let cadence = parse_optional_vector(&fields["samples_frame_cadence_ns"])?;
        let frame_count = evidence::canonical_u64("frame_count", &fields["frame_count"])?;
        if (frame_count == 0) != cadence.is_empty()
            || (!cadence.is_empty() && cadence.len() != count)
        {
            return Err(format!(
                "{FORMAT}: `{name}` frame cadence/sample count is inconsistent"
            ));
        }
        let frame_digests = fields["frame_digest_sequence"]
            .split(',')
            .filter(|value| !value.is_empty())
            .collect::<Vec<_>>();
        if frame_digests.len() != frame_count as usize {
            return Err(format!(
                "{FORMAT}: `{name}` frame digest count is inconsistent"
            ));
        }
        for digest in frame_digests {
            evidence::require_sha256("frame digest", digest)?;
        }
        if !matches!(
            fields["presentation_mode"].as_str(),
            "headless" | "drm-active"
        ) {
            return Err(format!("{FORMAT}: `{name}` has invalid presentation_mode"));
        }
        let sustained_duration =
            evidence::canonical_u64("sustained_duration_ns", &fields["sustained_duration_ns"])?;
        let sustained_frames =
            evidence::canonical_u64("sustained_frame_count", &fields["sustained_frame_count"])?;
        let sustained_launches =
            evidence::canonical_u64("sustained_launch_count", &fields["sustained_launch_count"])?;
        if fields["workload_class"] == "sequence" {
            if fields["presentation_mode"] != "drm-active"
                || sustained_duration < 120_000_000_000
                || sustained_launches != 1
                || sustained_frames < 2
            {
                return Err(format!(
                    "{FORMAT}: `{name}` lacks one continuous two-minute active-DRM sequence"
                ));
            }
            for key in ["sustained_record_sha256", "sustained_stdout_sha256"] {
                evidence::require_sha256(key, &fields[key])?;
            }
            let digests = fields["sustained_frame_digest_sequence"]
                .split(',')
                .collect::<Vec<_>>();
            if digests.len() != sustained_frames as usize {
                return Err(format!(
                    "{FORMAT}: `{name}` sustained frame digest count differs"
                ));
            }
            for digest in digests {
                evidence::require_sha256("sustained frame digest", digest)?;
            }
            let vsync = evidence::parse_u64_list(
                "sustained_vsync_sequence",
                &fields["sustained_vsync_sequence"],
            )?;
            if vsync.len() != sustained_frames as usize
                || vsync
                    .iter()
                    .enumerate()
                    .any(|(index, value)| *value != index as u64)
                || evidence::canonical_u64("sustained_refresh_hz", &fields["sustained_refresh_hz"])?
                    == 0
            {
                return Err(format!(
                    "{FORMAT}: `{name}` sustained vblank/cadence evidence differs"
                ));
            }
        } else if sustained_duration != 0
            || sustained_frames != 0
            || sustained_launches != 0
            || !fields["sustained_frame_digest_sequence"].is_empty()
            || !fields["sustained_record_sha256"].is_empty()
            || fields["sustained_refresh_hz"] != "0"
            || !fields["sustained_stdout_sha256"].is_empty()
            || !fields["sustained_vsync_sequence"].is_empty()
        {
            return Err(format!(
                "{FORMAT}: `{name}` has unexpected sustained evidence"
            ));
        }
        for key in ["temperature_after_millic", "temperature_before_millic"] {
            evidence::canonical_u64(key, &fields[key])?;
        }
        for key in ["throttle_after", "throttle_before"] {
            if fields[key] != "throttled=0x0" {
                return Err(format!("{FORMAT}: `{name}` was measured while throttled"));
            }
        }
        let error = evidence::canonical_u64(
            "measurement_error_cycles",
            &fields["measurement_error_cycles"],
        )?;
        let (class_error, envelope) = class_limits[fields["workload_class"].as_str()];
        if error != class_error {
            return Err(format!(
                "{FORMAT}: `{name}` measurement error differs from its sealed class value"
            ));
        }
        let (conservative, ratio) = per_core_bounds(&predicted, &samples, error);
        let verdict = if conservative { "pass" } else { "violation" };
        if fields["conservatism_verdict"] != verdict {
            return Err(format!(
                "{FORMAT}: `{name}` conservatism verdict is not derivable"
            ));
        }
        if !conservative {
            violations += 1;
            if fields["corpus_set"] == "calibration" {
                calibration_ok = false
            } else {
                holdout_ok = false
            }
        }
        if evidence::canonical_u64(
            "overprediction_ratio_milli",
            &fields["overprediction_ratio_milli"],
        )? != ratio
        {
            return Err(format!(
                "{FORMAT}: `{name}` overprediction ratio is not derivable"
            ));
        }
        max_ratio = max_ratio.max(ratio);
        let inside_envelope = ratio <= envelope;
        if fields["overprediction_envelope_verdict"]
            != if inside_envelope { "pass" } else { "breach" }
        {
            return Err(format!(
                "{FORMAT}: `{name}` overprediction envelope verdict is not derivable"
            ));
        }
        envelope_violations += u64::from(!inside_envelope);
        order_values.insert(
            name.to_string(),
            (*predicted.iter().max().unwrap(), derived[1]),
        );
    }

    let mut discordant = 0_u64;
    let mut prior_pair: Option<&str> = None;
    for fields in pairs.values() {
        require_keys("proxy validation pair", fields, PAIR_FIELDS)?;
        let name = fields["pair"].as_str();
        if prior_pair.is_some_and(|old| old >= name) {
            return Err(format!("{FORMAT}: pair rows are not strictly sorted"));
        }
        prior_pair = Some(name);
        let first = order_values
            .get(&fields["first_case"])
            .ok_or_else(|| format!("{FORMAT}: pair `{name}` names an unknown first case"))?;
        let second = order_values
            .get(&fields["second_case"])
            .ok_or_else(|| format!("{FORMAT}: pair `{name}` names an unknown second case"))?;
        if fields["first_case"] == fields["second_case"] {
            return Err(format!("{FORMAT}: pair `{name}` compares a case to itself"));
        }
        let noise = evidence::canonical_u64("pair noise_cycles", &fields["noise_cycles"])?;
        let predicted_order = order(first.0, second.0, 0);
        let measured_order = order(first.1, second.1, noise);
        if fields["predicted_order"] != predicted_order
            || fields["measured_order"] != measured_order
        {
            return Err(format!("{FORMAT}: pair `{name}` ordering is not derivable"));
        }
        let derived = predicted_order != measured_order && measured_order != "noise-tie";
        if fields["discordant"] != if derived { "yes" } else { "no" } {
            return Err(format!(
                "{FORMAT}: pair `{name}` discordance is not derivable"
            ));
        }
        discordant += u64::from(derived);
    }
    let rate = if pairs.is_empty() {
        0
    } else {
        discordant * 1000 / pairs.len() as u64
    };
    let summary = ValidationSummary {
        cases: cases.len(),
        pairs: pairs.len(),
        violations,
        max_ratio_milli: max_ratio,
        discordance_rate_milli: rate,
        envelope_violations,
    };
    for (key, want) in [
        ("conservatism_violations", violations),
        ("max_overprediction_ratio_milli", max_ratio),
        ("discordance_rate_milli", rate),
    ] {
        if evidence::canonical_u64(key, &globals[key])? != want {
            return Err(format!("{FORMAT}: summary `{key}` is not derivable"));
        }
    }
    let calibration = if calibration_ok { "pass" } else { "fail" };
    let holdout = if holdout_ok { "pass" } else { "fail" };
    let verdict = if calibration_ok && holdout_ok && discordant == 0 && envelope_violations == 0 {
        "pass"
    } else {
        "fail"
    };
    for (key, want) in [
        ("calibration_verdict", calibration),
        ("holdout_verdict", holdout),
        ("verdict", verdict),
    ] {
        if globals[key] != want {
            return Err(format!("{FORMAT}: `{key}` is not derivable"));
        }
    }
    Ok((record, summary))
}

pub(crate) fn verify_drift_lock() -> Result<(), String> {
    let root = crate::root();
    let result_path = root.join("bench/results/rasputin-proxy-validation-v1.txt");
    let text = std::fs::read_to_string(&result_path)
        .map_err(|error| format!("proxy drift lock: read {}: {error}", result_path.display()))?;
    let (record, summary) = parse_and_validate(&text)?;
    if record.fields["verdict"] != "pass" || summary.violations != 0 {
        return Err("proxy drift lock: active validation report is not passing".into());
    }
    if summary.envelope_violations != 0 {
        return Err("proxy drift lock: active report breaches a sealed class envelope".into());
    }
    let bindings = [
        ("cost_profile_sha256", root.join("bench/a76-pi5.toml")),
        (
            "corpus_manifest_sha256",
            root.join("bench/proxy-calibration-v1.txt"),
        ),
        (
            "holdout_manifest_sha256",
            root.join("bench/proxy-holdout-v1.txt"),
        ),
    ];
    for (field, path) in bindings {
        let bytes = std::fs::read(&path)
            .map_err(|error| format!("proxy drift lock: read {}: {error}", path.display()))?;
        let got = wrela_machine::sha256::sha256_hex(&bytes);
        if record.fields[field] != got {
            return Err(format!(
                "proxy drift lock: `{field}` is stale for {} (record {}, current {got})",
                path.display(),
                record.fields[field]
            ));
        }
    }
    let envelope_path = root.join("bench/proxy-envelopes-v1.txt");
    let envelope_text = std::fs::read_to_string(&envelope_path).map_err(|error| {
        format!(
            "proxy drift lock: read {}: {error}",
            envelope_path.display()
        )
    })?;
    let (envelope_record, envelopes) = parse_envelopes(&envelope_text)?;
    let envelope_digest = wrela_machine::sha256::sha256_hex(envelope_text.as_bytes());
    if record.fields["envelopes_sha256"] != envelope_digest {
        return Err(
            "proxy drift lock: validation report does not bind the active envelopes".into(),
        );
    }
    for envelope in &envelopes {
        for (suffix, want) in [
            (
                "measurement_error_cycles",
                envelope.measurement_error_cycles,
            ),
            (
                "overprediction_envelope_milli",
                envelope.overprediction_envelope_milli,
            ),
        ] {
            let field = format!("{}_{}", envelope.workload_class, suffix);
            if evidence::canonical_u64(&field, &record.fields[&field])? != want {
                return Err(format!(
                    "proxy drift lock: report `{field}` differs from envelopes"
                ));
            }
        }
    }
    for (report_field, envelope_field) in [
        ("corpus_manifest_sha256", "calibration_manifest_sha256"),
        ("cost_profile_sha256", "cost_profile_sha256"),
        ("host_identity_sha256", "host_identity_sha256"),
        ("host_profile_sha256", "host_profile_sha256"),
        ("lab_agent_sha256", "lab_agent_sha256"),
        ("proxy_rules_sha256", "proxy_rules_sha256"),
        ("vmm_binary_sha256", "vmm_binary_sha256"),
    ] {
        if record.fields[report_field] != envelope_record.fields[envelope_field] {
            return Err(format!(
                "proxy drift lock: report `{report_field}` differs from calibration envelope provenance"
            ));
        }
    }
    for (field, path) in [
        (
            "host_identity_sha256",
            root.join("bench/results/rasputin-host-identity-v1.txt"),
        ),
        (
            "host_profile_sha256",
            root.join("bench/results/rasputin-host-profile-v1.txt"),
        ),
        (
            "run_environment_sha256",
            root.join("bench/results/rasputin-run-environment-v1.txt"),
        ),
    ] {
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("proxy drift lock: read {}: {error}", path.display()))?;
        let format = match field {
            "host_identity_sha256" => evidence::HOST_IDENTITY,
            "host_profile_sha256" => evidence::HOST_PROFILE,
            "run_environment_sha256" => evidence::RUN_ENVIRONMENT,
            _ => unreachable!(),
        };
        let typed = evidence::parse(&text, format)?;
        evidence::validate_typed(&typed)?;
        if typed.fields["acceptance_verdict"] != "conforming"
            || typed.digest_hex()? != record.fields[field]
        {
            return Err(format!(
                "proxy drift lock: `{field}` is not the checked conforming record"
            ));
        }
    }
    let rules = proxy_rules_digest(&root)?;
    if record.fields["proxy_rules_sha256"] != rules {
        return Err(
            "proxy drift lock: proxy source rules changed without fresh hardware evidence".into(),
        );
    }
    let current_source = crate::pi::vmm_source_digest()?;
    if record.fields["vmm_source_sha256"] != current_source {
        return Err("proxy drift lock: VMM source changed without fresh hardware evidence".into());
    }
    let vmm = root.join("target/aarch64-unknown-linux-musl/release/wrela-vmm");
    let current_binary = std::fs::read(&vmm)
        .map(|bytes| wrela_machine::sha256::sha256_hex(&bytes))
        .map_err(|error| format!("proxy drift lock: read {}: {error}", vmm.display()))?;
    if record.fields["vmm_binary_sha256"] != current_binary {
        return Err(
            "proxy drift lock: current Linux VMM binary differs from physical evidence".into(),
        );
    }
    let command_identity = |program: &str, args: &[&str]| -> Result<String, String> {
        let output = std::process::Command::new(program)
            .args(args)
            .output()
            .map_err(|error| format!("proxy drift lock: run {program}: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "proxy drift lock: {program} identity command failed"
            ));
        }
        let text = String::from_utf8(output.stdout)
            .map_err(|_| format!("proxy drift lock: {program} identity is not UTF-8"))?;
        Ok(text.trim_end().to_string())
    };
    let rustc = wrela_machine::sha256::sha256_hex(command_identity("rustc", &["-Vv"])?.as_bytes());
    if record.fields["rustc_identity_sha256"] != rustc {
        return Err(
            "proxy drift lock: Rust toolchain changed without fresh hardware evidence".into(),
        );
    }
    let mut linker = std::fs::read(root.join("tools/zigcc-aarch64-linux-musl"))
        .map_err(|error| format!("proxy drift lock: read linker wrapper: {error}"))?;
    linker.extend_from_slice(command_identity("zig", &["version"])?.as_bytes());
    let linker = wrela_machine::sha256::sha256_hex(&linker);
    if record.fields["linker_identity_sha256"] != linker {
        return Err(
            "proxy drift lock: linker toolchain changed without fresh hardware evidence".into(),
        );
    }
    validate_manifest(&root.join("bench/proxy-calibration-v1.txt"), "calibration")?;
    validate_manifest(&root.join("bench/proxy-holdout-v1.txt"), "holdout")?;
    let calibration = read_manifest(&root.join("bench/proxy-calibration-v1.txt"), "calibration")?;
    let holdout = read_manifest(&root.join("bench/proxy-holdout-v1.txt"), "holdout")?;
    validate_candidate_pairs(&calibration, &holdout)?;
    let expected_cases = calibration
        .iter()
        .chain(&holdout)
        .map(|case| (case.case.clone(), case.set.clone()))
        .collect::<BTreeMap<_, _>>();
    let actual_cases = record
        .fields
        .iter()
        .filter_map(|(key, value)| key.ends_with(".case").then_some(value.clone()))
        .zip(
            record
                .fields
                .iter()
                .filter_map(|(key, value)| key.ends_with(".corpus_set").then_some(value.clone())),
        )
        .collect::<BTreeMap<_, _>>();
    if actual_cases != expected_cases {
        return Err("proxy drift lock: report cases do not exactly cover both manifests".into());
    }
    let expected_pairs = ["frame", "kernel", "sequence"]
        .into_iter()
        .map(|class| {
            let first = calibration
                .iter()
                .find(|case| case.workload_class == class)
                .expect("validated calibration class");
            let second = holdout
                .iter()
                .find(|case| case.workload_class == class)
                .expect("validated holdout class");
            (
                format!("{class}:{}", first.selection_decision),
                first.case.clone(),
                second.case.clone(),
            )
        })
        .collect::<BTreeSet<_>>();
    let mut actual_pairs = BTreeSet::new();
    for index in 0..3 {
        let prefix = format!("pair.{index:04}");
        let tuple = (
            record
                .fields
                .get(&format!("{prefix}.pair"))
                .ok_or("proxy drift lock: report lacks a rank-fidelity pair")?
                .clone(),
            record
                .fields
                .get(&format!("{prefix}.first_case"))
                .ok_or("proxy drift lock: report lacks a rank-fidelity first candidate")?
                .clone(),
            record
                .fields
                .get(&format!("{prefix}.second_case"))
                .ok_or("proxy drift lock: report lacks a rank-fidelity second candidate")?
                .clone(),
        );
        actual_pairs.insert(tuple);
    }
    if actual_pairs != expected_pairs
        || record
            .fields
            .keys()
            .any(|key| key.starts_with("pair.0003."))
    {
        return Err(
            "proxy drift lock: rank-fidelity pairs do not match the sealed compiler decisions"
                .into(),
        );
    }
    Ok(())
}

pub(crate) fn proxy_rules_digest(root: &Path) -> Result<String, String> {
    let mut bytes = Vec::new();
    let mut files = Vec::new();
    for directory in [
        root.join("crates/wrela-compiler/src"),
        root.join("crates/wrela-machine/src"),
        root.join("stdlib"),
    ] {
        collect_proxy_sources(&directory, &mut files)?;
    }
    for manifest in [
        root.join("bench/proxy-calibration-v1.txt"),
        root.join("bench/proxy-holdout-v1.txt"),
    ] {
        let set = if manifest
            .file_name()
            .is_some_and(|name| name == "proxy-calibration-v1.txt")
        {
            "calibration"
        } else {
            "holdout"
        };
        for case in read_manifest(&manifest, set)? {
            collect_proxy_sources(&proxy_fixture_dir(root, &case.case)?, &mut files)?;
        }
    }
    files.extend([
        root.join("Cargo.lock"),
        root.join("crates/xtask/src/evidence.rs"),
        root.join("crates/xtask/src/lane2_freq.rs"),
        root.join("crates/xtask/src/pi.rs"),
        root.join("crates/xtask/src/proxy_validation.rs"),
    ]);
    files.sort();
    files.dedup();
    for path in files {
        bytes.extend_from_slice(
            path.strip_prefix(root)
                .unwrap()
                .to_string_lossy()
                .as_bytes(),
        );
        bytes.push(0);
        bytes.extend_from_slice(
            &std::fs::read(&path)
                .map_err(|error| format!("proxy rules: read {}: {error}", path.display()))?,
        );
        bytes.push(0xff);
    }
    Ok(wrela_machine::sha256::sha256_hex(&bytes))
}

pub(crate) fn proxy_fixture_dir(root: &Path, case: &str) -> Result<PathBuf, String> {
    if case.is_empty()
        || case.starts_with('-')
        || case.len() > 128
        || !case
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
    {
        return Err(format!("proxy corpus: unsafe case name `{case}`"));
    }
    let dedicated = root.join("bench/proxy-fixtures").join(case);
    if dedicated.is_dir() {
        Ok(dedicated)
    } else {
        Ok(root.join("tests/golden").join(case))
    }
}

fn collect_proxy_sources(directory: &Path, out: &mut Vec<PathBuf>) -> Result<(), String> {
    let mut entries = std::fs::read_dir(directory)
        .map_err(|error| format!("proxy rules: read {}: {error}", directory.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("proxy rules: enumerate {}: {error}", directory.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let ty = entry
            .file_type()
            .map_err(|error| format!("proxy rules: stat {}: {error}", entry.path().display()))?;
        if ty.is_symlink() {
            return Err(format!(
                "proxy rules: source tree contains symlink {}",
                entry.path().display()
            ));
        }
        if ty.is_dir() {
            if entry.file_name() != "expected" {
                collect_proxy_sources(&entry.path(), out)?;
            }
        } else if ty.is_file()
            && entry
                .path()
                .extension()
                .is_some_and(|ext| matches!(ext.to_str(), Some("rs" | "wr" | "toml" | "txt")))
        {
            out.push(entry.path());
        }
    }
    Ok(())
}

fn validate_manifest(path: &Path, set: &str) -> Result<(), String> {
    let _ = read_manifest(path, set)?;
    Ok(())
}

fn indexed(key: &str, prefix: &str) -> Result<Option<(usize, String)>, String> {
    indexed_for(key, prefix, FORMAT)
}

fn indexed_for(key: &str, prefix: &str, format: &str) -> Result<Option<(usize, String)>, String> {
    let Some(rest) = key
        .strip_prefix(prefix)
        .and_then(|rest| rest.strip_prefix('.'))
    else {
        return Ok(None);
    };
    let (raw_index, field) = rest
        .split_once('.')
        .ok_or_else(|| format!("{format}: malformed indexed field `{key}`"))?;
    if raw_index.len() != 4 || !raw_index.bytes().all(|byte| byte.is_ascii_digit()) {
        return Err(format!(
            "{format}: indexed field `{key}` needs a four-digit index"
        ));
    }
    Ok(Some((raw_index.parse().unwrap(), field.to_string())))
}

fn require_keys(
    kind: &str,
    fields: &BTreeMap<String, String>,
    required: &[&str],
) -> Result<(), String> {
    let got = fields.keys().map(String::as_str).collect::<BTreeSet<_>>();
    let want = required.iter().copied().collect::<BTreeSet<_>>();
    if got != want {
        return Err(format!(
            "{kind}: exact field set mismatch: got {got:?}, want {want:?}"
        ));
    }
    Ok(())
}

fn contiguous(kind: &str, rows: &BTreeMap<usize, BTreeMap<String, String>>) -> Result<(), String> {
    contiguous_for(FORMAT, kind, rows)
}

fn contiguous_for(
    format: &str,
    kind: &str,
    rows: &BTreeMap<usize, BTreeMap<String, String>>,
) -> Result<(), String> {
    if rows.keys().copied().eq(0..rows.len()) {
        Ok(())
    } else {
        Err(format!(
            "{format}: {kind} row indexes are not contiguous from zero"
        ))
    }
}

fn parse_vectors(raw: &str) -> Result<Vec<Vec<u64>>, String> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    raw.split(';')
        .map(|row| evidence::parse_u64_list("samples_cycles_per_vcpu", row))
        .collect()
}

fn parse_optional_vector(raw: &str) -> Result<Vec<u64>, String> {
    if raw.is_empty() {
        Ok(Vec::new())
    } else {
        evidence::parse_u64_list("optional sample vector", raw)
    }
}

fn parse_counters(
    raw: &str,
    samples: usize,
    vcpus: usize,
) -> Result<BTreeMap<String, Vec<Vec<u64>>>, String> {
    let mut keys = Vec::new();
    let mut parsed = BTreeMap::new();
    for row in raw.split('/') {
        let (key, value) = row
            .split_once(':')
            .ok_or_else(|| format!("counter_rows: malformed `{row}`"))?;
        let vectors = parse_vectors(value)?;
        if vectors.len() != samples || vectors.iter().any(|row| row.len() != vcpus) {
            return Err(format!(
                "counter_rows: `{key}` needs {samples} sample(s) of {vcpus} vCPU value(s)"
            ));
        }
        keys.push(key);
        parsed.insert(key.to_string(), vectors);
    }
    if keys != REQUIRED_COUNTERS {
        return Err(format!(
            "counter_rows: required rows are exactly {REQUIRED_COUNTERS:?}"
        ));
    }
    Ok(parsed)
}

pub(crate) fn validate_counter_attribution(
    raw: &str,
    samples: usize,
    vcpus: usize,
    modeled_branch_paths: u64,
    modeled_memory_accesses: u64,
    modeled_memory_transitions: u64,
) -> Result<(), String> {
    if modeled_branch_paths == 0
        || modeled_memory_accesses == 0
        || modeled_memory_transitions == 0
        || modeled_memory_transitions > modeled_memory_accesses
    {
        return Err(
            "counter attribution: modeled branch and memory-transition counts are invalid".into(),
        );
    }
    let counters = parse_counters(raw, samples, vcpus)?;
    let branches = &counters["br_mis_pred"];
    let retired = &counters["inst_retired"];
    let l1 = &counters["l1d_cache_refill"];
    let l2 = &counters["l2d_cache_refill"];
    for sample in 0..samples {
        for vcpu in 0..vcpus {
            let mispredicts = branches[sample][vcpu];
            if mispredicts > modeled_branch_paths || mispredicts > retired[sample][vcpu] {
                return Err(format!(
                    "counter attribution: sample {sample} vCPU {vcpu} has {mispredicts} branch mispredicts beyond modeled paths or retired instructions"
                ));
            }
            let refills = l1[sample][vcpu].saturating_add(l2[sample][vcpu]);
            if refills > modeled_memory_transitions.saturating_mul(2) {
                return Err(format!(
                    "counter attribution: sample {sample} vCPU {vcpu} has {refills} cache refills beyond the two cache levels for {modeled_memory_transitions} modeled memory-class transitions"
                ));
            }
        }
    }
    Ok(())
}

fn is_utc_second(raw: &str) -> bool {
    raw.len() == 20
        && raw.as_bytes()[4] == b'-'
        && raw.as_bytes()[7] == b'-'
        && raw.as_bytes()[10] == b'T'
        && raw.as_bytes()[13] == b':'
        && raw.as_bytes()[16] == b':'
        && raw.as_bytes()[19] == b'Z'
        && raw.bytes().enumerate().all(|(index, byte)| {
            matches!(index, 4 | 7 | 10 | 13 | 16 | 19) || byte.is_ascii_digit()
        })
}

fn order(first: u64, second: u64, noise: u64) -> &'static str {
    if first.abs_diff(second) <= noise {
        "noise-tie"
    } else if first < second {
        "first"
    } else {
        "second"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write_test_file(root: &Path, relative: &str, bytes: &[u8]) {
        let path = root.join(relative);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn v1_class_envelopes_are_sealed_and_unknown_classes_fail_closed() {
        assert_eq!(sealed_overprediction_envelope_milli("frame"), Some(3090));
        assert_eq!(sealed_overprediction_envelope_milli("kernel"), Some(2779));
        assert_eq!(sealed_overprediction_envelope_milli("sequence"), Some(3110));
        assert_eq!(sealed_overprediction_envelope_milli("unknown"), None);
    }

    fn synthetic(measured: u64, predicted: u64, declared: &str) -> String {
        let d = "a".repeat(64);
        let mut r = Record::new(FORMAT).unwrap();
        for (key, value) in [
            ("build_features", "native-presentation"),
            ("build_profile", "release"),
            ("build_target", "aarch64-unknown-linux-musl"),
            ("calibration_verdict", declared),
            (
                "conservatism_violations",
                if declared == "pass" { "0" } else { "1" },
            ),
            (
                "counter_config",
                "armv8_pmuv3/exclude_host=1/exclude_guest=0;grouped;not-multiplexed",
            ),
            ("discordance_rate_milli", "0"),
            ("envelope_provenance", "repeated-conforming-calibration-v1"),
            ("frame_measurement_error_cycles", "0"),
            ("frame_overprediction_envelope_milli", "3000"),
            ("holdout_verdict", "pass"),
            ("kernel_measurement_error_cycles", "0"),
            ("kernel_overprediction_envelope_milli", "3000"),
            (
                "max_overprediction_ratio_milli",
                if predicted >= measured { "2000" } else { "500" },
            ),
            ("measurement_error_model", "per-class-per-core-max-range-v2"),
            ("operator", "test-operator"),
            ("proxy_revision", PROXY_REVISION),
            ("retrieval_method", "content-addressed-sftp-v1"),
            ("retrieved_at_utc", "2026-08-16T00:00:00Z"),
            ("sequence_measurement_error_cycles", "0"),
            ("sequence_overprediction_envelope_milli", "3000"),
            ("verdict", declared),
        ] {
            r.insert(key, value).unwrap();
        }
        for key in [
            "corpus_manifest_sha256",
            "cost_profile_sha256",
            "cargo_lock_sha256",
            "envelopes_sha256",
            "holdout_manifest_sha256",
            "host_identity_sha256",
            "host_profile_sha256",
            "lab_agent_sha256",
            "linker_identity_sha256",
            "proxy_rules_sha256",
            "run_environment_sha256",
            "rustc_identity_sha256",
            "vmm_binary_sha256",
            "vmm_source_sha256",
        ] {
            r.insert(key, d.clone()).unwrap();
        }
        for (field, value) in [
            ("branch_attribution_verdict", "pass".into()),
            ("cache_state", "warm".into()), ("case", "kernel-real".into()),
            ("conservatism_verdict", if predicted >= measured { "pass".into() } else { "violation".into() }),
            ("corpus_set", "calibration".into()),
            ("counter_rows", "br_mis_pred:1/cpu_cycles:2/inst_retired:3/l1d_cache_refill:4/l2d_cache_refill:5/stall_backend:6/stall_frontend:7".into()),
            ("frame_count", "0".into()), ("frame_digest_sequence", "".into()),
            ("image_sha256", d.clone()), ("measured_max_cycles", measured.to_string()),
            ("measured_median_cycles", measured.to_string()), ("measured_min_cycles", measured.to_string()),
            ("measurement_error_cycles", "0".into()),
            ("memory_attribution_verdict", "pass".into()),
            ("modeled_branch_paths", "10".into()),
            ("modeled_memory_accesses", "10".into()),
            ("modeled_memory_transitions", "10".into()),
            ("overprediction_envelope_verdict", "pass".into()),
            ("overprediction_ratio_milli", (predicted * 1000 / measured).to_string()),
            ("presentation_mode", "headless".into()),
            ("predicted_cycles_per_core", predicted.to_string()), ("sample_count", "1".into()),
            ("record_digests", d.clone()), ("report_sha256", d.clone()),
            ("run_environment_after_sha256", d.clone()), ("run_environment_before_sha256", d.clone()),
            ("samples_cycles_per_vcpu", measured.to_string()), ("stage1_tables_sha256", d.clone()),
            ("samples_frame_cadence_ns", "".into()), ("stdout_sha256", d.clone()),
            ("sustained_duration_ns", "0".into()), ("sustained_frame_count", "0".into()),
            ("sustained_frame_digest_sequence", "".into()), ("sustained_launch_count", "0".into()),
            ("sustained_record_sha256", "".into()), ("sustained_refresh_hz", "0".into()),
            ("sustained_stdout_sha256", "".into()), ("sustained_vsync_sequence", "".into()),
            ("temperature_after_millic", "40000".into()), ("temperature_before_millic", "39000".into()),
            ("throttle_after", "throttled=0x0".into()), ("throttle_before", "throttled=0x0".into()),
            ("workload_class", "kernel".into()), ("workload_sha256", d.clone()),
        ] { r.insert(&format!("case.0000.{field}"), value).unwrap(); }
        r.encode().unwrap()
    }

    #[test]
    fn summary_and_conservatism_are_derived() {
        parse_and_validate(&synthetic(100, 200, "pass")).unwrap();
        parse_and_validate(&synthetic(200, 100, "fail")).unwrap();
        let bad = synthetic(100, 200, "pass")
            .replace("conservatism_violations=0", "conservatism_violations=1");
        assert!(parse_and_validate(&bad).is_err());
    }

    #[test]
    fn proxy_bounds_compare_corresponding_cores() {
        let samples = vec![vec![900, 200], vec![900, 200], vec![900, 200]];
        assert_eq!(per_core_bounds(&[1_000, 100], &samples, 0), (false, 1_111));
        assert_eq!(per_core_bounds(&[1_000, 600], &samples, 0), (true, 3_000));
    }

    #[test]
    fn required_counter_set_and_canonical_indexes_are_closed() {
        let missing = synthetic(100, 200, "pass").replace("/stall_frontend:7", "");
        assert!(parse_and_validate(&missing).is_err());
        let bad_index = synthetic(100, 200, "pass").replace("case.0000.", "case.0.");
        assert!(parse_and_validate(&bad_index).is_err());
        let branch_contradiction =
            synthetic(100, 200, "pass").replace("br_mis_pred:1", "br_mis_pred:11");
        assert!(parse_and_validate(&branch_contradiction).is_err());
        let memory_contradiction =
            synthetic(100, 200, "pass").replace("l2d_cache_refill:5", "l2d_cache_refill:25");
        assert!(parse_and_validate(&memory_contradiction).is_err());
    }

    #[test]
    fn corpus_manifests_are_exact_and_candidate_pairs_share_a_real_decision() {
        let calibration = "calibration|frame|frame-a|real|frame-choice\ncalibration|kernel|kernel-a|real|kernel-choice\ncalibration|sequence|sequence-a|real|sequence-choice\n";
        let holdout = "holdout|frame|frame-b|real|frame-choice\nholdout|kernel|kernel-b|real|kernel-choice\nholdout|sequence|sequence-b|real|sequence-choice\n";
        let first = parse_manifest(calibration, Path::new("calibration"), "calibration").unwrap();
        let second = parse_manifest(holdout, Path::new("holdout"), "holdout").unwrap();
        validate_candidate_pairs(&first, &second).unwrap();
        assert!(
            parse_manifest(
                &(calibration.to_string()
                    + "calibration|sequence|sequence-extra|real|sequence-choice\n"),
                Path::new("calibration"),
                "calibration"
            )
            .is_err()
        );
        let wrong = holdout.replace("kernel-choice", "unrelated-choice");
        let wrong = parse_manifest(&wrong, Path::new("holdout"), "holdout").unwrap();
        assert!(validate_candidate_pairs(&first, &wrong).is_err());
        assert!(
            parse_manifest(
                calibration.trim_end(),
                Path::new("calibration"),
                "calibration"
            )
            .is_err()
        );
    }

    #[test]
    fn proxy_rules_bind_compiler_runtime_and_selected_fixture_sources() {
        let root =
            std::env::temp_dir().join(format!("wrela-proxy-rules-{:016x}", std::process::id()));
        std::fs::create_dir_all(&root).unwrap();
        let calibration = "calibration|frame|frame-a|real|frame-choice\ncalibration|kernel|kernel-a|real|kernel-choice\ncalibration|sequence|sequence-a|real|sequence-choice\n";
        let holdout = "holdout|frame|frame-b|real|frame-choice\nholdout|kernel|kernel-b|real|kernel-choice\nholdout|sequence|sequence-b|real|sequence-choice\n";
        write_test_file(
            &root,
            "bench/proxy-calibration-v1.txt",
            calibration.as_bytes(),
        );
        write_test_file(&root, "bench/proxy-holdout-v1.txt", holdout.as_bytes());
        for path in [
            "Cargo.lock",
            "crates/xtask/src/evidence.rs",
            "crates/xtask/src/lane2_freq.rs",
            "crates/xtask/src/pi.rs",
            "crates/xtask/src/proxy_validation.rs",
            "crates/wrela-machine/src/lib.rs",
        ] {
            write_test_file(&root, path, b"pinned\n");
        }
        write_test_file(
            &root,
            "crates/wrela-compiler/src/layout/stage1.rs",
            b"stage1-a\n",
        );
        write_test_file(&root, "stdlib/core/runtime.wr", b"runtime-a\n");
        for case in [
            "frame-a",
            "kernel-a",
            "sequence-a",
            "frame-b",
            "kernel-b",
            "sequence-b",
        ] {
            write_test_file(
                &root,
                &format!("tests/golden/{case}/input.wr"),
                format!("{case}\n").as_bytes(),
            );
            write_test_file(
                &root,
                &format!("tests/golden/{case}/expected/report.txt"),
                b"not-bound\n",
            );
        }
        write_test_file(
            &root,
            "bench/proxy-fixtures/sequence-a/input.wr",
            b"dedicated-sequence-a\n",
        );

        let base = proxy_rules_digest(&root).unwrap();
        write_test_file(
            &root,
            "crates/wrela-compiler/src/layout/stage1.rs",
            b"stage1-b\n",
        );
        let compiler = proxy_rules_digest(&root).unwrap();
        assert_ne!(base, compiler);
        write_test_file(&root, "stdlib/core/runtime.wr", b"runtime-b\n");
        let runtime = proxy_rules_digest(&root).unwrap();
        assert_ne!(compiler, runtime);
        write_test_file(&root, "tests/golden/kernel-a/input.wr", b"fixture-b\n");
        let fixture = proxy_rules_digest(&root).unwrap();
        assert_ne!(runtime, fixture);
        write_test_file(
            &root,
            "tests/golden/sequence-a/input.wr",
            b"shadowed-golden-change\n",
        );
        assert_eq!(fixture, proxy_rules_digest(&root).unwrap());
        write_test_file(
            &root,
            "bench/proxy-fixtures/sequence-a/input.wr",
            b"dedicated-sequence-b\n",
        );
        let dedicated = proxy_rules_digest(&root).unwrap();
        assert_ne!(fixture, dedicated);
        write_test_file(
            &root,
            "tests/golden/kernel-a/expected/report.txt",
            b"still-not-bound\n",
        );
        assert_eq!(dedicated, proxy_rules_digest(&root).unwrap());

        assert!(proxy_fixture_dir(&root, "../escape").is_err());

        std::fs::remove_dir_all(root).unwrap();
    }
}
