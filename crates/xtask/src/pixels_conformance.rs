use std::process::Command;

use crate::golden::{BootSel, GoldenOpts, build_and_sign_vmm, golden, heavy_pixels_jobs};
use crate::pixels_cache::{Cache, file_digest, is_sha256_hex, key_of, tree_digest};
use crate::root;

const EXPECTED: &str = "tests/pixels_truth/p8-visibility.txt";
const OUTPUT_DIR_ENV: &str = "WRELA_P8_CONFORMANCE_OUTPUT_DIR";

/// Version of the recorded truth schema and the numeric contract the scorer
/// enforces. It keys the score cache, so a truth-format or numeric-contract
/// change invalidates every cached score even when the evidence bytes are
/// unchanged. Move it with `GuestVisibilityExecution version=`.
const TRUTH_SCHEMA_VERSION: u32 = 2;
const CONFORMANCE_FIXTURE_VERSION: u32 = 2;

/// The compiler options both cached stages hold fixed. Naming them in the key
/// means a future flag cannot silently reuse a value derived under different
/// options.
const INSTRUMENTED_COMPILER_OPTIONS: &str = "--pixels-telemetry --image-digest-only";

fn compile_cache_key(sources: &str, compiler: &str, options: &str) -> String {
    key_of(&[
        ("sources", sources.to_string()),
        ("compiler", compiler.to_string()),
        ("options", options.to_string()),
    ])
}

struct ScoreKey<'a> {
    case: &'a str,
    fixture_version: u32,
    sources: &'a str,
    compiler: &'a str,
    options: &'a str,
    evidence: &'a str,
    scorer: &'a str,
    scorer_options: &'a str,
    numeric_contract: u32,
}

fn score_cache_key(parts: &ScoreKey<'_>) -> String {
    key_of(&[
        ("case", parts.case.to_string()),
        ("fixture_version", parts.fixture_version.to_string()),
        ("sources", parts.sources.to_string()),
        ("compiler", parts.compiler.to_string()),
        ("options", parts.options.to_string()),
        ("evidence", parts.evidence.to_string()),
        ("scorer", parts.scorer.to_string()),
        ("scorer_options", parts.scorer_options.to_string()),
        ("numeric_contract", parts.numeric_contract.to_string()),
    ])
}

fn boot_cache_key(image_digest: &str, vmm_digest: &str) -> String {
    key_of(&[
        ("image", image_digest.to_string()),
        ("vmm", vmm_digest.to_string()),
    ])
}

fn read_complete_raw_cache(paths: &[std::path::PathBuf; 3]) -> Option<Vec<Vec<u8>>> {
    paths.iter().map(|path| std::fs::read(path).ok()).collect()
}

fn read_raw_boot_cache(
    current: &[std::path::PathBuf; 3],
    legacy: Option<&[std::path::PathBuf; 3]>,
) -> (Option<Vec<Vec<u8>>>, bool) {
    if let Some(blobs) = read_complete_raw_cache(current) {
        return (Some(blobs), false);
    }
    let legacy = legacy.and_then(read_complete_raw_cache);
    let promote = legacy.is_some();
    (legacy, promote)
}

/// Concurrent instrumented guest boots.
///
/// Two by default: that matches the golden runner's HVF throttle, and more
/// intermittently starves multicore quiescence. `WRELA_P8_BOOT_WORKERS=3` is
/// an opt-in experiment, not a promotion — a third worker becomes the default
/// only with a recorded stability result behind it. Anything outside 1..=3 is
/// refused rather than clamped, so a typo does not quietly change the shape
/// of the run.
fn boot_workers() -> usize {
    match std::env::var("WRELA_P8_BOOT_WORKERS").ok().as_deref() {
        None => 2,
        Some(value) => match value.parse::<usize>() {
            Ok(workers @ 1..=3) => {
                if workers != 2 {
                    println!(
                        "pixels-conformance: experiment: {workers} boot worker(s)                          (WRELA_P8_BOOT_WORKERS); the default remains 2"
                    );
                }
                workers
            }
            _ => {
                println!(
                    "pixels-conformance: ignoring WRELA_P8_BOOT_WORKERS=`{value}`                      (expected 1..=3); using 2"
                );
                2
            }
        },
    }
}

/// The compiler entry source for one fixture case.
fn case_target(case: &str) -> Result<std::path::PathBuf, String> {
    let dir = root().join("tests/golden").join(case);
    let root_file = dir.join("root");
    if root_file.is_file() {
        let relative = std::fs::read_to_string(&root_file).map_err(|error| {
            format!("pixels conformance: read {}: {error}", root_file.display())
        })?;
        Ok(dir.join(relative.trim()))
    } else {
        Ok(dir.join("input.wr"))
    }
}

/// Digest of every Wrela source the instrumented image can possibly close
/// over: the whole stdlib plus the case's own fixture tree.
///
/// Deliberately a superset of the real closure. Over-covering costs a cache
/// miss when an unrelated stdlib file changes; under-covering would serve a
/// digest derived from source that no longer exists.
fn source_closure_digest(case: &str, target: &std::path::Path) -> Result<String, String> {
    let fixture = root().join("tests/golden").join(case);
    Ok(key_of(&[
        ("stdlib", tree_digest(&root().join("stdlib"), &["wr"])?),
        ("fixture", tree_digest(&fixture, &["wr", "txt"])?),
        (
            "target",
            target
                .strip_prefix(root())
                .unwrap_or(target)
                .display()
                .to_string(),
        ),
        ("target_content", file_digest(target)),
    ]))
}

/// Fingerprint of the scorer itself: the reference implementation the
/// `FrameScore` comes out of, plus the conformance driver in this file.
fn scorer_fingerprint() -> Result<String, String> {
    Ok(key_of(&[
        (
            "reference",
            tree_digest(
                &root().join("crates/wrela-compiler/src/pixels/reference"),
                &["rs"],
            )?,
        ),
        (
            "driver",
            file_digest(&root().join("crates/xtask/src/pixels_conformance.rs")),
        ),
        ("truth_schema", TRUTH_SCHEMA_VERSION.to_string()),
    ]))
}

/// Serialize a `FrameScore` as the score cache's body.
///
/// An explicit field-by-field format rather than a derived encoding: a field
/// added to `FrameScore` without a line here fails
/// `score_cache_body_round_trips_every_field` instead of being silently
/// dropped from cached results.
fn render_score(score: &wrela_compiler::pixels::reference::conformance::FrameScore) -> String {
    let issue = match score.first_issue {
        Some([x, y, kind]) => format!("{x},{y},{kind}"),
        None => "none".to_string(),
    };
    format!(
        "checked_interior {}\ninterior_mismatches {}\nedge_center_violations {}\n\
         ambiguous_identity {}\nunresolved {}\nskipped_unproven {}\nphantom_surface {}\n\
         q_checked {}\nnormal_checked {}\nevent_bytes_checked {}\n\
         boundary_limited_event_bytes {}\nraster_evidence_failures {}\nfirst_issue {issue}\n",
        score.checked_interior,
        score.interior_mismatches,
        score.edge_center_violations,
        score.ambiguous_identity,
        score.unresolved,
        score.skipped_unproven,
        score.phantom_surface,
        score.q_checked,
        score.normal_checked,
        score.event_bytes_checked,
        score.boundary_limited_event_bytes,
        score.raster_evidence_failures,
    )
}

fn parse_score(body: &str) -> Option<wrela_compiler::pixels::reference::conformance::FrameScore> {
    let mut score = wrela_compiler::pixels::reference::conformance::FrameScore::default();
    let mut seen = 0usize;
    for line in body.lines() {
        let (name, value) = line.split_once(' ')?;
        seen += 1;
        let field: &mut u32 = match name {
            "checked_interior" => &mut score.checked_interior,
            "interior_mismatches" => &mut score.interior_mismatches,
            "edge_center_violations" => &mut score.edge_center_violations,
            "ambiguous_identity" => &mut score.ambiguous_identity,
            "unresolved" => &mut score.unresolved,
            "skipped_unproven" => &mut score.skipped_unproven,
            "phantom_surface" => &mut score.phantom_surface,
            "q_checked" => &mut score.q_checked,
            "normal_checked" => &mut score.normal_checked,
            "event_bytes_checked" => &mut score.event_bytes_checked,
            "boundary_limited_event_bytes" => &mut score.boundary_limited_event_bytes,
            "raster_evidence_failures" => &mut score.raster_evidence_failures,
            "first_issue" => {
                score.first_issue = if value == "none" {
                    None
                } else {
                    let mut parts = value.split(',');
                    let mut next = || parts.next().and_then(|part| part.parse::<u16>().ok());
                    Some([next()?, next()?, next()?])
                };
                continue;
            }
            _ => return None,
        };
        *field = value.parse().ok()?;
    }
    (seen == 13).then_some(score)
}

fn score_passes(score: &wrela_compiler::pixels::reference::conformance::FrameScore) -> bool {
    score.interior_mismatches == 0
        && score.edge_center_violations == 0
        && score.unresolved == 0
        && score.skipped_unproven == 0
        && score.phantom_surface == 0
        && score.raster_evidence_failures == 0
}

fn write_actual_report(actual: &str) -> Result<(), String> {
    let Some(dir) = std::env::var_os(OUTPUT_DIR_ENV) else {
        return Ok(());
    };
    let dir = std::path::PathBuf::from(dir);
    std::fs::create_dir_all(&dir)
        .map_err(|error| format!("pixels conformance: create {}: {error}", dir.display()))?;
    let path = dir.join("p8-visibility.txt");
    let staged = dir.join(format!("p8-visibility.{}.tmp", std::process::id()));
    std::fs::write(&staged, actual)
        .map_err(|error| format!("pixels conformance: write {}: {error}", staged.display()))?;
    std::fs::rename(&staged, &path)
        .map_err(|error| format!("pixels conformance: publish {}: {error}", path.display()))
}
const GUEST_FRAME_DIGEST_MARKER: u64 = 4_922_225_244_575_680_596;
const GUEST_ALPHA_SAMPLE_MARKER: u64 = 5_780_180_186_688_408_645;
const GUEST_CERTIFIED_RUN_MARKER: u64 = 4_847_371_096_046_259_761;
const GUEST_FRAME_DUMP_MARKER: u64 = 7_599_824_371_187_933_777;

