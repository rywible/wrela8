//! Conservative camera projection and half-open pixel/tile spans.

use super::camera::CameraContract;
use super::features::FeatureRecord;
use super::ids::FeatureId;
use super::reference::interval::{F64Interval, next_down, next_up};
use super::world_bounds::Aabb64;

pub const TILE_WIDTH_V1: u32 = 16;
pub const TILE_HEIGHT_V1: u32 = 16;
pub const EVENT_COVERAGE_HALO_PIXELS_V1: u32 = 1;
pub const PROFILE_FILTER_HALO_PIXELS_V1: u32 = 1;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NormalizedScreenSpan {
    pub x: F64Interval,
    pub y: F64Interval,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HalfOpenRange {
    pub start: u32,
    pub end: u32,
}

impl HalfOpenRange {
    pub fn is_empty(self) -> bool {
        self.start >= self.end
    }

    pub fn overlaps(self, other: Self) -> bool {
        self.start < other.end && other.start < self.end
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PixelSpan {
    pub x: HalfOpenRange,
    pub y: HalfOpenRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TileSpan {
    pub x: HalfOpenRange,
    pub y: HalfOpenRange,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProjectionRule {
    IntervalProjectiveDivision,
    EyeOrNearPlaneFullScreen,
    UnboundedPlaneFullScreen,
    OutsideNearFar,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProjectedFeatureSpan {
    pub feature: FeatureId,
    pub normalized: NormalizedScreenSpan,
    pub pixels: PixelSpan,
    pub tiles: TileSpan,
    pub q: F64Interval,
    pub rule: ProjectionRule,
    /// Strict outward-rounded world-space distance from the near/far clip
    /// plane. Present exactly when `rule` is `OutsideNearFar`.
    pub outside_margin: Option<f64>,
    pub event_halo: u32,
    pub filter_halo: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraProjectionBox {
    pub eye: [F64Interval; 3],
    /// World-space camera right/up/forward rows.
    pub right: [F64Interval; 3],
    pub up: [F64Interval; 3],
    pub forward: [F64Interval; 3],
    pub aspect: F64Interval,
    pub tan_half_fov_y: F64Interval,
    pub near: f64,
    pub far: f64,
}

impl CameraProjectionBox {
    pub fn from_contract(contract: CameraContract) -> Result<Self, String> {
        Ok(Self {
            eye: contract.eye_component,
            right: contract.right_component,
            up: contract.up_component,
            forward: contract.forward_component,
            aspect: F64Interval::point(contract.aspect)?,
            tan_half_fov_y: F64Interval::point(contract.tan_half_fov_y)?,
            near: super::reference::interval::next_down(1.0 / contract.q.hi),
            far: super::reference::interval::next_up(1.0 / contract.q.lo),
        })
    }
}

fn dot_interval(a: [F64Interval; 3], b: [F64Interval; 3]) -> Result<F64Interval, String> {
    let mut result = F64Interval::point(0.0)?;
    for component in 0..3 {
        result = result.add_outward(a[component].mul_outward(b[component])?)?;
    }
    Ok(result)
}

fn full_screen(
    width: u32,
    height: u32,
    q: F64Interval,
    feature: FeatureId,
    rule: ProjectionRule,
) -> Result<ProjectedFeatureSpan, String> {
    let normalized = NormalizedScreenSpan {
        x: F64Interval::new(-1.0, 1.0)?,
        y: F64Interval::new(-1.0, 1.0)?,
    };
    let pixels = PixelSpan {
        x: HalfOpenRange {
            start: 0,
            end: width,
        },
        y: HalfOpenRange {
            start: 0,
            end: height,
        },
    };
    Ok(ProjectedFeatureSpan {
        feature,
        normalized,
        pixels,
        tiles: pixels_to_tiles(pixels, width, height),
        q,
        rule,
        outside_margin: None,
        event_halo: EVENT_COVERAGE_HALO_PIXELS_V1,
        filter_halo: PROFILE_FILTER_HALO_PIXELS_V1,
    })
}

fn outside_near_far(
    width: u32,
    height: u32,
    q: F64Interval,
    feature: FeatureId,
    margin: f64,
) -> Result<ProjectedFeatureSpan, String> {
    if !margin.is_finite() || margin <= 0.0 {
        return Err(format!(
            "pixels::projection_bounds: invalid outside-near/far margin {margin}"
        ));
    }
    let zero = F64Interval::point(0.0)?;
    let pixels = PixelSpan {
        x: HalfOpenRange { start: 0, end: 0 },
        y: HalfOpenRange { start: 0, end: 0 },
    };
    Ok(ProjectedFeatureSpan {
        feature,
        normalized: NormalizedScreenSpan { x: zero, y: zero },
        pixels,
        tiles: pixels_to_tiles(pixels, width, height),
        q,
        rule: ProjectionRule::OutsideNearFar,
        outside_margin: Some(margin),
        event_halo: EVENT_COVERAGE_HALO_PIXELS_V1,
        filter_halo: PROFILE_FILTER_HALO_PIXELS_V1,
    })
}

fn aabb_relative_to_eye(
    bounds: Aabb64,
    camera: CameraProjectionBox,
) -> Result<[F64Interval; 3], String> {
    Ok([
        F64Interval::new(bounds.min[0], bounds.max[0])?.sub_outward(camera.eye[0])?,
        F64Interval::new(bounds.min[1], bounds.max[1])?.sub_outward(camera.eye[1])?,
        F64Interval::new(bounds.min[2], bounds.max[2])?.sub_outward(camera.eye[2])?,
    ])
}

fn outward_floor(value: f64) -> Result<i64, String> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(format!(
            "pixels::projection_bounds: cannot outward-floor {value}"
        ));
    }
    Ok(next_down(value).floor() as i64)
}

fn outward_ceil(value: f64) -> Result<i64, String> {
    if !value.is_finite() || value < i64::MIN as f64 || value > i64::MAX as f64 {
        return Err(format!(
            "pixels::projection_bounds: cannot outward-ceil {value}"
        ));
    }
    Ok(next_up(value).ceil() as i64)
}

fn clamp_half_open(start: i64, end: i64, extent: u32, halo: u32) -> HalfOpenRange {
    let halo = i64::from(halo);
    HalfOpenRange {
        start: u32::try_from((start - halo).clamp(0, i64::from(extent))).unwrap_or(0),
        end: u32::try_from((end + halo).clamp(0, i64::from(extent))).unwrap_or(extent),
    }
}

pub fn normalized_to_pixels(
    normalized: NormalizedScreenSpan,
    width: u32,
    height: u32,
    halo: u32,
) -> Result<PixelSpan, String> {
    if width == 0 || height == 0 {
        return Err("pixels::projection_bounds: zero output extent".to_string());
    }
    let x_start = (normalized.x.lo + 1.0) * 0.5 * f64::from(width);
    let x_end = (normalized.x.hi + 1.0) * 0.5 * f64::from(width);
    // Framebuffer y grows downward.
    let y_start = (1.0 - normalized.y.hi) * 0.5 * f64::from(height);
    let y_end = (1.0 - normalized.y.lo) * 0.5 * f64::from(height);
    Ok(PixelSpan {
        x: clamp_half_open(outward_floor(x_start)?, outward_ceil(x_end)?, width, halo),
        y: clamp_half_open(outward_floor(y_start)?, outward_ceil(y_end)?, height, halo),
    })
}

pub fn pixels_to_tiles(pixels: PixelSpan, width: u32, height: u32) -> TileSpan {
    let tiles_x = width.div_ceil(TILE_WIDTH_V1);
    let tiles_y = height.div_ceil(TILE_HEIGHT_V1);
    let end_x = if pixels.x.end == 0 {
        0
    } else {
        pixels.x.end.div_ceil(TILE_WIDTH_V1)
    };
    let end_y = if pixels.y.end == 0 {
        0
    } else {
        pixels.y.end.div_ceil(TILE_HEIGHT_V1)
    };
    TileSpan {
        x: HalfOpenRange {
            start: pixels.x.start / TILE_WIDTH_V1,
            end: end_x.min(tiles_x),
        },
        y: HalfOpenRange {
            start: pixels.y.start / TILE_HEIGHT_V1,
            end: end_y.min(tiles_y),
        },
    }
}

pub fn project_aabb(
    feature: FeatureId,
    bounds: Aabb64,
    camera: CameraProjectionBox,
    contract: CameraContract,
    plane: bool,
) -> Result<ProjectedFeatureSpan, String> {
    let relative = aabb_relative_to_eye(bounds, camera)?;
    let view_x = dot_interval(relative, camera.right)?;
    let view_y = dot_interval(relative, camera.up)?;
    let view_z = dot_interval(relative, camera.forward)?;
    if view_z.hi < camera.near {
        let margin = next_down(camera.near - view_z.hi);
        if margin > 0.0 {
            return outside_near_far(contract.width, contract.height, contract.q, feature, margin);
        }
        return full_screen(
            contract.width,
            contract.height,
            contract.q,
            feature,
            ProjectionRule::EyeOrNearPlaneFullScreen,
        );
    }
    if view_z.lo > camera.far {
        let margin = next_down(view_z.lo - camera.far);
        if margin > 0.0 {
            return outside_near_far(contract.width, contract.height, contract.q, feature, margin);
        }
        return full_screen(
            contract.width,
            contract.height,
            contract.q,
            feature,
            ProjectionRule::EyeOrNearPlaneFullScreen,
        );
    }
    if view_z.lo <= camera.near {
        return full_screen(
            contract.width,
            contract.height,
            contract.q,
            feature,
            if plane {
                ProjectionRule::UnboundedPlaneFullScreen
            } else {
                ProjectionRule::EyeOrNearPlaneFullScreen
            },
        );
    }
    let x_denominator = camera
        .aspect
        .mul_outward(camera.tan_half_fov_y)?
        .mul_outward(view_z)?;
    let y_denominator = camera.tan_half_fov_y.mul_outward(view_z)?;
    if x_denominator.contains_zero() || y_denominator.contains_zero() {
        return full_screen(
            contract.width,
            contract.height,
            contract.q,
            feature,
            ProjectionRule::EyeOrNearPlaneFullScreen,
        );
    }
    let x = view_x.div_outward(x_denominator)?;
    let y = view_y.div_outward(y_denominator)?;
    let normalized = NormalizedScreenSpan {
        x: F64Interval::new(x.lo.max(-1.0), x.hi.min(1.0)).unwrap_or(F64Interval::new(-1.0, 1.0)?),
        y: F64Interval::new(y.lo.max(-1.0), y.hi.min(1.0)).unwrap_or(F64Interval::new(-1.0, 1.0)?),
    };
    let total_halo = EVENT_COVERAGE_HALO_PIXELS_V1
        .checked_add(PROFILE_FILTER_HALO_PIXELS_V1)
        .ok_or_else(|| "P015: projection halo overflow".to_string())?;
    let pixels = normalized_to_pixels(normalized, contract.width, contract.height, total_halo)?;
    // A finite feature whose real span is below one pixel must still occupy a
    // nonempty range after outward conversion.
    if pixels.x.is_empty() || pixels.y.is_empty() {
        return full_screen(
            contract.width,
            contract.height,
            contract.q,
            feature,
            ProjectionRule::EyeOrNearPlaneFullScreen,
        );
    }
    let Some(q) = view_z.reciprocal()?.intersect(contract.q) else {
        return full_screen(
            contract.width,
            contract.height,
            contract.q,
            feature,
            ProjectionRule::EyeOrNearPlaneFullScreen,
        );
    };
    Ok(ProjectedFeatureSpan {
        feature,
        normalized,
        pixels,
        tiles: pixels_to_tiles(pixels, contract.width, contract.height),
        q,
        rule: ProjectionRule::IntervalProjectiveDivision,
        outside_margin: None,
        event_halo: EVENT_COVERAGE_HALO_PIXELS_V1,
        filter_halo: PROFILE_FILTER_HALO_PIXELS_V1,
    })
}

pub fn derive(
    features: &[FeatureRecord],
    equations: &super::projective::ProjectiveEquations,
) -> Result<Vec<ProjectedFeatureSpan>, String> {
    let camera = CameraProjectionBox::from_contract(equations.camera)?;
    features
        .iter()
        .map(|feature| {
            project_aabb(
                feature.id,
                feature.world_bounds,
                camera,
                equations.camera,
                feature.kind == super::primitive::FeatureKind::Plane,
            )
        })
        .collect()
}

/// Independent point-sampling containment oracle used only by the compiler
/// verifier. It intentionally evaluates scalar camera algebra rather than the
/// interval projection implementation.
pub fn verify_independent_corner_samples(
    bounds: Aabb64,
    camera: CameraProjectionBox,
    contract: CameraContract,
    span: &ProjectedFeatureSpan,
) -> Result<(), String> {
    let midpoint = |interval: F64Interval| interval.lo + (interval.hi - interval.lo) * 0.5;
    let basis_samples = [
        (
            camera.right.map(midpoint),
            camera.up.map(midpoint),
            camera.forward.map(midpoint),
        ),
        (
            camera.right.map(|interval| interval.lo),
            camera.up.map(|interval| interval.lo),
            camera.forward.map(|interval| interval.lo),
        ),
        (
            camera.right.map(|interval| interval.hi),
            camera.up.map(|interval| interval.hi),
            camera.forward.map(|interval| interval.hi),
        ),
    ];
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    for eye_x in [camera.eye[0].lo, camera.eye[0].hi] {
        for eye_y in [camera.eye[1].lo, camera.eye[1].hi] {
            for eye_z in [camera.eye[2].lo, camera.eye[2].hi] {
                for (right, up, forward) in basis_samples {
                    for x in [bounds.min[0], bounds.max[0]] {
                        for y in [bounds.min[1], bounds.max[1]] {
                            for z in [bounds.min[2], bounds.max[2]] {
                                let relative = [x - eye_x, y - eye_y, z - eye_z];
                                let view_z = dot(forward, relative);
                                if view_z < camera.near || view_z > camera.far {
                                    continue;
                                }
                                let normalized_x = dot(right, relative)
                                    / (camera.aspect.lo * camera.tan_half_fov_y.lo * view_z);
                                let normalized_y =
                                    dot(up, relative) / (camera.tan_half_fov_y.lo * view_z);
                                if !(-1.0..=1.0).contains(&normalized_x)
                                    || !(-1.0..=1.0).contains(&normalized_y)
                                {
                                    continue;
                                }
                                let pixel_x = ((((normalized_x + 1.0) * 0.5)
                                    * f64::from(contract.width))
                                .floor() as u32)
                                    .min(contract.width - 1);
                                let pixel_y = ((((1.0 - normalized_y) * 0.5)
                                    * f64::from(contract.height))
                                .floor() as u32)
                                    .min(contract.height - 1);
                                if !(span.pixels.x.start <= pixel_x
                                    && pixel_x < span.pixels.x.end
                                    && span.pixels.y.start <= pixel_y
                                    && pixel_y < span.pixels.y.end)
                                {
                                    return Err(format!(
                                        "pixels::projection_bounds: independent corner ({x},{y},{z}) projects to ({pixel_x},{pixel_y}) outside feature {} span",
                                        span.feature
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }
    Ok(())
}

pub fn exact_max_overlap(spans: &[ProjectedFeatureSpan]) -> Result<(u32, u32), String> {
    let mut row_endpoints = Vec::new();
    for span in spans {
        row_endpoints.push((span.pixels.y.start, 1_i32));
        row_endpoints.push((span.pixels.y.end, -1_i32));
    }
    row_endpoints.sort_by_key(|(position, delta)| (*position, *delta));
    let mut active = 0_i32;
    let mut max_row = 0_i32;
    for (_, delta) in row_endpoints {
        active += delta;
        max_row = max_row.max(active);
    }
    let max_tile = spans
        .iter()
        .flat_map(|span| {
            (span.tiles.y.start..span.tiles.y.end)
                .flat_map(move |y| (span.tiles.x.start..span.tiles.x.end).map(move |x| (x, y)))
        })
        .fold(
            std::collections::BTreeMap::<(u32, u32), u32>::new(),
            |mut counts, tile| {
                *counts.entry(tile).or_default() += 1;
                counts
            },
        )
        .into_values()
        .max()
        .unwrap_or(0);
    Ok((
        u32::try_from(max_row)
            .map_err(|_| "P015: projected row overlap exceeds u32".to_string())?,
        max_tile,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> CameraContract {
        CameraContract {
            width: 64,
            height: 32,
            aspect: 2.0,
            tan_half_fov_y: 1.0,
            q: F64Interval::new(0.1, 10.0).unwrap(),
            eye_component: [F64Interval::point(0.0).unwrap(); 3],
            forward_component: [
                F64Interval::point(0.0).unwrap(),
                F64Interval::point(0.0).unwrap(),
                F64Interval::point(1.0).unwrap(),
            ],
            right_component: [
                F64Interval::point(1.0).unwrap(),
                F64Interval::point(0.0).unwrap(),
                F64Interval::point(0.0).unwrap(),
            ],
            up_component: [
                F64Interval::point(0.0).unwrap(),
                F64Interval::point(1.0).unwrap(),
                F64Interval::point(0.0).unwrap(),
            ],
            eye_rate_component: [F64Interval::point(0.0).unwrap(); 3],
            forward_rate_component: [F64Interval::point(0.0).unwrap(); 3],
            right_rate_component: [F64Interval::point(0.0).unwrap(); 3],
            up_rate_component: [F64Interval::point(0.0).unwrap(); 3],
            max_frame_motion: 0.0,
        }
    }

    fn fixed_camera() -> CameraProjectionBox {
        let zero = F64Interval::point(0.0).unwrap();
        let one = F64Interval::point(1.0).unwrap();
        CameraProjectionBox {
            eye: [zero; 3],
            right: [one, zero, zero],
            up: [zero, one, zero],
            forward: [zero, zero, one],
            aspect: F64Interval::point(2.0).unwrap(),
            tan_half_fov_y: one,
            near: 0.1,
            far: 10.0,
        }
    }

    #[test]
    fn projective_division_and_half_open_edges_are_outward() {
        let span = project_aabb(
            FeatureId(0),
            Aabb64::new([-0.01, -0.01, 1.0], [0.01, 0.01, 1.01]).unwrap(),
            fixed_camera(),
            contract(),
            false,
        )
        .unwrap();
        assert!(!span.pixels.x.is_empty());
        assert!(!span.pixels.y.is_empty());
        assert!(span.pixels.x.start <= 31 && span.pixels.x.end >= 33);
        assert!(span.pixels.y.start <= 15 && span.pixels.y.end >= 17);
    }

    #[test]
    fn eye_or_near_crossing_expands_to_full_screen() {
        let span = project_aabb(
            FeatureId(0),
            Aabb64::new([-1.0, -1.0, 0.05], [1.0, 1.0, 0.2]).unwrap(),
            fixed_camera(),
            contract(),
            false,
        )
        .unwrap();
        assert_eq!(span.rule, ProjectionRule::EyeOrNearPlaneFullScreen);
        assert_eq!(span.pixels.x, HalfOpenRange { start: 0, end: 64 });
        assert_eq!(span.pixels.y, HalfOpenRange { start: 0, end: 32 });
    }

    #[test]
    fn feature_strictly_outside_far_plane_has_an_empty_audited_span() {
        let span = project_aabb(
            FeatureId(0),
            Aabb64::new([-1.0, -1.0, 20.0], [1.0, 1.0, 21.0]).unwrap(),
            fixed_camera(),
            contract(),
            false,
        )
        .unwrap();
        assert_eq!(span.rule, ProjectionRule::OutsideNearFar);
        assert!(span.pixels.x.is_empty());
        assert!(span.pixels.y.is_empty());
        assert!(span.tiles.x.is_empty());
        assert!(span.tiles.y.is_empty());
        let margin = span.outside_margin.expect("audited far-plane gap");
        assert!(margin > 9.0 && margin <= 10.0);
    }

    #[test]
    fn outside_margin_is_the_outward_rounded_clip_gap() {
        let span = project_aabb(
            FeatureId(0),
            Aabb64::new([-0.1, -0.1, 10.001], [0.1, 0.1, 10.002]).unwrap(),
            fixed_camera(),
            contract(),
            false,
        )
        .unwrap();
        let margin = span.outside_margin.expect("strict far-plane gap");
        assert!(margin > 0.0);
        assert!(margin <= 0.001);
        assert_ne!(margin, 1.0);
    }

    #[test]
    fn independent_corner_oracle_is_contained_for_representative_feature_boxes() {
        let zero = F64Interval::point(0.0).unwrap();
        let one = F64Interval::point(1.0).unwrap();
        let eye_xy = F64Interval::new(-0.125, 0.125).unwrap();
        let camera = CameraProjectionBox {
            eye: [eye_xy, eye_xy, zero],
            right: [one, zero, zero],
            up: [zero, one, zero],
            forward: [zero, zero, one],
            aspect: F64Interval::point(2.0).unwrap(),
            tan_half_fov_y: one,
            near: 0.1,
            far: 10.0,
        };
        // Thin, transformed, repeated-cell-like, and deformation-expanded
        // boxes exercise the permanent fixture shapes without sharing the
        // interval projection implementation.
        let boxes = [
            Aabb64::new([-0.000_01, -0.5, 1.0], [0.000_01, 0.5, 1.001]).unwrap(),
            Aabb64::new([0.7, -0.4, 2.0], [1.1, 0.2, 2.7]).unwrap(),
            Aabb64::new([-2.0, -0.1, 4.0], [-1.7, 0.1, 4.3]).unwrap(),
            Aabb64::new([-0.8, -0.8, 0.8], [0.8, 0.8, 2.2]).unwrap(),
        ];
        for (feature, bounds) in boxes.into_iter().enumerate() {
            let span =
                project_aabb(FeatureId(feature as u32), bounds, camera, contract(), false).unwrap();
            assert_ne!(span.rule, ProjectionRule::OutsideNearFar);
            for eye_x in [-0.125, 0.125] {
                for eye_y in [-0.125, 0.125] {
                    for x in [bounds.min[0], bounds.max[0]] {
                        for y in [bounds.min[1], bounds.max[1]] {
                            for z in [bounds.min[2], bounds.max[2]] {
                                if !(camera.near..=camera.far).contains(&z) {
                                    continue;
                                }
                                let normalized_x = (x - eye_x) / (2.0 * z);
                                let normalized_y = (y - eye_y) / z;
                                if !(-1.0..=1.0).contains(&normalized_x)
                                    || !(-1.0..=1.0).contains(&normalized_y)
                                {
                                    continue;
                                }
                                let pixel_x = (((normalized_x + 1.0) * 0.5) * 64.0).floor() as u32;
                                let pixel_y = (((1.0 - normalized_y) * 0.5) * 32.0).floor() as u32;
                                assert!(
                                    span.pixels.x.start <= pixel_x.min(63)
                                        && pixel_x.min(63) < span.pixels.x.end
                                );
                                assert!(
                                    span.pixels.y.start <= pixel_y.min(31)
                                        && pixel_y.min(31) < span.pixels.y.end
                                );
                            }
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn exact_integer_and_subpixel_conversions_remain_half_open_nonempty() {
        let exact = normalized_to_pixels(
            NormalizedScreenSpan {
                x: F64Interval::point(0.0).unwrap(),
                y: F64Interval::point(0.0).unwrap(),
            },
            64,
            32,
            0,
        )
        .unwrap();
        // An exact zero-width analytic bound is outward-rounded to the
        // adjacent ownership cells.
        assert!(exact.x.start < exact.x.end);
        assert!(exact.y.start < exact.y.end);
    }
}
