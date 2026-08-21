//! Sealed deterministic probe-GI tables, capacities, and dependency records.

use super::config::RendererConfig;
use super::objects::ObjectPartition;

pub const PROBE_DIRECTION_COUNT_V1: u32 = 32;
pub const PROBE_SH_COEFFICIENTS_V1: u32 = 9;
pub const PROBE_LEVELS_MAX_V1: u32 = 3;
pub const PROBE_DIMS_MAX_V1: [u32; 3] = [16, 8, 16];
pub const PROBE_CELL_BYTES_V1: u64 = 288;
pub const PROBE_STATE_HEADER_BYTES_V1: u64 = 256;
pub const PROBE_LEVEL_HEADER_BYTES_V1: u64 = 64;
pub const PROBE_DEPENDENCY_SNAPSHOT_BYTES_V1: u64 = 1024;

// Checked-in integer directions are the immutable source of the numeric
// table.  Conversion is deterministic and its final byte digest is pinned by
// tests; there is no build-time sampling, RNG, or host feature choice.
const RAW_DIRECTIONS_V1: [[i8; 3]; 32] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
    [1, 1, 1],
    [1, 1, -1],
    [1, -1, 1],
    [1, -1, -1],
    [-1, 1, 1],
    [-1, 1, -1],
    [-1, -1, 1],
    [-1, -1, -1],
    [1, 1, 0],
    [1, -1, 0],
    [-1, 1, 0],
    [-1, -1, 0],
    [1, 0, 1],
    [1, 0, -1],
    [-1, 0, 1],
    [-1, 0, -1],
    [0, 1, 1],
    [0, 1, -1],
    [0, -1, 1],
    [0, -1, -1],
    [1, 2, 0],
    [-1, -2, 0],
    [2, 0, 1],
    [-2, 0, -1],
    [0, 1, 2],
    [0, -1, -2],
];

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProbeDirectionV1 {
    pub direction_bits: [u32; 3],
    pub weight_bits: u32,
    pub sh_basis_bits: [u32; 9],
}

impl ProbeDirectionV1 {
    pub fn direction(self) -> [f32; 3] {
        self.direction_bits.map(f32::from_bits)
    }

    pub fn weight(self) -> f32 {
        f32::from_bits(self.weight_bits)
    }
}

pub fn direction_table_v1() -> [ProbeDirectionV1; 32] {
    RAW_DIRECTIONS_V1.map(|raw| {
        let length = f64::from(
            i32::from(raw[0]).pow(2) + i32::from(raw[1]).pow(2) + i32::from(raw[2]).pow(2),
        )
        .sqrt();
        let x = f64::from(raw[0]) / length;
        let y = f64::from(raw[1]) / length;
        let z = f64::from(raw[2]) / length;
        let sh = [
            0.282_094_791_773_878_14,
            0.488_602_511_902_919_9 * y,
            0.488_602_511_902_919_9 * z,
            0.488_602_511_902_919_9 * x,
            1.092_548_430_592_079_2 * x * y,
            1.092_548_430_592_079_2 * y * z,
            0.315_391_565_252_520_05 * (3.0 * z * z - 1.0),
            1.092_548_430_592_079_2 * x * z,
            0.546_274_215_296_039_6 * (x * x - y * y),
        ];
        ProbeDirectionV1 {
            direction_bits: [x as f32, y as f32, z as f32].map(f32::to_bits),
            weight_bits: ((std::f64::consts::PI / 8.0) as f32).to_bits(),
            sh_basis_bits: sh.map(|value| (value as f32).to_bits()),
        }
    })
}

pub fn table_digest_v1() -> [u8; 32] {
    let mut bytes = Vec::with_capacity(32 * 13 * 4);
    for record in direction_table_v1() {
        for bits in record
            .direction_bits
            .into_iter()
            .chain([record.weight_bits])
            .chain(record.sh_basis_bits)
        {
            bytes.extend(bits.to_le_bytes());
        }
    }
    wrela_machine::sha256::sha256(&bytes)
}