fn exact_axis_coverage_oracle(a: i64, b: i64, c: i64) -> Result<u8, String> {
    use wrela_compiler::pixels::reference::{
        coverage::{BoundaryOwner, HalfPlane, half_plane_area},
        iv32::FixedDomain,
    };

    // The fixture pins its upper visible edge to y=207/256 in the selected
    // pixel. Compute the exact box-filtered coverage independently of the
    // generated guest implementation; this is not a digest self-comparison.
    let area = half_plane_area(
        HalfPlane { a, b, c },
        FixedDomain::full(-8),
        BoundaryOwner::LowerOrLeft,
    )
    .map_err(|error| format!("pixels conformance: tile-boundary oracle: {error}"))?;
    if area.lo != area.hi {
        return Err(format!(
            "pixels conformance: tile-boundary oracle is not exact: {area:?}"
        ));
    }
    u8::try_from(area.lo)
        .map_err(|_| format!("pixels conformance: tile-boundary oracle is not a byte: {area:?}"))
}

fn tile_boundary_event_oracle(x: usize, y: usize) -> Result<Option<u8>, String> {
    // The fixture's projected box has independently pinned fractional edges:
    // x coverage 225/255 in columns 50 and 77, and y coverage 207/255 in
    // rows 13 and 17. Columns 51/76 are retained as event lanes beside the
    // vertical edges even though their horizontal coverage is full. Validate
    // the complete 68-pixel event partition, including both sides of the
    // 64-pixel scanout-tile seam and the four corner products.
    if !(13..=17).contains(&y) || !(50..=77).contains(&x) {
        return Ok(None);
    }
    let horizontal_event = matches!(y, 13 | 17);
    let vertical_event = matches!(x, 50 | 51 | 76 | 77);
    if !horizontal_event && !vertical_event {
        return Ok(None);
    }
    let x_coverage = if matches!(x, 50 | 77) {
        exact_axis_coverage_oracle(-256, 0, 225)?
    } else {
        255
    };
    let y_coverage = if horizontal_event {
        exact_axis_coverage_oracle(0, -256, 207)?
    } else {
        255
    };
    Ok(Some(
        (u16::from(x_coverage) * u16::from(y_coverage) + 127)
            .checked_div(255)
            .and_then(|value| u8::try_from(value).ok())
            .ok_or_else(|| "pixels conformance: tile-boundary coverage overflow".to_string())?,
    ))
}

fn diagonal_material_event_oracle(x: usize, y: usize) -> Result<Option<u8>, String> {
    use wrela_compiler::pixels::reference::coverage::{BoundaryOwner, HalfPlane, half_plane_byte};

    // The pinned z=0 material plane, eye z=-4, and world offset 11/128 project
    // to d + sx - sy >= 11/32 for d=x-y-16. The threshold stays well away from a
    // byte-rounding boundary in both owned lanes.
    let x = i64::try_from(x)
        .map_err(|_| "pixels conformance: diagonal x coordinate overflow".to_string())?;
    let y = i64::try_from(y)
        .map_err(|_| "pixels conformance: diagonal y coordinate overflow".to_string())?;
    let d = x - y - 16;
    if !matches!(d, 0 | 1) {
        return Ok(None);
    }
    let positive = HalfPlane {
        a: 32,
        b: -32,
        c: 32 * d - 11,
    };
    let (selected, owner) = if d == 1 {
        (positive, BoundaryOwner::LowerOrLeft)
    } else {
        (
            HalfPlane {
                a: -positive.a,
                b: -positive.b,
                c: -positive.c,
            },
            BoundaryOwner::UpperOrRight,
        )
    };
    half_plane_byte(selected, owner)
        .map(Some)
        .map_err(|error| format!("pixels conformance: diagonal oracle: {error}"))
}

