//! The single-source compiler-reserved Pixels surface.
//!
//! A generated Pixels intrinsic used to need four hand-edited syncs — the
//! `glue` generator, the `loader` empty-image stub, the two `sema` binding
//! lists, and the `core.render` import line — and missing any one of them
//! surfaced only when a guest stdlib compiled, milestones after the edit.
//! This table is the one place that knows the surface: `loader` builds its
//! stub module from it, `sema` binds renderer bodies from it, and the tests
//! below pin it against `stdlib/core/render.wr` and the `glue` output so a
//! name can no longer exist in three of the four places.
//!
//! Auditing what it caught the day it landed: `sema` bound two symbols
//! (`__wrela_pixels_p7_feature_x_invariant`, `__wrela_pixels_p7_object_scalar`)
//! that no generator has ever emitted, and five stub entries
//! (`__wrela_pixels_p7_filter_horner` and friends) named functions that appear
//! in no generator and no stdlib module at all.

/// Which module owns a reserved symbol's definition.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SurfaceOrigin {
    /// Written by hand in `stdlib/core/render.wr`.
    CoreRender,
    /// Emitted per image by `pixels::glue` into `core.__image_pixels`.
    ImagePixels,
}

pub struct SurfaceSymbol {
    pub name: &'static str,
    pub origin: SurfaceOrigin,
    /// Parameter list and return type, e.g. `(renderer: usize) -> bool`.
    /// Empty for `CoreRender` symbols, which the loader never stubs.
    pub signature: &'static str,
    /// Body of the empty-image stub, newline separated and already indented.
    pub empty_body: &'static str,
    /// Whether `sema` injects this name into renderer bodies. Generated
    /// helpers that only other generated code calls are `false`: they still
    /// need a stub so an empty image typechecks, but nothing may name them.
    pub injected: bool,
}

impl SurfaceSymbol {
    pub fn is_image_pixels(&self) -> bool {
        self.origin == SurfaceOrigin::ImagePixels
    }
}

/// Names injected into renderer bodies from `core.render`.
pub fn injected_core_render_names() -> impl Iterator<Item = &'static str> {
    SURFACE
        .iter()
        .filter(|symbol| symbol.origin == SurfaceOrigin::CoreRender && symbol.injected)
        .map(|symbol| symbol.name)
}

/// Names injected into renderer bodies from `core.__image_pixels`.
pub fn injected_image_pixels_names() -> impl Iterator<Item = &'static str> {
    SURFACE
        .iter()
        .filter(|symbol| symbol.is_image_pixels() && symbol.injected)
        .map(|symbol| symbol.name)
}

/// The `core.__image_pixels` module used when a package compiles with no
/// sealed renderer: every generated intrinsic present with a fail-closed
/// body, so ordinary code typechecks and the Pixels path stays inert.
pub fn empty_image_pixels_source() -> String {
    let mut source = String::from(
        "module __image_pixels\n\n\
         from core.render_interval import FixedDomain, Iv32\n\n\
         pub const N_RENDERERS: usize = 0\n\
         pub const N_RENDER_WORKERS: usize = 0\n",
    );
    for symbol in SURFACE.iter().filter(|symbol| symbol.is_image_pixels()) {
        source.push_str("\npub fn ");
        source.push_str(symbol.name);
        source.push_str(symbol.signature);
        source.push_str(":\n");
        source.push_str(symbol.empty_body);
        source.push('\n');
    }
    source
}

