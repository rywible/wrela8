//! Canonical, dependency-free evidence records used by the host lab lane.

use std::collections::{BTreeMap, BTreeSet};

pub(crate) const MAX_RECORD_BYTES: usize = 1 << 20;
pub(crate) const MAX_FIELDS: usize = 4096;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Record {
    pub format: String,
    pub fields: BTreeMap<String, String>,
}

impl Record {
    pub(crate) fn new(format: &str) -> Result<Self, String> {
        validate_atom("format", format)?;
        Ok(Self {
            format: format.to_string(),
            fields: BTreeMap::new(),
        })
    }

    pub(crate) fn insert(&mut self, key: &str, value: impl Into<String>) -> Result<(), String> {
        validate_atom("field name", key)?;
        if key == "format" {
            return Err("evidence record: `format` is reserved for the first line".into());
        }
        if self.fields.len() >= MAX_FIELDS {
            return Err(format!(
                "evidence record: more than {MAX_FIELDS} fields are not accepted"
            ));
        }
        if self.fields.insert(key.to_string(), value.into()).is_some() {
            return Err(format!("evidence record: repeated field `{key}`"));
        }
        Ok(())
    }

    pub(crate) fn require_exact_fields(&self, allowed: &[&str]) -> Result<(), String> {
        let allowed = allowed.iter().copied().collect::<BTreeSet<_>>();
        for key in self.fields.keys() {
            if !allowed.contains(key.as_str()) {
                return Err(format!(
                    "{}: unknown field `{key}` is not accepted",
                    self.format
                ));
            }
        }
        for key in allowed {
            if !self.fields.contains_key(key) {
                return Err(format!("{}: missing required field `{key}`", self.format));
            }
        }
        Ok(())
    }

    pub(crate) fn encode(&self) -> Result<String, String> {
        let mut out = format!("format={}\n", self.format);
        for (key, value) in &self.fields {
            out.push_str(key);
            out.push('=');
            percent_encode_into(value.as_bytes(), &mut out);
            out.push('\n');
        }
        if out.len() > MAX_RECORD_BYTES {
            return Err(format!(
                "{}: encoded record is {} bytes, over the {} byte ceiling",
                self.format,
                out.len(),
                MAX_RECORD_BYTES
            ));
        }
        Ok(out)
    }

    pub(crate) fn digest_hex(&self) -> Result<String, String> {
        Ok(wrela_machine::sha256::sha256_hex(self.encode()?.as_bytes()))
    }
}

pub(crate) fn parse(text: &str, expected_format: &str) -> Result<Record, String> {
    if text.len() > MAX_RECORD_BYTES {
        return Err(format!(
            "evidence record: {} bytes exceeds the {} byte ceiling",
            text.len(),
            MAX_RECORD_BYTES
        ));
    }
    if !text.is_ascii() {
        return Err(
            "evidence record: raw non-ASCII bytes are forbidden; use UTF-8 percent encoding".into(),
        );
    }
    if !text.ends_with('\n') {
        return Err("evidence record: canonical records end with one LF".into());
    }
    if text.contains('\r') || text.contains('\0') {
        return Err("evidence record: CR and NUL are forbidden".into());
    }
    let mut lines = text.lines();
    let first = lines
        .next()
        .ok_or_else(|| "evidence record: missing format line".to_string())?;
    let format = first
        .strip_prefix("format=")
        .ok_or_else(|| "evidence record: first line must be `format=<version>`".to_string())?;
    validate_atom("format", format)?;
    if format != expected_format {
        return Err(format!(
            "evidence record: unknown format `{format}` (expected `{expected_format}`)"
        ));
    }
    let mut record = Record::new(format)?;
    let mut previous: Option<&str> = None;
    for line in lines {
        if line.is_empty() {
            return Err("evidence record: blank lines are not canonical".into());
        }
        let (key, encoded) = line
            .split_once('=')
            .ok_or_else(|| format!("{format}: line `{line}` has no `=`"))?;
        validate_atom("field name", key)?;
        if previous.is_some_and(|old| old >= key) {
            return Err(format!(
                "{format}: fields are not strictly sorted (`{}` before `{key}`)",
                previous.unwrap_or_default()
            ));
        }
        previous = Some(key);
        let bytes = percent_decode(encoded)?;
        let value = String::from_utf8(bytes)
            .map_err(|_| format!("{format}: field `{key}` is not valid UTF-8"))?;
        record.insert(key, value)?;
    }
    Ok(record)
}