pub fn pixels_conformance(
    update: bool,
    assume_guest_fixtures_verified: bool,
    deep_worker_variants: bool,
    only: Option<&[String]>,
) -> Result<(), String> {
    const ALL_GUEST_CASES: [&str; 14] = [
        "boot-pixels-plane",
        "boot-pixels-plane-one-core",
        "check-pixels-camera-inside",
        "check-pixels-close-depth",
        "check-pixels-displace",
        "check-pixels-enclosed-feature",
        "check-pixels-hard-csg",
        "check-pixels-material-edge",
        "check-pixels-repeat",
        "check-pixels-simultaneous-event",
        "check-pixels-smooth-csg",
        "check-pixels-tangent",
        "check-pixels-thin-feature",
        "check-pixels-tile-boundary",
    ];
    // A case subset is a development probe, not the gate: it runs every guest
    // and oracle assertion for the scenes named, but the archived report is a
    // whole-corpus artifact, so a partial run never compares against — or
    // rewrites — the recorded truth. The full corpus is what `verify` runs.
    if let Some(selected) = only {
        if update {
            return Err(
                "pixels conformance: --update rewrites the whole-corpus record, so it \
                 cannot be combined with --case"
                    .to_string(),
            );
        }
        if let Some(unknown) = selected
            .iter()
            .find(|case| !ALL_GUEST_CASES.contains(&case.as_str()))
        {
            return Err(format!(
                "pixels conformance: `{unknown}` is not a conformance case; known cases: {}",
                ALL_GUEST_CASES.join(", ")
            ));
        }
    }
    // The scoring pass derives frame-wide aggregates from both plane fixtures,
    // so a subset always carries them. They are the two cheapest cases in the
    // corpus, and including them keeps a subset probe running exactly the same
    // scoring code as the full run rather than a special-cased variant of it.
    const SUBSET_REQUIRED_CASES: [&str; 2] = ["boot-pixels-plane", "boot-pixels-plane-one-core"];
    let guest_cases: Vec<&str> = ALL_GUEST_CASES
        .into_iter()
        .filter(|case| {
            only.is_none_or(|selected| {
                SUBSET_REQUIRED_CASES.contains(case) || selected.iter().any(|want| want == case)
            })
        })
        .collect();
    if guest_cases.is_empty() {
        return Err("pixels conformance: case selection matched no conformance case".to_string());
    }
    let guest_cases = guest_cases.as_slice();
    const PROBE_CASE: &str = "check-pixels-visibility-probe";
    // One parallel golden pass over the whole case set: images compile on
    // every core and guest boots share the throttled VMM slots, instead of
    // one fully serial `golden()` invocation per case. `verify` runs its
    // full boot golden lane (which covers every one of these cases) before
    // this stage, so it skips the duplicate pass.
    if !assume_guest_fixtures_verified {
        golden(&GoldenOpts {
            update: false,
            cases: Some(
                guest_cases
                    .iter()
                    .copied()
                    .chain([PROBE_CASE])
                    .map(str::to_string)
                    .collect(),
            ),
            boot: BootSel::Only,
            ..GoldenOpts::default()
        })
        .map_err(|error| format!("pixels conformance: guest fixtures failed: {error}"))?;
    }
    let probe_transcript = root()
        .join("tests/golden")
        .join(PROBE_CASE)
        .join("expected/test.txt");
    let probe_values = std::fs::read_to_string(&probe_transcript)
        .map_err(|error| {
            format!(
                "pixels conformance: read verified guest transcript {}: {error}",
                probe_transcript.display()
            )
        })?
        .lines()
        .filter_map(|line| line.strip_prefix("p7 "))
        .map(|value| {
            u64::from_str_radix(value, 16)
                .map_err(|error| format!("pixels conformance: malformed probe: {error}"))
        })
        .collect::<Result<Vec<_>, _>>()?;
    let probe = probe_values.get(0..4).ok_or_else(|| {
        "pixels conformance: guest visibility probe omitted evidence words".to_string()
    })?;
    let packed = probe[3];
    let signed_component = |shift| {
        i32::try_from(i64::from((packed >> shift) as u16 as i16) * 1_000_000 / 32_767)
            .map_err(|_| "pixels conformance: decoded normal component overflow".to_string())
    };
    let guest_visibility_probe =
        wrela_compiler::pixels::reference::conformance::GuestVisibilityProbe {
            hit: probe[0] >> 32 == 1,
            identity: (probe[0] & u64::from(u32::MAX)) as u32,
            q_lo: probe[1] as i64 as i32,
            q_hi: probe[2] as i64 as i32,
            normal_valid: ((packed >> 56) & 1) == 1,
            normal: [
                signed_component(32)?,
                signed_component(16)?,
                signed_component(0)?,
            ],
            coverage: ((packed >> 48) & 255) as u8,
        };
    let mut observations = Vec::with_capacity(guest_cases.len());
    let mut semantic_targets = Vec::with_capacity(guest_cases.len());
    for &case in guest_cases {
        let transcript = root()
            .join("tests/golden")
            .join(case)
            .join("expected/test.txt");
        let text = std::fs::read_to_string(&transcript).map_err(|error| {
            format!(
                "pixels conformance: read verified guest transcript {}: {error}",
                transcript.display()
            )
        })?;
        let values = text
            .lines()
            .filter_map(|line| line.strip_prefix("p7 "))
            .map(|value| {
                u64::from_str_radix(value, 16).map_err(|error| {
                    format!(
                        "pixels conformance: malformed observation in {}: {error}",
                        transcript.display()
                    )
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        if values.len() < 3 {
            return Err(format!(
                "pixels conformance: `{case}` emitted {} observations, expected at least three",
                values.len()
            ));
        }
        let alpha_marker = values
            .iter()
            .rposition(|value| *value == GUEST_ALPHA_SAMPLE_MARKER);
        let alpha_samples = match alpha_marker {
            Some(marker) => {
                let alpha_word = *values.get(marker + 1).ok_or_else(|| {
                    format!("pixels conformance: `{case}` truncated alpha samples")
                })?;
                [
                    (alpha_word & 255) as u8,
                    ((alpha_word >> 8) & 255) as u8,
                    ((alpha_word >> 16) & 255) as u8,
                ]
            }
            None => {
                return Err(format!(
                    "pixels conformance: `{case}` omitted alpha sample marker"
                ));
            }
        };
        let frame_digest = values
            .iter()
            .rposition(|value| *value == GUEST_FRAME_DIGEST_MARKER)
            .map(|marker| {
                values
                    .get(marker + 1..marker + 5)
                    .ok_or_else(|| format!("pixels conformance: `{case}` truncated frame digest"))?
                    .try_into()
                    .map_err(|_| "pixels conformance: frame digest width changed".to_string())
            })
            .transpose()?;
        if frame_digest.is_none() {
            return Err(format!(
                "pixels conformance: `{case}` omitted frame digest marker"
            ));
        }
        let visibility_probe = if case == "boot-pixels-plane" {
            Some(guest_visibility_probe)
        } else {
            None
        };
        let counts = if case == "check-pixels-tile-boundary" {
            // The fixture range-checks and packs both u32 counts into one
            // trace descriptor so its 128x31 frame metadata fits the bounded
            // console arena without dropping either invariant.
            [values[0], values[1] & u64::from(u32::MAX), values[1] >> 32]
        } else {
            [values[0], values[1], values[2]]
        };
        observations.push(
            wrela_compiler::pixels::reference::conformance::GuestObservation {
                case: case.to_string(),
                certificate_runs: counts[0],
                event_corridors: counts[1],
                revalidated_proposals: counts[2],
                frame_digest: frame_digest.unwrap_or([0; 4]),
                alpha_samples,
                visibility_probe,
                run_evidence: None,
                frame_dump: None,
            },
        );
        let source = root().join("tests/golden").join(case).join("root");
        let target = if source.is_file() {
            let relative = std::fs::read_to_string(&source).map_err(|error| {
                format!("pixels conformance: read {}: {error}", source.display())
            })?;
            root().join("tests/golden").join(case).join(relative.trim())
        } else {
            root().join("tests/golden").join(case).join("input.wr")
        };
        semantic_targets.push(target);
    }
    let decode_frame_dump = |payload: &[u64],
                             external_frame: Option<&[u8]>|
     -> Result<
        (
            wrela_compiler::pixels::reference::conformance::FrameDump,
            Vec<u64>,
        ),
        String,
    > {
        if payload.len() < 15 {
            return Err("pixels conformance: frame dump payload is truncated".to_string());
        }
        let unpack = |word: u64| -> [f32; 2] {
            [
                f32::from_bits((word & u64::from(u32::MAX)) as u32),
                f32::from_bits((word >> 32) as u32),
            ]
        };
        let mut camera = [0.0_f32; 12];
        for (slot, word) in payload[..6].iter().enumerate() {
            let pair = unpack(*word);
            camera[slot * 2] = pair[0];
            camera[slot * 2 + 1] = pair[1];
        }
        let mut params = [0.0_f32; 16];
        for (slot, word) in payload[6..14].iter().enumerate() {
            let pair = unpack(*word);
            params[slot * 2] = pair[0];
            params[slot * 2 + 1] = pair[1];
        }
        let frame_words = usize::try_from(payload[14] & u64::from(u32::MAX))
            .map_err(|_| "pixels conformance: frame dump word count overflow".to_string())?;
        let telemetry_words = usize::try_from(payload[14] >> 32)
            .map_err(|_| "pixels conformance: telemetry word count overflow".to_string())?;
        let telemetry_start = if external_frame.is_some() && payload.len() == 15 + telemetry_words {
            15
        } else {
            15 + frame_words
        };
        if payload.len() != telemetry_start + telemetry_words {
            return Err(format!(
                "pixels conformance: frame dump carries {} words, expected {}",
                payload.len(),
                telemetry_start + telemetry_words
            ));
        }
        let bytes = if let Some(frame) = external_frame {
            if frame.len() != frame_words * 8 {
                return Err(format!(
                    "pixels conformance: VMM frame dump carries {} bytes, expected {}",
                    frame.len(),
                    frame_words * 8
                ));
            }
            frame.to_vec()
        } else {
            let frame = &payload[15..15 + frame_words];
            let mut bytes = Vec::with_capacity(frame_words * 8);
            for word in frame {
                bytes.extend_from_slice(&word.to_le_bytes());
            }
            bytes
        };
        let telemetry = &payload[telemetry_start..];
        let schema = telemetry.first().copied().ok_or_else(|| {
            "pixels conformance: CertificateTelemetry section is absent".to_string()
        })?;
        let version = schema & 0xffff;
        let count = usize::try_from(schema >> 16)
            .map_err(|_| "pixels conformance: telemetry schema count overflow".to_string())?;
        if version
            != u64::from(
                wrela_compiler::pixels::reference::telemetry::CERTIFICATE_TELEMETRY_VERSION,
            )
            || count
                != wrela_compiler::pixels::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2
                    as usize
            || telemetry.len() != count * 4 + 1
        {
            return Err(format!(
                "pixels conformance: incomplete CertificateTelemetry schema: version={version} count={count} words={}",
                telemetry.len()
            ));
        }
        Ok((
            wrela_compiler::pixels::reference::conformance::FrameDump {
                camera,
                params,
                bytes,
                raster_evidence: Vec::new(),
            },
            (0..count)
                .map(|counter| {
                    (0..4).try_fold(0_u64, |merged, worker| {
                        merged
                            .checked_add(telemetry[1 + worker * count + counter])
                            .ok_or_else(|| {
                                "pixels conformance: merged telemetry counter overflow".to_string()
                            })
                    })
                })
                .collect::<Result<Vec<_>, _>>()?,
        ))
    };
    #[derive(Clone)]
    struct InstrumentedRun {
        transcript: Vec<u8>,
        telemetry: [u64; 4],
        evidence: Option<[u64; 16]>,
        frame_digest: Option<[u64; 4]>,
        frame_dump: wrela_compiler::pixels::reference::conformance::FrameDump,
        telemetry_counters: Vec<u64>,
        state: Vec<u8>,
    }
    let vmm = build_and_sign_vmm()?;
    // The instrumented boots and the image-digest probe both invoke
    // `target/debug/wrela`; build it here so a conformance run never trusts
    // a stale binary from an earlier session.
    crate::run(
        Command::new("cargo").args(["build", "--quiet", "-p", "wrela-compiler", "--bin", "wrela"]),
        "cargo build wrela",
    )?;
    // A deterministic guest's observable outputs are a pure function of the
    // instrumented image and the VMM binary — the gate itself locks guest
    // determinism through the record/replay and worker-invariance checks —
    // so re-booting an unchanged image only re-derives bytes a previous run
    // already produced. The cache stores each boot's three raw artifacts
    // (guest transcript, frame dump, state dump) keyed by the exact image
    // and VMM digests; any compiler, stdlib, fixture, or VMM change yields a
    // new image or VMM digest and therefore a live boot. Parsing and every
    // assertion downstream of the raw bytes always run. WRELA_P8_BOOT_CACHE=0
    // forces live boots.
    let vmm_digest = wrela_compiler::report::sha256_hex(
        &std::fs::read(&vmm)
            .map_err(|error| format!("pixels conformance: read VMM binary for digest: {error}"))?,
    );
    let boot = Cache::boot();
    let boot_cache_dir = boot.dir().to_path_buf();
    let legacy_boot_cache_dir = boot.legacy_dir().map(std::path::Path::to_path_buf);
    let boot_cache_enabled = boot.enabled();
    if boot_cache_enabled {
        std::fs::create_dir_all(&boot_cache_dir).map_err(|error| {
            format!(
                "pixels conformance: create {}: {error}",
                boot_cache_dir.display()
            )
        })?;
    }
    let vmm_digest = &vmm_digest;
    let boot_cache_dir = &boot_cache_dir;
    let legacy_boot_cache_dir = legacy_boot_cache_dir.as_deref();
    // The compiler binary fingerprint keys the compile-closure cache. The VMM
    // binary digest deliberately does *not*: it keys the boot cache only, and
    // an image digest is a function of the compiler and its sources.
    let compiler_fingerprint = file_digest(&root().join("target/debug/wrela"));
    let compile_cache = Cache::compile_closure();
    let score_cache = Cache::score();
    let scorer = scorer_fingerprint()?;
    let compiler_fingerprint = &compiler_fingerprint;
    let compile_cache = &compile_cache;
    let score_cache = &score_cache;
    let scorer = &scorer;
    static FRAME_DUMP_ID: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let instrumented = |case: &str,
                        target_override: Option<&std::path::Path>|
     -> Result<InstrumentedRun, String> {
        let wrela = root().join("target/debug/wrela");
        let target = match target_override {
            Some(target) => target.to_path_buf(),
            None => {
                let source = root().join("tests/golden").join(case).join("root");
                if source.is_file() {
                    let relative = std::fs::read_to_string(&source).map_err(|error| {
                        format!("pixels conformance: read {}: {error}", source.display())
                    })?;
                    root().join("tests/golden").join(case).join(relative.trim())
                } else {
                    root().join("tests/golden").join(case).join("input.wr")
                }
            }
        };
        // Compile-closure cache. The cached value is exactly one thing: the
        // mapping from source closure to instrumented image digest. It
        // stores no image and makes no claim past that step — when the VMM
        // changed and a fresh boot is required, the boot below still runs,
        // and every assertion downstream of the raw bytes always runs.
        let sources = source_closure_digest(case, &target)?;
        let compile_key = compile_cache_key(
            &sources,
            compiler_fingerprint,
            INSTRUMENTED_COMPILER_OPTIONS,
        );
        let image_digest = match compile_cache.get(&compile_key) {
            Some(digest) if is_sha256_hex(digest.trim()) => digest.trim().to_string(),
            _ => {
                let digest_output = Command::new(&wrela)
                    .current_dir(root())
                    .arg("test")
                    .arg(&target)
                    .arg("--pixels-telemetry")
                    .arg("--image-digest-only")
                    .arg("--vmm")
                    .arg(&vmm)
                    .output()
                    .map_err(|error| {
                        format!("pixels conformance: digest instrumented `{case}`: {error}")
                    })?;
                if !digest_output.status.success() {
                    // A failed compile is never cached.
                    return Err(format!(
                        "pixels conformance: instrumented `{case}` image digest failed:\n{}{}",
                        String::from_utf8_lossy(&digest_output.stdout),
                        String::from_utf8_lossy(&digest_output.stderr),
                    ));
                }
                let digest = String::from_utf8_lossy(&digest_output.stdout)
                    .lines()
                    .find_map(|line| line.strip_prefix("p8-image-digest ").map(str::to_string))
                    .ok_or_else(|| {
                        format!(
                            "pixels conformance: instrumented `{case}` omitted its image digest"
                        )
                    })?;
                compile_cache.put(&compile_key, &digest);
                digest
            }
        };
        let boot_key = boot_cache_key(&image_digest, vmm_digest);
        let cache_paths = ["stdout", "frame", "state"]
            .map(|kind| boot_cache_dir.join(format!("{boot_key}.{kind}")));
        let legacy_cache_paths = legacy_boot_cache_dir.map(|dir| {
            ["stdout", "frame", "state"].map(|kind| dir.join(format!("{boot_key}.{kind}")))
        });
        let (cached, promote_legacy): (Option<Vec<Vec<u8>>>, bool) = if boot_cache_enabled {
            read_raw_boot_cache(&cache_paths, legacy_cache_paths.as_ref())
        } else {
            (None, false)
        };
        let (stdout_bytes, frame_bytes, state) = if let Some(mut blobs) = cached {
            println!("pixels-conformance: boot cache hit for `{case}`");
            let state = blobs.pop().expect("three cached blobs");
            let frame = blobs.pop().expect("three cached blobs");
            let stdout = blobs.pop().expect("three cached blobs");
            (stdout, frame, state)
        } else {
            println!("pixels-conformance: boot cache miss for `{case}`; running signed guest");
            let boot_started = std::time::Instant::now();
            let frame_dump_path = std::env::temp_dir().join(format!(
                "wrela-p8-frame-{}-{}.bgra",
                std::process::id(),
                FRAME_DUMP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = std::fs::remove_file(&frame_dump_path);
            let state_dump_path = std::env::temp_dir().join(format!(
                "wrela-p8-state-{}-{}.bin",
                std::process::id(),
                FRAME_DUMP_ID.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
            ));
            let _ = std::fs::remove_file(&state_dump_path);
            let mut command = Command::new(&wrela);
            command
                .current_dir(root())
                .arg("test")
                .arg(&target)
                .arg("--pixels-telemetry")
                .arg("--vmm")
                .arg(&vmm);
            command.env("WRELA_P8_FRAME_DUMP", &frame_dump_path);
            command.env("WRELA_P8_STATE_DUMP", &state_dump_path);
            let output = command.output().map_err(|error| {
                format!("pixels conformance: run instrumented `{case}`: {error}")
            })?;
            if !output.status.success() {
                return Err(format!(
                    "pixels conformance: instrumented `{case}` failed:\n{}{}",
                    String::from_utf8_lossy(&output.stdout),
                    String::from_utf8_lossy(&output.stderr),
                ));
            }
            let frame_bytes = std::fs::read(&frame_dump_path).map_err(|error| {
                format!(
                    "pixels conformance: read VMM frame dump {}: {error}",
                    frame_dump_path.display()
                )
            })?;
            let _ = std::fs::remove_file(&frame_dump_path);
            let state = std::fs::read(&state_dump_path).map_err(|error| {
                format!(
                    "pixels conformance: read renderer state dump {}: {error}",
                    state_dump_path.display()
                )
            })?;
            let _ = std::fs::remove_file(&state_dump_path);
            if boot_cache_enabled {
                for (path, bytes) in cache_paths
                    .iter()
                    .zip([&output.stdout, &frame_bytes, &state])
                {
                    let staged = path.with_extension(format!("tmp-{}", std::process::id()));
                    std::fs::write(&staged, bytes)
                        .and_then(|()| std::fs::rename(&staged, path))
                        .map_err(|error| {
                            format!(
                                "pixels conformance: write boot cache {}: {error}",
                                path.display()
                            )
                        })?;
                }
            }
            println!(
                "pixels-conformance: booted `{case}` in {:.1}s",
                boot_started.elapsed().as_secs_f64()
            );
            (output.stdout, frame_bytes, state)
        };
        let raw_frame_bytes = frame_bytes.clone();
        let values = String::from_utf8_lossy(&stdout_bytes)
            .lines()
            .filter_map(|line| line.strip_prefix("p7 "))
            .map(|value| {
                u64::from_str_radix(value, 16).map_err(|error| {
                    format!("pixels conformance: instrumented observation: {error}")
                })
            })
            .collect::<Result<Vec<_>, _>>()?;
        let dump_marker = values
            .iter()
            .rposition(|value| *value == GUEST_FRAME_DUMP_MARKER)
            .ok_or_else(|| {
                format!("pixels conformance: instrumented `{case}` omitted the frame dump")
            })?;
        let external_frame = Some(frame_bytes);
        let (frame_dump, telemetry_counters) =
            decode_frame_dump(&values[dump_marker + 1..], external_frame.as_deref())
                .map_err(|error| format!("pixels conformance: instrumented `{case}`: {error}"))?;
        // The transcript protocol places the four telemetry-digest words (for
        // the plane cases that trace them) immediately before the dump
        // marker; other fixtures carry their displayed digest there, and the
        // value is only consulted for the plane worker-invariance check.
        let telemetry = values
            .get(dump_marker.saturating_sub(4)..dump_marker)
            .ok_or_else(|| {
                format!("pixels conformance: instrumented `{case}` omitted telemetry digest")
            })?
            .try_into()
            .map_err(|_| "pixels conformance: telemetry digest width changed".to_string())?;
        let frame_digest = values
            .iter()
            .rposition(|value| *value == GUEST_FRAME_DIGEST_MARKER)
            .map(|marker| {
                values
                    .get(marker + 1..marker + 5)
                    .ok_or_else(|| {
                        format!("pixels conformance: instrumented `{case}` truncated frame digest")
                    })?
                    .try_into()
                    .map_err(|_| "pixels conformance: frame digest width changed".to_string())
            })
            .transpose()?;
        if frame_digest.is_none() {
            return Err(format!(
                "pixels conformance: instrumented `{case}` omitted its frame digest"
            ));
        }
        let evidence = values
            .iter()
            .position(|value| *value == GUEST_CERTIFIED_RUN_MARKER)
            .map(|marker| -> Result<[u64; 16], String> {
                let evidence: [u64; 16] = values
                    .get(marker + 1..marker + 17)
                    .ok_or_else(|| {
                        format!("pixels conformance: instrumented `{case}` truncated run evidence")
                    })?
                    .try_into()
                    .map_err(|_| "pixels conformance: run evidence width changed".to_string())?;
                wrela_compiler::pixels::reference::sweep::decode_certified_run_record(evidence)
                    .map_err(|error| {
                        format!("pixels conformance: instrumented `{case}` run evidence: {error:?}")
                    })?;
                Ok(evidence)
            })
            .transpose()?;
        if evidence.is_none() && case != "check-pixels-tile-boundary" {
            return Err(format!(
                "pixels conformance: instrumented `{case}` omitted run evidence"
            ));
        }
        if promote_legacy {
            for (path, bytes) in cache_paths
                .iter()
                .zip([&stdout_bytes, &raw_frame_bytes, &state])
            {
                let staged = path.with_extension(format!("tmp-{}", std::process::id()));
                std::fs::write(&staged, bytes)
                    .and_then(|()| std::fs::rename(&staged, path))
                    .map_err(|error| {
                        format!(
                            "pixels conformance: promote boot cache {}: {error}",
                            path.display()
                        )
                    })?;
            }
        }
        Ok(InstrumentedRun {
            transcript: stdout_bytes,
            telemetry,
            evidence,
            frame_digest,
            frame_dump,
            telemetry_counters,
            state,
        })
    };
    // The raster evidence lives in the instrumented state dump; pull it into
    // the frame dump so scoring and the class checks below can read it.
    // Idempotent so the eager scoring worker and the strict serial pass can
    // both call it without re-slicing the state.
    let extract_raster_evidence = |case: &str,
                                   renderer: &wrela_compiler::pixels::CompiledRenderer,
                                   run: &mut InstrumentedRun|
     -> Result<(), String> {
        if !run.frame_dump.raster_evidence.is_empty() {
            return Ok(());
        }
        let counter_bytes = u64::from(renderer.config.worker_count)
            .checked_mul(
                8 * wrela_compiler::pixels::reference::telemetry::CERTIFICATE_TELEMETRY_COUNTERS_V2,
            )
            .ok_or_else(|| "pixels conformance: telemetry counter byte overflow".to_string())?;
        let evidence_offset = renderer
            .mutable_layout
            .telemetry
            .offset
            .checked_add(counter_bytes)
            .and_then(|offset| usize::try_from(offset).ok())
            .ok_or_else(|| "pixels conformance: raster evidence offset overflow".to_string())?;
        let evidence_pixels = usize::try_from(renderer.config.width)
            .ok()
            .and_then(|width| {
                usize::try_from(renderer.config.height)
                    .ok()
                    .and_then(|height| width.checked_mul(height))
            })
            .ok_or_else(|| "pixels conformance: raster evidence extent overflow".to_string())?;
        let evidence_end = evidence_offset
                .checked_add(evidence_pixels * 24)
                .filter(|end| *end <= run.state.len())
                .ok_or_else(|| {
                    format!(
                        "pixels conformance: instrumented `{case}` state omits raster evidence at {evidence_offset}+{} of {} bytes",
                        evidence_pixels * 24,
                        run.state.len()
                    )
                })?;
        run.frame_dump.raster_evidence = run.state[evidence_offset..evidence_end]
            .chunks_exact(24)
            .map(|bytes| {
                [
                    u64::from_le_bytes(bytes[..8].try_into().expect("eight-byte chunk")),
                    u64::from_le_bytes(bytes[8..16].try_into().expect("eight-byte chunk")),
                    u64::from_le_bytes(bytes[16..].try_into().expect("eight-byte chunk")),
                ]
            })
            .collect();
        Ok(())
    };
    // Case-level score cache. The key covers everything `score_frame`
    // actually consumes, without leaning on the image digest to stand in for
    // compiler internals: the source closure, the compiler fingerprint and
    // its options, the case identity and fixture version, the complete
    // recorded evidence (transcript-derived frame dump, raster evidence, and
    // state dump), the scorer's own source fingerprint, and the truth-schema
    // / numeric-contract version. The VMM binary digest keys the boot cache
    // and is explicitly *not* part of this key: it does not enter the score.
    let cached_score = |case: &str,
                        renderer: &wrela_compiler::pixels::CompiledRenderer,
                        run: &InstrumentedRun|
     -> Result<
        Option<wrela_compiler::pixels::reference::conformance::FrameScore>,
        String,
    > {
        let target = case_target(case)?;
        let evidence = key_of(&[
            (
                "stdout_transcript",
                wrela_compiler::report::sha256_hex(&run.transcript),
            ),
            (
                "frame_bytes",
                wrela_compiler::report::sha256_hex(&run.frame_dump.bytes),
            ),
            (
                "raster_evidence",
                wrela_compiler::report::sha256_hex(
                    run.frame_dump
                        .raster_evidence
                        .iter()
                        .flat_map(|words| words.iter().flat_map(|word| word.to_le_bytes()))
                        .collect::<Vec<u8>>()
                        .as_slice(),
                ),
            ),
            ("state", wrela_compiler::report::sha256_hex(&run.state)),
            (
                "telemetry",
                wrela_compiler::report::sha256_hex(
                    run.telemetry_counters
                        .iter()
                        .flat_map(|counter| counter.to_le_bytes())
                        .collect::<Vec<u8>>()
                        .as_slice(),
                ),
            ),
        ]);
        let sources = source_closure_digest(case, &target)?;
        let key = score_cache_key(&ScoreKey {
            case,
            fixture_version: CONFORMANCE_FIXTURE_VERSION,
            sources: &sources,
            compiler: compiler_fingerprint,
            options: INSTRUMENTED_COMPILER_OPTIONS,
            evidence: &evidence,
            scorer,
            scorer_options: "full-frame",
            numeric_contract: TRUTH_SCHEMA_VERSION,
        });
        if let Some(body) = score_cache.get(&key) {
            if let Some(score) = parse_score(&body).filter(score_passes) {
                return Ok(Some(score));
            }
            println!(
                "pixels-conformance: unreadable or failing cached score for `{case}`; rescoring"
            );
        }
        let score = wrela_compiler::pixels::reference::conformance::score_frame(
            case,
            renderer,
            &run.frame_dump,
        )
        .map_err(|error| format!("`{case}` frame scoring failed: {error}"))?;
        // A derived mismatch is still a failed score even though scoring
        // returned normally; cache only the same passing shape `run_scored`
        // accepts.
        if score_passes(&score) {
            score_cache.put(&key, &render_score(&score));
        }
        Ok(Some(score))
    };
    let cached_score = &cached_score;
    // Instrumented guest runs (VMM-bound) and semantic reference compiles
    // (CPU-bound) are independent per case: run both through small worker
    // pools concurrently. Two concurrent guests match the golden runner's
    // HVF throttle (more intermittently starves multicore quiescence), and
    // two concurrent in-process compiles bound the arena memory peak.
    // Frame scoring is CPU-bound, dominates the non-boot cost of the gate,
    // and needs only a case's own boot result and semantic renderer — so a
    // scoring worker consumes each case the moment both halves land, hiding
    // the entire scoring bill inside the boot window instead of appending it.
    let (instrumented_runs, semantic_results, frame_scores): (
        Vec<Result<InstrumentedRun, String>>,
        Vec<Result<wrela_compiler::pixels::CompiledRenderer, String>>,
        Vec<Result<Option<wrela_compiler::pixels::reference::conformance::FrameScore>, String>>,
    ) = {
        let boot_slots: Vec<std::sync::Mutex<Option<Result<InstrumentedRun, String>>>> =
            guest_cases
                .iter()
                .map(|_| std::sync::Mutex::new(None))
                .collect();
        let semantic_slots: Vec<
            std::sync::Mutex<Option<Result<wrela_compiler::pixels::CompiledRenderer, String>>>,
        > = guest_cases
            .iter()
            .map(|_| std::sync::Mutex::new(None))
            .collect();
        let score_slots: Vec<
            std::sync::Mutex<
                Option<
                    Result<
                        Option<wrela_compiler::pixels::reference::conformance::FrameScore>,
                        String,
                    >,
                >,
            >,
        > = guest_cases
            .iter()
            .map(|_| std::sync::Mutex::new(None))
            .collect();
        // A case is scoreable once both its boot and its semantic compile
        // have landed; whichever worker finishes a case second hands it to
        // the scoring worker.
        let score_ready: Vec<std::sync::atomic::AtomicU8> = guest_cases
            .iter()
            .map(|_| std::sync::atomic::AtomicU8::new(0))
            .collect();
        let (score_sender, score_receiver) = std::sync::mpsc::channel::<usize>();
        let score_receiver = std::sync::Mutex::new(score_receiver);
        let boot_cursor = std::sync::atomic::AtomicUsize::new(0);
        let semantic_cursor = std::sync::atomic::AtomicUsize::new(0);
        // The compiler opt selection is thread-local; workers inherit the
        // invoking thread's active set explicitly.
        let active_opts = wrela_compiler::opts::active_opts();
        let instrumented = &instrumented;
        let extract_raster_evidence = &extract_raster_evidence;
        let boot_slots_ref = &boot_slots;
        let semantic_slots_ref = &semantic_slots;
        let score_slots_ref = &score_slots;
        let score_ready_ref = &score_ready;
        let score_receiver_ref = &score_receiver;
        let boot_cursor_ref = &boot_cursor;
        let semantic_cursor_ref = &semantic_cursor;
        let semantic_targets_ref = &semantic_targets;
        let active_opts_ref = &active_opts;
        std::thread::scope(|scope| {
            for _ in 0..boot_workers() {
                let score_sender = score_sender.clone();
                scope.spawn(move || {
                    loop {
                        let index =
                            boot_cursor_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&case) = guest_cases.get(index) else {
                            return;
                        };
                        let run = instrumented(case, None);
                        *boot_slots_ref[index]
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(run);
                        if score_ready_ref[index].fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                            == 1
                        {
                            let _ = score_sender.send(index);
                        }
                    }
                });
            }
            for _ in 0..heavy_pixels_jobs().min(guest_cases.len()) {
                let score_sender = score_sender.clone();
                scope.spawn(move || {
                    wrela_compiler::opts::apply_opts(active_opts_ref);
                    loop {
                        let index =
                            semantic_cursor_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&case) = guest_cases.get(index) else {
                            return;
                        };
                        let target = &semantic_targets_ref[index];
                        let compiled =
                            wrela_compiler::cost::stage::load_pixels_programs(target)
                                .map_err(|error| {
                                    format!(
                                        "pixels conformance: compile semantic `{case}`: {error}"
                                    )
                                })
                                .and_then(|programs| {
                                    programs.compiled_renderers.into_iter().next().ok_or_else(|| {
                                    format!("pixels conformance: `{case}` has no sealed renderer")
                                })
                                });
                        *semantic_slots_ref[index]
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(compiled);
                        if score_ready_ref[index].fetch_add(1, std::sync::atomic::Ordering::AcqRel)
                            == 1
                        {
                            let _ = score_sender.send(index);
                        }
                    }
                });
            }
            // Close the scope's own sender so the scoring workers' receive
            // loops end once the boot and compile workers have retired. Four
            // scoring workers keep the adversarial tail from serializing
            // behind smooth-CSG/repeated-clutter without materially changing
            // memory use: compiled renderers already live in `semantic_slots`
            // and a score owns only its small frame/reference scratch.
            drop(score_sender);
            for _ in 0..4.min(crate::golden::default_jobs()).min(guest_cases.len()) {
                scope.spawn(move || {
                    loop {
                        let received = score_receiver_ref
                            .lock()
                            .unwrap_or_else(|e| e.into_inner())
                            .recv();
                        let Ok(index) = received else {
                            return;
                        };
                        let case = guest_cases[index];
                        let started = std::time::Instant::now();
                        let mut boot_guard = boot_slots_ref[index]
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let semantic_guard = semantic_slots_ref[index]
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        let score = match (boot_guard.as_mut(), semantic_guard.as_ref()) {
                            (Some(Ok(run)), Some(Ok(renderer))) => {
                                match extract_raster_evidence(case, renderer, run) {
                                    Ok(()) => cached_score(case, renderer, run),
                                    // The strict serial pass below re-runs the
                                    // extraction and surfaces this same error
                                    // before any score is consumed.
                                    Err(error) => Err(error),
                                }
                            }
                            // A failed boot or compile fails the run before
                            // scores are consumed; the placeholder is never read.
                            _ => Err(format!("pixels conformance: `{case}` was not scored")),
                        };
                        drop(semantic_guard);
                        drop(boot_guard);
                        println!(
                            "pixels-conformance: scored `{case}` in {:.1}s",
                            started.elapsed().as_secs_f64()
                        );
                        *score_slots_ref[index]
                            .lock()
                            .unwrap_or_else(|e| e.into_inner()) = Some(score);
                    }
                });
            }
        });
        (
            boot_slots
                .into_iter()
                .map(|slot| {
                    slot.into_inner()
                        .unwrap_or_else(|e| e.into_inner())
                        .unwrap_or_else(|| {
                            Err("pixels conformance: instrumented run missing".into())
                        })
                })
                .collect(),
            semantic_slots
                .into_iter()
                .map(|slot| {
                    slot.into_inner()
                        .unwrap_or_else(|e| e.into_inner())
                        .unwrap_or_else(|| {
                            Err("pixels conformance: semantic compile missing".into())
                        })
                })
                .collect(),
            score_slots
                .into_iter()
                .map(|slot| {
                    slot.into_inner()
                        .unwrap_or_else(|e| e.into_inner())
                        .unwrap_or_else(|| Err("pixels conformance: frame score missing".into()))
                })
                .collect(),
        )
    };
    let mut semantic_renderers = Vec::with_capacity(guest_cases.len());
    for renderer in semantic_results {
        semantic_renderers.push(renderer?);
    }
    let mut instrumented_four = None;
    let mut instrumented_one = None;
    let mut evidence_four = None;
    let mut evidence_one = None;
    let mut telemetry_sections = Vec::with_capacity(guest_cases.len());
    let mut worker_baselines = std::collections::BTreeMap::new();
    for (index, (case, run)) in guest_cases
        .iter()
        .copied()
        .zip(instrumented_runs)
        .enumerate()
    {
        let mut run = run?;
        if run.telemetry == [0; 4] {
            return Err(format!(
                "pixels conformance: instrumented `{case}` telemetry is absent"
            ));
        }
        let renderer = &semantic_renderers[index];
        extract_raster_evidence(case, renderer, &mut run)?;
        let evidence_classes = run.frame_dump.raster_evidence.iter().try_fold(
            [0_u64; 4],
            |mut counts, words| {
                let class = usize::try_from(words[2] >> 62)
                    .map_err(|_| "pixels conformance: raster evidence class overflow".to_string())?;
                if !(1..=3).contains(&class) {
                    return Err(format!(
                        "pixels conformance: instrumented `{case}` has invalid raster evidence class {class}"
                    ));
                }
                counts[class] = counts[class]
                    .checked_add(1)
                    .ok_or_else(|| "pixels conformance: raster evidence count overflow".to_string())?;
                Ok(counts)
            },
        )?;
        if evidence_classes[1]
            .checked_add(evidence_classes[3])
            .is_none_or(|regular| regular != run.telemetry_counters[137])
            || evidence_classes[2] != run.telemetry_counters[148]
        {
            return Err(format!(
                "pixels conformance: instrumented `{case}` raster evidence is incomplete: hit={} event={} background={} telemetry_regular={} telemetry_event={}",
                evidence_classes[1],
                evidence_classes[2],
                evidence_classes[3],
                run.telemetry_counters[137],
                run.telemetry_counters[148],
            ));
        }
        worker_baselines.insert(case.to_string(), run.clone());
        // Telemetry decision-inertness (P7.3): the instrumented layout must
        // display exactly the bytes the production layout displayed; the
        // golden transcript pins the production digest.
        if run
            .frame_digest
            .is_some_and(|digest| digest != observations[index].frame_digest)
        {
            return Err(format!(
                "pixels conformance: instrumented `{case}` changed displayed bytes: \
                 production digest {:016x?}, instrumented digest {:016x?}",
                observations[index].frame_digest, run.frame_digest
            ));
        }
        observations[index].run_evidence = run.evidence;
        if case == "boot-pixels-plane" {
            instrumented_four = Some(run.telemetry);
            evidence_four = run.evidence;
        } else if case == "boot-pixels-plane-one-core" {
            instrumented_one = Some(run.telemetry);
            evidence_one = run.evidence;
        }
        if case == "check-pixels-tile-boundary" {
            const WIDTH: usize = 128;
            const HEIGHT: usize = 31;
            if run.frame_dump.bytes.len() != WIDTH * HEIGHT * 4 {
                return Err(format!(
                    "pixels conformance: tile-boundary frame has {} bytes, expected {}",
                    run.frame_dump.bytes.len(),
                    WIDTH * HEIGHT * 4,
                ));
            }
            let mut checked = 0_usize;
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let evidence_class = run.frame_dump.raster_evidence[y * WIDTH + x][2] >> 62;
                    let expected = tile_boundary_event_oracle(x, y)?;
                    if (evidence_class == 2) != expected.is_some() {
                        return Err(format!(
                            "pixels conformance: tile-boundary event partition differs at ({x},{y}): evidence_class={evidence_class}, oracle={expected:?}"
                        ));
                    }
                    let Some(expected) = expected else { continue };
                    checked += 1;
                    let pixel = (y * WIDTH + x) * 4;
                    let bgra: [u8; 4] = run.frame_dump.bytes[pixel..pixel + 4]
                        .try_into()
                        .expect("validated frame extent contains selected event pixel");
                    if bgra != [expected, 0, 0, 255] {
                        return Err(format!(
                            "pixels conformance: exact tile-boundary event byte differs at ({x},{y}): expected BGRA [{expected}, 0, 0, 255], got {bgra:?}"
                        ));
                    }
                }
            }
            if checked != 68 {
                return Err(format!(
                    "pixels conformance: exact tile-boundary oracle checked {checked} event bytes, expected 68"
                ));
            }
        }
        if case == "check-pixels-material-edge" {
            const WIDTH: usize = 64;
            const HEIGHT: usize = 32;
            if run.frame_dump.bytes.len() != WIDTH * HEIGHT * 4 {
                return Err(format!(
                    "pixels conformance: diagonal material frame has {} bytes, expected {}",
                    run.frame_dump.bytes.len(),
                    WIDTH * HEIGHT * 4,
                ));
            }
            let mut checked = 0_usize;
            for y in 0..HEIGHT {
                for x in 0..WIDTH {
                    let evidence_class = run.frame_dump.raster_evidence[y * WIDTH + x][2] >> 62;
                    let expected = diagonal_material_event_oracle(x, y)?;
                    if (evidence_class == 2) != expected.is_some() {
                        return Err(format!(
                            "pixels conformance: diagonal event partition differs at ({x},{y}): evidence_class={evidence_class}, oracle={expected:?}"
                        ));
                    }
                    let Some(expected) = expected else { continue };
                    checked += 1;
                    let pixel = (y * WIDTH + x) * 4;
                    let bgra: [u8; 4] = run.frame_dump.bytes[pixel..pixel + 4]
                        .try_into()
                        .expect("validated frame extent contains selected diagonal event pixel");
                    if bgra != [expected, 0, 0, 255] {
                        return Err(format!(
                            "pixels conformance: exact diagonal event byte differs at ({x},{y}): expected BGRA [{expected}, 0, 0, 255], got {bgra:?}"
                        ));
                    }
                }
            }
            if checked != 64 {
                return Err(format!(
                    "pixels conformance: exact diagonal oracle checked {checked} event bytes, expected 64"
                ));
            }
        }
        if case == "check-pixels-simultaneous-event" {
            let simultaneous = run
                .frame_dump
                .raster_evidence
                .iter()
                .any(|evidence| evidence[2] >> 62 == 2 && ((evidence[1] >> 32) & 0xffff) >= 2);
            if !simultaneous {
                return Err(
                    "pixels conformance: simultaneous-event guest frame has no EventPixel referencing two sealed events"
                        .to_string(),
                );
            }
        }
        observations[index].frame_dump = Some(run.frame_dump);
        telemetry_sections.push(run.telemetry_counters);
    }
    // The plane pair, the adversarial class census, and the archived report are
    // whole-corpus facts. A case subset still runs every per-scene guest and
    // oracle assertion, but it cannot state them, so they are skipped rather
    // than failed.
    let whole_corpus = only.is_none();
    let instrumented_four = match instrumented_four {
        Some(value) => value,
        None if !whole_corpus => [0; 4],
        None => return Err("pixels conformance: four-core evidence missing".to_string()),
    };
    let instrumented_one = match instrumented_one {
        Some(value) => value,
        None if !whole_corpus => [0; 4],
        None => return Err("pixels conformance: one-core evidence missing".to_string()),
    };
    let evidence_four = match evidence_four {
        Some(value) => value,
        None if !whole_corpus => [0; 16],
        None => return Err("pixels conformance: four-core run missing".to_string()),
    };
    let evidence_one = match evidence_one {
        Some(value) => value,
        None if !whole_corpus => [0; 16],
        None => return Err("pixels conformance: one-core run missing".to_string()),
    };
    if whole_corpus && (instrumented_four == [0; 4] || instrumented_four != instrumented_one) {
        return Err(format!(
            "pixels conformance: instrumented one/four-worker telemetry differs or is absent: \
             one={instrumented_one:?} four={instrumented_four:?}"
        ));
    }
    if evidence_four != evidence_one {
        let word = evidence_four
            .iter()
            .zip(evidence_one)
            .position(|(four, one)| *four != one)
            .unwrap_or(0);
        return Err(format!(
            "pixels conformance: instrumented one/four-worker proof evidence differs at word \
             {word}: one={:016x} four={:016x}",
            evidence_one[word], evidence_four[word],
        ));
    }
    if whole_corpus && telemetry_sections[0] != telemetry_sections[1] {
        let counter = telemetry_sections[0]
            .iter()
            .zip(&telemetry_sections[1])
            .position(|(four, one)| four != one)
            .unwrap_or(0);
        return Err(format!(
            "pixels conformance: one/four-worker CertificateTelemetry sections differ at \
             counter {counter}: four={} one={}",
            telemetry_sections[0][counter], telemetry_sections[1][counter]
        ));
    }
    for (case, renderer) in guest_cases.iter().zip(&semantic_renderers) {
        if renderer
            .program
            .program()
            .record_count(wrela_machine::pixels::FrameProgramTableKindV1::Kinetic)
            != 0
        {
            return Err(format!(
                "pixels conformance: `{case}` enabled kinetic mode in the locked P8 workload"
            ));
        }
    }
    let density_class = |case: &str| match case {
        "boot-pixels-plane" | "boot-pixels-plane-one-core" => "control-plane",
        "check-pixels-hard-csg" => "control-hard-csg",
        "check-pixels-smooth-csg" => "smooth-support-overlap",
        "check-pixels-repeat" => "repeated-clutter",
        "check-pixels-displace" => "mixed-near-far-scale",
        "check-pixels-close-depth" | "check-pixels-simultaneous-event" => "close-sheets-depth-swap",
        "check-pixels-tangent" => "grazing-near-tangency",
        "check-pixels-thin-feature" => "control-thin-feature",
        "check-pixels-tile-boundary" => "tile-boundary-ownership",
        "check-pixels-enclosed-feature" => "control-enclosed-feature",
        "check-pixels-material-edge" => "control-material-edge",
        "check-pixels-camera-inside" => "mixed-near-far-scale",
        _ => "unclassified",
    };
    let required_adversarial = [
        "grazing-near-tangency",
        "smooth-support-overlap",
        "close-sheets-depth-swap",
        "mixed-near-far-scale",
        "repeated-clutter",
        "tile-boundary-ownership",
    ];
    for required in required_adversarial {
        let mut covered = false;
        for (case, counters) in guest_cases.iter().zip(&telemetry_sections) {
            if density_class(case) != required {
                continue;
            }
            covered = true;
            let event_activity: u64 = counters[48..55].iter().sum();
            let predicate_activity: u64 = counters[56..63].iter().sum();
            if event_activity == 0 || predicate_activity == 0 {
                return Err(format!(
                    "pixels conformance: density class `{required}` fixture `{case}` has no intended event/predicate activity"
                ));
            }
        }
        if !covered && whole_corpus {
            return Err(format!(
                "pixels conformance: adversarial density class `{required}` is absent"
            ));
        }
    }
    let mut telemetry_archive = String::new();
    for (case, counters) in guest_cases.iter().zip(&telemetry_sections) {
        let run_endings: u64 = counters[15..23].iter().sum();
        let margin_owners: u64 = counters[23..31].iter().sum();
        if run_endings != margin_owners {
            return Err(format!(
                "pixels conformance: `{case}` has an unclassified run ending or margin owner"
            ));
        }
        use std::fmt::Write as _;
        writeln!(
            telemetry_archive,
            "CertificateTelemetry version=2 case={case} density_class={} counters={}",
            density_class(case),
            counters
                .iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .join(",")
        )
        .expect("String writes cannot fail");
        let packet_pixels = counters[142]
            .checked_mul(4)
            .and_then(|pixels| pixels.checked_add(counters[143]))
            .ok_or_else(|| {
                format!("pixels conformance: `{case}` raster-path pixel count overflow")
            })?;
        let classified_regular = counters[144].checked_add(counters[147]).ok_or_else(|| {
            format!("pixels conformance: `{case}` raster classification count overflow")
        })?;
        if packet_pixels != counters[137]
            || classified_regular != counters[137]
            || counters[145] > counters[144]
            || counters[146] != 0
            || counters[148] != counters[138]
            || counters[149] != 0
        {
            return Err(format!(
                "pixels conformance: `{case}` incomplete production raster evidence: \
                 regular={} packet_pixels={packet_pixels} q={} normal={} position={} \
                 background={} corridor={} event={} failures={}",
                counters[137],
                counters[144],
                counters[145],
                counters[146],
                counters[147],
                counters[138],
                counters[148],
                counters[149],
            ));
        }
        writeln!(
            telemetry_archive,
            "RasterEvidence case={case} packets={} scalar_edges={} q_checked={} normal_checked={} world_positions={} background={} event_pixels={} failures=0 status=pass",
            counters[142],
            counters[143],
            counters[144],
            counters[145],
            counters[146],
            counters[147],
            counters[148],
        )
        .expect("String writes cannot fail");
    }
    // Every adversarial density class is executed again with four workers.
    // The ordinary fixtures use the one-worker Image default. Building temporary
    // variants under `tests/golden` preserves the compiler-internal fixture
    // trust boundary while avoiding a second maintained copy of each scene.
    struct VariantTree(std::path::PathBuf);
    impl Drop for VariantTree {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }
    // A killed run cannot execute `VariantTree`'s drop, so sweep any tree an
    // earlier run left behind. `golden_case_dirs` enumerates every directory
    // under `tests/golden` without filtering dotfiles, so a leaked tree is one
    // stray `expected/` away from being collected as a real golden case.
    const VARIANT_PREFIX: &str = ".p8-four-core-";
    for stale in std::fs::read_dir(root().join("tests/golden"))
        .map_err(|error| format!("pixels conformance: scan tests/golden: {error}"))?
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_dir()
                && path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .is_some_and(|name| name.starts_with(VARIANT_PREFIX))
        })
    {
        std::fs::remove_dir_all(&stale).map_err(|error| {
            format!(
                "pixels conformance: remove stale four-core variant tree {}: {error}",
                stale.display()
            )
        })?;
    }
    let variant_root = root()
        .join("tests/golden")
        .join(format!("{VARIANT_PREFIX}{}", std::process::id()));
    std::fs::create_dir_all(&variant_root).map_err(|error| {
        format!(
            "pixels conformance: create four-core variant tree {}: {error}",
            variant_root.display()
        )
    })?;
    let _variant_tree = VariantTree(variant_root.clone());
    // One/four-worker invariance is a P8.11 criterion, but each class re-boots a
    // scene that has already been rendered and scored purely to compare it
    // against itself, and those boots are minutes apiece. The property splits
    // into two halves that are covered separately:
    //
    //   * Tile partitioning across workers is covered permanently and cheaply by
    //     the boot goldens. `boot-pixels-partial-mode` (cores=1) and
    //     `boot-pixels-partial-mode-four-core` (cores=4) render a 65x33 mode —
    //     four scanout tiles — to the same visible digest in under a second.
    //     That is strictly better partition coverage than any scene here: at
    //     64x32 a frame is a single tile, so only worker zero receives work and
    //     a four-core variant of those scenes proves almost nothing about
    //     partitioning.
    //   * Telemetry invariance is what these variants uniquely add, and one
    //     scene demonstrates it. The fast lane therefore keeps the cheapest
    //     adversarial class (`check-pixels-tangent`, among the lowest per-pixel
    //     fallback counts in the corpus) rather than the most expensive
    //     (`check-pixels-tile-boundary`, the highest, at twice the pixels).
    //
    // `verify-deep` still runs every class.
    const FAST_WORKER_PAIRS: [(&str, &str); 1] =
        [("grazing-near-tangency", "check-pixels-tangent")];
    const DEEP_WORKER_PAIRS: [(&str, &str); 5] = [
        ("smooth-support-overlap", "check-pixels-smooth-csg"),
        ("close-sheets-depth-swap", "check-pixels-simultaneous-event"),
        ("mixed-near-far-scale", "check-pixels-displace"),
        ("repeated-clutter", "check-pixels-repeat"),
        ("tile-boundary-ownership", "check-pixels-tile-boundary"),
    ];
    let worker_pairs: Vec<(&str, &str)> = FAST_WORKER_PAIRS
        .into_iter()
        .chain(
            DEEP_WORKER_PAIRS
                .into_iter()
                .filter(|_| deep_worker_variants),
        )
        .filter(|(_, case)| guest_cases.contains(case))
        .collect();
    let mut variant_runs = Vec::with_capacity(worker_pairs.len());
    for (class, case) in worker_pairs {
        if density_class(case) != class {
            return Err(format!(
                "pixels conformance: worker-pair `{case}` changed density class from `{class}`"
            ));
        }
        let root_file = root().join("tests/golden").join(case).join("root");
        let relative = std::fs::read_to_string(&root_file).map_err(|error| {
            format!("pixels conformance: read {}: {error}", root_file.display())
        })?;
        let source = root().join("tests/golden").join(case).join(relative.trim());
        let text = std::fs::read_to_string(&source)
            .map_err(|error| format!("pixels conformance: read {}: {error}", source.display()))?;
        const DEFAULT_WORKER_IMAGE: &str = "target=Target.wrela_machine_v1)";
        if text.matches(DEFAULT_WORKER_IMAGE).count() != 1 {
            return Err(format!(
                "pixels conformance: `{case}` no longer has one default one-worker Image declaration"
            ));
        }
        // The runtime harness places test roots round-robin. Keep only the
        // instrumented render test so its aggregate-reply await remains on
        // core 0; the permanent one-core golden already verifies the removed
        // compile-time pin assertions verbatim.
        let mut four_core = text.clone();
        if case != "check-pixels-tile-boundary" {
            let pinned_start = four_core
                .find("@test(runtime)\nfn pinned_scene_contract():")
                .ok_or_else(|| {
                    format!("pixels conformance: `{case}` omitted its pinned scene contract")
                })?;
            let pinned_end = four_core[pinned_start..]
                .find("\n\n@")
                .map(|offset| pinned_start + offset + 2)
                .ok_or_else(|| {
                    format!("pixels conformance: `{case}` pin test has no following declaration")
                })?;
            four_core.replace_range(pinned_start..pinned_end, "");
        }
        let four_core = four_core.replace(
            DEFAULT_WORKER_IMAGE,
            "target=Target.wrela_machine_v1, cores=4)",
        );
        const DEFAULT_DISPLAY: &str = "refresh_hz=60)";
        if four_core.matches(DEFAULT_DISPLAY).count() != 1 {
            return Err(format!(
                "pixels conformance: `{case}` no longer has one default Display driver declaration"
            ));
        }
        let four_core = four_core.replace(DEFAULT_DISPLAY, "refresh_hz=60, core=0)");
        let four_core = if four_core.contains("img.actor(") {
            if four_core.matches("mailbox=1)").count() != 1 {
                return Err(format!(
                    "pixels conformance: `{case}` probe actor declaration changed"
                ));
            }
            four_core.replace("mailbox=1)", "mailbox=1, core=0)")
        } else {
            four_core
        };
        let destination = variant_root.join(case).join(relative.trim());
        let parent = destination.parent().ok_or_else(|| {
            format!(
                "pixels conformance: four-core destination {} has no parent",
                destination.display()
            )
        })?;
        std::fs::create_dir_all(parent)
            .map_err(|error| format!("pixels conformance: create {}: {error}", parent.display()))?;
        std::fs::write(&destination, four_core).map_err(|error| {
            format!(
                "pixels conformance: write four-core variant {}: {error}",
                destination.display()
            )
        })?;
        variant_runs.push((class, case, destination));
    }
    // The four-core variant boots are independent per case; two concurrent
    // guests match the HVF throttle.
    let four_runs: Vec<Result<InstrumentedRun, String>> = {
        let slots: Vec<std::sync::Mutex<Option<Result<InstrumentedRun, String>>>> = variant_runs
            .iter()
            .map(|_| std::sync::Mutex::new(None))
            .collect();
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let instrumented = &instrumented;
        let slots_ref = &slots;
        let cursor_ref = &cursor;
        let variant_runs_ref = &variant_runs;
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(move || {
                    loop {
                        let index = cursor_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some((_, case, destination)) = variant_runs_ref.get(index) else {
                            return;
                        };
                        let run = instrumented(case, Some(destination.as_path()));
                        *slots_ref[index].lock().unwrap_or_else(|e| e.into_inner()) = Some(run);
                    }
                });
            }
        });
        slots
            .into_iter()
            .map(|slot| {
                slot.into_inner()
                    .unwrap_or_else(|e| e.into_inner())
                    .unwrap_or_else(|| Err("pixels conformance: four-core run missing".into()))
            })
            .collect()
    };
    for ((class, case, _), four) in variant_runs.iter().zip(four_runs) {
        let (class, case) = (*class, *case);
        let four = four?;
        let one = worker_baselines.get(case).ok_or_else(|| {
            format!("pixels conformance: one-worker baseline for `{case}` is absent")
        })?;
        if one.frame_digest != four.frame_digest
            || one.frame_dump.bytes != four.frame_dump.bytes
            || one.telemetry != four.telemetry
            || one.telemetry_counters != four.telemetry_counters
        {
            let byte = one
                .frame_dump
                .bytes
                .iter()
                .zip(&four.frame_dump.bytes)
                .position(|(one, four)| one != four);
            let counter = one
                .telemetry_counters
                .iter()
                .zip(&four.telemetry_counters)
                .position(|(one, four)| one != four);
            return Err(format!(
                "pixels conformance: `{class}` one/four-worker output or telemetry differs: \
                 frame_digest={} byte={byte:?} telemetry={} counter={counter:?}",
                one.frame_digest != four.frame_digest,
                one.telemetry != four.telemetry,
            ));
        }
        use std::fmt::Write as _;
        // Only the fast-lane classes enter the archived report, so the recorded
        // truth is identical whether or not the deep classes ran. A deep-only
        // divergence still fails the lane through the comparison above.
        if FAST_WORKER_PAIRS.iter().any(|(fast, _)| *fast == class) {
            writeln!(
                telemetry_archive,
                "WorkerInvariant density_class={class} one=1 four=4 bytes={} telemetry=identical status=pass",
                one.frame_dump.bytes.len(),
            )
            .expect("String writes cannot fail");
        }
    }
    {
        use std::fmt::Write as _;
        writeln!(
            telemetry_archive,
            "WorkerInvariantScope fast={} deferred={} deferred_lane=verify-deep",
            FAST_WORKER_PAIRS.len(),
            DEEP_WORKER_PAIRS.len(),
        )
        .expect("String writes cannot fail");
    }
    let scored = wrela_compiler::pixels::reference::conformance::run_scored(
        &observations,
        &semantic_renderers,
        frame_scores,
    )?;
    if !whole_corpus {
        // Every guest, oracle, and worker assertion for the selected scenes has
        // already run and passed to reach here; only the corpus-wide record is
        // out of scope for a subset.
        println!(
            "pixels-conformance: {} case(s) verified against the semantic oracle \
             (subset probe — {EXPECTED} is only compared by a whole-corpus run)\n{scored}",
            guest_cases.len(),
        );
        return Ok(());
    }
    let actual = format!(
        "GuestVisibilityExecution version=2 cases={} kinetic=false telemetry={:016x}{:016x}{:016x}{:016x} status=pass\n{telemetry_archive}{scored}",
        guest_cases.len(),
        instrumented_four[0],
        instrumented_four[1],
        instrumented_four[2],
        instrumented_four[3],
    );
    write_actual_report(&actual)?;
    let path = root().join(EXPECTED);
    let expected = std::fs::read_to_string(&path)
        .map_err(|error| format!("pixels conformance: read {}: {error}", path.display()))?;
    if actual != expected {
        if update {
            std::fs::write(&path, &actual).map_err(|error| {
                format!("pixels conformance: write {}: {error}", path.display())
            })?;
            println!("pixels-conformance: updated {EXPECTED}");
            return Ok(());
        }
        return Err(format!(
            "pixels conformance: deterministic score report differs from {EXPECTED}\n\
             --- expected\n{expected}--- actual\n{actual}"
        ));
    }
    print!("{actual}");
    Ok(())
}

