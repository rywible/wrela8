//! The single-source worker error-code table.
//!
//! P7 worker sub-codes are one byte wide and cross three layers: guest
//! refinement tiers report them through `P7CertifiedRow`/`P7RootList`
//! records, the coordinator maps them onto `RenderError` variants, and the
//! host decodes them out of trace words and `CertificateExhausted` tile
//! words. Before this table existed, each layer restated the code list by
//! hand and the taxonomy lived in one comment at the mapping site — B1 (the
//! 70–77 misclassification) was the direct product. Every layer now derives
//! from this table: the guest-side classifier `__wrela_pixels_p7_worker_error_class`
//! is generated from it (see `glue::write_worker_error_class`), the host
//! lookup functions below decode trace words with the same data, and the
//! unit tests pin uniqueness plus classification totality.

/// How the coordinator maps a worker sub-code onto a `RenderError` variant.
///
/// The discriminants are the wire protocol between the generated classifier
/// and `Renderer.__worker_error` in `stdlib/core/render.wr` — change them
/// only together.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
#[repr(u8)]
pub enum WorkerErrorClass {
    /// Corrupt compiler-generated state; mapped to `InternalInvariant`.
    Invariant = 0,
    /// A proven capacity ceiling was exceeded; mapped to
    /// `CapacityExceeded(kind = code)`.
    Capacity = 1,
    /// Event isolation exhausted its bounded budget; mapped to
    /// `EventIsolationExhausted(tile)`.
    EventIsolationExhausted = 2,
    /// A bounded refinement tier ran out of budget — an *expected*
    /// fail-closed outcome (§2.6); mapped to `CertificateExhausted` with the
    /// sub-code in bits 16+ of the tile word.
    CertificateExhausted = 3,
    /// Root isolation exhausted its bounded budget; mapped to
    /// `RootIsolationExhausted(tile)`.
    RootIsolationExhausted = 4,
    /// Fixed-point range exceeded during evaluation; mapped to
    /// `FixedPointRangeExceeded(tile)`.
    FixedPointRange = 5,
}

pub struct WorkerErrorSpec {
    pub code: u8,
    pub name: &'static str,
    pub class: WorkerErrorClass,
    pub doc: &'static str,
}

