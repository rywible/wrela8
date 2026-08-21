//! Normative deterministic finite one-bounce P10 probe-GI model.

use crate::pixels::probe::{ProbeDirectionV1, ProbeLevelV1, ProbeProgramV1};

const SH_COSINE_BAND: [f64; 9] = [
    std::f64::consts::PI,
    2.0 * std::f64::consts::PI / 3.0,
    2.0 * std::f64::consts::PI / 3.0,
    2.0 * std::f64::consts::PI / 3.0,
    std::f64::consts::PI / 4.0,
    std::f64::consts::PI / 4.0,
    std::f64::consts::PI / 4.0,
    std::f64::consts::PI / 4.0,
    std::f64::consts::PI / 4.0,
];

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct BoundedF32 {
    pub candidate: f32,
    pub radius: f32,
}

impl BoundedF32 {
    pub fn interval(self) -> [f64; 2] {
        [
            f64::from(self.candidate) - f64::from(self.radius),
            f64::from(self.candidate) + f64::from(self.radius),
        ]
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct AxisDistanceMoment {
    pub mean: BoundedF32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ProbeCell {
    pub coefficients: [[BoundedF32; 9]; 3],
    pub distance_moments: [AxisDistanceMoment; 6],
    pub valid: bool,
    pub scene_version: u32,
    pub light_version: u32,
    pub material_version: u32,
    pub world_cell: [i32; 3],
}

impl Default for ProbeCell {
    fn default() -> Self {
        Self {
            coefficients: [[BoundedF32::default(); 9]; 3],
            distance_moments: [AxisDistanceMoment::default(); 6],
            valid: false,
            scene_version: 0,
            light_version: 0,
            material_version: 0,
            world_cell: [0; 3],
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SecondaryRayResult {
    /// Complete-segment miss environment or finite hit outgoing diffuse+emissive.
    pub radiance: [f64; 3],
    pub distance: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DependencyVersions {
    pub scene: u32,
    pub light: u32,
    pub material: u32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    pub fn swept(first: Self, second: Self) -> Self {
        Self {
            min: std::array::from_fn(|axis| first.min[axis].min(second.min[axis])),
            max: std::array::from_fn(|axis| first.max[axis].max(second.max[axis])),
        }
    }

    pub fn intersects_sphere(self, center: [f64; 3], radius: f64) -> bool {
        let mut distance_squared = 0.0;
        for axis in 0..3 {
            let delta = if center[axis] < self.min[axis] {
                self.min[axis] - center[axis]
            } else if center[axis] > self.max[axis] {
                center[axis] - self.max[axis]
            } else {
                0.0
            };
            distance_squared += delta * delta;
        }
        distance_squared <= radius * radius
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct ProbeState {
    pub levels: Vec<ProbeLevelV1>,
    pub origins: Vec<[i32; 3]>,
    pub cells: Vec<ProbeCell>,
    pub directions: [ProbeDirectionV1; 32],
    pub support_radius: f64,
}

fn snapped_cell(camera: [f64; 3], spacing: f64) -> [i32; 3] {
    std::array::from_fn(|axis| (camera[axis] / spacing).floor() as i32)
}

fn cell_coord(level: ProbeLevelV1, origin: [i32; 3], local: u32) -> [i32; 3] {
    let x = local % level.dims[0];
    let yz = local / level.dims[0];
    let y = yz % level.dims[1];
    let z = yz / level.dims[1];
    [
        origin[0] + x as i32,
        origin[1] + y as i32,
        origin[2] + z as i32,
    ]
}

fn position(cell: [i32; 3], spacing: f32) -> [f64; 3] {
    cell.map(|value| f64::from(value) * f64::from(spacing))
}

fn axis_bucket(direction: [f32; 3]) -> usize {
    let (axis, value) = direction
        .iter()
        .copied()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
        .expect("three axes");
    axis * 2 + usize::from(value < 0.0)
}

fn bounded(candidate: f32, absolute_sum: f32) -> Result<BoundedF32, &'static str> {
    if !candidate.is_finite() || !absolute_sum.is_finite() || absolute_sum < 0.0 {
        return Err("non-finite probe value");
    }
    // This is deliberately the same f32 expression as the guest.  The
    // 2e-5 relative envelope dominates gamma_128 for the sealed 32-term
    // multiply/add chain; the floor covers zero/subnormal candidates and
    // rounding while forming the radius.
    let radius = absolute_sum * 0.00002_f32 + 0.000001_f32;
    if !radius.is_finite() {
        return Err("probe radius exceeds f32");
    }
    Ok(BoundedF32 { candidate, radius })
}

fn accumulate_probe<F>(
    probe_position: [f64; 3],
    directions: &[ProbeDirectionV1; 32],
    mut trace: F,
) -> Result<ProbeCell, &'static str>
where
    F: FnMut([f64; 3], u32, [f32; 3]) -> Result<SecondaryRayResult, &'static str>,
{
    let mut coefficients = [[0.0_f32; 9]; 3];
    let mut coefficient_absolute_sum = [[0.0_f32; 9]; 3];
    let mut distance_sums = [0.0_f32; 6];
    let mut distance_absolute_sums = [0.0_f32; 6];
    let mut distance_counts = [0_u32; 6];
    for (direction_id, record) in directions.iter().copied().enumerate() {
        let direction = record.direction();
        let sample = trace(probe_position, direction_id as u32, direction)?;
        if sample.radiance.iter().any(|v| !v.is_finite() || *v < 0.0)
            || !sample.distance.is_finite()
            || sample.distance < 0.0
        {
            return Err("invalid complete secondary-segment result");
        }
        let weight = record.weight();
        for channel in 0..3 {
            for coefficient in 0..9 {
                let term = (sample.radiance[channel] as f32 * weight)
                    * f32::from_bits(record.sh_basis_bits[coefficient]);
                coefficients[channel][coefficient] += term;
                coefficient_absolute_sum[channel][coefficient] += term.abs();
            }
        }
        let bucket = axis_bucket(direction);
        let distance = sample.distance as f32;
        distance_sums[bucket] += distance;
        distance_absolute_sums[bucket] += distance;
        distance_counts[bucket] += 1;
    }
    let mut cell = ProbeCell::default();
    for channel in 0..3 {
        for coefficient in 0..9 {
            cell.coefficients[channel][coefficient] = bounded(
                coefficients[channel][coefficient],
                coefficient_absolute_sum[channel][coefficient],
            )?;
        }
    }
    for axis in 0..6 {
        if distance_counts[axis] == 0 {
            return Err("probe axis has no distance ray");
        }
        let divisor = distance_counts[axis] as f32;
        cell.distance_moments[axis].mean = bounded(
            distance_sums[axis] / divisor,
            distance_absolute_sums[axis] / divisor,
        )?;
    }
    Ok(cell)
}

pub fn contiguous_worker_ranges(
    count: u32,
    workers: u32,
) -> Result<Vec<std::ops::Range<u32>>, &'static str> {
    if workers == 0 {
        return Err("probe initialization needs a worker");
    }
    let mut ranges = Vec::with_capacity(workers as usize);
    for worker in 0..workers {
        let start = u64::from(count) * u64::from(worker) / u64::from(workers);
        let end = u64::from(count) * u64::from(worker + 1) / u64::from(workers);
        ranges.push(start as u32..end as u32);
    }
    Ok(ranges)
}

impl ProbeState {
    pub fn invalid(program: &ProbeProgramV1, camera: [f64; 3]) -> Self {
        let origins = program
            .levels
            .iter()
            .map(|level| {
                let center = snapped_cell(camera, f64::from(level.spacing));
                std::array::from_fn(|axis| center[axis] - (level.dims[axis] as i32 / 2))
            })
            .collect();
        let support_radius = program
            .dependencies
            .first()
            .map_or(0.0, |d| d.support_radius);
        Self {
            levels: program.levels.clone(),
            origins,
            cells: vec![ProbeCell::default(); program.probe_count as usize],
            directions: program.directions,
            support_radius,
        }
    }

    /// Stages all work and commits in probe-ID order only after every one of
    /// the 32-ray segments succeeds. Cancellation leaves the state all-invalid.
    pub fn initialize<F>(
        &mut self,
        workers: u32,
        versions: DependencyVersions,
        mut trace: F,
    ) -> Result<(), &'static str>
    where
        F: FnMut(u32, [f64; 3], u32, [f32; 3]) -> Result<SecondaryRayResult, &'static str>,
    {
        let ranges = contiguous_worker_ranges(self.cells.len() as u32, workers)?;
        let mut staged = Vec::with_capacity(self.cells.len());
        for range in ranges {
            for id in range {
                let (level_index, level) = self
                    .levels
                    .iter()
                    .copied()
                    .enumerate()
                    .find(|(_, level)| {
                        id >= level.first_probe && id < level.first_probe + level.probe_count
                    })
                    .ok_or("probe ID has no level")?;
                let local = id - level.first_probe;
                let world_cell = cell_coord(level, self.origins[level_index], local);
                let probe_position = position(world_cell, level.spacing);
                let mut cell = accumulate_probe(
                    probe_position,
                    &self.directions,
                    |origin, direction_id, direction| trace(id, origin, direction_id, direction),
                )?;
                cell.world_cell = world_cell;
                cell.scene_version = versions.scene;
                cell.light_version = versions.light;
                cell.material_version = versions.material;
                staged.push((id, cell));
            }
        }
        if staged.len() != self.cells.len() {
            return Err("partial probe initialization");
        }
        staged.sort_unstable_by_key(|(id, _)| *id);
        for (expected, (id, mut cell)) in staged.into_iter().enumerate() {
            if id != expected as u32 {
                return Err("probe IDs are not contiguous");
            }
            cell.valid = true;
            self.cells[expected] = cell;
        }
        Ok(())
    }

    /// Recomputes every invalid cell and commits only after the complete
    /// invalidation set succeeds. Valid cells are byte-for-byte retained.
    pub fn update_invalid<F>(
        &mut self,
        workers: u32,
        versions: DependencyVersions,
        mut trace: F,
    ) -> Result<usize, &'static str>
    where
        F: FnMut(u32, [f64; 3], u32, [f32; 3]) -> Result<SecondaryRayResult, &'static str>,
    {
        let ranges = contiguous_worker_ranges(self.cells.len() as u32, workers)?;
        let mut staged = Vec::with_capacity(self.invalid_count());
        for range in ranges {
            for id in range {
                if self.cells[id as usize].valid {
                    continue;
                }
                let (level_index, level) = self
                    .levels
                    .iter()
                    .copied()
                    .enumerate()
                    .find(|(_, level)| {
                        id >= level.first_probe && id < level.first_probe + level.probe_count
                    })
                    .ok_or("invalid probe ID has no level")?;
                let local = id - level.first_probe;
                let world_cell = cell_coord(level, self.origins[level_index], local);
                let probe_position = position(world_cell, level.spacing);
                let mut cell = accumulate_probe(
                    probe_position,
                    &self.directions,
                    |origin, direction_id, direction| trace(id, origin, direction_id, direction),
                )?;
                cell.world_cell = world_cell;
                cell.scene_version = versions.scene;
                cell.light_version = versions.light;
                cell.material_version = versions.material;
                staged.push((id, cell));
            }
        }
        staged.sort_unstable_by_key(|(id, _)| *id);
        let count = staged.len();
        for (id, mut cell) in staged {
            if self.cells[id as usize].valid {
                return Err("probe update attempted to overwrite a valid cell");
            }
            cell.valid = true;
            self.cells[id as usize] = cell;
        }
        Ok(count)
    }

    pub fn invalidate_swept_aabb(&mut self, bounds: Aabb) -> usize {
        let mut count = 0;
        for (index, cell) in self.cells.iter_mut().enumerate() {
            let level = self
                .levels
                .iter()
                .find(|level| {
                    index >= level.first_probe as usize
                        && index < (level.first_probe + level.probe_count) as usize
                })
                .expect("probe belongs to a level");
            if cell.valid
                && bounds.intersects_sphere(
                    position(cell.world_cell, level.spacing),
                    self.support_radius,
                )
            {
                cell.valid = false;
                count += 1;
            }
        }
        count
    }

    pub fn invalidate_environment(&mut self) -> usize {
        let count = self.cells.iter().filter(|cell| cell.valid).count();
        for cell in &mut self.cells {
            cell.valid = false;
        }
        count
    }

    /// Exposure and post settings are absent by construction: they do not
    /// affect incident radiance and therefore perform no invalidation.
    pub fn invalidate_exposure(&mut self) -> usize {
        0
    }

    pub fn invalidate_dependency(
        &mut self,
        kind: u32,
        influence: Option<Aabb>,
    ) -> Result<usize, &'static str> {
        match kind {
            1..=3 => influence
                .map(|bounds| self.invalidate_swept_aabb(bounds))
                .ok_or("bounded probe dependency omitted its influence AABB"),
            4 => Ok(self.invalidate_environment()),
            _ => Err("unknown probe dependency kind"),
        }
    }

    /// Retains exact world-coordinate cells and invalidates newly exposed IDs.
    pub fn remap_for_camera(&mut self, camera: [f64; 3]) -> usize {
        let old = self.cells.clone();
        let mut retained = vec![false; old.len()];
        let mut invalid = 0;
        for (level_index, level) in self.levels.iter().copied().enumerate() {
            let center = snapped_cell(camera, f64::from(level.spacing));
            let new_origin = std::array::from_fn(|axis| center[axis] - level.dims[axis] as i32 / 2);
            for local in 0..level.probe_count {
                let id = (level.first_probe + local) as usize;
                let wanted = cell_coord(level, new_origin, local);
                let range =
                    level.first_probe as usize..(level.first_probe + level.probe_count) as usize;
                if let Some((old_id, cell)) = old.iter().enumerate().find(|(old_id, cell)| {
                    range.contains(old_id)
                        && !retained[*old_id]
                        && cell.valid
                        && cell.world_cell == wanted
                }) {
                    self.cells[id] = *cell;
                    retained[old_id] = true;
                } else {
                    self.cells[id] = ProbeCell {
                        world_cell: wanted,
                        ..ProbeCell::default()
                    };
                    invalid += 1;
                }
            }
            self.origins[level_index] = new_origin;
        }
        invalid
    }

    pub fn invalid_count(&self) -> usize {
        self.cells.iter().filter(|cell| !cell.valid).count()
    }

    pub fn assert_presentable(&self) -> Result<(), &'static str> {
        if self.cells.iter().all(|cell| cell.valid) {
            Ok(())
        } else {
            Err("presented frame would read an invalid probe")
        }
    }

    pub fn shade(
        &self,
        position: [f64; 3],
        normal: [f64; 3],
        camera: [f64; 3],
        albedo: [f64; 3],
    ) -> Result<([f64; 3], [[f64; 2]; 3]), &'static str> {
        self.assert_presentable()?;
        if self.levels.is_empty() {
            return Ok(([0.0; 3], [[0.0; 2]; 3]));
        }
        let distance = position
            .iter()
            .zip(camera)
            .map(|(a, b)| (a - b) * (a - b))
            .sum::<f64>()
            .sqrt();
        let fine = self
            .levels
            .iter()
            .position(|level| distance <= f64::from(level.spacing) * 4.0)
            .unwrap_or(self.levels.len() - 1);
        let coarse = (fine + 1).min(self.levels.len() - 1);
        let blend = if fine == coarse {
            0.0
        } else {
            ((distance / (f64::from(self.levels[fine].spacing) * 4.0)) - 0.5).clamp(0.0, 1.0)
        };
        let a = self.sample_level(fine, position, normal)?;
        let b = self.sample_level(coarse, position, normal)?;
        let mut candidate = [0.0; 3];
        let mut interval = [[0.0; 2]; 3];
        for channel in 0..3 {
            candidate[channel] = ((1.0 - blend) * a.0[channel] + blend * b.0[channel]).max(0.0)
                * albedo[channel]
                / std::f64::consts::PI;
            interval[channel] = [
                ((1.0 - blend) * a.1[channel][0] + blend * b.1[channel][0]).max(0.0)
                    * albedo[channel]
                    / std::f64::consts::PI,
                ((1.0 - blend) * a.1[channel][1] + blend * b.1[channel][1]).max(0.0)
                    * albedo[channel]
                    / std::f64::consts::PI,
            ];
        }
        Ok((candidate, interval))
    }

    fn sample_level(
        &self,
        level_index: usize,
        point: [f64; 3],
        normal: [f64; 3],
    ) -> Result<([f64; 3], [[f64; 2]; 3]), &'static str> {
        let level = self.levels[level_index];
        let spacing = f64::from(level.spacing);
        let grid = std::array::from_fn::<_, 3, _>(|axis| {
            point[axis] / spacing - f64::from(self.origins[level_index][axis])
        });
        let mut base = grid.map(|v| v.floor() as i32);
        let mut frac = std::array::from_fn::<_, 3, _>(|axis| grid[axis] - f64::from(base[axis]));
        for axis in 0..3 {
            if level.dims[axis] == 1 {
                base[axis] = 0;
                frac[axis] = 0.0;
            } else if base[axis] < 0 {
                base[axis] = 0;
                frac[axis] = 0.0;
            } else if base[axis] > level.dims[axis] as i32 - 2 {
                base[axis] = level.dims[axis] as i32 - 2;
                frac[axis] = 1.0;
            }
        }
        let sh = real_sh(normal)?;
        let mut samples = Vec::with_capacity(8);
        for dz in 0..=1 {
            for dy in 0..=1 {
                for dx in 0..=1 {
                    let local = [
                        if level.dims[0] == 1 { 0 } else { base[0] + dx },
                        if level.dims[1] == 1 { 0 } else { base[1] + dy },
                        if level.dims[2] == 1 { 0 } else { base[2] + dz },
                    ];
                    let local_id = local[0] as u32
                        + level.dims[0] * (local[1] as u32 + level.dims[1] * local[2] as u32);
                    let cell = self.cells[(level.first_probe + local_id) as usize];
                    if !cell.valid {
                        return Err("invalid probe read");
                    }
                    let w = [
                        if dx == 0 { 1.0 - frac[0] } else { frac[0] },
                        if dy == 0 { 1.0 - frac[1] } else { frac[1] },
                        if dz == 0 { 1.0 - frac[2] } else { frac[2] },
                    ]
                    .into_iter()
                    .product::<f64>();
                    let probe_position = position(cell.world_cell, level.spacing);
                    let visibility = leak_weight(cell, point, probe_position, spacing);
                    samples.push((cell, w * visibility));
                }
            }
        }
        let weight_sum = samples.iter().map(|(_, weight)| *weight).sum::<f64>();
        if weight_sum == 0.0 {
            return Ok(([0.0; 3], [[0.0; 2]; 3]));
        }
        let mut candidate = [0.0; 3];
        let mut interval = [[0.0; 2]; 3];
        for (cell, weight) in samples {
            let weight = weight / weight_sum;
            for channel in 0..3 {
                for coefficient in 0..9 {
                    let scale = weight * sh[coefficient] * SH_COSINE_BAND[coefficient];
                    candidate[channel] +=
                        f64::from(cell.coefficients[channel][coefficient].candidate) * scale;
                    let bounds = cell.coefficients[channel][coefficient].interval();
                    if scale >= 0.0 {
                        interval[channel][0] += bounds[0] * scale;
                        interval[channel][1] += bounds[1] * scale;
                    } else {
                        interval[channel][0] += bounds[1] * scale;
                        interval[channel][1] += bounds[0] * scale;
                    }
                }
            }
        }
        Ok((candidate, interval))
    }
}

fn real_sh(normal: [f64; 3]) -> Result<[f64; 9], &'static str> {
    let length = normal.iter().map(|v| v * v).sum::<f64>().sqrt();
    if !length.is_finite() || length == 0.0 {
        return Err("invalid shading normal");
    }
    let [x, y, z] = normal.map(|v| v / length);
    Ok([
        0.282_094_791_773_878_14,
        0.488_602_511_902_919_9 * y,
        0.488_602_511_902_919_9 * z,
        0.488_602_511_902_919_9 * x,
        1.092_548_430_592_079_2 * x * y,
        1.092_548_430_592_079_2 * y * z,
        0.315_391_565_252_520_05 * (3.0 * z * z - 1.0),
        1.092_548_430_592_079_2 * x * z,
        0.546_274_215_296_039_6 * (x * x - y * y),
    ])
}

/// Fixed leak rule: select the dominant signed axis, compare the sample
/// distance to that ray-family mean, smoothstep over one cell, and clamp.
fn leak_weight(cell: ProbeCell, sample: [f64; 3], probe: [f64; 3], spacing: f64) -> f64 {
    let delta = std::array::from_fn::<_, 3, _>(|axis| sample[axis] - probe[axis]);
    let distance = delta.iter().map(|v| v * v).sum::<f64>().sqrt();
    if distance == 0.0 {
        return 1.0;
    }
    let axis = delta
        .iter()
        .copied()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.abs().total_cmp(&b.abs()))
        .unwrap()
        .0;
    let bucket = axis * 2 + usize::from(delta[axis] < 0.0);
    let mean = cell.distance_moments[bucket].mean.interval()[1].max(0.0);
    let t = ((mean + spacing - distance) / spacing).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::probe::{ProbeProgramV1, direction_table_v1, table_digest_v1};

    fn program() -> ProbeProgramV1 {
        ProbeProgramV1 {
            enabled: true,
            static_preinitialized: false,
            levels: vec![ProbeLevelV1 {
                level: 0,
                dims: [2, 2, 2],
                spacing: 1.0,
                first_probe: 0,
                probe_count: 8,
            }],
            directions: direction_table_v1(),
            dependencies: vec![],
            probe_count: 8,
            invalidation_capacity: 8,
            all_invalid_secondary_rays: 256,
            storage_bytes: 0,
            table_digest: table_digest_v1(),
        }
    }

    fn constant_trace(
        _: u32,
        _: [f64; 3],
        _: u32,
        _: [f32; 3],
    ) -> Result<SecondaryRayResult, &'static str> {
        Ok(SecondaryRayResult {
            radiance: [1.0, 0.5, 0.25],
            distance: 100.0,
        })
    }