#[cfg(test)]
mod tests {

    use wrela_compiler::pixels::reference::conformance::FrameScore;

    #[test]
    fn raw_boot_cache_migration_requires_a_complete_generation() {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let root = std::env::temp_dir().join(format!(
            "wrela-raw-boot-migration-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
        ));
        let current_dir = root.join("current");
        let legacy_dir = root.join("legacy");
        std::fs::create_dir_all(&current_dir).expect("current dir");
        std::fs::create_dir_all(&legacy_dir).expect("legacy dir");
        let paths = |dir: &std::path::Path| ["stdout", "frame", "state"].map(|kind| dir.join(kind));
        let current = paths(&current_dir);
        let legacy = paths(&legacy_dir);
        for (index, path) in legacy.iter().enumerate() {
            std::fs::write(path, [index as u8]).expect("legacy artifact");
        }

        let (blobs, promote) = super::read_raw_boot_cache(&current, Some(&legacy));
        assert!(promote);
        assert_eq!(blobs.expect("complete legacy generation").len(), 3);

        std::fs::remove_file(&legacy[2]).expect("truncate generation");
        let (blobs, promote) = super::read_raw_boot_cache(&current, Some(&legacy));
        assert!(blobs.is_none(), "partial generations are always a miss");
        assert!(!promote);

        for (index, path) in current.iter().enumerate() {
            std::fs::write(path, [9 + index as u8]).expect("current artifact");
        }
        let (blobs, promote) = super::read_raw_boot_cache(&current, Some(&legacy));
        assert!(!promote, "the current generation wins");
        assert_eq!(blobs.expect("current generation")[0], vec![9]);
        std::fs::remove_dir_all(root).expect("remove fixture");
    }