/// Every symbol the compiler injects into renderer bodies or generates
/// into `core.__image_pixels`, in one table.
pub const SURFACE: &[SurfaceSymbol] = &[
    SurfaceSymbol {
        name: "__wrela_pixels_f32_to_bits",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(value: f32) -> u32",
        empty_body: "    return value.to[u32]()",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_program_validate",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_program_header",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [u64; 6]",
        empty_body: "    return [0; 6]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_program_digest_byte",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, byte: usize) -> u8",
        empty_body: "    return 0",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_tile_feature_count",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, tile: u32) -> u32",
        empty_body: "    return 4294967295",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_tile_feature",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, tile: u32, ordinal: u32) -> [u64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_tile_event_count",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, tile: u32) -> u32",
        empty_body: "    return 4294967295",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_tile_event",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, tile: u32, ordinal: u32) -> [u64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_program_record",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, table: u16, id: u32) -> [u64; 5]",
        empty_body: "    return [0; 5]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_program_operand",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, table: u16, id: u32, ordinal: u16) -> [u64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_abs",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(value: f32) -> f32",
        empty_body: "    if value < 0.0:\n        return -value\n    return value",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_power",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(value: f32, exponent: u64) -> [f32; 2]",
        empty_body: "    return [0.0; 2]",
        injected: false,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_root_coefficient",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, target: u32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 2]",
        empty_body: "    return [0.0; 2]",
        injected: false,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_root_polynomial_base",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> u32",
        empty_body: "    return 4294967295",
        injected: false,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_feature_polynomial",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32, u: f32, v: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 11]",
        empty_body: "    return [0.0; 11]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_feature_polynomial_uv_bounds",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32) -> [f32; 20]",
        empty_body: "    return [0.0; 20]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_event_polynomial",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, event: u32, u: f32, v: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 11]",
        empty_body: "    return [0.0; 11]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_torus_event_magnitudes",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, event: u32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 65]",
        empty_body: "    return [0.0; 65]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_standard_torus_coefficients",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, event: u32, read camera: [f32; 12]) -> [f32; 57]",
        empty_body: "    return [0.0; 57]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_standard_interval_mul",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(read a: [f32; 2], read b: [f32; 2]) -> [f32; 2]",
        empty_body: "    return [0.0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_standard_torus_cell_positive_hit",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(u: f32, v: f32, ru: f32, rv: f32, eye: f32) -> [i64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_standard_torus_feature",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_standard_torus_pixel_bounds",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(read coefficients: [f32; 57], u: f32, v: f32, ru: f32, rv: f32) -> [f32; 8]",
        empty_body: "    return [0.0; 8]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_standard_torus_value",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(read coefficients: [f32; 57], u: f32, v: f32) -> [f32; 3]",
        empty_body: "    return [0.0; 3]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_standard_torus_discriminant",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(read coefficients: [f32; 57], read pixel_bounds: [f32; 8], u: f32, v: f32, ru: f32, rv: f32) -> [f32; 5]",
        empty_body: "    return [0.0; 5]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_standard_torus_positive_hit",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(u: f32, v: f32, eye: f32) -> [i64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_event_polynomial_uv2_bounds",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, event: u32, u: f32, v: f32, ru: f32, rv: f32, read magnitudes: [f32; 65]) -> [f32; 4]",
        empty_body: "    return [0.0; 4]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_event_clip_q",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, event: u32) -> [f32; 2]",
        empty_body: "    return [0.0, 0.0]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_event_predicate_curve",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, event: u32, u: f32, v: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 20]",
        empty_body: "    return [0.0; 20]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_deformation_miss_model",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, event: u32) -> [f32; 21]",
        empty_body: "    return [0.0; 21]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_pinned_camera",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [f32; 13]",
        empty_body: "    return [0.0; 13]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_projected_union_mode",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> u64",
        empty_body: "    return 0",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_feature_filter_excludes_root",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32, read uv: [f32; 4], read q: [i32; 2], read params: [f32; 16], read camera: [f32; 12], exponent: i32) -> [i64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_feature_normal",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32, u: f32, v: f32, q: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 4]",
        empty_body: "    return [0.0; 4]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_feature_support_q_span",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32, object: u32, read uv: [f32; 2], read q_domain: [i64; 3]) -> [i64; 3]",
        empty_body: "    return [0; 3]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_material_event_coverage",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, x: u32, y: u32, q: f32, hit: bool, read params: [f32; 16], read camera: [f32; 12]) -> [i64; 3]",
        empty_body: "    return [0; 3]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_feature_valid",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32, u: f32, v: f32, q: f32, read params: [f32; 16], read camera: [f32; 12]) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_feature_valid_filter",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32, read uv: [f32; 4], read q: [i32; 2], exponent: i32, read params: [f32; 16], read camera: [f32; 12]) -> [i64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_feature_q_span",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32) -> [f32; 3]",
        empty_body: "    return [0.0; 3]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_feature_world_bounds",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, feature: u32) -> [f32; 7]",
        empty_body: "    return [0.0; 7]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_object_composed_root",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, object: u32) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_object_support",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, object: u32) -> [f32; 2]",
        empty_body: "    return [0.0; 2]",
        injected: false,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_object_q_tube",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, object: u32, read uv: [f32; 4], q_lo: f32, q_hi: f32, read params: [f32; 16], read camera: [f32; 12]) -> [f32; 7]",
        empty_body: "    return [0.0; 7]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_initial_inside",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, u: f32, v: f32, q_near: f32, read params: [f32; 16], read camera: [f32; 12]) -> [u64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_polynomial_at_q",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(read polynomial: [f32; 11], q: f32) -> [f32; 2]",
        empty_body: "    return [0.0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_param_slot",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, path_key: u64) -> [u64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_numeric_config",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [i64; 14]",
        empty_body: "    return [0; 14]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_csg_occupancy",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, inside_bits: u64) -> [i64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_event_class",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(representation: u64, kind: u64) -> u64",
        empty_body: "    return 0",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_worker_error_class",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(error: u8) -> u8",
        empty_body: "    return 0",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_csg_influence",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> u64",
        empty_body: "    return 18446744073709551615",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_frame_snapshot_store",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, read params: [f32; 16], param_count: u16, read camera: [f32; 12], read light_kinds: [u64; 8], read light_scalars: [f32; 120], read post: [f32; 4], frame_index: u64) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_frame_snapshot_params",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [f32; 16]",
        empty_body: "    return [0.0; 16]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_frame_snapshot_camera",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [f32; 12]",
        empty_body: "    return [0.0; 12]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_frame_snapshot_light_kinds",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [u64; 8]",
        empty_body: "    return [0; 8]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_frame_snapshot_light_scalars",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [f32; 120]",
        empty_body: "    return [0.0; 120]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_frame_snapshot_post",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [f32; 4]",
        empty_body: "    return [0.0; 4]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_frame_snapshot_meta",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [u64; 3]",
        empty_body: "    return [0; 3]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_worker_assignment",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32) -> [u64; 7]",
        empty_body: "    return [0; 7]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_framebuffer_reset",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_framebuffer_write",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, pixel: u32, r: u8, g: u8, b: u8, a: u8) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_framebuffer_byte",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, byte: usize) -> [u64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_framebuffer_digest",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> [u64; 5]",
        empty_body: "    return [0; 5]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_framebuffer_alpha_samples",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize) -> u64",
        empty_body: "    return 18446744073709551615",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_framebuffer_pixel_written",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, pixel: u32) -> bool",
        empty_body: "    return false",
        injected: false,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_reset",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, generation: u64) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_charge",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, slot: usize, amount: u64) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_counter",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, slot: usize) -> [u64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_store_coverage",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, corridor: bool, record: u32, read values: [i64; 8]) -> bool",
        empty_body: "    return false",
        injected: false,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_store_certified_run",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, record: u32, read values: [i64; 8], read model: [i64; 8], read normal: [i64; 6], read slacks: [i64; 2], sample_meta: i64) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_load_certified_run_word",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, record: u32, word: usize) -> [u64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_store_root",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, record: u32, read values: [i64; 8]) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_load_root",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, record: u32) -> [i64; 9]",
        empty_body: "    return [0; 9]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_store_root_tmp",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, record: u32, read values: [i64; 8]) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_workspace_load_root_tmp",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, record: u32) -> [i64; 9]",
        empty_body: "    return [0; 9]",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_telemetry_reset",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_telemetry_charge",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, counter: usize, amount: u64) -> bool",
        empty_body: "    return false",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_telemetry_counter",
        origin: SurfaceOrigin::ImagePixels,
        signature: "(renderer: usize, worker: u32, counter: usize) -> [u64; 2]",
        empty_body: "    return [0; 2]",
        injected: true,
    },
    SurfaceSymbol {
        name: "Renderer",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RendererWorker0",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RendererWorker1",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RendererWorker2",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RendererWorker3",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RendererWorkers",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "FrameInputSnapshot",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RendererWorkerAssignment",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RendererWorkerJob",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "P7CertifiedRow",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "P7RowCertificationContext",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "P7BoolResult",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "P7RootList",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "P7RootContext",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "P7WalkTarget",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "P7StoredRoot",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "P7RowCandidates",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "P7VisibilitySample",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RendererFrameBounds",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RenderPath",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RenderFrame",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RenderedFrame",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "RenderError",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "Camera",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "Light",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "LightFrame",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p5_finite",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_program_validate",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_program_digest_byte",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_run_job",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_digest_result",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_interval_from_f32",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_analytic_front",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_active_object_feature",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_append_root",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_collect_roots",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_collect_roots_box",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_certify_row",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_csg_evaluate",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_debug_frame_digest",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_debug_alpha_samples",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_debug_frame_dump_word",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_density_bin",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_deformation_misses_cell",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_interval_sin",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_event_pixel_span",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_event_polynomial_at_q",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_half_plane_byte",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_half_plane_units",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_predicate_curve_cell",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_predicate_region_coverage",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_predicate_region_area",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_arrangement_boundary_crosses",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_combined_arrangement_coverage",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_clip_curve_misses_cell",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_point_union_occupancy",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_quartic_discriminant",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_event_coverage",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_structural_corridor_pixel",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_certify_pixel_row",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_choose_arrangement_axis",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_resolve_silhouette_owner",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_resolve_pixel_arrangement",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_try_certify_regular_row",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_union_silhouette_coverage",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_union_silhouette_coverage_at_slack",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_event_owner",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_isolate_power_roots",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_isolate_smooth_object",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_object_identity_span",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_job_value",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_pixel_visibility_from_candidates",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_proposal_matches",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_load_root",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_row_candidates",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_row_candidates_with_proposal",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_raw_to_f32",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_run_length_bin",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_charge_run_telemetry",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_revalidated_proposal_count",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_run_worker_with_proposals",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_select_visibility",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_silhouette_coverage",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_deformation_silhouette_misses",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_support_q_span",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_smooth_object_q_span",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_store_root",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_verify_analytic_front",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_worker_error",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_worker_error_code",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "__wrela_pixels_p7_worker_error_tile",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "pixels_p7_worker_job_4_0",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "pixels_p7_worker_job_4_1",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "pixels_p7_worker_job_4_2",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
    SurfaceSymbol {
        name: "pixels_p7_worker_job_4_3",
        origin: SurfaceOrigin::CoreRender,
        signature: "",
        empty_body: "",
        injected: true,
    },
];