    #[test]
    fn one_and_three_worker_initialization_are_identical() {
        let versions = DependencyVersions {
            scene: 2,
            light: 3,
            material: 4,
        };
        let mut one = ProbeState::invalid(&program(), [0.0; 3]);
        let mut three = one.clone();
        one.initialize(1, versions, constant_trace).unwrap();
        three.initialize(3, versions, constant_trace).unwrap();
        assert_eq!(one, three);
        assert_eq!(one.invalid_count(), 0);
    }

    #[test]
    fn cancellation_commits_no_partial_validity() {
        let mut state = ProbeState::invalid(&program(), [0.0; 3]);
        let result = state.initialize(
            3,
            DependencyVersions {
                scene: 1,
                light: 1,
                material: 1,
            },
            |id, origin, direction, vector| {
                if id == 3 && direction == 7 {
                    Err("cancelled")
                } else {
                    constant_trace(id, origin, direction, vector)
                }
            },
        );
        assert_eq!(result, Err("cancelled"));
        assert_eq!(state.invalid_count(), 8);
    }

    #[test]
    fn remap_retains_overlapping_world_cells() {
        let mut state = ProbeState::invalid(&program(), [0.0; 3]);
        state
            .initialize(
                1,
                DependencyVersions {
                    scene: 1,
                    light: 1,
                    material: 1,
                },
                constant_trace,
            )
            .unwrap();
        let retained = state
            .cells
            .iter()
            .find(|cell| cell.world_cell == [0, 0, 0])
            .copied()
            .unwrap();
        assert!(state.remap_for_camera([1.0, 0.0, 0.0]) > 0);
        assert!(state.cells.iter().any(|cell| *cell == retained));
    }