    fn sample_score() -> FrameScore {
        FrameScore {
            checked_interior: 11,
            interior_mismatches: 0,
            edge_center_violations: 1,
            ambiguous_identity: 2,
            unresolved: 3,
            skipped_unproven: 4,
            phantom_surface: 5,
            q_checked: 6,
            normal_checked: 7,
            event_bytes_checked: 8,
            boundary_limited_event_bytes: 9,
            raster_evidence_failures: 10,
            first_issue: Some([12, 13, 4]),
        }
    }

    /// Every field survives the cache body, in both directions.
    ///
    /// A field added to `FrameScore` without a line in `render_score` would
    /// silently read back as its default from a cached hit — a wrong result,
    /// which is the one thing a cache may never produce. The field-count
    /// assertion in `parse_score` plus this round trip catch that.
    #[test]
    fn score_cache_body_round_trips_every_field() {
        let score = sample_score();
        let body = super::render_score(&score);
        assert_eq!(super::parse_score(&body), Some(score));

        // No field is a no-op: perturbing each one changes the body.
        let mut fields = 0usize;
        for line in body.lines() {
            fields += 1;
            let (name, _) = line.split_once(' ').expect("field line");
            assert!(
                body.matches(&format!("\n{name} ")).count() + usize::from(body.starts_with(name))
                    == 1,
                "field `{name}` appears more than once"
            );
        }
        assert_eq!(fields, 13, "every FrameScore field is serialized");

        let mut without_issue = score;
        without_issue.first_issue = None;
        let body = super::render_score(&without_issue);
        assert_eq!(super::parse_score(&body), Some(without_issue));
    }

