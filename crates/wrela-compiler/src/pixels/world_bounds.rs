//! Structural world-space bounds independent of screen samples.

use std::collections::{BTreeMap, BTreeSet};

use super::bounds::ValueBounds;
use super::config::RendererConfig;
use super::graph::{Axis, FieldKind, Primitive, TransformProgram};
use super::ids::{FieldId, ScalarId};
use super::reference::interval::F64Interval;
use super::reference::interval::{next_down, next_up, next_up_f32};
use super::support::SupportTable;
use super::symbolic::SymbolicGraph;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb64 {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb64 {
    pub fn new(min: [f64; 3], max: [f64; 3]) -> Result<Self, String> {
        if min
            .iter()
            .chain(max.iter())
            .any(|component| !component.is_finite())
        {
            return Err(format!(
                "pixels::world_bounds: non-finite AABB {min:?}..{max:?}"
            ));
        }
        if (0..3).any(|component| min[component] > max[component]) {
            return Err(format!("pixels::world_bounds: empty AABB {min:?}..{max:?}"));
        }
        Ok(Self { min, max })
    }

    pub fn union(self, other: Self) -> Self {
        Self {
            min: std::array::from_fn(|component| self.min[component].min(other.min[component])),
            max: std::array::from_fn(|component| self.max[component].max(other.max[component])),
        }
    }

    pub fn intersect(self, other: Self) -> Option<Self> {
        Self::new(
            std::array::from_fn(|component| self.min[component].max(other.min[component])),
            std::array::from_fn(|component| self.max[component].min(other.max[component])),
        )
        .ok()
    }

    pub fn expand(self, amount: f64) -> Result<Self, String> {
        if !amount.is_finite() || amount < 0.0 {
            return Err(format!(
                "pixels::world_bounds: invalid expansion amount {amount}"
            ));
        }
        Self::new(
            self.min.map(|value| next_down(value - amount)),
            self.max.map(|value| next_up(value + amount)),
        )
    }

    pub fn clip(self, world: Self) -> Option<Self> {
        self.intersect(world)
    }

    fn axis_interval(self, component: usize) -> Result<F64Interval, String> {
        F64Interval::new(self.min[component], self.max[component])
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorldBound {
    pub bounds: Option<Aabb64>,
    pub rule: &'static str,
    pub contributors: Vec<String>,
    pub pruned_reason: Option<&'static str>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WorldBounds {
    pub world: Aabb64,
    pub fields: BTreeMap<FieldId, WorldBound>,
}

pub(crate) fn repeats_below_transform(graph: &SymbolicGraph) -> Result<BTreeSet<FieldId>, String> {
    let mut repeats = BTreeSet::new();
    let mut seen = BTreeSet::new();
    let mut stack = vec![(graph.field_root, false)];
    while let Some((id, transformed)) = stack.pop() {
        if !seen.insert((id, transformed)) {
            continue;
        }
        match &graph.fields.get(id)?.kind {
            FieldKind::Primitive(_) => {}
            FieldKind::HardUnion { a, b }
            | FieldKind::HardIntersection { a, b }
            | FieldKind::HardSubtract { a, b }
            | FieldKind::SmoothUnion { a, b, .. }
            | FieldKind::SmoothIntersection { a, b, .. }
            | FieldKind::SmoothSubtract { a, b, .. } => {
                stack.extend([(*a, transformed), (*b, transformed)]);
            }
            FieldKind::Neg { child } | FieldKind::Mark { child, .. } => {
                stack.push((*child, transformed));
            }
            FieldKind::Transform { child, .. } => stack.push((*child, true)),
            FieldKind::FiniteRepeat { child, .. } => {
                if transformed {
                    repeats.insert(id);
                }
                stack.push((*child, transformed));
            }
            FieldKind::BoundedDisplace { base, .. } => stack.push((*base, transformed)),
        }
    }
    Ok(repeats)
}

impl WorldBounds {
    pub fn get(&self, id: FieldId) -> Result<&WorldBound, String> {
        self.fields
            .get(&id)
            .ok_or_else(|| format!("pixels::world_bounds: missing predecessor {id}"))
    }

    pub fn live(&self, id: FieldId) -> Result<Aabb64, String> {
        self.get(id)?
            .bounds
            .ok_or_else(|| format!("pixels::world_bounds: field {id} was pruned"))
    }
}

pub fn relevant_repeat_indices(
    base: Aabb64,
    world: Aabb64,
    axis: Axis,
    first: i32,
    count: u32,
    period: F64Interval,
) -> Result<Vec<i32>, String> {
    if count == 0 {
        return Ok(Vec::new());
    }
    if period.lo <= 0.0 || !period.hi.is_finite() {
        return Err(format!(
            "P012: repetition has no finite relevant-instance bound in the renderer world box: period={period:?}"
        ));
    }
    let end = i64::from(first)
        .checked_add(i64::from(count))
        .ok_or_else(|| "P012: repetition instance index range overflows i64".to_string())?;
    if end > i64::from(i32::MAX) + 1 {
        return Err("P012: repetition instance index range exceeds i32".to_string());
    }
    if base == world {
        let ceiling = super::capacities::PixelsCeilings::MACHINE_V1.repeat_analysis_candidates;
        if count > ceiling {
            return Err(format!(
                "P015: renderer capacity `repeated_instances` needs at least {count} relevant slots, which exceeds the analysis safety ceiling of {ceiling}"
            ));
        }
        return (i64::from(first)..end)
            .map(|index| {
                i32::try_from(index)
                    .map_err(|_| "P012: repetition instance index exceeds i32".to_string())
            })
            .collect();
    }
    let component = match axis {
        Axis::X => 0,
        Axis::Y => 1,
        Axis::Z => 2,
    };
    // An instance can intersect only when its translation interval intersects
    // this interval. Positive and negative indices use opposite period
    // endpoints, so solve those cases separately.
    let target_lo = next_down(world.min[component] - base.max[component]);
    let target_hi = next_up(world.max[component] - base.min[component]);
    let clamp_i64 = |value: f64| {
        if value <= f64::from(i32::MIN) {
            i64::from(i32::MIN)
        } else if value >= f64::from(i32::MAX) {
            i64::from(i32::MAX)
        } else {
            value as i64
        }
    };
    let authored_lo = i64::from(first);
    let authored_hi = end;
    let mut candidate_ranges = Vec::with_capacity(2);
    if target_hi >= 0.0 {
        let lo = if target_lo <= 0.0 {
            0
        } else {
            clamp_i64((target_lo / period.hi).ceil())
        }
        .max(0)
        .max(authored_lo);
        let hi = clamp_i64((target_hi / period.lo).floor())
            .saturating_add(1)
            .min(i64::from(i32::MAX) + 1)
            .min(authored_hi);
        if lo < hi {
            candidate_ranges.push((lo, hi));
        }
    }
    if target_lo <= 0.0 {
        let magnitude_lo = if target_hi >= 0.0 {
            1
        } else {
            clamp_i64((-target_hi / period.hi).ceil()).max(1)
        };
        let magnitude_hi = clamp_i64((-target_lo / period.lo).floor());
        let lo = (-magnitude_hi).max(authored_lo);
        let hi = (-magnitude_lo).saturating_add(1).min(0).min(authored_hi);
        if lo < hi {
            candidate_ranges.push((lo, hi));
        }
    }
    candidate_ranges.sort_unstable();
    let candidate_count = candidate_ranges.iter().try_fold(0_i64, |sum, (lo, hi)| {
        sum.checked_add(hi - lo)
            .ok_or_else(|| "P015: repeated instance candidate count overflow".to_string())
    })?;
    let ceiling =
        i64::from(super::capacities::PixelsCeilings::MACHINE_V1.repeat_analysis_candidates);
    if candidate_count > ceiling {
        return Err(format!(
            "P015: renderer capacity `repeated_instances` needs at least {candidate_count} relevant slots, which exceeds the analysis safety ceiling of {ceiling}"
        ));
    }
    let mut relevant = Vec::new();
    for (candidate_lo, candidate_hi) in candidate_ranges {
        for index in candidate_lo..candidate_hi {
            let index = i32::try_from(index)
                .map_err(|_| "P012: repetition instance index overflows i32".to_string())?;
            let shift = repeat_translation_interval(first, index, period)?;
            let shifted_min = next_down(base.min[component] + shift.lo);
            let shifted_max = next_up(base.max[component] + shift.hi);
            if shifted_max >= world.min[component] && shifted_min <= world.max[component] {
                relevant.push(index);
            }
        }
    }
    relevant.sort_unstable();
    relevant.dedup();
    Ok(relevant)
}

pub(crate) fn authored_repeat_indices(first: i32, count: u32) -> Result<Vec<i32>, String> {
    let end = i64::from(first)
        .checked_add(i64::from(count))
        .ok_or_else(|| "P012: repetition instance index range overflows i64".to_string())?;
    if end > i64::from(i32::MAX) + 1 {
        return Err("P012: repetition instance index range exceeds i32".to_string());
    }
    let ceiling = super::capacities::PixelsCeilings::MACHINE_V1.repeat_analysis_candidates;
    if count > ceiling {
        return Err(format!(
            "P015: renderer capacity `repeated_instances` needs at least {count} authored slots, which exceeds the analysis safety ceiling of {ceiling}"
        ));
    }
    (i64::from(first)..end)
        .map(|index| {
            i32::try_from(index)
                .map_err(|_| "P012: repetition instance index exceeds i32".to_string())
        })
        .collect()
}

pub fn repeat_translation_interval(
    first: i32,
    index: i32,
    period: F64Interval,
) -> Result<F64Interval, String> {
    let ordinal = i64::from(index)
        .checked_sub(i64::from(first))
        .ok_or_else(|| "pixels::world_bounds: repeat ordinal underflow".to_string())?;
    let ordinal = u32::try_from(ordinal)
        .map_err(|_| "pixels::world_bounds: repeat ordinal exceeds u32".to_string())?;
    // Match symbolic lowering exactly: `(first as f32 + ordinal as f32) *
    // period`, with a source-f32 rounding step after the addition and after
    // the multiplication.
    let source_index = (first as f32) + (ordinal as f32);
    F64Interval::point(f64::from(source_index))?.mul_f32(period)
}

fn interval(values: &ValueBounds, id: ScalarId) -> Result<F64Interval, String> {
    values.get(id)
}

fn vector(values: &ValueBounds, ids: [ScalarId; 3]) -> Result<[F64Interval; 3], String> {
    Ok([
        interval(values, ids[0])?,
        interval(values, ids[1])?,
        interval(values, ids[2])?,
    ])
}

fn intervals_aabb(values: [F64Interval; 3]) -> Result<Aabb64, String> {
    Aabb64::new(
        values.map(|value| next_down(value.lo)),
        values.map(|value| next_up(value.hi)),
    )
}

fn radius(values: &ValueBounds, id: ScalarId) -> Result<f64, String> {
    let value = interval(values, id)?;
    if value.lo < 0.0 {
        return Err(format!(
            "pixels::world_bounds: radius {id} may be negative over {value:?}"
        ));
    }
    Ok(value.hi)
}

fn half_extent(values: &ValueBounds, id: ScalarId) -> Result<f64, String> {
    let value = interval(values, id)?;
    if value.lo < 0.0 {
        return Err(format!(
            "pixels::world_bounds: half extent {id} may be negative over {value:?}"
        ));
    }
    Ok(value.hi)
}

fn primitive_bounds(
    primitive: &Primitive,
    values: &ValueBounds,
    world: Aabb64,
) -> Result<Aabb64, String> {
    let analytic = match primitive {
        Primitive::Plane { .. } => Ok(world),
        Primitive::Sphere { center, radius: r } => {
            intervals_aabb(vector(values, *center)?)?.expand(radius(values, *r)?)
        }
        Primitive::Box { center, half } => {
            let center = vector(values, *center)?;
            let half = [
                half_extent(values, half[0])?,
                half_extent(values, half[1])?,
                half_extent(values, half[2])?,
            ];
            intervals_aabb(std::array::from_fn(|component| F64Interval {
                lo: center[component].lo - half[component],
                hi: center[component].hi + half[component],
            }))
        }
        Primitive::RoundBox {
            center,
            half,
            radius: r,
        } => {
            let center = vector(values, *center)?;
            let half = [
                half_extent(values, half[0])?,
                half_extent(values, half[1])?,
                half_extent(values, half[2])?,
            ];
            intervals_aabb(std::array::from_fn(|component| F64Interval {
                lo: center[component].lo - half[component],
                hi: center[component].hi + half[component],
            }))?
            .expand(radius(values, *r)?)
        }
        Primitive::Capsule { a, b, radius: r } | Primitive::FiniteCylinder { a, b, radius: r } => {
            let a = vector(values, *a)?;
            let b = vector(values, *b)?;
            intervals_aabb(std::array::from_fn(|component| {
                a[component].hull(b[component])
            }))?
            .expand(radius(values, *r)?)
        }
        Primitive::FiniteCone {
            a,
            b,
            radius_a,
            radius_b,
        } => {
            let a = vector(values, *a)?;
            let b = vector(values, *b)?;
            let radius = radius(values, *radius_a)?.max(radius(values, *radius_b)?);
            intervals_aabb(std::array::from_fn(|component| {
                a[component].hull(b[component])
            }))?
            .expand(radius)
        }
        Primitive::Torus {
            center,
            major,
            minor,
            ..
        } => {
            let extent = next_up(radius(values, *major)? + radius(values, *minor)?);
            intervals_aabb(vector(values, *center)?)?.expand(extent)
        }
    }?;
    if matches!(primitive, Primitive::Plane { .. }) {
        return Ok(analytic);
    }
    analytic.expand(source_f32_position_envelope(analytic, world)?)
}

/// Spatial allowance for the authoritative source-f32 primitive programs.
///
/// Primitive formulas contain at most 64 arithmetic/selection steps on a
/// coordinate path (the fixed square-root iteration is contractive after its
/// first finite step). Two f32 lattice cells per step cover input conversion
/// and result rounding. Using 128 cells at the largest relevant coordinate
/// magnitude therefore encloses roots that lie just outside the real-valued
/// analytic boundary. This is intentionally a spatial envelope; smooth and
/// deformation support are accounted separately.
fn source_f32_position_envelope(analytic: Aabb64, world: Aabb64) -> Result<f64, String> {
    const SOURCE_F32_POSITION_ULPS_V1: f64 = 128.0;
    let magnitude = analytic
        .min
        .iter()
        .chain(analytic.max.iter())
        .chain(world.min.iter())
        .chain(world.max.iter())
        .map(|value| value.abs())
        .fold(1.0_f64, f64::max);
    let magnitude_f32 = magnitude as f32;
    if !magnitude_f32.is_finite() {
        return Err("P004: field primitive coordinate envelope exceeds finite f32".to_string());
    }
    let ulp = f64::from(next_up_f32(magnitude_f32)) - f64::from(magnitude_f32);
    let envelope = next_up(ulp.max(f64::from(f32::from_bits(1))) * SOURCE_F32_POSITION_ULPS_V1);
    if !envelope.is_finite() {
        return Err("P004: field primitive source-f32 envelope is non-finite".to_string());
    }
    Ok(envelope)
}

fn require_positive(values: &ValueBounds, id: ScalarId, description: &str) -> Result<(), String> {
    let interval = values.get(id)?;
    if interval.lo <= 0.0 {
        return Err(format!(
            "P004: field operation `primitive` is not available in `AaaByteExact`: {description} must stay finite and strictly positive over the complete renderer domain, got {interval:?}"
        ));
    }
    Ok(())
}

fn require_nonzero_vector(
    values: &ValueBounds,
    vector: [ScalarId; 3],
    description: &str,
) -> Result<(), String> {
    for component in vector {
        if !values.get(component)?.contains_zero() {
            return Ok(());
        }
    }
    Err(format!(
        "P004: field operation `primitive` is not available in `AaaByteExact`: {description} may become the zero vector over the complete renderer domain"
    ))
}

fn require_nonzero_segment(
    values: &ValueBounds,
    a: [ScalarId; 3],
    b: [ScalarId; 3],
    description: &str,
) -> Result<(), String> {
    let mut squared = F64Interval::point(0.0)?;
    for component in 0..3 {
        squared = squared.add_f32(
            values
                .get(a[component])?
                .sub_f32(values.get(b[component])?)?
                .square_f32()?,
        )?;
    }
    if squared.lo <= 0.0 {
        return Err(format!(
            "P004: field operation `primitive` is not available in `AaaByteExact`: {description} endpoints may coincide over the complete renderer domain"
        ));
    }
    Ok(())
}

fn validate_primitive_domain(
    graph: &SymbolicGraph,
    primitive: &Primitive,
    values: &ValueBounds,
) -> Result<(), String> {
    match primitive {
        Primitive::Plane { normal, .. } => require_nonzero_vector(values, *normal, "plane normal"),
        Primitive::Sphere { radius, .. } => require_positive(values, *radius, "sphere radius"),
        Primitive::Box { half, .. } => {
            for component in half {
                require_positive(values, *component, "box half extent")?;
            }
            Ok(())
        }
        Primitive::RoundBox { half, radius, .. } => {
            for component in half {
                require_positive(values, *component, "round-box half extent")?;
            }
            require_positive(values, *radius, "round-box radius")
        }
        Primitive::Capsule { a, b, radius } => {
            if a.iter().zip(b).all(|(a, b)| {
                matches!(
                    (graph.scalar.get(*a), graph.scalar.get(*b)),
                    (Ok(a), Ok(b)) if a.op == b.op
                )
            }) {
                return require_positive(values, *radius, "capsule radius");
            }
            require_nonzero_segment(values, *a, *b, "capsule")?;
            require_positive(values, *radius, "capsule radius")
        }
        Primitive::FiniteCylinder { a, b, radius } => {
            require_nonzero_segment(values, *a, *b, "finite cylinder")?;
            require_positive(values, *radius, "finite-cylinder radius")
        }
        Primitive::FiniteCone {
            a,
            b,
            radius_a,
            radius_b,
        } => {
            require_nonzero_segment(values, *a, *b, "finite cone")?;
            require_positive(values, *radius_a, "finite-cone first radius")?;
            // A zero tip is the canonical closed cone; negative radii are not.
            let radius_b = values.get(*radius_b)?;
            if radius_b.lo < 0.0 {
                return Err(format!(
                    "P004: field operation `finite_cone` is not available in `AaaByteExact`: second radius must stay nonnegative, got {radius_b:?}"
                ));
            }
            Ok(())
        }
        Primitive::Torus {
            axis, major, minor, ..
        } => {
            require_nonzero_vector(values, *axis, "torus axis")?;
            require_positive(values, *major, "torus major radius")?;
            require_positive(values, *minor, "torus minor radius")
        }
    }
}

pub(crate) fn transform_bounds(
    child: Aabb64,
    transform: &TransformProgram,
    values: &ValueBounds,
) -> Result<Aabb64, String> {
    match transform {
        TransformProgram::Translate { by } => {
            let by = vector(values, *by)?;
            Aabb64::new(
                std::array::from_fn(|component| next_down(child.min[component] + by[component].lo)),
                std::array::from_fn(|component| next_up(child.max[component] + by[component].hi)),
            )
        }
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => transform_rigid(
            child,
            [
                F64Interval::point(0.0)?,
                F64Interval::point(0.0)?,
                F64Interval::point(0.0)?,
            ],
            [
                vector(values, *row_x)?,
                vector(values, *row_y)?,
                vector(values, *row_z)?,
            ],
        ),
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => transform_rigid(
            child,
            vector(values, *translation)?,
            [
                vector(values, *row_x)?,
                vector(values, *row_y)?,
                vector(values, *row_z)?,
            ],
        ),
        TransformProgram::UniformScale { .. } => Ok(child),
        TransformProgram::SourceRigidSequence { composed, .. }
        | TransformProgram::RigidSequence { composed, .. } => {
            transform_bounds(child, composed, values)
        }
    }
}

fn transform_rigid(
    child: Aabb64,
    translation: [F64Interval; 3],
    rows: [[F64Interval; 3]; 3],
) -> Result<Aabb64, String> {
    // Source rows map world to local. Invert the complete interval matrix:
    // constant approximate rotations are not their own exact inverse, and
    // parameterized transforms must enclose every admitted coefficient set.
    let minor = |a: F64Interval,
                 b: F64Interval,
                 c: F64Interval,
                 d: F64Interval|
     -> Result<F64Interval, String> {
        a.mul_outward(d)?.sub_outward(b.mul_outward(c)?)
    };
    let cofactors = [
        [
            minor(rows[1][1], rows[1][2], rows[2][1], rows[2][2])?,
            minor(rows[1][2], rows[1][0], rows[2][2], rows[2][0])?,
            minor(rows[1][0], rows[1][1], rows[2][0], rows[2][1])?,
        ],
        [
            minor(rows[0][2], rows[0][1], rows[2][2], rows[2][1])?,
            minor(rows[0][0], rows[0][2], rows[2][0], rows[2][2])?,
            minor(rows[0][1], rows[0][0], rows[2][1], rows[2][0])?,
        ],
        [
            minor(rows[0][1], rows[0][2], rows[1][1], rows[1][2])?,
            minor(rows[0][2], rows[0][0], rows[1][2], rows[1][0])?,
            minor(rows[0][0], rows[0][1], rows[1][0], rows[1][1])?,
        ],
    ];
    let determinant = rows[0][0]
        .mul_outward(cofactors[0][0])?
        .add_outward(rows[0][1].mul_outward(cofactors[0][1])?)?
        .add_outward(rows[0][2].mul_outward(cofactors[0][2])?)?;
    if determinant.contains_zero() {
        return Err(format!(
            "P004: rigid rotation matrix is singular within compiler interval arithmetic: determinant={determinant:?}"
        ));
    }
    // adjugate is the transpose of the cofactor matrix.
    let zero = F64Interval::point(0.0)?;
    let mut inverse = [[zero; 3]; 3];
    for row in 0..3 {
        for column in 0..3 {
            inverse[row][column] = cofactors[column][row].div_outward(determinant)?;
        }
    }
    let local = [
        child.axis_interval(0)?,
        child.axis_interval(1)?,
        child.axis_interval(2)?,
    ];
    let mut world = [
        F64Interval::point(0.0)?,
        F64Interval::point(0.0)?,
        F64Interval::point(0.0)?,
    ];
    for component in 0..3 {
        world[component] = translation[component];
        for local_component in 0..3 {
            world[component] = world[component].add_outward(
                inverse[component][local_component].mul_outward(local[local_component])?,
            )?;
        }
    }
    let exact = intervals_aabb(world)?;
    // The source transform performs at most one subtraction, three products,
    // and two additions per component in f32. A 64-epsilon envelope safely
    // dominates those roundings and the inverse amplification admitted by
    // the orthonormality check.
    let magnitude = local
        .iter()
        .chain(&translation)
        .map(|value| value.abs_upper())
        .chain(exact.min.iter().chain(&exact.max).map(|value| value.abs()))
        .fold(1.0, f64::max);
    exact.expand(next_up(magnitude * f64::from(f32::EPSILON) * 64.0))
}

fn validate_negation_domains(
    graph: &SymbolicGraph,
    id: FieldId,
    hard_bounded: bool,
) -> Result<(), String> {
    fn bounded_shape(graph: &SymbolicGraph, id: FieldId) -> Result<bool, String> {
        Ok(match &graph.fields.get(id)?.kind {
            FieldKind::Primitive(Primitive::Plane { .. }) | FieldKind::Neg { .. } => false,
            FieldKind::Primitive(_) => true,
            FieldKind::HardUnion { a, b } | FieldKind::SmoothUnion { a, b, .. } => {
                bounded_shape(graph, *a)? && bounded_shape(graph, *b)?
            }
            FieldKind::HardIntersection { a, b } | FieldKind::SmoothIntersection { a, b, .. } => {
                bounded_shape(graph, *a)? || bounded_shape(graph, *b)?
            }
            FieldKind::HardSubtract { a, .. } | FieldKind::SmoothSubtract { a, .. } => {
                bounded_shape(graph, *a)?
            }
            FieldKind::Transform { child, .. }
            | FieldKind::FiniteRepeat { child, .. }
            | FieldKind::Mark { child, .. } => bounded_shape(graph, *child)?,
            FieldKind::BoundedDisplace { base, .. } => bounded_shape(graph, *base)?,
        })
    }
    match &graph.fields.get(id)?.kind {
        FieldKind::Neg { child } => {
            if !hard_bounded {
                return Err(format!(
                    "P012: negated field {id} does not define a finite object; place the complement inside a bounded hard intersection or subtraction"
                ));
            }
            validate_negation_domains(graph, *child, true)
        }
        FieldKind::HardIntersection { a, b } => {
            validate_negation_domains(graph, *a, hard_bounded || bounded_shape(graph, *b)?)?;
            validate_negation_domains(graph, *b, hard_bounded || bounded_shape(graph, *a)?)
        }
        FieldKind::HardSubtract { a, b } => {
            let left_bounded = bounded_shape(graph, *a)?;
            validate_negation_domains(graph, *a, hard_bounded)?;
            validate_negation_domains(graph, *b, hard_bounded || left_bounded)
        }
        FieldKind::HardUnion { a, b }
        | FieldKind::SmoothUnion { a, b, .. }
        | FieldKind::SmoothIntersection { a, b, .. }
        | FieldKind::SmoothSubtract { a, b, .. } => {
            validate_negation_domains(graph, *a, hard_bounded)?;
            validate_negation_domains(graph, *b, hard_bounded)
        }
        FieldKind::Transform { child, .. }
        | FieldKind::FiniteRepeat { child, .. }
        | FieldKind::Mark { child, .. } => validate_negation_domains(graph, *child, hard_bounded),
        FieldKind::BoundedDisplace { base, .. } => {
            validate_negation_domains(graph, *base, hard_bounded)
        }
        FieldKind::Primitive(_) => Ok(()),
    }
}

pub(crate) fn smooth_node_expansion(
    values: &ValueBounds,
    support: &SupportTable,
    a: FieldId,
    b: FieldId,
    k: ScalarId,
) -> Result<f64, String> {
    let descendant_budget = support
        .get(a)?
        .max_budget
        .hi
        .max(support.get(b)?.max_budget.hi);
    let scalar_radius =
        super::support::smooth_source_support_radius(values.get(k)?, descendant_budget)?;
    let value_to_distance = support
        .get(a)?
        .max_value_to_distance
        .hi
        .max(support.get(b)?.max_value_to_distance.hi);
    Ok(next_up(scalar_radius.hi * value_to_distance))
}

pub fn derive(
    graph: &SymbolicGraph,
    config: &RendererConfig,
    values: &ValueBounds,
    support: &SupportTable,
) -> Result<WorldBounds, String> {
    validate_negation_domains(graph, graph.field_root, false)?;
    let world = Aabb64::new(
        [
            config.world_min.x.into(),
            config.world_min.y.into(),
            config.world_min.z.into(),
        ],
        [
            config.world_max.x.into(),
            config.world_max.y.into(),
            config.world_max.z.into(),
        ],
    )?;
    let mut result = WorldBounds {
        world,
        fields: BTreeMap::new(),
    };
    let repeats_below_transform = repeats_below_transform(graph)?;
    for (id, node) in graph.fields.iter() {
        let child = |id| result.get(id);
        let (bounds, rule, contributors, pruned_reason) = match &node.kind {
            FieldKind::Primitive(primitive) => (
                {
                    validate_primitive_domain(graph, primitive, values)?;
                    Some(primitive_bounds(primitive, values, world)?)
                },
                match primitive {
                    Primitive::Plane { .. } => "plane-world-clip",
                    _ => "primitive-analytic",
                },
                Vec::new(),
                None,
            ),
            FieldKind::HardUnion { a, b } => {
                let a = child(*a)?.bounds;
                let b = child(*b)?.bounds;
                (
                    match (a, b) {
                        (Some(a), Some(b)) => Some(a.union(b)),
                        (Some(bound), None) | (None, Some(bound)) => Some(bound),
                        (None, None) => None,
                    },
                    "hard-union",
                    vec![format!("left={a:?}"), format!("right={b:?}")],
                    (a.is_none() && b.is_none()).then_some("both-union-children-empty"),
                )
            }
            FieldKind::HardIntersection { a, b } => {
                let bounds = match (child(*a)?.bounds, child(*b)?.bounds) {
                    (Some(a), Some(b)) => a.intersect(b),
                    _ => None,
                };
                (
                    bounds,
                    "hard-intersection",
                    vec![format!("{a}"), format!("{b}")],
                    bounds.is_none().then_some("empty-intersection"),
                )
            }
            FieldKind::HardSubtract { a, .. } => (
                child(*a)?.bounds,
                "hard-subtraction-left",
                vec![format!("{a}")],
                child(*a)?.pruned_reason,
            ),
            FieldKind::SmoothUnion { a, b, k } => {
                let expansion = smooth_node_expansion(values, support, *a, *b, *k)?;
                let bounds = match (child(*a)?.bounds, child(*b)?.bounds) {
                    (Some(a), Some(b)) => Some(a.union(b).expand(expansion)?),
                    (Some(bound), None) | (None, Some(bound)) => Some(bound.expand(expansion)?),
                    (None, None) => None,
                };
                (
                    bounds,
                    "smooth-union-support",
                    vec![format!("source-f32-smooth-support={expansion}")],
                    bounds.is_none().then_some("smooth-children-empty"),
                )
            }
            FieldKind::SmoothIntersection { a, b, k } => {
                let expansion = smooth_node_expansion(values, support, *a, *b, *k)?;
                let bounds = match (child(*a)?.bounds, child(*b)?.bounds) {
                    (Some(a), Some(b)) => a
                        .intersect(b)
                        .map(|bound| bound.expand(expansion))
                        .transpose()?,
                    _ => None,
                };
                (
                    bounds,
                    "smooth-intersection-support",
                    vec![format!("source-f32-smooth-support={expansion}")],
                    bounds.is_none().then_some("empty-smooth-intersection"),
                )
            }
            FieldKind::SmoothSubtract { a, k, .. } => {
                let FieldKind::SmoothSubtract { b, .. } = &node.kind else {
                    unreachable!("matched smooth subtraction")
                };
                let expansion = smooth_node_expansion(values, support, *a, *b, *k)?;
                (
                    child(*a)?
                        .bounds
                        .map(|bound| bound.expand(expansion))
                        .transpose()?,
                    "smooth-subtraction-left-support",
                    vec![format!("source-f32-smooth-support={expansion}")],
                    child(*a)?.pruned_reason,
                )
            }
            FieldKind::Neg { child: field } => (
                Some(world),
                "negation-domain",
                vec![format!("{field}")],
                child(*field)?.pruned_reason,
            ),
            FieldKind::Transform {
                child: field,
                transform,
            } => (
                child(*field)?
                    .bounds
                    .map(|bound| {
                        if bound == world {
                            Ok(world)
                        } else {
                            transform_bounds(bound, transform, values)
                        }
                    })
                    .transpose()?,
                "rigid-transform",
                vec![format!("{field}")],
                child(*field)?.pruned_reason,
            ),
            FieldKind::FiniteRepeat {
                child: field,
                axis,
                first,
                count,
                period,
            } => {
                if *count == 0 {
                    (
                        None,
                        "finite-repeat",
                        vec!["count=0".to_string()],
                        Some("zero-repeat-count"),
                    )
                } else {
                    let period = values.get(*period)?;
                    if period.lo <= 0.0 {
                        return Err(format!(
                            "P012: repetition has no finite relevant-instance bound: world={world:?} period={period:?}"
                        ));
                    }
                    let base = child(*field)?.bounds;
                    let component = match axis {
                        Axis::X => 0,
                        Axis::Y => 1,
                        Axis::Z => 2,
                    };
                    let mut repeated = (base == Some(world)).then_some(world);
                    if let Some(base) = base {
                        for index in if base == world || repeats_below_transform.contains(&id) {
                            authored_repeat_indices(*first, *count)?
                        } else {
                            relevant_repeat_indices(base, world, *axis, *first, *count, period)?
                        } {
                            let shift = repeat_translation_interval(*first, index, period)?;
                            let mut instance = base;
                            instance.min[component] = next_down(instance.min[component] + shift.lo);
                            instance.max[component] = next_up(instance.max[component] + shift.hi);
                            repeated = Some(
                                repeated
                                    .map(|prior: Aabb64| prior.union(instance))
                                    .unwrap_or(instance),
                            );
                        }
                    }
                    (
                        repeated,
                        "finite-repeat-enumeration",
                        vec![format!("first={first} count={count} period={period:?}")],
                        repeated.is_none().then_some("no-relevant-repeat-instance"),
                    )
                }
            }
            FieldKind::BoundedDisplace { base, contract, .. } => {
                let amount = next_up(
                    values.get(contract.amplitude_bound)?.abs_upper()
                        * support.get(*base)?.max_value_to_distance.hi,
                );
                (
                    child(*base)?
                        .bounds
                        .map(|bound| {
                            if bound == world {
                                Ok(world)
                            } else {
                                bound.expand(amount)
                            }
                        })
                        .transpose()?,
                    "bounded-displacement",
                    vec![format!("amplitude={amount}")],
                    child(*base)?.pruned_reason,
                )
            }
            FieldKind::Mark { child: field, .. } => (
                child(*field)?.bounds,
                "identity-mark",
                vec![format!("{field}")],
                child(*field)?.pruned_reason,
            ),
        };
        result.fields.insert(
            id,
            WorldBound {
                bounds,
                rule,
                contributors,
                pruned_reason,
            },
        );
    }
    Ok(result)
}

#[cfg(test)]
mod tests {
    use super::super::bounds::ScalarBound;
    use super::*;

    #[test]
    fn repeat_below_enclosing_transform_disables_early_world_pruning() {
        let mut fields = super::super::graph::FieldArena::new(2);
        let origin = super::super::arena::NodeOrigin::synthetic("repeat-transform");
        let primitive = fields
            .push(
                super::super::graph::FieldNode {
                    kind: FieldKind::Primitive(Primitive::Sphere {
                        center: [ScalarId(0); 3],
                        radius: ScalarId(1),
                    }),
                    scalar_value: ScalarId(0),
                },
                origin.clone(),
            )
            .unwrap();
        let repeat = fields
            .push(
                super::super::graph::FieldNode {
                    kind: FieldKind::FiniteRepeat {
                        child: primitive,
                        axis: Axis::X,
                        first: 100,
                        count: 1,
                        period: ScalarId(1),
                    },
                    scalar_value: ScalarId(0),
                },
                origin.clone(),
            )
            .unwrap();
        let root = fields
            .push(
                super::super::graph::FieldNode {
                    kind: FieldKind::Transform {
                        child: repeat,
                        transform: TransformProgram::Translate {
                            by: [ScalarId(0); 3],
                        },
                    },
                    scalar_value: ScalarId(0),
                },
                origin,
            )
            .unwrap();
        let graph = SymbolicGraph {
            renderer_index: 0,
            field_key: String::new(),
            material_key: String::new(),
            params_type: crate::sema::types::Type::Unit,
            material_type: crate::sema::types::Type::Unit,
            params: Vec::new(),
            scalar: super::super::scalar::ScalarArena::new(1),
            fields,
            materials: super::super::material_graph::MaterialArena::new(3),
            field_root: root,
            material_root: super::super::ids::MaterialId(0),
            obligations: Vec::new(),
            quota: Default::default(),
        };
        assert_eq!(
            repeats_below_transform(&graph).unwrap(),
            BTreeSet::from([repeat])
        );
    }

    fn value_bounds(intervals: &[(ScalarId, F64Interval)]) -> ValueBounds {
        ValueBounds {
            scalar: intervals
                .iter()
                .map(|(id, value)| {
                    (
                        *id,
                        ScalarBound {
                            value: *value,
                            rule: "test",
                        },
                    )
                })
                .collect(),
        }
    }

    #[test]
    fn thin_and_point_bounds_remain_nonempty() {
        let point = Aabb64::new([1.0; 3], [1.0; 3]).unwrap();
        assert!(point.expand(1.0e-12).unwrap().min[0] <= 1.0 - 1.0e-12);
    }

    #[test]
    fn sphere_bound_contains_source_f32_zero_beyond_real_analytic_endpoint() {
        let center = f64::from(8.0e-8_f32);
        let values = value_bounds(&[
            (ScalarId(0), F64Interval::point(center).unwrap()),
            (ScalarId(1), F64Interval::point(0.0).unwrap()),
            (ScalarId(2), F64Interval::point(0.0).unwrap()),
            (ScalarId(3), F64Interval::point(1.0).unwrap()),
        ]);
        let world = Aabb64::new([-2.0; 3], [2.0; 3]).unwrap();
        let bound = primitive_bounds(
            &Primitive::Sphere {
                center: [ScalarId(0), ScalarId(1), ScalarId(2)],
                radius: ScalarId(3),
            },
            &values,
            world,
        )
        .unwrap();
        let source_root = f64::from(next_up_f32(1.0));
        assert_eq!((source_root as f32 - center as f32), 1.0_f32);
        assert!(bound.max[0] >= source_root);
    }

    #[test]
    fn disjoint_intersection_is_prunable() {
        let a = Aabb64::new([0.0; 3], [1.0; 3]).unwrap();
        let b = Aabb64::new([2.0; 3], [3.0; 3]).unwrap();
        assert_eq!(a.intersect(b), None);
    }

    #[test]
    fn ranged_repeat_period_keeps_cells_reachable_only_at_large_endpoint() {
        let base = Aabb64::new([0.0; 3], [1.0; 3]).unwrap();
        let positive_world = Aabb64::new([49.0, -1.0, -1.0], [51.0, 2.0, 2.0]).unwrap();
        let period = F64Interval::new(1.0, 100.0).unwrap();
        let positive =
            relevant_repeat_indices(base, positive_world, Axis::X, 0, 100, period).unwrap();
        assert!(positive.contains(&1));
        assert!(positive.contains(&50));

        let negative_world = Aabb64::new([-51.0, -1.0, -1.0], [-49.0, 2.0, 2.0]).unwrap();
        let negative =
            relevant_repeat_indices(base, negative_world, Axis::X, -100, 100, period).unwrap();
        assert!(negative.contains(&-1));
        assert!(negative.contains(&-50));
    }

    #[test]
    fn repeat_analysis_keeps_off_world_bases_and_world_spanning_instances() {
        let world = Aabb64::new([-1.0; 3], [1.0; 3]).unwrap();
        let off_world = Aabb64::new([99.0, -0.5, -0.5], [101.0, 0.5, 0.5]).unwrap();
        let shifted = relevant_repeat_indices(
            off_world,
            world,
            Axis::X,
            -2,
            4,
            F64Interval::point(100.0).unwrap(),
        )
        .unwrap();
        assert_eq!(shifted, vec![-1]);

        let spanning = relevant_repeat_indices(
            world,
            world,
            Axis::X,
            -2,
            4,
            F64Interval::point(100.0).unwrap(),
        )
        .unwrap();
        assert_eq!(spanning, vec![-2, -1, 0, 1]);
    }

    #[test]
    fn repeat_range_may_end_immediately_after_i32_max() {
        assert_eq!(
            authored_repeat_indices(i32::MAX, 1).unwrap(),
            vec![i32::MAX]
        );
        let world = Aabb64::new([-1.0; 3], [1.0; 3]).unwrap();
        assert_eq!(
            relevant_repeat_indices(
                world,
                world,
                Axis::X,
                i32::MAX,
                1,
                F64Interval::point(1.0).unwrap(),
            )
            .unwrap(),
            vec![i32::MAX]
        );
    }

    #[test]
    fn repeat_translation_matches_source_f32_index_and_multiply_order() {
        let period = F64Interval::point(f64::from(0.1_f32)).unwrap();
        let translation = repeat_translation_interval(0, 3, period).unwrap();
        let source = f64::from((0.0_f32 + 3.0_f32) * 0.1_f32);
        assert_eq!(translation, F64Interval::point(source).unwrap());

        let first = 16_777_216_i32;
        let source_index = (first as f32) + 1.0_f32;
        let rounded = repeat_translation_interval(first, first + 1, period).unwrap();
        assert!(rounded.contains(f64::from(source_index * 0.1_f32)));
    }

    #[test]
    fn repeat_candidate_ceiling_reports_exact_contributor() {
        let world = Aabb64::new([-1.0; 3], [1.0; 3]).unwrap();
        let error = relevant_repeat_indices(
            world,
            world,
            Axis::X,
            0,
            1_000_001,
            F64Interval::point(1.0).unwrap(),
        )
        .unwrap_err();
        assert!(error.contains("repeated_instances"));
        assert!(error.contains("1000001 relevant slots"));
        assert!(error.contains("1000000"));
    }

    #[test]
    fn approximate_rigid_rows_use_the_interval_inverse_not_the_transpose() {
        let point = |value| F64Interval::point(value).unwrap();
        let child = Aabb64::new([-1.0; 3], [1.0; 3]).unwrap();
        let epsilon = 0.001_f64;
        let rows = [
            [point(1.0), point(epsilon), point(0.0)],
            [point(-epsilon), point(1.0), point(0.0)],
            [point(0.0), point(0.0), point(1.0)],
        ];
        let transformed =
            transform_rigid(child, [point(0.0), point(0.0), point(0.0)], rows).unwrap();
        let determinant = 1.0 + epsilon * epsilon;
        let expected_corner = [
            (1.0 - epsilon) / determinant,
            (1.0 + epsilon) / determinant,
            1.0,
        ];
        for (component, expected) in expected_corner.into_iter().enumerate() {
            assert!(
                transformed
                    .axis_interval(component)
                    .unwrap()
                    .contains(expected)
            );
        }
        assert_ne!(expected_corner[0].to_bits(), (1.0 - epsilon).to_bits());
    }

    #[test]
    fn primitive_domain_checks_reject_zero_radii_vectors_and_varying_degeneracy() {
        let zero = F64Interval::point(0.0).unwrap();
        let one = F64Interval::point(1.0).unwrap();
        let varying = F64Interval::new(-1.0, 1.0).unwrap();
        let values = value_bounds(&[
            (ScalarId(0), zero),
            (ScalarId(1), one),
            (ScalarId(2), varying),
        ]);
        let graph = SymbolicGraph {
            renderer_index: 0,
            field_key: String::new(),
            material_key: String::new(),
            params_type: crate::sema::types::Type::Unit,
            material_type: crate::sema::types::Type::Unit,
            params: Vec::new(),
            scalar: super::super::scalar::ScalarArena::new(1),
            fields: super::super::graph::FieldArena::new(2),
            materials: super::super::material_graph::MaterialArena::new(3),
            field_root: FieldId(0),
            material_root: super::super::ids::MaterialId(0),
            obligations: Vec::new(),
            quota: Default::default(),
        };
        assert!(
            validate_primitive_domain(
                &graph,
                &Primitive::Plane {
                    normal: [ScalarId(0); 3],
                    offset: ScalarId(0),
                },
                &values,
            )
            .unwrap_err()
            .contains("zero vector")
        );
        assert!(
            validate_primitive_domain(
                &graph,
                &Primitive::Sphere {
                    center: [ScalarId(0); 3],
                    radius: ScalarId(0),
                },
                &values,
            )
            .unwrap_err()
            .contains("strictly positive")
        );
        assert!(
            validate_primitive_domain(
                &graph,
                &Primitive::FiniteCylinder {
                    a: [ScalarId(2), ScalarId(0), ScalarId(0)],
                    b: [ScalarId(2), ScalarId(0), ScalarId(0)],
                    radius: ScalarId(1),
                },
                &values,
            )
            .unwrap_err()
            .contains("endpoints may coincide")
        );
    }
}