pub(crate) fn canonical_u64(kind: &str, raw: &str) -> Result<u64, String> {
    if raw.is_empty()
        || (raw.len() > 1 && raw.starts_with('0'))
        || !raw.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err(format!(
            "{kind}: `{raw}` is not a canonical unsigned integer"
        ));
    }
    raw.parse::<u64>()
        .map_err(|error| format!("{kind}: `{raw}`: {error}"))
}

pub(crate) fn require_sha256(kind: &str, raw: &str) -> Result<(), String> {
    if raw.len() != 64
        || !raw
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(format!(
            "{kind}: expected exactly 64 lowercase hexadecimal digits"
        ));
    }
    Ok(())
}

pub(crate) const HOST_IDENTITY: &str = "wrela-host-identity-v1";
pub(crate) const HOST_PROFILE: &str = "wrela-host-profile-v1";
pub(crate) const RUN_ENVIRONMENT: &str = "wrela-run-environment-v1";
pub(crate) const BACKEND_CONFORMANCE: &str = "wrela-backend-conformance-v1";
pub(crate) const PI_BENCHMARK: &str = "wrela-pi-benchmark-v1";
pub(crate) const STAGE1_PAIR: &str = "wrela-stage1-pair-v1";

const IDENTITY_FIELDS: &[&str] = &[
    "acceptance_verdict",
    "architecture",
    "board_model",
    "cpu_model",
    "cpu_part",
    "cpu_revision",
    "device_tree_sha256",
    "drm_module",
    "eeprom_config_sha256",
    "eeprom_version",
    "kernel_config_sha256",
    "kernel_release",
    "host_page_size",
    "kvm_capabilities",
    "kvm_module",
    "module_set_sha256",
    "pmu_identity",
];

const PROFILE_FIELDS: &[&str] = &[
    "acceptance_verdict",
    "cpu_isolation",
    "frequency_policy",
    "governor",
    "hardening_config_sha256",
    "hardening_mode",
    "host_affinity",
    "host_protection_profile",
    "memory_reservation",
    "name",
    "provenance",
    "vcpu_affinity",
];

const ENVIRONMENT_FIELDS: &[&str] = &[
    "acceptance_verdict",
    "actual_frequencies_khz",
    "available_memory_bytes",
    "context_switches",
    "display_mode",
    "irq_census",
    "observed_at_utc",
    "online_cores",
    "temperature_millic",
    "throttle_flags",
];

/// Validate a typed evidence record after the shared canonical grammar has
/// accepted it. Schemas are exact: versioning, rather than ignored fields,
/// is the only extension mechanism.
pub(crate) fn validate_typed(record: &Record) -> Result<(), String> {
    match record.format.as_str() {
        HOST_IDENTITY => {
            record.require_exact_fields(IDENTITY_FIELDS)?;
            for key in [
                "device_tree_sha256",
                "eeprom_config_sha256",
                "kernel_config_sha256",
                "module_set_sha256",
            ] {
                require_sha256(key, &record.fields[key])?;
            }
            require_enum(record, "architecture", &["aarch64"])?;
            canonical_u64("host_page_size", &record.fields["host_page_size"])?;
            require_enum(record, "acceptance_verdict", &["conforming", "refused"])?;
            validate_capability_rows(&record.fields["kvm_capabilities"])?;
        }
        HOST_PROFILE => {
            record.require_exact_fields(PROFILE_FIELDS)?;
            require_sha256(
                "hardening_config_sha256",
                &record.fields["hardening_config_sha256"],
            )?;
            require_enum(
                record,
                "hardening_mode",
                &["diagnostic", "development", "product"],
            )?;
            require_enum(
                record,
                "acceptance_verdict",
                &["conforming", "nonconforming"],
            )?;
        }
        RUN_ENVIRONMENT => {
            record.require_exact_fields(ENVIRONMENT_FIELDS)?;
            for key in [
                "available_memory_bytes",
                "context_switches",
                "temperature_millic",
            ] {
                canonical_u64(key, &record.fields[key])?;
            }
            require_enum(
                record,
                "acceptance_verdict",
                &["conforming", "nonconforming"],
            )?;
            validate_sorted_csv("online_cores", &record.fields["online_cores"])?;
        }
        BACKEND_CONFORMANCE => validate_conformance(record)?,
        PI_BENCHMARK => validate_benchmark(record)?,
        STAGE1_PAIR => validate_stage1_pair(record)?,
        other => return Err(format!("evidence record: unknown typed format `{other}`")),
    }
    Ok(())
}