    #[test]
    fn a_malformed_or_short_score_body_is_a_miss_rather_than_a_default() {
        assert_eq!(super::parse_score(""), None, "an empty body is a miss");
        assert_eq!(
            super::parse_score("checked_interior 11\n"),
            None,
            "a truncated body must not read back as mostly-default"
        );
        let body = super::render_score(&sample_score());
        assert_eq!(
            super::parse_score(&body.replace("unresolved 3", "unresolved x")),
            None,
            "a non-numeric field is a miss"
        );
        assert_eq!(
            super::parse_score(&body.replace("unresolved 3", "unknown_field 3")),
            None,
            "an unknown field is a miss"
        );
        assert_eq!(
            super::parse_score(&body.replace("first_issue 12,13,4", "first_issue 12,13")),
            None,
            "a short first_issue is a miss"
        );
    }

    #[test]
    fn only_passing_scores_are_cacheable() {
        let mut passing = FrameScore::default();
        passing.checked_interior = 1;
        assert!(super::score_passes(&passing));
        for mutate in [
            |score: &mut FrameScore| score.interior_mismatches = 1,
            |score: &mut FrameScore| score.edge_center_violations = 1,
            |score: &mut FrameScore| score.unresolved = 1,
            |score: &mut FrameScore| score.skipped_unproven = 1,
            |score: &mut FrameScore| score.phantom_surface = 1,
            |score: &mut FrameScore| score.raster_evidence_failures = 1,
        ] {
            let mut failed = passing;
            mutate(&mut failed);
            assert!(!super::score_passes(&failed));
        }
    }