pub fn sealed_direction_table_v1() -> [ProbeDirectionV1; 32] {
    direction_table_v1()
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeLevelV1 {
    pub level: u32,
    pub dims: [u32; 3],
    pub spacing: f32,
    pub first_probe: u32,
    pub probe_count: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeDependencyV1 {
    /// 1 object, 2 light, 3 material, 4 environment. Exposure/post are absent.
    pub kind: u32,
    pub stable_id: u32,
    pub bounds: [f64; 6],
    pub support_radius: f64,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProbeProgramV1 {
    pub enabled: bool,
    pub static_preinitialized: bool,
    pub levels: Vec<ProbeLevelV1>,
    pub directions: [ProbeDirectionV1; 32],
    pub dependencies: Vec<ProbeDependencyV1>,
    pub probe_count: u32,
    pub invalidation_capacity: u32,
    pub all_invalid_secondary_rays: u64,
    pub storage_bytes: u64,
    pub table_digest: [u8; 32],
}

fn checked_mul(a: u64, b: u64, what: &str) -> Result<u64, String> {
    a.checked_mul(b)
        .ok_or_else(|| format!("P015: probe {what} arithmetic overflow"))
}

fn checked_add(a: u64, b: u64, what: &str) -> Result<u64, String> {
    a.checked_add(b)
        .ok_or_else(|| format!("P015: probe {what} arithmetic overflow"))
}

pub fn compile(
    config: &RendererConfig,
    objects: &ObjectPartition,
    material_count: u32,
) -> Result<ProbeProgramV1, String> {
    let directions = sealed_direction_table_v1();
    let table_digest = table_digest_v1();
    if !config.probes_enabled {
        return Ok(ProbeProgramV1 {
            enabled: false,
            static_preinitialized: false,
            levels: Vec::new(),
            directions,
            dependencies: Vec::new(),
            probe_count: 0,
            invalidation_capacity: 0,
            all_invalid_secondary_rays: 0,
            storage_bytes: 0,
            table_digest,
        });
    }
    if config.probe_levels == 0
        || config.probe_levels > PROBE_LEVELS_MAX_V1
        || config
            .probe_dims
            .iter()
            .zip(PROBE_DIMS_MAX_V1)
            .any(|(actual, maximum)| *actual == 0 || *actual > maximum)
        || !config.probe_base_spacing.is_finite()
        || config.probe_base_spacing <= 0.0
    {
        return Err("P015: sealed probe configuration exceeds v1 maxima".to_string());
    }
    let per_level = config
        .probe_dims
        .into_iter()
        .try_fold(1_u64, |product, value| {
            checked_mul(product, u64::from(value), "dimension")
        })?;
    let probe_count_u64 = checked_mul(per_level, u64::from(config.probe_levels), "probe count")?;
    let probe_count =
        u32::try_from(probe_count_u64).map_err(|_| "P015: probe count exceeds u32".to_string())?;
    let mut levels = Vec::with_capacity(config.probe_levels as usize);
    for level in 0..config.probe_levels {
        let spacing = config.probe_base_spacing * 4.0_f32.powi(level as i32);
        if !spacing.is_finite() {
            return Err("P015: probe level spacing is not finite".to_string());
        }
        levels.push(ProbeLevelV1 {
            level,
            dims: config.probe_dims,
            spacing,
            first_probe: u32::try_from(u64::from(level) * per_level)
                .map_err(|_| "P015: probe first ID exceeds u32".to_string())?,
            probe_count: u32::try_from(per_level)
                .map_err(|_| "P015: probe level count exceeds u32".to_string())?,
        });
    }
    let level_headers = checked_mul(
        u64::from(config.probe_levels),
        PROBE_LEVEL_HEADER_BYTES_V1,
        "level headers",
    )?;
    let generations = checked_mul(
        checked_mul(probe_count_u64, PROBE_CELL_BYTES_V1, "cell storage")?,
        2,
        "probe generations",
    )?;
    let invalidation_queue = checked_mul(probe_count_u64, 4, "invalidation queue")?
        .max(PROBE_DEPENDENCY_SNAPSHOT_BYTES_V1);
    let unaligned = checked_add(
        checked_add(
            checked_add(PROBE_STATE_HEADER_BYTES_V1, level_headers, "state headers")?,
            generations,
            "state generations",
        )?,
        invalidation_queue,
        "state invalidation queue",
    )?;
    let storage_bytes = unaligned
        .checked_add(63)
        .map(|value| value & !63)
        .ok_or_else(|| "P015: probe storage alignment overflow".to_string())?;
    let all_invalid_secondary_rays = checked_mul(
        probe_count_u64,
        u64::from(PROBE_DIRECTION_COUNT_V1),
        "all-invalid ray workload",
    )?;
    let support_radius = f64::from(config.probe_base_spacing)
        * 4.0_f64.powi((config.probe_levels - 1) as i32)
        * 1.75;
    let world_bounds = [
        f64::from(config.world_min.x),
        f64::from(config.world_min.y),
        f64::from(config.world_min.z),
        f64::from(config.world_max.x),
        f64::from(config.world_max.y),
        f64::from(config.world_max.z),
    ];
    let mut dependencies = objects
        .objects
        .iter()
        .map(|object| ProbeDependencyV1 {
            kind: 1,
            stable_id: object.id.0,
            bounds: [
                object.bounds.min[0],
                object.bounds.min[1],
                object.bounds.min[2],
                object.bounds.max[0],
                object.bounds.max[1],
                object.bounds.max[2],
            ],
            support_radius,
        })
        .collect::<Vec<_>>();
    dependencies.extend(
        (0..config.light_capacity).map(|stable_id| ProbeDependencyV1 {
            kind: 2,
            stable_id,
            bounds: world_bounds,
            support_radius,
        }),
    );
    dependencies.extend((0..material_count).map(|stable_id| ProbeDependencyV1 {
        kind: 3,
        stable_id,
        bounds: world_bounds,
        support_radius,
    }));
    dependencies.push(ProbeDependencyV1 {
        kind: 4,
        stable_id: 0,
        bounds: world_bounds,
        support_radius,
    });
    Ok(ProbeProgramV1 {
        enabled: true,
        static_preinitialized: config.probes_static_preinitialized,
        levels,
        directions,
        dependencies,
        probe_count,
        invalidation_capacity: probe_count,
        all_invalid_secondary_rays,
        storage_bytes,
        table_digest,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn direction_table_is_unit_finite_and_weights_cover_four_pi() {
        let table = sealed_direction_table_v1();
        let mut weight = 0.0_f64;
        for record in table {
            let direction = record.direction().map(f64::from);
            let length2 = direction
                .into_iter()
                .map(|value| value * value)
                .sum::<f64>();
            assert!((length2 - 1.0).abs() <= 2.0e-7, "{length2}");
            assert!(
                record
                    .sh_basis_bits
                    .into_iter()
                    .all(|bits| f32::from_bits(bits).is_finite())
            );
            weight += f64::from(record.weight());
        }
        assert!((weight - std::f64::consts::TAU * 2.0).abs() <= 2.0e-6);
        assert_eq!(
            table_digest_v1(),
            [
                124, 185, 105, 84, 97, 172, 87, 206, 0, 130, 154, 215, 45, 215, 205, 79, 64, 202,
                67, 8, 134, 119, 255, 134, 80, 30, 17, 186, 113, 51, 25, 232
            ],
        );
    }

    #[test]
    fn exact_probe_cell_layout_covers_coefficients_moments_and_versions() {
        let coefficient_candidates_and_radii = 9 * 3 * 2 * 4;
        let distance_moments_and_radii = 6 * 2 * 4;
        let validity_and_versions = 24;
        assert_eq!(
            coefficient_candidates_and_radii + distance_moments_and_radii + validity_and_versions,
            PROBE_CELL_BYTES_V1
        );
    }
}