/// Every worker sub-code with defined semantics. A code absent from this
/// table classifies as `Invariant` (corrupt state) — that is the fail-closed
/// default, so forgetting to register a new code can only make the renderer
/// report a harsher class, never a softer one.
pub const WORKER_ERRORS: &[WorkerErrorSpec] = &[
    WorkerErrorSpec {
        code: 1,
        name: "row-record-capacity",
        class: WorkerErrorClass::Capacity,
        doc: "per-row sample/stack records exceeded the generated capacity",
    },
    WorkerErrorSpec {
        code: 2,
        name: "event-isolation-exhausted",
        class: WorkerErrorClass::EventIsolationExhausted,
        doc: "event isolation exhausted its bounded budget",
    },
    WorkerErrorSpec {
        code: 3,
        name: "run-record-capacity",
        class: WorkerErrorClass::Capacity,
        doc: "certified-run records exceeded the generated capacity",
    },
    WorkerErrorSpec {
        code: 4,
        name: "ambiguous-visibility",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "a root group stayed order-ambiguous after every side-state rule",
    },
    WorkerErrorSpec {
        code: 6,
        name: "root-isolation-exhausted",
        class: WorkerErrorClass::RootIsolationExhausted,
        doc: "root isolation exhausted its bounded budget",
    },
    WorkerErrorSpec {
        code: 7,
        name: "fixed-point-range",
        class: WorkerErrorClass::FixedPointRange,
        doc: "fixed-point evaluation left the representable range",
    },
    WorkerErrorSpec {
        code: 16,
        name: "candidate-capacity",
        class: WorkerErrorClass::Capacity,
        doc: "per-row candidate/proposal records exceeded the proven ceiling",
    },
    WorkerErrorSpec {
        code: 17,
        name: "root-capacity",
        class: WorkerErrorClass::Capacity,
        doc: "root/run/corridor records exceeded the proven ceiling",
    },
    WorkerErrorSpec {
        code: 41,
        name: "structural-event-unproven",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "an event sample had no structural corridor to justify it",
    },
    WorkerErrorSpec {
        code: 42,
        name: "quadtree-cell-budget",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "the subpixel quadtree exceeded its cell budget",
    },
    WorkerErrorSpec {
        code: 43,
        name: "quadtree-stack-depth",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "the subpixel quadtree exceeded its traversal stack",
    },
    WorkerErrorSpec {
        code: 44,
        name: "arrangement-identities",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "more than two hit identities inside one pixel cell",
    },
    WorkerErrorSpec {
        code: 45,
        name: "arrangement-background-mix",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "two hit identities plus background inside one pixel cell",
    },
    WorkerErrorSpec {
        code: 46,
        name: "arrangement-identity-range",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "a secondary identity fell outside the representable range",
    },
    WorkerErrorSpec {
        code: 47,
        name: "coverage-byte-not-unique",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "the coverage byte stayed ambiguous at the generated depth cap",
    },
    WorkerErrorSpec {
        code: 48,
        name: "event-coverage-failed",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "sealed event coverage evaluation failed for the pixel",
    },
    WorkerErrorSpec {
        code: 49,
        name: "event-coverage-missing",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "an event sample had no sealed event coverage to integrate",
    },
    WorkerErrorSpec {
        code: 50,
        name: "covering-identity-unresolved",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "positive coverage with no resolvable covering identity",
    },
    WorkerErrorSpec {
        code: 61,
        name: "silhouette-event-record",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: malformed event record",
    },
    WorkerErrorSpec {
        code: 62,
        name: "silhouette-polynomial",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: event polynomial evaluation failed",
    },
    WorkerErrorSpec {
        code: 63,
        name: "silhouette-bounds",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: uv bounds evaluation failed",
    },
    WorkerErrorSpec {
        code: 64,
        name: "silhouette-interval",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: interval conversion failed",
    },
    WorkerErrorSpec {
        code: 65,
        name: "silhouette-discriminant",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: discriminant evaluation failed",
    },
    WorkerErrorSpec {
        code: 66,
        name: "silhouette-occupancy",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: side occupancy evaluation failed",
    },
    WorkerErrorSpec {
        code: 67,
        name: "silhouette-owner",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: participant owner lookup failed",
    },
    WorkerErrorSpec {
        code: 68,
        name: "silhouette-magnitude",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: magnitude evaluation failed",
    },
    WorkerErrorSpec {
        code: 69,
        name: "silhouette-projection",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: projected mode evaluation failed",
    },
    WorkerErrorSpec {
        code: 70,
        name: "silhouette-refine-stack",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: refinement stack exceeded",
    },
    WorkerErrorSpec {
        code: 71,
        name: "silhouette-degenerate",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: degenerate curve magnitude",
    },
    WorkerErrorSpec {
        code: 72,
        name: "silhouette-area-lo",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: area lower-bound arithmetic failed",
    },
    WorkerErrorSpec {
        code: 73,
        name: "silhouette-area-hi",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: area upper-bound arithmetic failed",
    },
    WorkerErrorSpec {
        code: 74,
        name: "silhouette-byte-not-unique",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: byte ambiguous at refinement cap",
    },
    WorkerErrorSpec {
        code: 75,
        name: "silhouette-conflict",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: conflicting event coverages",
    },
    WorkerErrorSpec {
        code: 76,
        name: "silhouette-area-range",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: area outside the unit range",
    },
    WorkerErrorSpec {
        code: 77,
        name: "silhouette-area-order",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "analytic silhouette coverage: area bounds out of order",
    },
    WorkerErrorSpec {
        code: 100,
        name: "analytic-coverage-indefinite",
        class: WorkerErrorClass::CertificateExhausted,
        doc: "no analytic integrator produced a definite byte for the pixel",
    },
];

/// Class for a worker sub-code; codes not in the table are `Invariant`.
pub fn worker_error_class(code: u8) -> WorkerErrorClass {
    WORKER_ERRORS
        .iter()
        .find(|spec| spec.code == code)
        .map_or(WorkerErrorClass::Invariant, |spec| spec.class)
}

