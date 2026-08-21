//! The checked-in Pixels hot-path census lane.
//!
//! Emits one census artifact per basis fixture into `tests/census/p8-baseline/`
//! and compares it against the checked-in bytes. `--update` rewrites them.
//!
//! The artifact is the measurement baseline every later P8R diff cites, so
//! the lane is strict on purpose: a named function or region marker that
//! stops existing fails the lane rather than shortening the report, and the
//! two-run/two-directory determinism check runs before a result can enter the
//! content-addressed census cache. A warm invocation revalidates the cached
//! bytes against the checked-in authority without recompiling unchanged input.

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use wrela_compiler::pixels::hot_census;

use crate::golden::heavy_pixels_jobs;
use crate::pixels_cache::{Cache, key_of};
use crate::root;

/// Directory holding the checked-in baseline artifacts.
pub(crate) const BASELINE_DIR: &str = "tests/census/p8-baseline";

/// Basis fixtures the census measures, and the artifact each writes.
///
/// One fixture is enough to reach every named target, and a second would
/// double the lane's cost for a copy of the same emitted renderer. The
/// chosen case is the adversarial normal-moments scene, which reaches the
/// analytic coverage tier, the packet raster, and the smooth-object
/// isolation path in one compile.
#[derive(Clone, Copy)]
enum TargetSet {
    P8HotPathsPreDecomposition,
    P8HotPathsLegacyFp,
    P8HotPaths,
    PacketSubstrate,
}

struct Basis {
    case: &'static str,
    source: &'static str,
    artifact: &'static str,
    targets: TargetSet,
}

impl Basis {
    fn target_id(&self) -> &'static str {
        match self.targets {
            TargetSet::P8HotPathsPreDecomposition => "p8-pre-decomposition",
            TargetSet::P8HotPathsLegacyFp => "p8-legacy-fp",
            TargetSet::P8HotPaths => "p8-direct-fp",
            TargetSet::PacketSubstrate => "packet-substrate",
        }
    }

    fn is_immutable_phase(&self) -> bool {
        !matches!(self.targets, TargetSet::P8HotPaths) || self.case == "p8r4-direct-fp"
    }
}

const BASIS: &[Basis] = &[
    Basis {
        case: "p8r2-pre-decomposition",
        source: "tests/golden/check-pixels-normal-moments/src/examples/check_pixels_normal_moments.wr",
        artifact: "p8r2-pre-decomposition.txt",
        targets: TargetSet::P8HotPathsPreDecomposition,
    },
    Basis {
        case: "p8r3-post-decomposition",
        source: "tests/golden/check-pixels-normal-moments/src/examples/check_pixels_normal_moments.wr",
        artifact: "p8r3-post-decomposition.txt",
        targets: TargetSet::P8HotPathsLegacyFp,
    },
    Basis {
        case: "p8r4-direct-fp",
        source: "tests/golden/check-pixels-normal-moments/src/examples/check_pixels_normal_moments.wr",
        artifact: "p8r4-direct-fp.txt",
        targets: TargetSet::P8HotPaths,
    },
    Basis {
        case: "check-pixels-normal-moments",
        source: "tests/golden/check-pixels-normal-moments/src/examples/check_pixels_normal_moments.wr",
        artifact: "check-pixels-normal-moments.txt",
        targets: TargetSet::P8HotPaths,
    },
    Basis {
        case: "p8r5-packet",
        source: "stdlib/core/render_raster.wr",
        artifact: "p8r5-packet.txt",
        targets: TargetSet::PacketSubstrate,
    },
];

const PHASE_ARTIFACTS: &[(&str, &str, &[&str])] = &[
    (
        "P8-close",
        "p8-close.txt",
        &[
            "phase = P8-close",
            "basis_commit = 44bcfcdce7d55ba062227dc96de3c49d9e3d91db",
            "fn.__wrela_pixels_p8_raster_regular",
        ],
    ),
    (
        "P8R.2-pre-decomposition",
        "p8r2-pre-decomposition.txt",
        &[
            "schema = 4",
            "phase = P8R.2-pre-decomposition",
            "phase_backend = legacy-gpr-fp",
            "authority = immutable-emitted-kernel",
            "basis_commit = 44bcfcdce7d55ba062227dc96de3c49d9e3d91db",
            "source_tree_sha256 = ",
            "image_sha256 = ",
            "kernel_words_sha256 = ",
            "codegen_dump_sha256 = ",
            "## [M] measured counts",
            "region.raster.scalar_prefix",
            "region.raster.packet_loop",
            "region.raster.scalar_suffix",
            "region.raster.charge",
            "region.coverage.entry",
            "region.coverage.cell_walk",
            "fn.__wrela_pixels_p8_geometry_lane_valid",
            "fn.__wrela_pixels_p8_geometry_packet_valid",
            "family.p8_write present=[\"__wrela_pixels_p8_write\", \"__wrela_pixels_p8_write4\"] absent=[]",
            "fn.sqrt_scalar",
            "family.sealed_numeric present=[\"sqrt_scalar\", \"rsqrt_scalar\", \"raster_rsqrt\"] absent=[]",
            "## [I] modelled cycles",
        ],
    ),
    (
        "P8R.3-post-decomposition",
        "p8r3-post-decomposition.txt",
        &[
            "schema = 4",
            "phase = P8R.3-post-decomposition",
            "phase_backend = legacy-gpr-fp",
            "authority = immutable-emitted-kernel",
            "source_tree_sha256 = ",
            "image_sha256 = ",
            "kernel_words_sha256 = ",
            "codegen_dump_sha256 = ",
            "## [M] measured counts",
            "region.coverage.cell_walk.class.fp_move",
            "## [I] modelled cycles",
            "fn.__wrela_pixels_p7_union_silhouette_coverage_at_slack",
        ],
    ),
    (
        "P8R.4-direct-fp",
        "p8r4-direct-fp.txt",
        &[
            "schema = 4",
            "phase = P8R.4-direct-fp",
            "phase_backend = direct-fp",
            "authority = immutable-emitted-kernel",
            "source_tree_sha256 = ",
            "image_sha256 = ",
            "kernel_words_sha256 = ",
            "codegen_dump_sha256 = ",
            "## [M] measured counts",
            "region.coverage.cell_walk.class.fp_load",
            "## [I] modelled cycles",
        ],
    ),
    (
        "P8R.5-packet",
        "p8r5-packet.txt",
        &[
            "case = p8r5-packet",
            "phase = P8R.5-packet",
            "authority = immutable-emitted-kernel",
            "source_tree_sha256 = ",
            "class.asimd_fp_add_sub",
            "class.asimd_fp_mul",
            "class.asimd_fp_fma",
            "class.asimd_fp_cmp",
            "class.asimd_fp_cvt",
            "class.asimd_int",
            "class.fp_load_q",
            "class.fp_store_q",
        ],
    ),
    (
        "P8R.7-final",
        "check-pixels-normal-moments.txt",
        &[
            "case = check-pixels-normal-moments",
            "phase = P8R.7-final",
            "source_tree_sha256 = ",
            "image_sha256 = ",
            "kernel_words_sha256 = ",
            "codegen_dump_sha256 = ",
            "region.raster.packet_loop",
            "region.coverage.cell_walk",
            "region.coverage.cell_walk.latency_weighted_cycles",
        ],
    ),
];