    /// Each key component, perturbed, invalidates exactly its dependents.
    ///
    /// The compile-closure key must not depend on the evidence or the scorer;
    /// the score key must depend on both, and on neither the VMM binary.
    #[test]
    fn the_key_perturbation_matrix_holds() {
        let compile = |sources: &str, compiler: &str, options: &str| {
            super::compile_cache_key(sources, compiler, options)
        };
        let score = |case: &str,
                     fixture_version: u32,
                     sources: &str,
                     compiler: &str,
                     options: &str,
                     evidence: &str,
                     scorer: &str,
                     scorer_options: &str,
                     numeric_contract: u32| {
            super::score_cache_key(&super::ScoreKey {
                case,
                fixture_version,
                sources,
                compiler,
                options,
                evidence,
                scorer,
                scorer_options,
                numeric_contract,
            })
        };
        let boot = |image: &str, vmm: &str| super::boot_cache_key(image, vmm);
        let compile_base = compile("s", "c", "o");
        let score_base = score("k", 2, "s", "c", "o", "e", "r", "so", 2);
        let boot_base = boot("i", "v");

        // (component, compile changes, score changes, boot changes)
        let matrix = [
            (
                "sources",
                compile("s!", "c", "o") != compile_base,
                score("k", 2, "s!", "c", "o", "e", "r", "so", 2) != score_base,
                false,
            ),
            (
                "compiler",
                compile("s", "c!", "o") != compile_base,
                score("k", 2, "s", "c!", "o", "e", "r", "so", 2) != score_base,
                false,
            ),
            (
                "compiler options",
                compile("s", "c", "o!") != compile_base,
                score("k", 2, "s", "c", "o!", "e", "r", "so", 2) != score_base,
                false,
            ),
            (
                "case",
                false,
                score("k!", 2, "s", "c", "o", "e", "r", "so", 2) != score_base,
                false,
            ),
            (
                "fixture version",
                false,
                score("k", 3, "s", "c", "o", "e", "r", "so", 2) != score_base,
                false,
            ),
            (
                "evidence",
                false,
                score("k", 2, "s", "c", "o", "e!", "r", "so", 2) != score_base,
                false,
            ),
            (
                "scorer",
                false,
                score("k", 2, "s", "c", "o", "e", "r!", "so", 2) != score_base,
                false,
            ),
            (
                "scorer options",
                false,
                score("k", 2, "s", "c", "o", "e", "r", "so!", 2) != score_base,
                false,
            ),
            (
                "numeric contract",
                false,
                score("k", 2, "s", "c", "o", "e", "r", "so", 3) != score_base,
                false,
            ),
            ("image", false, false, boot("i!", "v") != boot_base),
            ("vmm", false, false, boot("i", "v!") != boot_base),
        ];
        for (component, compile_changed, score_changed, boot_changed) in matrix {
            let expected = match component {
                "sources" | "compiler" | "compiler options" => (true, true, false),
                "case" | "fixture version" | "evidence" | "scorer" | "scorer options"
                | "numeric contract" => (false, true, false),
                "image" | "vmm" => (false, false, true),
                _ => unreachable!(),
            };
            assert_eq!(
                (compile_changed, score_changed, boot_changed),
                expected,
                "`{component}` must invalidate exactly its dependent caches"
            );
        }
    }