fn validate_stage1_pair(record: &Record) -> Result<(), String> {
    record.require_exact_fields(&[
        "acceptance_verdict",
        "host_identity_sha256",
        "host_page_size",
        "host_profile_sha256",
        "image_sha256",
        "kernel_release",
        "measurement_scope",
        "mmu_off_exit_counts",
        "mmu_off_record_digests",
        "mmu_off_vcpu_run_ns",
        "mmu_on_exit_counts",
        "mmu_on_record_digests",
        "mmu_on_vcpu_run_ns",
        "output_sha256",
        "report_sha256",
        "run_environment_after_sha256",
        "run_environment_before_sha256",
        "sample_count",
        "stage1_tables_sha256",
        "temperature_after_millic",
        "temperature_before_millic",
        "throttle_after",
        "throttle_before",
        "vmm_binary_sha256",
        "vmm_source_sha256",
        "warmup_count",
        "workload",
        "workload_sha256",
    ])?;
    for key in [
        "host_identity_sha256",
        "host_profile_sha256",
        "image_sha256",
        "output_sha256",
        "report_sha256",
        "run_environment_after_sha256",
        "run_environment_before_sha256",
        "stage1_tables_sha256",
        "vmm_binary_sha256",
        "vmm_source_sha256",
        "workload_sha256",
    ] {
        require_sha256(key, &record.fields[key])?;
    }
    require_enum(record, "acceptance_verdict", &["pass"])?;
    require_enum(record, "measurement_scope", &["kvm-vcpu-run-paired-v1"])?;
    let count = canonical_u64("sample_count", &record.fields["sample_count"])? as usize;
    if count == 0 || canonical_u64("warmup_count", &record.fields["warmup_count"])? == 0 {
        return Err("wrela-stage1-pair-v1: samples and warmups must be positive".into());
    }
    canonical_u64("host_page_size", &record.fields["host_page_size"])?;
    for key in [
        "mmu_off_exit_counts",
        "mmu_off_vcpu_run_ns",
        "mmu_on_exit_counts",
        "mmu_on_vcpu_run_ns",
    ] {
        let values = parse_u64_list(key, &record.fields[key])?;
        if values.len() != count || values.iter().any(|value| *value == 0) {
            return Err(format!(
                "wrela-stage1-pair-v1: `{key}` must contain {count} positive samples"
            ));
        }
    }
    for key in ["mmu_off_record_digests", "mmu_on_record_digests"] {
        validate_digest_list(key, &record.fields[key], count)?;
    }
    for key in ["temperature_after_millic", "temperature_before_millic"] {
        canonical_u64(key, &record.fields[key])?;
    }
    if record.fields["throttle_before"] != "throttled=0x0"
        || record.fields["throttle_after"] != "throttled=0x0"
    {
        return Err("wrela-stage1-pair-v1: throttled samples are nonconforming".into());
    }
    Ok(())
}

fn require_enum(record: &Record, key: &str, allowed: &[&str]) -> Result<(), String> {
    let value = record
        .fields
        .get(key)
        .ok_or_else(|| format!("{}: missing required field `{key}`", record.format))?;
    if allowed.contains(&value.as_str()) {
        Ok(())
    } else {
        Err(format!(
            "{}: field `{key}` has unsupported value `{value}`",
            record.format
        ))
    }
}