fn artifact_path(basis: &Basis) -> PathBuf {
    root().join(BASELINE_DIR).join(basis.artifact)
}

fn collect_source_files(dir: &Path, extension: &str, out: &mut Vec<PathBuf>) -> Result<(), String> {
    for entry in std::fs::read_dir(dir)
        .map_err(|error| format!("pixels-census: read {}: {error}", dir.display()))?
    {
        let path = entry
            .map_err(|error| format!("pixels-census: read {} entry: {error}", dir.display()))?
            .path();
        if path.is_dir() {
            collect_source_files(&path, extension, out)?;
        } else if path.extension().and_then(|value| value.to_str()) == Some(extension) {
            out.push(path);
        }
    }
    Ok(())
}

/// Hash the compiler and stdlib source that determines the census, plus the
/// basis bytes under their stable repository label. This identifies dirty
/// worktrees where a commit id alone would falsely name the measured source.
fn source_tree_digest_with_stdlib(
    basis: &Basis,
    basis_path: &Path,
    stdlib_core: &Path,
) -> Result<String, String> {
    let repo = root();
    let mut paths = Vec::new();
    collect_source_files(&repo.join("crates/wrela-compiler/src"), "rs", &mut paths)?;
    paths.extend([
        repo.join("bench/a76-pi5.toml"),
        repo.join("crates/xtask/src/pixels_census.rs"),
        repo.join("tests/census.toml"),
    ]);
    paths.sort();
    let mut bytes = Vec::new();
    for path in paths {
        let relative = path
            .strip_prefix(&repo)
            .map_err(|_| format!("pixels-census: {} is outside repository", path.display()))?;
        bytes.extend_from_slice(relative.to_string_lossy().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(
            &std::fs::read(&path)
                .map_err(|error| format!("pixels-census: read {}: {error}", path.display()))?,
        );
        bytes.push(0xff);
    }
    let mut stdlib_paths = Vec::new();
    collect_source_files(stdlib_core, "wr", &mut stdlib_paths)?;
    stdlib_paths.sort();
    for path in stdlib_paths {
        let relative = path.strip_prefix(stdlib_core).map_err(|_| {
            format!(
                "pixels-census: {} escaped stdlib root {}",
                path.display(),
                stdlib_core.display()
            )
        })?;
        bytes.extend_from_slice(b"stdlib/core/");
        bytes.extend_from_slice(relative.to_string_lossy().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(
            &std::fs::read(&path)
                .map_err(|error| format!("pixels-census: read {}: {error}", path.display()))?,
        );
        bytes.push(0xff);
    }
    let mut sealed_data = Vec::new();
    collect_source_files(&root().join("stdlib/data/pixels"), "bin", &mut sealed_data)?;
    sealed_data.sort();
    for path in sealed_data {
        let relative = path
            .strip_prefix(&repo)
            .map_err(|_| format!("pixels-census: {} escaped repository root", path.display()))?;
        bytes.extend_from_slice(relative.to_string_lossy().as_bytes());
        bytes.push(0);
        bytes.extend_from_slice(
            &std::fs::read(&path)
                .map_err(|error| format!("pixels-census: read {}: {error}", path.display()))?,
        );
        bytes.push(0xff);
    }
    bytes.extend_from_slice(basis.source.as_bytes());
    bytes.push(0);
    bytes.extend_from_slice(&std::fs::read(basis_path).map_err(|error| {
        format!(
            "pixels-census: read basis {}: {error}",
            basis_path.display()
        )
    })?);
    Ok(wrela_compiler::report::sha256_hex(&bytes))
}

fn source_tree_digest(basis: &Basis, basis_path: &Path) -> Result<String, String> {
    source_tree_digest_with_stdlib(basis, basis_path, &root().join("stdlib/core"))
}

fn census_cache_key_from_parts(
    case: &str,
    target_set: &str,
    source_tree: &str,
    toolchain: &str,
) -> String {
    key_of(&[
        ("contract", "pixels-census-v1".to_string()),
        ("case", case.to_string()),
        ("target_set", target_set.to_string()),
        ("source_tree", source_tree.to_string()),
        ("toolchain", toolchain.to_string()),
    ])
}

fn census_cache_key(basis: &Basis, toolchain: &str) -> Result<String, String> {
    let source = root().join(basis.source);
    Ok(census_cache_key_from_parts(
        basis.case,
        basis.target_id(),
        &source_tree_digest(basis, &source)?,
        toolchain,
    ))
}

fn kernel_words_digest(program: &wrela_compiler::codegen::CodegenProgram) -> String {
    let mut bytes = Vec::new();
    for (key, function) in &program.fns {
        bytes.extend_from_slice(key.as_bytes());
        bytes.push(0);
        for word in &function.code {
            bytes.extend_from_slice(&word.word.to_le_bytes());
        }
        bytes.push(0xff);
    }
    wrela_compiler::report::sha256_hex(&bytes)
}

const P8_BASIS_COMMIT: &str = "44bcfcdce7d55ba062227dc96de3c49d9e3d91db";

fn git_show_basis(path: &str) -> Result<String, String> {
    let output = std::process::Command::new("git")
        .current_dir(root())
        .args(["show", &format!("{P8_BASIS_COMMIT}:{path}")])
        .output()
        .map_err(|error| format!("pixels-census: read P8 basis `{path}`: {error}"))?;
    if !output.status.success() {
        return Err(format!(
            "pixels-census: pinned P8 basis `{P8_BASIS_COMMIT}:{path}` is unavailable:\n{}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    String::from_utf8(output.stdout)
        .map_err(|error| format!("pixels-census: P8 basis `{path}` is not UTF-8: {error}"))
}

fn insert_before_in_function(
    text: &mut String,
    function: &str,
    anchor: &str,
    insertion: &str,
) -> Result<(), String> {
    let start = text
        .find(function)
        .ok_or_else(|| format!("pixels-census: P8 basis omits `{function}`"))?;
    let relative = text[start..].find(anchor).ok_or_else(|| {
        format!("pixels-census: P8 basis `{function}` omits marker anchor `{anchor}`")
    })?;
    text.insert_str(start + relative, insertion);
    Ok(())
}

fn p8r2_marked_sources() -> Result<(String, String), String> {
    let mut render = git_show_basis("stdlib/core/render.wr")?;
    // The current raster support module contains the P8R.2 marker API plus
    // later packet-only declarations. Those later declarations are not
    // reachable from the P8 hot targets; using the live support module keeps
    // the current generated image glue type-correct while the authoritative
    // monolithic renderer body remains the pinned pre-decomposition source.
    let raster = std::fs::read_to_string(root().join("stdlib/core/render_raster.wr"))
        .map_err(|error| format!("pixels-census: read live raster support: {error}"))?;

    let import_start = render
        .find("from core.render_raster import ")
        .ok_or_else(|| "pixels-census: P8 render basis raster import drifted".to_string())?;
    let import_end = render[import_start..]
        .find('\n')
        .map(|offset| import_start + offset + 1)
        .ok_or_else(|| {
            "pixels-census: P8 render basis raster import is unterminated".to_string()
        })?;
    render.insert_str(
        import_end,
        "from core.render_raster import PIXELS_REGION_COVERAGE_CELL_WALK, PIXELS_REGION_COVERAGE_ENTRY, PIXELS_REGION_RASTER_CHARGE, PIXELS_REGION_RASTER_PACKET_LOOP, PIXELS_REGION_RASTER_SCALAR_PREFIX, PIXELS_REGION_RASTER_SCALAR_SUFFIX, pixels_census_region\n\
         from core.render_arrangement import __wrela_pixels_p8r_dispatch_handler\n",
    );

    insert_before_in_function(
        &mut render,
        "fn __wrela_pixels_p8_raster_regular",
        "    raster_x = start\n",
        "    pixels_census_region(PIXELS_REGION_RASTER_SCALAR_PREFIX)\n",
    )?;
    insert_before_in_function(
        &mut render,
        "fn __wrela_pixels_p8_raster_regular",
        "    if end -% raster_x >= 4:\n",
        "    pixels_census_region(PIXELS_REGION_RASTER_PACKET_LOOP)\n",
    )?;
    insert_before_in_function(
        &mut render,
        "fn __wrela_pixels_p8_raster_regular",
        "    @budget(bound=3)\n    while raster_x < end:\n",
        "    pixels_census_region(PIXELS_REGION_RASTER_SCALAR_SUFFIX)\n",
    )?;
    insert_before_in_function(
        &mut render,
        "fn __wrela_pixels_p8_raster_regular",
        "    span = (end -% start).to[u64]()\n",
        "    pixels_census_region(PIXELS_REGION_RASTER_CHARGE)\n",
    )?;
    insert_before_in_function(
        &mut render,
        "pub fn __wrela_pixels_p7_union_silhouette_coverage_at_slack",
        "    worker = (worker_tile & 4294967295).to[u32]()\n",
        "    pixels_census_region(PIXELS_REGION_COVERAGE_ENTRY)\n",
    )?;
    insert_before_in_function(
        &mut render,
        "pub fn __wrela_pixels_p7_union_silhouette_coverage_at_slack",
        "    @budget(bound=262144)\n    while stack_count > 0:\n",
        "    pixels_census_region(PIXELS_REGION_COVERAGE_CELL_WALK)\n",
    )?;
    Ok((render, raster))
}

fn copy_tree(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::create_dir_all(destination)
        .map_err(|error| format!("pixels-census: create {}: {error}", destination.display()))?;
    let mut entries = std::fs::read_dir(source)
        .map_err(|error| format!("pixels-census: read {}: {error}", source.display()))?
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| format!("pixels-census: read {} entry: {error}", source.display()))?;
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let from = entry.path();
        let to = destination.join(entry.file_name());
        if from.is_dir() {
            copy_tree(&from, &to)?;
        } else {
            std::fs::copy(&from, &to).map_err(|error| {
                format!(
                    "pixels-census: copy {} to {}: {error}",
                    from.display(),
                    to.display()
                )
            })?;
        }
    }
    Ok(())
}

fn render_p8r2_case(basis: &Basis, build: &str) -> Result<String, String> {
    let temp = std::env::temp_dir().join(format!(
        "wrela-pixels-census-{}-{}-{build}",
        std::process::id(),
        basis.case
    ));
    let _ = std::fs::remove_dir_all(&temp);
    let result = (|| {
        let source = root().join(basis.source);
        let file_name = source.file_name().ok_or_else(|| {
            format!(
                "pixels-census: source {} has no file name",
                source.display()
            )
        })?;
        let copied = temp.join("src/examples").join(file_name);
        std::fs::create_dir_all(copied.parent().expect("copied source parent"))
            .map_err(|error| format!("pixels-census: create source tree: {error}"))?;
        std::fs::copy(&source, &copied)
            .map_err(|error| format!("pixels-census: copy {}: {error}", source.display()))?;
        let stdlib = temp.join("stdlib");
        copy_tree(&root().join("stdlib"), &stdlib)?;
        let (render, raster) = p8r2_marked_sources()?;
        std::fs::write(stdlib.join("core/render.wr"), render)
            .map_err(|error| format!("pixels-census: write P8R.2 render source: {error}"))?;
        std::fs::write(stdlib.join("core/render_raster.wr"), raster)
            .map_err(|error| format!("pixels-census: write P8R.2 raster source: {error}"))?;
        // Current generated image glue seals the P8R.3 handler names through
        // imports even though it never calls them. A minimal module prevents
        // those future imports from pulling the decomposed authoritative walk
        // into the pre-decomposition census and creating duplicate targets.
        std::fs::write(
            stdlib.join("core/render_arrangement.wr"),
            "module render_arrangement\n\n\
             pub fn __wrela_pixels_p8r_clip_handler() -> u64:\n    return 0\n\n\
             pub fn __wrela_pixels_p8r_deformation_handler() -> u64:\n    return 0\n\n\
             pub fn __wrela_pixels_p8r_dispatch_handler() -> u64:\n    return 0\n\n\
             pub fn __wrela_pixels_p8r_polynomial_handler() -> u64:\n    return 0\n\n\
             pub fn __wrela_pixels_p8r_predicate_handler() -> u64:\n    return 0\n\n\
             pub fn __wrela_pixels_p8r_smooth_band_handler() -> u64:\n    return 0\n\n\
             pub fn __wrela_pixels_p8r_torus_handler() -> u64:\n    return 0\n",
        )
        .map_err(|error| format!("pixels-census: write P8R.2 handler shim: {error}"))?;
        render_case_at_with_stdlib(basis, &copied, Some(&stdlib.join("core")))
    })();
    let cleanup = std::fs::remove_dir_all(&temp)
        .map_err(|error| format!("pixels-census: remove {}: {error}", temp.display()));
    match (result, cleanup) {
        (Ok(text), Ok(())) => Ok(text),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

/// Produce one basis fixture's census artifact text.
fn render_case(basis: &Basis) -> Result<String, String> {
    if matches!(basis.targets, TargetSet::P8HotPathsPreDecomposition) {
        return render_p8r2_case(basis, "first");
    }
    let path = root().join(basis.source);
    render_case_at(basis, &path)
}

fn render_case_at(basis: &Basis, path: &Path) -> Result<String, String> {
    render_case_at_with_stdlib(basis, path, None)
}

fn render_case_at_with_stdlib(
    basis: &Basis,
    path: &Path,
    stdlib_core: Option<&Path>,
) -> Result<String, String> {
    let previous_opts = wrela_compiler::opts::active_opts();
    wrela_compiler::opts::apply_mode(wrela_compiler::opts::CompileMode::Release);
    let rendered = (|| {
        let source_tree_sha256 = match stdlib_core {
            Some(stdlib) => source_tree_digest_with_stdlib(basis, path, stdlib)?,
            None => source_tree_digest(basis, path)?,
        };
        if matches!(basis.targets, TargetSet::PacketSubstrate) {
            let program = hot_census::p8r5_packet_census_program()?;
            let table = wrela_compiler::cost::load_default()?;
            let dump = wrela_compiler::codegen::dump(&program);
            let digests = vec![
                ("case".to_string(), basis.case.to_string()),
                ("source".to_string(), basis.source.to_string()),
                ("phase".to_string(), "P8R.5-packet".to_string()),
                ("phase_backend".to_string(), "packet-a64".to_string()),
                ("source_tree_sha256".to_string(), source_tree_sha256),
                (
                    "kernel_words_sha256".to_string(),
                    kernel_words_digest(&program),
                ),
                (
                    "codegen_dump_sha256".to_string(),
                    wrela_compiler::report::sha256_hex(dump.as_bytes()),
                ),
                ("cost_provenance".to_string(), table.provenance_digest()),
            ];
            let targets = hot_census::CensusTargets {
                functions: vec!["pixels_packet_census".to_string()],
                families: Vec::new(),
                required_regions: BTreeMap::new(),
            };
            return hot_census::report(&program, &table, &Default::default(), &targets, &digests);
        }
        let direct_fp = !matches!(
            basis.targets,
            TargetSet::P8HotPathsPreDecomposition | TargetSet::P8HotPathsLegacyFp
        );
        let (program, placement) = if matches!(basis.targets, TargetSet::P8HotPathsPreDecomposition)
        {
            wrela_compiler::cost::codegen_cost_stage_census_snapshot(path, direct_fp)?
        } else {
            wrela_compiler::cost::codegen_cost_stage_census(path, direct_fp, false)?
        };
        let table = wrela_compiler::cost::load_default()?;
        let dump = wrela_compiler::codegen::dump(&program);
        let mut digests = vec![
            ("case".to_string(), basis.case.to_string()),
            ("source".to_string(), basis.source.to_string()),
            (
                "phase".to_string(),
                match basis.targets {
                    TargetSet::P8HotPathsPreDecomposition => "P8R.2-pre-decomposition",
                    TargetSet::P8HotPathsLegacyFp => "P8R.3-post-decomposition",
                    TargetSet::P8HotPaths if basis.case == "p8r4-direct-fp" => "P8R.4-direct-fp",
                    TargetSet::P8HotPaths => "P8R.7-final",
                    TargetSet::PacketSubstrate => unreachable!(),
                }
                .to_string(),
            ),
            (
                "phase_backend".to_string(),
                if direct_fp {
                    "direct-fp".to_string()
                } else {
                    "legacy-gpr-fp".to_string()
                },
            ),
            ("source_tree_sha256".to_string(), source_tree_sha256),
            (
                "kernel_words_sha256".to_string(),
                kernel_words_digest(&program),
            ),
            (
                "codegen_dump_sha256".to_string(),
                wrela_compiler::report::sha256_hex(dump.as_bytes()),
            ),
            ("cost_provenance".to_string(), table.provenance_digest()),
        ];
        if matches!(basis.targets, TargetSet::P8HotPathsPreDecomposition) {
            digests.push(("basis_commit".to_string(), P8_BASIS_COMMIT.to_string()));
        }
        if matches!(basis.targets, TargetSet::P8HotPathsPreDecomposition) {
            digests.push((
                "image_sha256".to_string(),
                wrela_compiler::cost::shipped_image_digest_census_snapshot(path)?,
            ));
        } else {
            digests.push((
                "image_sha256".to_string(),
                wrela_compiler::cost::shipped_image_digest_census(path, direct_fp)?,
            ));
        }
        let targets = match basis.targets {
            TargetSet::P8HotPathsPreDecomposition
            | TargetSet::P8HotPathsLegacyFp
            | TargetSet::P8HotPaths => hot_census::p8_baseline_targets(),
            TargetSet::PacketSubstrate => unreachable!("handled above"),
        };
        hot_census::report(&program, &table, &placement, &targets, &digests)
    })();
    wrela_compiler::opts::apply_opts(&previous_opts);
    rendered
}

fn render_from_second_build_dir(basis: &Basis) -> Result<String, String> {
    if matches!(basis.targets, TargetSet::P8HotPathsPreDecomposition) {
        return render_p8r2_case(basis, "second");
    }
    let source_path = root().join(basis.source);
    let file_name = source_path.file_name().ok_or_else(|| {
        format!(
            "pixels-census: source {} has no file name",
            source_path.display()
        )
    })?;
    let temp = std::env::temp_dir().join(format!(
        "wrela-pixels-census-{}-{}",
        std::process::id(),
        basis.case
    ));
    std::fs::create_dir(&temp)
        .map_err(|e| format!("pixels-census: create {}: {e}", temp.display()))?;
    let result = (|| {
        let examples = temp.join("src/examples");
        std::fs::create_dir_all(&examples)
            .map_err(|e| format!("pixels-census: create {}: {e}", examples.display()))?;
        let copied = examples.join(file_name);
        std::fs::copy(&source_path, &copied).map_err(|e| {
            format!(
                "pixels-census: copy {} to {}: {e}",
                source_path.display(),
                copied.display()
            )
        })?;
        std::os::unix::fs::symlink(root().join("stdlib"), temp.join("stdlib"))
            .map_err(|e| format!("pixels-census: link second stdlib: {e}"))?;
        render_case_at(basis, &copied)
    })();
    let cleanup = std::fs::remove_dir_all(&temp)
        .map_err(|e| format!("pixels-census: remove {}: {e}", temp.display()));
    match (result, cleanup) {
        (Ok(text), Ok(())) => Ok(text),
        (Err(error), _) | (Ok(_), Err(error)) => Err(error),
    }
}

fn write_atomic(path: &Path, text: &str) -> Result<(), String> {
    let parent = path
        .parent()
        .ok_or_else(|| format!("census artifact {} has no parent", path.display()))?;
    std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    let tmp = path.with_extension("txt.tmp");
    std::fs::write(&tmp, text).map_err(|e| format!("write {}: {e}", tmp.display()))?;
    std::fs::rename(&tmp, path).map_err(|e| format!("rename {}: {e}", path.display()))
}

fn check_phase_artifact(label: &str, text: &str, required: &[&str]) -> Result<(), String> {
    for needle in required {
        if !text.contains(needle) {
            return Err(format!(
                "pixels-census: immutable phase `{label}` omits required evidence `{needle}`"
            ));
        }
    }
    Ok(())
}

fn header_value<'a>(text: &'a str, key: &str) -> Result<&'a str, String> {
    text.lines()
        .find_map(|line| line.strip_prefix(&format!("{key} = ")))
        .ok_or_else(|| format!("pixels-census: artifact omits header `{key}`"))
}

const P8R4_THRESHOLD_FNS: &[&str] = &[
    "__wrela_pixels_p7_union_silhouette_coverage_at_slack",
    "__wrela_pixels_p7_isolate_smooth_object",
    "__wrela_pixels_p7_collect_roots_box",
    "sqrt_scalar",
    "rsqrt_scalar",
    "raster_rsqrt",
];

fn measured_fp_moves(text: &str) -> Result<u64, String> {
    let measured = text
        .split_once("## [M] measured counts")
        .and_then(|(_, rest)| {
            rest.split_once("## [I] modelled cycles")
                .map(|(part, _)| part)
        })
        .ok_or_else(|| "pixels-census: malformed measured/modelled sections".to_string())?;
    let mut included = false;
    let mut total = 0u64;
    for line in measured.lines() {
        if let Some(function) = line
            .strip_prefix("[fn.")
            .and_then(|value| value.strip_suffix(']'))
        {
            included = P8R4_THRESHOLD_FNS.contains(&function);
        } else if included && line.contains(".class.fp_move = ") {
            total = total
                .checked_add(
                    line.rsplit_once(" = ")
                        .and_then(|(_, value)| value.parse::<u64>().ok())
                        .ok_or_else(|| format!("pixels-census: malformed fp_move row `{line}`"))?,
                )
                .ok_or_else(|| "pixels-census: fp_move total overflow".to_string())?;
        }
    }
    Ok(total)
}

fn proxy_cycle_denominator(text: &str) -> Result<u64, String> {
    let mut total = 0u64;
    for function in P8R4_THRESHOLD_FNS {
        total = total
            .checked_add(proxy_cycle_value(text, function)?)
            .ok_or_else(|| "pixels-census: threshold denominator overflow".to_string())?;
    }
    Ok(total)
}

fn proxy_cycle_value(text: &str, function: &str) -> Result<u64, String> {
    let prefix = format!("fn.{function}.proxy_cycles = ");
    text.lines()
        .find_map(|line| line.strip_prefix(&prefix))
        .ok_or_else(|| format!("pixels-census: missing threshold score `{prefix}`"))?
        .parse::<u64>()
        .map_err(|_| format!("pixels-census: malformed threshold score `{prefix}`"))
}

fn decimal_with_commas(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, byte) in digits.bytes().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(char::from(byte));
    }
    grouped
}

fn check_threshold_narrative(
    p8r4: &str,
    final_report: &str,
    decision: &str,
    final_delta: &str,
) -> Result<(), String> {
    let moves = measured_fp_moves(p8r4)?;
    let denominator = proxy_cycle_denominator(p8r4)?;
    let final_denominator = proxy_cycle_denominator(final_report)?;
    let table = wrela_compiler::cost::load_default()?;
    let move_cycles = moves
        .checked_mul(table.latency(wrela_compiler::cost::CostRule::FpMove))
        .ok_or_else(|| "pixels-census: fp_move threshold numerator overflow".to_string())?;
    let functions = P8R4_THRESHOLD_FNS.join(",");
    for (document, label, lines) in [
        (
            decision,
            "P8R.4 decision",
            vec![
                format!("commit_2_fp_move_count = {moves}"),
                format!("commit_2_fp_move_cycles = {move_cycles}"),
                format!("commit_2_proxy_cycle_denominator = {denominator}"),
                format!("threshold_functions = {functions}"),
            ],
        ),
        (
            final_delta,
            "P8R final delta",
            vec![
                format!("commit_2_threshold_functions = {functions}"),
                format!("commit_2_proxy_cycle_denominator = {denominator}"),
                format!("final_recensus_proxy_cycle_denominator = {final_denominator}"),
            ],
        ),
    ] {
        for line in lines {
            if !document.lines().any(|candidate| candidate == line) {
                return Err(format!(
                    "pixels-census: {label} omits machine-checked line `{line}`"
                ));
            }
        }
    }

    let decision_words = decision.split_whitespace().collect::<Vec<_>>().join(" ");
    let ratio = format!(
        "The census lane fails unless `{move_cycles} / {}` remains below the sealed 10% threshold",
        decimal_with_commas(denominator)
    );
    if !decision_words.contains(&ratio) {
        return Err(format!(
            "pixels-census: P8R.4 decision prose must state the machine-checked ratio `{move_cycles} / {denominator}`"
        ));
    }

    let final_words = final_delta.split_whitespace().collect::<Vec<_>>().join(" ");
    let scores = P8R4_THRESHOLD_FNS
        .iter()
        .map(|function| proxy_cycle_value(p8r4, function))
        .collect::<Result<Vec<_>, _>>()?;
    let final_basis = format!(
        "The machine-checked six-function commit-2 denominator is {} proxy cycles: union {}, isolate {}, roots {}, sealed `sqrt_scalar` {}, sealed `rsqrt_scalar` {}, and sealed `raster_rsqrt` {}.",
        decimal_with_commas(denominator),
        decimal_with_commas(scores[0]),
        decimal_with_commas(scores[1]),
        decimal_with_commas(scores[2]),
        decimal_with_commas(scores[3]),
        decimal_with_commas(scores[4]),
        decimal_with_commas(scores[5]),
    );
    if !final_words.contains(&final_basis) {
        return Err(
            "pixels-census: final delta prose does not describe the canonical six-function commit-2 basis"
                .to_string(),
        );
    }
    Ok(())
}

fn check_phase_consistency(baseline: &Path) -> Result<(), String> {
    let p8r2 = std::fs::read_to_string(baseline.join("p8r2-pre-decomposition.txt"))
        .map_err(|error| format!("pixels-census: read P8R.2 artifact: {error}"))?;
    let p8r3 = std::fs::read_to_string(baseline.join("p8r3-post-decomposition.txt"))
        .map_err(|error| format!("pixels-census: read P8R.3 artifact: {error}"))?;
    let p8r4 = std::fs::read_to_string(baseline.join("p8r4-direct-fp.txt"))
        .map_err(|error| format!("pixels-census: read P8R.4 artifact: {error}"))?;
    let final_report = std::fs::read_to_string(baseline.join("check-pixels-normal-moments.txt"))
        .map_err(|error| format!("pixels-census: read final artifact: {error}"))?;
    for text in [&p8r2, &p8r3, &p8r4, &final_report] {
        check_artifact_shape(text)?;
    }
    let provenance = header_value(&p8r2, "cost_provenance")?;
    if [&p8r3, &p8r4]
        .iter()
        .any(|text| header_value(text, "cost_provenance").ok() != Some(provenance))
    {
        return Err("pixels-census: P8R.2/P8R.3/P8R.4 cost provenance differs".to_string());
    }
    if header_value(&p8r3, "phase_backend")? != "legacy-gpr-fp"
        || header_value(&p8r2, "phase_backend")? != "legacy-gpr-fp"
        || header_value(&p8r4, "phase_backend")? != "direct-fp"
        || header_value(&final_report, "phase_backend")? != "direct-fp"
    {
        return Err("pixels-census: phase backend identities are inconsistent".to_string());
    }
    let p8r3_kernel = header_value(&p8r3, "kernel_words_sha256")?;
    let p8r2_kernel = header_value(&p8r2, "kernel_words_sha256")?;
    let p8r4_kernel = header_value(&p8r4, "kernel_words_sha256")?;
    if p8r3_kernel == p8r4_kernel {
        return Err(
            "pixels-census: direct FP did not change the P8R.3 kernel identity".to_string(),
        );
    }
    if p8r2_kernel == p8r3_kernel {
        return Err(
            "pixels-census: decomposition did not change the P8R.2 kernel identity".to_string(),
        );
    }
    let before_moves = measured_fp_moves(&p8r3)?;
    let after_moves = measured_fp_moves(&p8r4)?;
    if after_moves != 10 || before_moves <= after_moves {
        return Err(format!(
            "pixels-census: direct-FP fp_move transition must decrease to 10, got {before_moves} -> {after_moves}"
        ));
    }
    let denominator = proxy_cycle_denominator(&p8r4)?;
    let table = wrela_compiler::cost::load_default()?;
    let move_cycles = after_moves
        .checked_mul(table.latency(wrela_compiler::cost::CostRule::FpMove))
        .ok_or_else(|| "pixels-census: fp_move threshold numerator overflow".to_string())?;
    if move_cycles.saturating_mul(10) >= denominator {
        return Err(format!(
            "pixels-census: fp_move modeled contribution {move_cycles}/{denominator} reaches the 10% P8R.4c threshold"
        ));
    }
    let decision_path = baseline.join("p8r4-direct-fp.md");
    let decision = std::fs::read_to_string(&decision_path)
        .map_err(|error| format!("pixels-census: read {}: {error}", decision_path.display()))?;
    for line in ["census_artifact = tests/census/p8-baseline/p8r4-direct-fp.txt"] {
        if !decision.lines().any(|candidate| candidate == line) {
            return Err(format!(
                "pixels-census: P8R.4 decision record omits machine-checked line `{line}`"
            ));
        }
    }
    let final_delta_path = baseline.join("p8r-final-deltas.md");
    let final_delta = std::fs::read_to_string(&final_delta_path).map_err(|error| {
        format!(
            "pixels-census: read {}: {error}",
            final_delta_path.display()
        )
    })?;
    check_threshold_narrative(&p8r4, &final_report, &decision, &final_delta)?;
    Ok(())
}

fn check_phase_artifacts() -> Result<(), String> {
    let baseline = root().join(BASELINE_DIR);
    let manifest_path = baseline.join("phase-manifest.txt");
    let manifest = std::fs::read_to_string(&manifest_path)
        .map_err(|error| format!("pixels-census: read {}: {error}", manifest_path.display()))?;
    let mut named_paths = std::collections::BTreeSet::new();
    for (label, file, required) in PHASE_ARTIFACTS {
        if !named_paths.insert(*file) {
            return Err(format!(
                "pixels-census: immutable phases alias the same artifact `{file}`"
            ));
        }
        let relative = format!("{BASELINE_DIR}/{file}");
        if !manifest.contains(&format!("phase.{label} = {relative}")) {
            return Err(format!(
                "pixels-census: phase manifest does not map `{label}` to `{relative}`"
            ));
        }
        let path = baseline.join(file);
        let text = std::fs::read_to_string(&path)
            .map_err(|error| format!("pixels-census: read {}: {error}", path.display()))?;
        check_phase_artifact(label, &text, required)?;
        if *label != "P8R.7-final" {
            let digest = wrela_compiler::report::sha256_hex(text.as_bytes());
            let pin = format!("phase.{label}.sha256 = {digest}");
            if !manifest.lines().any(|line| line == pin) {
                return Err(format!(
                    "pixels-census: immutable phase `{label}` is not pinned by exact artifact digest `{digest}`"
                ));
            }
        }
    }
    check_phase_consistency(&baseline)
}

/// Structural properties every census artifact must have.
///
/// Checked inside the lane rather than in a unit test: the lane already holds
/// a rendered artifact, and rendering a second one purely to assert its shape
/// would spend the default unit lane's locked placement budget on a duplicate
/// renderer compile.
fn is_canonical_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn check_artifact_shape(text: &str) -> Result<(), String> {
    let measured = text
        .find("## [M] measured counts")
        .ok_or_else(|| "pixels-census: the artifact has no [M] section".to_string())?;
    let modelled = text
        .find("## [I] modelled cycles")
        .ok_or_else(|| "pixels-census: the artifact has no [I] section".to_string())?;
    if measured >= modelled {
        return Err("pixels-census: the [M] section must precede the [I] section".to_string());
    }
    if text[..modelled].contains("proxy_cycles") {
        return Err(
            "pixels-census: a modelled cycle count appears inside the [M] section; \
             measured facts and modelled scores are never merged"
                .to_string(),
        );
    }
    if !text.contains(&format!("schema = {}", hot_census::CENSUS_SCHEMA_VERSION)) {
        return Err("pixels-census: the artifact header omits its schema version".to_string());
    }
    for field in [
        "source_tree_sha256",
        "kernel_words_sha256",
        "codegen_dump_sha256",
    ] {
        let value = text
            .lines()
            .find_map(|line| line.strip_prefix(&format!("{field} = ")))
            .ok_or_else(|| format!("pixels-census: the artifact omits {field}"))?;
        if !is_canonical_sha256(value) {
            return Err(format!(
                "pixels-census: {field} is not a canonical lowercase SHA-256 digest"
            ));
        }
    }
    if text.contains("phase_backend = legacy-gpr-fp\n")
        || text.contains("phase_backend = direct-fp\n")
    {
        let value = text
            .lines()
            .find_map(|line| line.strip_prefix("image_sha256 = "))
            .ok_or_else(|| "pixels-census: a hot phase artifact omits image_sha256".to_string())?;
        if !is_canonical_sha256(value) {
            return Err(
                "pixels-census: image_sha256 is not a canonical lowercase SHA-256 digest"
                    .to_string(),
            );
        }
    }
    // Build-directory and working-directory independence: an artifact that
    // embedded either would carry an absolute path.
    let absolute = root().display().to_string();
    if text.contains(&absolute) {
        return Err(
            "pixels-census: the artifact embeds the absolute repository path, so it is not \
             reproducible from another directory"
                .to_string(),
        );
    }
    Ok(())
}

pub(crate) fn pixels_census(update: bool) -> Result<(), String> {
    check_phase_artifacts()?;
    let cache = Cache::census();
    let rustc_binary = std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into());
    let rustc = std::process::Command::new(rustc_binary)
        .args(["--version", "--verbose"])
        .output()
        .map_err(|error| format!("pixels-census: fingerprint Rust toolchain: {error}"))?;
    if !rustc.status.success() {
        return Err("pixels-census: Rust toolchain fingerprint failed".to_string());
    }
    let toolchain = wrela_compiler::report::sha256_hex(&rustc.stdout);
    let keys = BASIS
        .iter()
        .map(|basis| census_cache_key(basis, &toolchain))
        .collect::<Result<Vec<_>, _>>()?;
    let cached = BASIS
        .iter()
        .zip(&keys)
        .map(|(basis, key)| {
            if basis.is_immutable_phase() {
                std::fs::read_to_string(artifact_path(basis)).ok()
            } else {
                cache.get(key)
            }
        })
        .collect::<Vec<_>>();

    // A cold lane has two independent builds per basis. Run those independent
    // compiler jobs concurrently, capped by the same measured memory bound as
    // renderer golden compilation. Results remain indexed in BASIS order, so
    // diagnostics and publication order are deterministic.
    let builds: Vec<std::sync::Mutex<Option<Result<String, String>>>> = (0..BASIS.len() * 2)
        .map(|_| std::sync::Mutex::new(None))
        .collect();
    let cursor = std::sync::atomic::AtomicUsize::new(0);
    let workers = heavy_pixels_jobs().min(BASIS.len() * 2);
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let task = cursor.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if task >= BASIS.len() * 2 {
                        return;
                    }
                    let basis_index = task / 2;
                    if cached[basis_index].is_some() {
                        continue;
                    }
                    let basis = &BASIS[basis_index];
                    let result = if task % 2 == 0 {
                        render_case(basis)
                    } else {
                        render_from_second_build_dir(basis)
                    };
                    *builds[task].lock().unwrap_or_else(|e| e.into_inner()) = Some(result);
                }
            });
        }
    });

    let mut checked = 0usize;
    for (index, basis) in BASIS.iter().enumerate() {
        let (first, cache_hit) = if let Some(text) = &cached[index] {
            (text.clone(), true)
        } else {
            let first = builds[index * 2]
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .ok_or_else(|| {
                    format!("pixels-census: missing first build for `{}`", basis.case)
                })??;
            let second = builds[index * 2 + 1]
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .take()
                .ok_or_else(|| {
                    format!("pixels-census: missing second build for `{}`", basis.case)
                })??;
            if first != second {
                return Err(format!(
                    "pixels-census: `{}` is not deterministic across two runs and two build directories",
                    basis.case,
                ));
            }
            (first, false)
        };
        check_artifact_shape(&first)?;
        let path = artifact_path(basis);
        if update && !basis.is_immutable_phase() {
            write_atomic(&path, &first)?;
            println!("pixels-census: wrote {}", path.display());
        } else {
            let want = std::fs::read_to_string(&path).map_err(|e| {
                format!(
                    "pixels-census: read {}: {e}\nrerun: cargo xtask pixels-census --update",
                    path.display()
                )
            })?;
            if want != first {
                return Err(format!(
                    "pixels-census: `{}` differs from {}\n\
                     Read and explain the diff, then rerun with --update.",
                    basis.case,
                    path.display()
                ));
            }
        }
        if !cache_hit && !basis.is_immutable_phase() {
            cache.put(&keys[index], &first);
        }
        checked += 1;
    }
    check_phase_artifacts()?;
    println!("pixels-census: {checked} basis artifact(s) match {BASELINE_DIR}");
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Cheap on purpose. Everything that needs an actual render is asserted
    /// by `check_artifact_shape` inside `cargo xtask pixels-census`, which is
    /// its own `verify` stage.
    #[test]
    fn every_basis_fixture_exists_and_names_a_distinct_artifact() {
        let mut names: Vec<&str> = BASIS.iter().map(|basis| basis.case).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), BASIS.len(), "basis case names must be unique");
        for basis in BASIS {
            assert!(
                root().join(basis.source).is_file(),
                "basis `{}` source {} does not exist",
                basis.case,
                basis.source,
            );
            assert!(
                artifact_path(basis).is_file(),
                "basis `{}` has no checked-in artifact",
                basis.case,
            );
        }
    }

    #[test]
    fn only_the_final_recensus_is_regenerated_from_the_live_tree() {
        let mutable = BASIS
            .iter()
            .filter(|basis| !basis.is_immutable_phase())
            .map(|basis| basis.case)
            .collect::<Vec<_>>();
        assert_eq!(mutable, ["check-pixels-normal-moments"]);
    }

    #[test]
    fn every_census_cache_key_component_is_load_bearing() {
        let baseline = census_cache_key_from_parts("case", "direct-fp", "source", "tool");
        for changed in [
            census_cache_key_from_parts("other", "direct-fp", "source", "tool"),
            census_cache_key_from_parts("case", "legacy-fp", "source", "tool"),
            census_cache_key_from_parts("case", "direct-fp", "changed", "tool"),
            census_cache_key_from_parts("case", "direct-fp", "source", "changed"),
        ] {
            assert_ne!(baseline, changed);
        }
    }

    #[test]
    fn artifact_shape_rejects_a_leaked_path_and_a_merged_section() {
        let good = format!(
            "# pixels hot-path census\nschema = {}\n\
             phase_backend = legacy-gpr-fp\n\
             source_tree_sha256 = cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc\n\
             image_sha256 = dddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddddd\n\
             kernel_words_sha256 = aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\n\
             codegen_dump_sha256 = bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb\n\
             \n## [M] measured counts\n\
             region.raster.packet_loop words=3\n\n## [I] modelled cycles\n\
             fn.x.proxy_cycles = 5\n",
            hot_census::CENSUS_SCHEMA_VERSION
        );
        check_artifact_shape(&good).expect("the canonical shape");

        let missing_image = good
            .lines()
            .filter(|line| !line.starts_with("image_sha256 = "))
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            check_artifact_shape(&missing_image)
                .unwrap_err()
                .contains("omits image_sha256")
        );

        let leaked = format!("{good}source_dir = {}\n", root().display());
        assert!(
            check_artifact_shape(&leaked)
                .unwrap_err()
                .contains("absolute repository path")
        );

        let merged = good.replace(
            "region.raster.packet_loop words=3",
            "region.raster.packet_loop words=3 proxy_cycles=9",
        );
        assert!(
            check_artifact_shape(&merged)
                .unwrap_err()
                .contains("[M] section")
        );

        let unversioned = good.replace(
            &format!("schema = {}", hot_census::CENSUS_SCHEMA_VERSION),
            "schema = future",
        );
        assert!(
            check_artifact_shape(&unversioned)
                .unwrap_err()
                .contains("schema version")
        );

        let one_section = good.replace("## [I] modelled cycles", "## later");
        assert!(
            check_artifact_shape(&one_section)
                .unwrap_err()
                .contains("[I]")
        );
    }

    #[test]
    fn phase_evidence_rejects_a_missing_required_anchor() {
        check_phase_artifacts().expect("repository phase evidence");
        let error = check_phase_artifact(
            "P8R.5-packet",
            "case = p8r5-packet\nclass.asimd_fp_fma = 1\n",
            &["case = p8r5-packet", "class.fp_store_q"],
        )
        .expect_err("missing operation evidence must fail closed");
        assert!(error.contains("class.fp_store_q"), "{error}");
    }

    #[test]
    fn threshold_narrative_cannot_diverge_from_machine_checked_artifacts() {
        let baseline = root().join(BASELINE_DIR);
        let p8r4 = std::fs::read_to_string(baseline.join("p8r4-direct-fp.txt"))
            .expect("read P8R.4 artifact");
        let final_report =
            std::fs::read_to_string(baseline.join("check-pixels-normal-moments.txt"))
                .expect("read final artifact");
        let decision =
            std::fs::read_to_string(baseline.join("p8r4-direct-fp.md")).expect("read decision");
        let final_delta = std::fs::read_to_string(baseline.join("p8r-final-deltas.md"))
            .expect("read final delta");
        check_threshold_narrative(&p8r4, &final_report, &decision, &final_delta)
            .expect("repository narratives must match");

        let wrong_decision = decision.replace("30 / 34,504", "30 / 34,340");
        assert!(
            check_threshold_narrative(&p8r4, &final_report, &wrong_decision, &final_delta)
                .unwrap_err()
                .contains("machine-checked ratio")
        );
        let wrong_final = final_delta.replace("six-function", "four-function");
        assert!(
            check_threshold_narrative(&p8r4, &final_report, &decision, &wrong_final)
                .unwrap_err()
                .contains("six-function")
        );
    }
}