    #[test]
    fn the_scorer_fingerprint_covers_the_reference_and_the_truth_schema() {
        let fingerprint = super::scorer_fingerprint().expect("scorer fingerprint");
        assert_eq!(fingerprint.len(), 64, "a sha256 hex digest");
        assert_eq!(
            fingerprint,
            super::scorer_fingerprint().expect("stable"),
            "the fingerprint must be deterministic"
        );
        assert_eq!(super::TRUTH_SCHEMA_VERSION, 2);
    }

    #[test]
    fn boot_workers_default_to_two_and_refuse_nonsense() {
        // SAFETY: this test owns the variable for its duration and clears it.
        unsafe { std::env::remove_var("WRELA_P8_BOOT_WORKERS") };
        assert_eq!(super::boot_workers(), 2);
        unsafe { std::env::set_var("WRELA_P8_BOOT_WORKERS", "3") };
        assert_eq!(super::boot_workers(), 3, "the third worker is opt-in");
        unsafe { std::env::set_var("WRELA_P8_BOOT_WORKERS", "9") };
        assert_eq!(super::boot_workers(), 2, "out-of-range falls back to two");
        unsafe { std::env::set_var("WRELA_P8_BOOT_WORKERS", "lots") };
        assert_eq!(super::boot_workers(), 2, "nonsense falls back to two");
        unsafe { std::env::remove_var("WRELA_P8_BOOT_WORKERS") };
    }

    #[test]
    fn tile_boundary_oracle_is_exact_and_high_contrast() {
        assert_eq!(super::exact_axis_coverage_oracle(0, -256, 207), Ok(207));
        assert_eq!(super::exact_axis_coverage_oracle(-256, 0, 225), Ok(225));
        assert_eq!(super::tile_boundary_event_oracle(50, 13), Ok(Some(183)));
        assert_eq!(super::tile_boundary_event_oracle(63, 13), Ok(Some(207)));
        assert_eq!(super::tile_boundary_event_oracle(64, 13), Ok(Some(207)));
        assert_eq!(super::tile_boundary_event_oracle(50, 14), Ok(Some(225)));
        assert_eq!(super::tile_boundary_event_oracle(52, 14), Ok(None));
    }

    #[test]
    fn diagonal_material_oracle_pins_two_lanes_and_half_coverage() {
        assert_eq!(super::diagonal_material_event_oracle(15, 0), Ok(None));
        assert_eq!(super::diagonal_material_event_oracle(16, 0), Ok(Some(200)));
        assert_eq!(super::diagonal_material_event_oracle(17, 0), Ok(Some(240)));
        assert_eq!(super::diagonal_material_event_oracle(47, 31), Ok(Some(200)));
        assert_eq!(super::diagonal_material_event_oracle(48, 31), Ok(Some(240)));
    }
}