fn validate_capability_rows(raw: &str) -> Result<(), String> {
    let required = [
        "api_version",
        "immediate_exit",
        "ipa_size",
        "isa_baseline",
        "one_reg",
        "readonly_mem",
        "target",
        "user_memory",
        "vcpu_count",
        "writable_id_regs",
    ];
    let mut seen = Vec::new();
    for row in raw.split(',') {
        let (key, value) = row
            .split_once(':')
            .ok_or_else(|| format!("kvm_capabilities: malformed row `{row}`"))?;
        if !matches!(value, "yes" | "no" | "not-queried") {
            return Err(format!("kvm_capabilities: invalid verdict `{value}`"));
        }
        seen.push(key);
    }
    if seen != required {
        return Err(format!(
            "kvm_capabilities: rows must be exactly {:?} in sorted order",
            required
        ));
    }
    Ok(())
}

fn validate_sorted_csv(kind: &str, raw: &str) -> Result<(), String> {
    let mut previous: Option<u64> = None;
    if raw.is_empty() {
        return Err(format!("{kind}: empty list is not canonical"));
    }
    for item in raw.split(',') {
        let value = canonical_u64(kind, item)?;
        if previous.is_some_and(|old| old >= value) {
            return Err(format!("{kind}: list is not strictly sorted and unique"));
        }
        previous = Some(value);
    }
    Ok(())
}

fn validate_conformance(record: &Record) -> Result<(), String> {
    record.require_exact_fields(&[
        "backend",
        "backend_binary_sha256",
        "case",
        "choice_count",
        "cross_replay_matrix_sha256",
        "exit_class",
        "exit_count",
        "frame_count",
        "frame_digest_sequence",
        "guest_exit_code",
        "image_sha256",
        "kvm_host_identity_sha256",
        "kvm_host_profile_sha256",
        "machine_revision",
        "record_sha256",
        "report_sha256",
        "source_contract_sha256",
        "transcript_sha256",
    ])?;
    require_enum(record, "backend", &["hvf", "kvm"])?;
    for key in [
        "image_sha256",
        "backend_binary_sha256",
        "cross_replay_matrix_sha256",
        "kvm_host_identity_sha256",
        "kvm_host_profile_sha256",
        "record_sha256",
        "report_sha256",
        "source_contract_sha256",
        "transcript_sha256",
    ] {
        require_sha256(key, &record.fields[key])?;
    }
    for key in [
        "choice_count",
        "exit_count",
        "frame_count",
        "guest_exit_code",
    ] {
        canonical_u64(key, &record.fields[key])?;
    }
    let frames = canonical_u64("frame_count", &record.fields["frame_count"])? as usize;
    validate_digest_list(
        "frame_digest_sequence",
        &record.fields["frame_digest_sequence"],
        frames,
    )
}