    #[test]
    fn static_exposure_and_invalid_present_are_fail_closed() {
        let mut state = ProbeState::invalid(&program(), [0.0; 3]);
        assert_eq!(state.invalidate_exposure(), 0);
        assert!(state.assert_presentable().is_err());
        state
            .initialize(
                1,
                DependencyVersions {
                    scene: 1,
                    light: 1,
                    material: 1,
                },
                constant_trace,
            )
            .unwrap();
        assert!(
            state
                .shade([0.0; 3], [0.0, 1.0, 0.0], [0.0; 3], [1.0; 3])
                .is_ok()
        );
    }

    #[test]
    fn static_frame_updates_zero_and_changed_dependencies_update_every_invalid_cell() {
        let versions = DependencyVersions {
            scene: 1,
            light: 1,
            material: 1,
        };
        let mut state = ProbeState::invalid(&program(), [0.0; 3]);
        state.initialize(3, versions, constant_trace).unwrap();
        assert_eq!(state.update_invalid(3, versions, constant_trace), Ok(0));
        state.support_radius = 0.1;
        assert!(
            state
                .invalidate_dependency(
                    2,
                    Some(Aabb {
                        min: [-1.1, -0.1, -0.1],
                        max: [0.1, 0.1, 0.1],
                    })
                )
                .unwrap()
                > 0
        );
        let invalid = state.invalid_count();
        assert_eq!(
            state.update_invalid(
                1,
                DependencyVersions {
                    scene: 1,
                    light: 2,
                    material: 1
                },
                constant_trace
            ),
            Ok(invalid)
        );
        state.assert_presentable().unwrap();
        assert!(state.invalidate_dependency(9, None).is_err());
    }

    #[test]
    fn swept_bounds_invalidate_a_conservative_neighborhood() {
        let mut state = ProbeState::invalid(&program(), [0.0; 3]);
        state
            .initialize(
                1,
                DependencyVersions {
                    scene: 1,
                    light: 1,
                    material: 1,
                },
                constant_trace,
            )
            .unwrap();
        state.support_radius = 0.1;
        let swept = Aabb::swept(
            Aabb {
                min: [-1.1, -0.1, -0.1],
                max: [-0.9, 0.1, 0.1],
            },
            Aabb {
                min: [-0.1; 3],
                max: [0.1; 3],
            },
        );
        let invalidated = state.invalidate_swept_aabb(swept);
        assert!(invalidated > 0 && invalidated < state.cells.len());
    }
}
