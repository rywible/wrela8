use std::process::Command;

use crate::golden::{BootSel, GoldenOpts, build_and_sign_vmm, golden};
use crate::root;

const EXPECTED: &str = "tests/pixels_truth/p7-visibility.txt";
const GUEST_FRAME_DIGEST_MARKER: u64 = 4_922_225_244_575_680_596;
const GUEST_ALPHA_SAMPLE_MARKER: u64 = 5_780_180_186_688_408_645;
const GUEST_CERTIFIED_RUN_MARKER: u64 = 4_847_371_096_046_259_761;
const GUEST_FRAME_DUMP_MARKER: u64 = 7_599_824_371_187_933_777;

pub fn pixels_conformance() -> Result<(), String> {
    let guest_cases = [
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
        "check-pixels-torus-roots",
    ];
    const PROBE_CASE: &str = "check-pixels-visibility-probe";
    // One parallel golden pass over the whole case set: images compile on
    // every core and guest boots share the throttled VMM slots, instead of
    // one fully serial `golden()` invocation per case.
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
    let mut semantic_renderers = Vec::with_capacity(guest_cases.len());
    for case in guest_cases {
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
            .rposition(|value| *value == GUEST_ALPHA_SAMPLE_MARKER)
            .ok_or_else(|| format!("pixels conformance: `{case}` omitted alpha sample marker"))?;
        let alpha_word = *values
            .get(alpha_marker + 1)
            .ok_or_else(|| format!("pixels conformance: `{case}` truncated alpha samples"))?;
        let alpha_samples = [
            (alpha_word & 255) as u8,
            ((alpha_word >> 8) & 255) as u8,
            ((alpha_word >> 16) & 255) as u8,
        ];
        let frame_digest = values
            .iter()
            .rposition(|value| *value == GUEST_FRAME_DIGEST_MARKER)
            .ok_or_else(|| format!("pixels conformance: `{case}` omitted frame digest marker"))
            .and_then(|marker| {
                values
                    .get(marker + 1..marker + 5)
                    .ok_or_else(|| format!("pixels conformance: `{case}` truncated frame digest"))?
                    .try_into()
                    .map_err(|_| "pixels conformance: frame digest width changed".to_string())
            })?;
        let visibility_probe = if case == "boot-pixels-plane" {
            Some(guest_visibility_probe)
        } else {
            None
        };
        observations.push(
            wrela_compiler::pixels::reference::conformance::GuestObservation {
                case: case.to_string(),
                certificate_runs: values[0],
                event_corridors: values[1],
                revalidated_proposals: values[2],
                frame_digest,
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
        let programs = wrela_compiler::cost::stage::load_pixels_programs(&target)
            .map_err(|error| format!("pixels conformance: compile semantic `{case}`: {error}"))?;
        let renderer = programs
            .compiled_renderers
            .into_iter()
            .next()
            .ok_or_else(|| format!("pixels conformance: `{case}` has no sealed renderer"))?;
        semantic_renderers.push(renderer);
    }
    let decode_frame_dump = |payload: &[u64]| -> Result<
        wrela_compiler::pixels::reference::conformance::FrameDump,
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
        let frame_words = usize::try_from(payload[14])
            .map_err(|_| "pixels conformance: frame dump word count overflow".to_string())?;
        let frame = payload
            .get(15..15 + frame_words)
            .ok_or_else(|| "pixels conformance: frame dump words are truncated".to_string())?;
        if payload.len() != 15 + frame_words {
            return Err(format!(
                "pixels conformance: frame dump carries {} words, expected {}",
                payload.len(),
                15 + frame_words
            ));
        }
        let mut bytes = Vec::with_capacity(frame_words * 8);
        for word in frame {
            bytes.extend_from_slice(&word.to_le_bytes());
        }
        Ok(wrela_compiler::pixels::reference::conformance::FrameDump {
            camera,
            params,
            bytes,
        })
    };
    struct InstrumentedRun {
        telemetry: [u64; 4],
        evidence: [u64; 16],
        frame_digest: [u64; 4],
        frame_dump: wrela_compiler::pixels::reference::conformance::FrameDump,
    }
    let instrumented = |case: &str| -> Result<InstrumentedRun, String> {
        let vmm = build_and_sign_vmm()?;
        let wrela = root().join("target/debug/wrela");
        let source = root().join("tests/golden").join(case).join("root");
        let target = if source.is_file() {
            let relative = std::fs::read_to_string(&source).map_err(|error| {
                format!("pixels conformance: read {}: {error}", source.display())
            })?;
            root().join("tests/golden").join(case).join(relative.trim())
        } else {
            root().join("tests/golden").join(case).join("input.wr")
        };
        let output = Command::new(&wrela)
            .current_dir(root())
            .arg("test")
            .arg(&target)
            .arg("--pixels-telemetry")
            .arg("--vmm")
            .arg(vmm)
            .output()
            .map_err(|error| format!("pixels conformance: run instrumented `{case}`: {error}"))?;
        if !output.status.success() {
            return Err(format!(
                "pixels conformance: instrumented `{case}` failed:\n{}{}",
                String::from_utf8_lossy(&output.stdout),
                String::from_utf8_lossy(&output.stderr),
            ));
        }
        let values = String::from_utf8_lossy(&output.stdout)
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
        let frame_dump = decode_frame_dump(&values[dump_marker + 1..])?;
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
            .and_then(|marker| values.get(marker + 1..marker + 5))
            .ok_or_else(|| {
                format!("pixels conformance: instrumented `{case}` omitted its frame digest")
            })?
            .try_into()
            .map_err(|_| "pixels conformance: frame digest width changed".to_string())?;
        let evidence_marker = values
            .iter()
            .position(|value| *value == GUEST_CERTIFIED_RUN_MARKER)
            .ok_or_else(|| {
                format!("pixels conformance: instrumented `{case}` omitted run evidence")
            })?;
        let evidence = values
            .get(evidence_marker + 1..evidence_marker + 17)
            .ok_or_else(|| {
                format!("pixels conformance: instrumented `{case}` truncated run evidence")
            })?
            .try_into()
            .map_err(|_| "pixels conformance: run evidence width changed".to_string())?;
        wrela_compiler::pixels::reference::sweep::decode_certified_run_record(evidence).map_err(
            |error| format!("pixels conformance: instrumented `{case}` run evidence: {error:?}"),
        )?;
        Ok(InstrumentedRun {
            telemetry,
            evidence,
            frame_digest,
            frame_dump,
        })
    };
    // Instrumented runs are independent per case: run them through a small
    // worker pool. Two concurrent guests match the golden runner's HVF
    // throttle (more intermittently starves multicore quiescence).
    let instrumented_runs: Vec<Result<InstrumentedRun, String>> = {
        let slots: Vec<std::sync::Mutex<Option<Result<InstrumentedRun, String>>>> = guest_cases
            .iter()
            .map(|_| std::sync::Mutex::new(None))
            .collect();
        let cursor = std::sync::atomic::AtomicUsize::new(0);
        let instrumented = &instrumented;
        let slots_ref = &slots;
        let cursor_ref = &cursor;
        std::thread::scope(|scope| {
            for _ in 0..2 {
                scope.spawn(move || {
                    loop {
                        let index = cursor_ref.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                        let Some(&case) = guest_cases.get(index) else {
                            return;
                        };
                        let run = instrumented(case);
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
                    .unwrap_or_else(|| Err("pixels conformance: instrumented run missing".into()))
            })
            .collect()
    };
    let mut instrumented_four = None;
    let mut instrumented_one = None;
    let mut evidence_four = None;
    let mut evidence_one = None;
    for (index, (case, run)) in guest_cases
        .iter()
        .copied()
        .zip(instrumented_runs)
        .enumerate()
    {
        let run = run?;
        if run.telemetry == [0; 4] {
            return Err(format!(
                "pixels conformance: instrumented `{case}` telemetry is absent"
            ));
        }
        // Telemetry decision-inertness (P7.3): the instrumented layout must
        // display exactly the bytes the production layout displayed; the
        // golden transcript pins the production digest.
        if run.frame_digest != observations[index].frame_digest {
            return Err(format!(
                "pixels conformance: instrumented `{case}` changed displayed bytes: \
                 production digest {:016x?}, instrumented digest {:016x?}",
                observations[index].frame_digest, run.frame_digest
            ));
        }
        observations[index].run_evidence = Some(run.evidence);
        if case == "boot-pixels-plane" {
            instrumented_four = Some(run.telemetry);
            evidence_four = Some(run.evidence);
        } else if case == "boot-pixels-plane-one-core" {
            instrumented_one = Some(run.telemetry);
            evidence_one = Some(run.evidence);
        }
        observations[index].frame_dump = Some(run.frame_dump);
    }
    let instrumented_four = instrumented_four
        .ok_or_else(|| "pixels conformance: four-core evidence missing".to_string())?;
    let instrumented_one = instrumented_one
        .ok_or_else(|| "pixels conformance: one-core evidence missing".to_string())?;
    let evidence_four =
        evidence_four.ok_or_else(|| "pixels conformance: four-core run missing".to_string())?;
    let evidence_one =
        evidence_one.ok_or_else(|| "pixels conformance: one-core run missing".to_string())?;
    if instrumented_four == [0; 4] || instrumented_four != instrumented_one {
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
    let scored =
        wrela_compiler::pixels::reference::conformance::run(&observations, &semantic_renderers)?;
    let actual = format!(
        "GuestVisibilityExecution version=1 cases={} telemetry={:016x}{:016x}{:016x}{:016x} status=pass\n{scored}",
        guest_cases.len(),
        instrumented_four[0],
        instrumented_four[1],
        instrumented_four[2],
        instrumented_four[3],
    );
    let path = root().join(EXPECTED);
    let expected = std::fs::read_to_string(&path)
        .map_err(|error| format!("pixels conformance: read {}: {error}", path.display()))?;
    if actual != expected {
        return Err(format!(
            "pixels conformance: deterministic score report differs from {EXPECTED}\n\
             --- expected\n{expected}--- actual\n{actual}"
        ));
    }
    print!("{actual}");
    Ok(())
}