#[cfg(test)]
mod tests {
    use super::*;

    fn render_wr() -> String {
        std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib/core/render.wr"),
        )
        .expect("stdlib/core/render.wr is readable")
    }

    /// Every name any stdlib module imports from the generated image module.
    /// A name here that the table does not carry would resolve only when a
    /// real image is sealed, and would fail every empty-image build.
    fn stdlib_image_pixels_imports() -> std::collections::BTreeSet<String> {
        let stdlib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
        let mut imports = std::collections::BTreeSet::new();
        let mut directories = vec![stdlib];
        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(&directory).expect("stdlib is readable") {
                let path = entry.expect("stdlib entry").path();
                if path.is_dir() {
                    directories.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "wr") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("stdlib module is readable");
                for line in source.lines() {
                    let Some(names) = line.strip_prefix("from core.__image_pixels import ") else {
                        continue;
                    };
                    imports.extend(names.split(',').map(|name| name.trim().to_string()));
                }
            }
        }
        assert!(
            !imports.is_empty(),
            "no stdlib module imports the generated image module"
        );
        imports
    }

    #[test]
    fn names_are_unique() {
        let mut seen = std::collections::BTreeSet::new();
        for symbol in SURFACE {
            assert!(
                seen.insert(symbol.name),
                "`{}` appears twice in the surface table",
                symbol.name
            );
        }
    }

    #[test]
    fn every_generated_intrinsic_is_in_a_compiler_reserved_namespace() {
        // Generated symbols are the ones the reserved-name fence has to
        // protect: user code must never be able to define or spell one.
        // `CoreRender` entries are deliberately mixed — `Renderer`, `Camera`
        // and friends are ordinary public stdlib API that renderer bodies
        // name directly.
        for symbol in SURFACE.iter().filter(|symbol| symbol.is_image_pixels()) {
            assert!(
                crate::sema::is_compiler_reserved_source_name(symbol.name),
                "generated intrinsic `{}` is outside the reserved namespace",
                symbol.name
            );
        }
    }

    #[test]
    fn every_image_pixels_symbol_has_a_stub_and_every_stub_parses() {
        for symbol in SURFACE.iter().filter(|symbol| symbol.is_image_pixels()) {
            assert!(
                symbol.signature.starts_with('(') && symbol.signature.contains("->"),
                "generated intrinsic `{}` has no stub signature; an empty image \
                 would not typecheck",
                symbol.name
            );
            assert!(
                !symbol.empty_body.is_empty(),
                "generated intrinsic `{}` has no stub body",
                symbol.name
            );
        }
        let source = empty_image_pixels_source();
        let tokens = crate::syntax::lexer::lex(&source).expect("the stub module lexes");
        crate::syntax::parser::parse(tokens).expect("the stub module parses");
    }

    #[test]
    fn core_render_defines_every_symbol_it_is_credited_with() {
        let source = render_wr();
        for symbol in SURFACE
            .iter()
            .filter(|symbol| symbol.origin == SurfaceOrigin::CoreRender)
        {
            let defined = source.contains(&format!("fn {}(", symbol.name))
                || source.contains(&format!("struct {}:", symbol.name))
                || source.contains(&format!("struct {}[", symbol.name))
                || source.contains(&format!("enum {}:", symbol.name));
            assert!(
                defined,
                "the surface table injects `{}` from core.render, but \
                 stdlib/core/render.wr does not define it",
                symbol.name
            );
        }
    }

    #[test]
    fn every_generated_intrinsic_a_stdlib_module_calls_is_imported_by_it() {
        // The failure this prevents actually happened: `core.render` gained a
        // call to `__wrela_pixels_p7_event_class` without the matching import
        // line, and instead of failing to resolve it silently bound the
        // empty-image stub. The stub answers 0 to everything, which reads as
        // "not a curve, bounds no occupancy" — so the analytic coverage tier
        // declined every pixel and a permanent fixture went red with an
        // exhausted certificate, three layers away from the missing import.
        let stdlib = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../stdlib");
        let generated: Vec<&str> = SURFACE
            .iter()
            .filter(|symbol| symbol.is_image_pixels())
            .map(|symbol| symbol.name)
            .collect();
        let mut directories = vec![stdlib];
        while let Some(directory) = directories.pop() {
            for entry in std::fs::read_dir(&directory).expect("stdlib is readable") {
                let path = entry.expect("stdlib entry").path();
                if path.is_dir() {
                    directories.push(path);
                    continue;
                }
                if path.extension().is_none_or(|extension| extension != "wr") {
                    continue;
                }
                let source = std::fs::read_to_string(&path).expect("stdlib module is readable");
                let imported: std::collections::BTreeSet<&str> = source
                    .lines()
                    .filter_map(|line| line.strip_prefix("from core.__image_pixels import "))
                    .flat_map(|names| names.split(',').map(str::trim))
                    .collect();
                for name in &generated {
                    if imported.contains(name) {
                        continue;
                    }
                    // Word-boundary search: `..._event_class` must not be
                    // satisfied by `..._event_classifier`.
                    let called = source
                        .match_indices(name)
                        .any(|(at, _)| source[at + name.len()..].starts_with('('));
                    assert!(
                        !called,
                        "{} calls `{name}` but does not import it from \
                         core.__image_pixels; it would silently bind the \
                         empty-image stub instead of the generated intrinsic",
                        path.display()
                    );
                }
            }
        }
    }

    #[test]
    fn every_generated_intrinsic_is_live() {
        // A table entry nobody generates and nobody imports is dead weight
        // that still has to be kept in sync. This is the check that retired
        // five phantom stubs (`__wrela_pixels_p7_filter_horner` and friends,
        // which appeared in no generator and no stdlib module) and two dead
        // sema bindings (`__wrela_pixels_p7_object_scalar`,
        // `__wrela_pixels_p7_feature_x_invariant`).
        let glue = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src/pixels/glue.rs"),
        )
        .expect("the generator source is readable");
        let imported = stdlib_image_pixels_imports();
        for symbol in SURFACE.iter().filter(|symbol| symbol.is_image_pixels()) {
            assert!(
                imported.contains(symbol.name) || glue.contains(symbol.name),
                "`{}` is in the surface table but no generator emits it and no \
                 stdlib module imports it; remove the entry",
                symbol.name
            );
        }
    }

    #[test]
    fn every_stdlib_import_of_the_generated_module_is_registered() {
        // The failure this prevents: a new intrinsic imported by a stdlib
        // module but missing from the table gets no empty-image stub, so
        // every package that compiles without a sealed renderer breaks.
        let registered: std::collections::BTreeSet<&str> = SURFACE
            .iter()
            .filter(|symbol| symbol.is_image_pixels())
            .map(|symbol| symbol.name)
            .collect();
        for imported in stdlib_image_pixels_imports() {
            assert!(
                registered.contains(imported.as_str()),
                "a stdlib module imports `{imported}` from the generated image \
                 module, but the surface table has no entry; add it to SURFACE \
                 (crates/wrela-compiler/src/pixels/surface.rs) with its \
                 empty-image stub"
            );
        }
    }

    #[test]
    fn every_injected_generated_intrinsic_is_importable() {
        let imported = stdlib_image_pixels_imports();
        for name in injected_image_pixels_names() {
            assert!(
                imported.contains(name),
                "`{name}` is injected into renderer bodies but no stdlib module \
                 imports it from the generated image module; either import it \
                 or clear its `injected` flag"
            );
        }
    }
}