fn validate_benchmark(record: &Record) -> Result<(), String> {
    const FIELDS: &[&str] = &[
        "acceptance_verdict",
        "cpu_model",
        "display_mode",
        "frame_count",
        "frame_digest_sequence",
        "frequency_policy",
        "governor",
        "guest_exit_count",
        "host_affinity",
        "host_identity_sha256",
        "host_profile_sha256",
        "host_protection_profile",
        "image_sha256",
        "kernel_release",
        "max_ns",
        "measurement_scope",
        "median_ns",
        "min_ns",
        "network_time_excluded",
        "online_cores",
        "optional_counter_rows",
        "output_sha256",
        "record_digest_sequence",
        "report_sha256",
        "run_environment_after_sha256",
        "run_environment_before_sha256",
        "sample_count",
        "samples_ns",
        "stage1_tables_sha256",
        "temperature_after_millic",
        "temperature_before_millic",
        "throttle_after",
        "throttle_before",
        "vcpu_affinity",
        "vcpu_run_ns_samples",
        "vmm_binary_sha256",
        "vmm_source_sha256",
        "warmup_count",
        "workload",
        "workload_sha256",
    ];
    record.require_exact_fields(FIELDS)?;
    for key in [
        "host_identity_sha256",
        "host_profile_sha256",
        "image_sha256",
        "output_sha256",
        "report_sha256",
        "run_environment_after_sha256",
        "run_environment_before_sha256",
        "stage1_tables_sha256",
        "vmm_binary_sha256",
        "vmm_source_sha256",
        "workload_sha256",
    ] {
        require_sha256(key, &record.fields[key])?;
    }
    let count = canonical_u64("sample_count", &record.fields["sample_count"])? as usize;
    if count == 0 {
        return Err("wrela-pi-benchmark-v1: sample_count must be positive".into());
    }
    let samples = parse_u64_list("samples_ns", &record.fields["samples_ns"])?;
    if samples.len() != count {
        return Err("wrela-pi-benchmark-v1: sample_count disagrees with samples_ns".into());
    }
    validate_digest_list(
        "record_digest_sequence",
        &record.fields["record_digest_sequence"],
        count,
    )?;
    let vcpu_samples = record.fields["vcpu_run_ns_samples"]
        .split(';')
        .map(|row| parse_u64_list("vcpu_run_ns_samples", row))
        .collect::<Result<Vec<_>, _>>()?;
    let width = vcpu_samples.first().map_or(0, Vec::len);
    if vcpu_samples.len() != count
        || width == 0
        || vcpu_samples
            .iter()
            .any(|row| row.len() != width || row.iter().any(|value| *value == 0))
    {
        return Err(
            "wrela-pi-benchmark-v1: vCPU run samples differ in count/width or contain zero".into(),
        );
    }
    require_enum(record, "acceptance_verdict", &["pass"])?;
    require_enum(record, "display_mode", &["headless", "drm"])?;
    require_enum(
        record,
        "measurement_scope",
        &["protected-vmm-process-perf-stat-v1"],
    )?;
    require_enum(record, "network_time_excluded", &["yes"])?;
    if canonical_u64("warmup_count", &record.fields["warmup_count"])? == 0 {
        return Err("wrela-pi-benchmark-v1: at least one warmup is required".into());
    }
    for key in [
        "frame_count",
        "guest_exit_count",
        "temperature_after_millic",
        "temperature_before_millic",
    ] {
        canonical_u64(key, &record.fields[key])?;
    }
    validate_benchmark_counters(&record.fields["optional_counter_rows"])?;
    if record.fields["throttle_before"] != "throttled=0x0"
        || record.fields["throttle_after"] != "throttled=0x0"
    {
        return Err("wrela-pi-benchmark-v1: throttled runs are nonconforming".into());
    }
    let mut sorted = samples.clone();
    sorted.sort_unstable();
    let median = sorted[(sorted.len() - 1) / 2];
    for (key, value) in [
        ("min_ns", sorted[0]),
        ("median_ns", median),
        ("max_ns", *sorted.last().expect("nonempty")),
    ] {
        if canonical_u64(key, &record.fields[key])? != value {
            return Err(format!(
                "wrela-pi-benchmark-v1: `{key}` is not derivable from samples"
            ));
        }
    }
    let frames = canonical_u64("frame_count", &record.fields["frame_count"])? as usize;
    validate_digest_list(
        "frame_digest_sequence",
        &record.fields["frame_digest_sequence"],
        frames,
    )
}

fn validate_benchmark_counters(raw: &str) -> Result<(), String> {
    let required = [
        "br_mis_pred",
        "cpu_cycles",
        "inst_retired",
        "l1d_cache_refill",
        "l2d_cache_refill",
        "stall_backend",
        "stall_frontend",
    ];
    let mut found = Vec::new();
    for row in raw.split(',') {
        let (name, value) = row
            .split_once(':')
            .ok_or_else(|| format!("optional_counter_rows: malformed `{row}`"))?;
        canonical_u64("optional counter", value)?;
        found.push(name);
    }
    if found != required {
        return Err(format!(
            "optional_counter_rows: required rows are exactly {required:?}"
        ));
    }
    Ok(())
}

pub(crate) fn parse_u64_list(kind: &str, raw: &str) -> Result<Vec<u64>, String> {
    if raw.is_empty() {
        return Ok(Vec::new());
    }
    raw.split(',')
        .map(|item| canonical_u64(kind, item))
        .collect()
}

pub(crate) fn validate_digest_list(kind: &str, raw: &str, count: usize) -> Result<(), String> {
    let values = if raw.is_empty() {
        Vec::new()
    } else {
        raw.split(',').collect()
    };
    if values.len() != count {
        return Err(format!(
            "{kind}: expected {count} digest(s), found {}",
            values.len()
        ));
    }
    values
        .into_iter()
        .try_for_each(|value| require_sha256(kind, value))
}