/// Human-readable name for a worker sub-code, for trace decoding.
pub fn worker_error_name(code: u8) -> &'static str {
    WORKER_ERRORS
        .iter()
        .find(|spec| spec.code == code)
        .map_or("internal-invariant", |spec| spec.name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes_are_unique_and_sorted() {
        for pair in WORKER_ERRORS.windows(2) {
            assert!(
                pair[0].code < pair[1].code,
                "worker error table must stay sorted and duplicate-free: {} then {}",
                pair[0].code,
                pair[1].code
            );
        }
    }

    #[test]
    fn every_silhouette_sub_code_is_certificate_exhausted() {
        for code in 61..=77 {
            assert_eq!(
                worker_error_class(code),
                WorkerErrorClass::CertificateExhausted,
                "silhouette sub-code {code} regressed out of the exhausted class (B1)"
            );
        }
    }

    #[test]
    fn stdlib_worker_error_literals_are_registered() {
        // Every literal the guest sweep can report as a worker error must
        // have table semantics or be a deliberate internal-invariant code.
        // This is the lint that makes B1-style drift impossible: adding a
        // new failure code to render.wr without registering it here fails
        // the build, not a fixture three milestones later.
        let render = renderer_sources();
        // Codes reported through row/sample failure paths and direct worker
        // returns. Keeping both spellings here closes the loophole where a
        // direct `run_job` invariant could borrow an unrelated registered
        // capacity/exhaustion code without the single-source lint noticing.
        let mut seen = std::collections::BTreeSet::new();
        for pattern in ["failure_with(", "__wrela_pixels_p7_worker_error("] {
            for (start, _) in render.match_indices(pattern) {
                let rest = &render[start + pattern.len()..];
                let digits: String = rest.chars().take_while(|c| c.is_ascii_digit()).collect();
                if let Ok(code) = digits.parse::<u16>() {
                    if code <= 255 {
                        seen.insert(code as u8);
                    }
                }
            }
        }
        // Internal codes deliberately classified as invariants (corrupt
        // state, never expected on a valid frame). Everything else must be
        // in the table. 60 is not a code: it is the base of the dynamic
        // `failure_with(60 + sub)` silhouette expression whose reachable
        // values 61..=77 are all registered above.
        let internal: std::collections::BTreeSet<u8> = [0, 5, 8, 9, 10, 11, 12, 13, 14, 15, 60]
            .into_iter()
            .collect();
        for code in seen {
            assert!(
                WORKER_ERRORS.iter().any(|spec| spec.code == code) || internal.contains(&code),
                "render.wr reports worker error {code} but the single-source \
                 table has no entry and it is not a registered internal code; \
                 add it to WORKER_ERRORS (crates/wrela-compiler/src/pixels/worker_errors.rs)"
            );
        }
    }

    /// Every renderer module's source, concatenated.
    ///
    /// The renderer spans `stdlib/core/render.wr` and its sibling
    /// `render_*.wr` modules, so a guard that reads only `render.wr` would
    /// stop seeing the code it guards the moment a function moved.
    fn renderer_sources() -> String {
        let core = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/core");
        let mut paths: Vec<std::path::PathBuf> = std::fs::read_dir(&core)
            .expect("stdlib/core is readable")
            .map(|entry| entry.expect("stdlib/core entry").path())
            .filter(|path| {
                path.extension().is_some_and(|extension| extension == "wr")
                    && path
                        .file_stem()
                        .and_then(|stem| stem.to_str())
                        .is_some_and(|stem| stem == "render" || stem.starts_with("render_"))
            })
            .collect();
        paths.sort();
        paths
            .iter()
            .map(|path| std::fs::read_to_string(path).expect("renderer module is readable"))
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn run_job_preflight_reports_only_internal_invariants() {
        let render = renderer_sources();
        let start = render
            .find("pub fn __wrela_pixels_p7_run_job(")
            .expect("run_job exists");
        let end = render[start..]
            .find("    telemetry_enabled = ")
            .map(|offset| start + offset)
            .expect("run_job preflight boundary exists");
        let preflight = &render[start..end];
        let reported = [5_u8, 8, 9, 11];
        for code in reported {
            assert!(
                preflight.contains(&format!("__wrela_pixels_p7_worker_error({code}, 0)")),
                "run_job preflight no longer reports internal-invariant code {code}"
            );
            assert_eq!(worker_error_class(code), WorkerErrorClass::Invariant);
        }
        for borrowed in [1_u8, 2, 3, 7] {
            assert!(
                !preflight.contains(&format!("__wrela_pixels_p7_worker_error({borrowed}, 0)")),
                "run_job preflight borrowed semantic code {borrowed}"
            );
        }
    }
}