fn validate_atom(kind: &str, atom: &str) -> Result<(), String> {
    if atom.is_empty()
        || !atom.bytes().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || matches!(byte, b'-' | b'_' | b'.')
        })
    {
        return Err(format!(
            "evidence record: {kind} `{atom}` must use lowercase ASCII letters, digits, `-`, `_`, or `.`"
        ));
    }
    Ok(())
}

fn percent_encode_into(bytes: &[u8], out: &mut String) {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    for &byte in bytes {
        if byte.is_ascii_alphanumeric()
            || matches!(byte, b' ' | b'.' | b'_' | b'-' | b'/' | b':' | b',')
        {
            out.push(byte as char);
        } else {
            out.push('%');
            out.push(HEX[(byte >> 4) as usize] as char);
            out.push(HEX[(byte & 0xf) as usize] as char);
        }
    }
}

fn percent_decode(raw: &str) -> Result<Vec<u8>, String> {
    let bytes = raw.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut cursor = 0;
    while cursor < bytes.len() {
        if bytes[cursor] != b'%' {
            if !bytes[cursor].is_ascii() {
                return Err("evidence record: raw non-ASCII value byte".into());
            }
            out.push(bytes[cursor]);
            cursor += 1;
            continue;
        }
        let pair = bytes
            .get(cursor + 1..cursor + 3)
            .ok_or_else(|| "evidence record: truncated percent escape".to_string())?;
        let hi = hex_nibble(pair[0])?;
        let lo = hex_nibble(pair[1])?;
        let decoded = (hi << 4) | lo;
        if decoded.is_ascii_alphanumeric()
            || matches!(decoded, b' ' | b'.' | b'_' | b'-' | b'/' | b':' | b',')
        {
            return Err(format!(
                "evidence record: `%{:02X}` is an unnecessary noncanonical escape",
                decoded
            ));
        }
        out.push(decoded);
        cursor += 3;
    }
    Ok(out)
}

fn hex_nibble(byte: u8) -> Result<u8, String> {
    match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'A'..=b'F' => Ok(byte - b'A' + 10),
        _ => Err("evidence record: percent escapes use uppercase hexadecimal".into()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn canonical_round_trip_and_digest() {
        let mut record = Record::new("wrela-host-profile-v1").unwrap();
        record.insert("affinity", "cores=1-3\ncore0").unwrap();
        record.insert("mode", "product").unwrap();
        let encoded = record.encode().unwrap();
        assert_eq!(
            encoded,
            "format=wrela-host-profile-v1\naffinity=cores%3D1-3%0Acore0\nmode=product\n"
        );
        assert_eq!(parse(&encoded, "wrela-host-profile-v1").unwrap(), record);
        assert_eq!(record.digest_hex().unwrap().len(), 64);
    }

    #[test]
    fn hostile_records_fail_closed() {
        for (text, needle) in [
            ("mode=x\nformat=wrela-host-profile-v1\n", "first line"),
            (
                "format=wrela-host-profile-v1\nb=1\na=2\n",
                "strictly sorted",
            ),
            (
                "format=wrela-host-profile-v1\na=1\na=2\n",
                "strictly sorted",
            ),
            ("format=wrela-host-profile-v1\na=%2f\n", "uppercase"),
            ("format=wrela-host-profile-v1\na=%41\n", "unnecessary"),
            ("format=wrela-host-profile-v1\na=1", "end with one LF"),
        ] {
            let error = parse(text, "wrela-host-profile-v1").unwrap_err();
            assert!(
                error.contains(needle),
                "{error:?} did not contain {needle:?}"
            );
        }
    }

    #[test]
    fn schemas_reject_unknown_missing_and_bad_scalars() {
        let mut record = Record::new("wrela-run-environment-v1").unwrap();
        record.insert("temperature_millic", "42000").unwrap();
        assert!(
            record
                .require_exact_fields(&["temperature_millic", "throttle"])
                .is_err()
        );
        record.insert("throttle", "0").unwrap();
        assert!(
            record
                .require_exact_fields(&["temperature_millic", "throttle"])
                .is_ok()
        );
        assert!(canonical_u64("sample", "01").is_err());
        assert!(require_sha256("digest", &"0".repeat(64)).is_ok());
        assert!(require_sha256("digest", &"A".repeat(64)).is_err());
    }

    #[test]
    fn identity_schema_pins_not_queried_capability_rows() {
        let mut record = Record::new(HOST_IDENTITY).unwrap();
        for (key, value) in [
            ("acceptance_verdict", "refused"),
            ("architecture", "aarch64"),
            ("board_model", "Raspberry Pi 5 Model B Rev 1.0"),
            ("cpu_model", "Cortex-A76"),
            ("cpu_part", "0xd0b"),
            ("cpu_revision", "1"),
            ("device_tree_sha256", &"1".repeat(64)),
            ("drm_module", "not-queried"),
            ("eeprom_config_sha256", &"2".repeat(64)),
            ("eeprom_version", "not-queried"),
            ("kernel_config_sha256", &"3".repeat(64)),
            ("kernel_release", "6.18.34"),
            ("host_page_size", "16384"),
            (
                "kvm_capabilities",
                "api_version:not-queried,immediate_exit:not-queried,ipa_size:not-queried,isa_baseline:not-queried,one_reg:not-queried,readonly_mem:not-queried,target:not-queried,user_memory:not-queried,vcpu_count:not-queried,writable_id_regs:not-queried",
            ),
            ("kvm_module", "not-queried"),
            ("module_set_sha256", &"4".repeat(64)),
            ("pmu_identity", "not-queried"),
        ] {
            record.insert(key, value).unwrap();
        }
        validate_typed(&parse(&record.encode().unwrap(), HOST_IDENTITY).unwrap()).unwrap();
        record
            .fields
            .insert("kvm_capabilities".into(), "api_version:maybe".into());
        assert!(validate_typed(&record).is_err());
    }

    #[test]
    fn benchmark_median_counts_and_digests_fail_closed() {
        let digest = "a".repeat(64);
        let mut record = Record::new(PI_BENCHMARK).unwrap();
        for key in [
            "host_identity_sha256",
            "host_profile_sha256",
            "image_sha256",
            "output_sha256",
            "report_sha256",
            "run_environment_after_sha256",
            "run_environment_before_sha256",
            "stage1_tables_sha256",
            "vmm_binary_sha256",
            "vmm_source_sha256",
            "workload_sha256",
        ] {
            record.insert(key, digest.clone()).unwrap();
        }
        for (key, value) in [
            ("acceptance_verdict", "pass"),
            ("cpu_model", "Cortex-A76"),
            ("display_mode", "headless"),
            ("frame_count", "0"),
            ("frame_digest_sequence", ""),
            ("frequency_policy", "2400000"),
            ("governor", "performance"),
            ("guest_exit_count", "4"),
            ("host_affinity", "0"),
            ("host_protection_profile", "product"),
            ("kernel_release", "6.18.34"),
            ("max_ns", "30"),
            ("measurement_scope", "protected-vmm-process-perf-stat-v1"),
            ("median_ns", "20"),
            ("min_ns", "10"),
            ("network_time_excluded", "yes"),
            ("online_cores", "0,1,2,3"),
            (
                "optional_counter_rows",
                "br_mis_pred:1,cpu_cycles:2,inst_retired:3,l1d_cache_refill:4,l2d_cache_refill:5,stall_backend:6,stall_frontend:7",
            ),
            (
                "record_digest_sequence",
                &format!("{digest},{digest},{digest}"),
            ),
            ("sample_count", "3"),
            ("samples_ns", "30,10,20"),
            ("temperature_after_millic", "41000"),
            ("temperature_before_millic", "40000"),
            ("throttle_after", "throttled=0x0"),
            ("throttle_before", "throttled=0x0"),
            ("vcpu_affinity", "1,2,3"),
            ("vcpu_run_ns_samples", "30;10;20"),
            ("warmup_count", "1"),
            ("workload", "boot-actor-smoke"),
        ] {
            record.insert(key, value).unwrap();
        }
        validate_typed(&record).unwrap();
        record.fields.insert("median_ns".into(), "21".into());
        assert!(validate_typed(&record).is_err());
    }
}
