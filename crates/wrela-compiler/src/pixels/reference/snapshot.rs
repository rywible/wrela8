//! Owned frame-input validation and padding-free coefficient snapshots.

/// This is the v1 compiler admission ceiling. Accepting a program and then
/// discovering that its parameters do not fit at frame time would turn a
/// compile-time capacity proof into a runtime surprise.
pub const MAX_PARAMETER_SLOTS: usize = 16;
pub const MAX_LIGHTS: usize = 8;
const CANONICAL_FRAME_SCALARS: usize = 16;
const LIGHT_SCALARS: usize = 15;
const MAX_SNAPSHOT_BYTES: usize =
    (MAX_PARAMETER_SLOTS + CANONICAL_FRAME_SCALARS + MAX_LIGHTS * LIGHT_SCALARS) * 4;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

impl Vec3 {
    fn dot(self, other: Self) -> f32 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }

    fn scaled(self, value: f32) -> Self {
        Self {
            x: self.x * value,
            y: self.y * value,
            z: self.z * value,
        }
    }

    fn subtract(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }

    fn finite(self) -> bool {
        self.x.is_finite() && self.y.is_finite() && self.z.is_finite()
    }

    fn normalized(self) -> Option<Self> {
        let squared = self.dot(self);
        if !squared.is_finite() || squared <= 0.0 {
            return None;
        }
        let inverse = squared.sqrt().recip();
        let result = self.scaled(inverse);
        result.finite().then_some(result)
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CameraInput {
    pub eye: Vec3,
    pub forward: Vec3,
    pub right: Vec3,
    pub up: Vec3,
    pub near: f32,
    pub far: f32,
    pub output_mode: u32,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct LightInput {
    pub kind: u8,
    pub position: Vec3,
    pub direction: Vec3,
    pub axis_u: Vec3,
    pub axis_v: Vec3,
    pub radiance: Vec3,
}

impl LightInput {
    fn finite(self) -> bool {
        self.position.finite()
            && self.direction.finite()
            && self.axis_u.finite()
            && self.axis_v.finite()
            && self.radiance.finite()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct FrameInput<P> {
    pub params_owner: P,
    pub parameters: [f32; MAX_PARAMETER_SLOTS],
    pub parameter_count: u16,
    pub camera: CameraInput,
    pub lights: [LightInput; MAX_LIGHTS],
    pub light_count: u8,
    pub exposure: f32,
    pub environment: Vec3,
    pub texture_ids: [u32; MAX_LIGHTS],
    pub texture_count: u8,
    pub frame_index: u64,
}

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ParameterContract {
    pub min: f32,
    pub max: f32,
    pub max_delta: f32,
    pub max_second_delta: f32,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FrameContract {
    pub parameters: [ParameterContract; MAX_PARAMETER_SLOTS],
    pub parameter_count: u16,
    pub near: f32,
    pub far: f32,
    pub output_mode: u32,
    pub light_kinds: [u8; MAX_LIGHTS],
    pub light_count: u8,
    pub texture_ids: [u32; MAX_LIGHTS],
    pub texture_count: u8,
    pub exposure_min: f32,
    pub exposure_max: f32,
    pub environment_min: Vec3,
    pub environment_max: Vec3,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SnapshotError {
    NonFiniteInput { component: u16 },
    ParameterOutOfRange { slot: u16 },
    FrameContractMismatch { component: u16 },
    InvalidCamera,
    CapacityExceeded,
}

#[derive(Clone, Debug, PartialEq)]
pub struct CoeffSnapshot {
    bytes: [u8; MAX_SNAPSHOT_BYTES],
    byte_count: u16,
    parameter_deltas: [f32; MAX_PARAMETER_SLOTS],
    has_parameter_deltas: bool,
    pub reuse_eligible: bool,
    pub frame_index: u64,
}

impl CoeffSnapshot {
    pub fn bytes(&self) -> &[u8] {
        &self.bytes[..usize::from(self.byte_count)]
    }

    pub fn digest(&self) -> [u8; 32] {
        wrela_machine::sha256::sha256(self.bytes())
    }

    fn parameter(&self, slot: usize) -> Option<f32> {
        let start = slot.checked_mul(4)?;
        let bytes: [u8; 4] = self.bytes.get(start..start + 4)?.try_into().ok()?;
        Some(f32::from_bits(u32::from_le_bytes(bytes)))
    }
}

pub fn validate_and_pack<P>(
    input: FrameInput<P>,
    contract: &FrameContract,
    previous_presented: Option<&CoeffSnapshot>,
    kinetic_reuse_requested: bool,
) -> Result<(P, CoeffSnapshot), (P, SnapshotError)> {
    let FrameInput {
        params_owner,
        parameters,
        parameter_count,
        camera,
        lights,
        light_count,
        exposure,
        environment,
        texture_ids,
        texture_count,
        frame_index,
    } = input;
    macro_rules! fail {
        ($error:expr) => {
            return Err((params_owner, $error))
        };
    }
    if usize::from(parameter_count) > MAX_PARAMETER_SLOTS
        || usize::from(contract.parameter_count) > MAX_PARAMETER_SLOTS
        || parameter_count != contract.parameter_count
        || usize::from(light_count) > MAX_LIGHTS
        || usize::from(contract.light_count) > MAX_LIGHTS
        || usize::from(texture_count) > MAX_LIGHTS
        || usize::from(contract.texture_count) > MAX_LIGHTS
    {
        fail!(SnapshotError::CapacityExceeded);
    }
    if !contract.near.is_finite()
        || !contract.far.is_finite()
        || contract.near <= 0.0
        || contract.near >= contract.far
        || !contract.exposure_min.is_finite()
        || !contract.exposure_max.is_finite()
        || contract.exposure_min > contract.exposure_max
        || !contract.environment_min.finite()
        || !contract.environment_max.finite()
        || contract.environment_min.x > contract.environment_max.x
        || contract.environment_min.y > contract.environment_max.y
        || contract.environment_min.z > contract.environment_max.z
        || contract.light_kinds[..usize::from(contract.light_count)]
            .iter()
            .any(|kind| *kind > 4)
    {
        fail!(SnapshotError::FrameContractMismatch { component: 0 });
    }
    for parameter in &contract.parameters[..usize::from(parameter_count)] {
        if !parameter.min.is_finite()
            || !parameter.max.is_finite()
            || !parameter.max_delta.is_finite()
            || !parameter.max_second_delta.is_finite()
            || parameter.min > parameter.max
            || parameter.max_delta < 0.0
            || parameter.max_second_delta < 0.0
        {
            fail!(SnapshotError::FrameContractMismatch { component: 0 });
        }
    }
    if camera.near.to_bits() != contract.near.to_bits()
        || camera.far.to_bits() != contract.far.to_bits()
        || camera.output_mode != contract.output_mode
        || light_count != contract.light_count
        || texture_count != contract.texture_count
        || texture_ids[..usize::from(texture_count)]
            != contract.texture_ids[..usize::from(texture_count)]
    {
        fail!(SnapshotError::FrameContractMismatch { component: 1 });
    }
    if !exposure.is_finite()
        || !environment.finite()
        || !camera.eye.finite()
        || !camera.forward.finite()
        || !camera.right.finite()
        || !camera.up.finite()
    {
        fail!(SnapshotError::NonFiniteInput { component: 2 });
    }
    if exposure < contract.exposure_min
        || exposure > contract.exposure_max
        || environment.x < contract.environment_min.x
        || environment.y < contract.environment_min.y
        || environment.z < contract.environment_min.z
        || environment.x > contract.environment_max.x
        || environment.y > contract.environment_max.y
        || environment.z > contract.environment_max.z
    {
        fail!(SnapshotError::ParameterOutOfRange {
            slot: parameter_count,
        });
    }
    for (slot, value) in parameters[..usize::from(parameter_count)]
        .iter()
        .copied()
        .enumerate()
    {
        if !value.is_finite() {
            fail!(SnapshotError::NonFiniteInput {
                component: u16::try_from(slot).unwrap_or(u16::MAX),
            });
        }
        let range = contract.parameters[slot];
        if value < range.min || value > range.max {
            fail!(SnapshotError::ParameterOutOfRange {
                slot: u16::try_from(slot).unwrap_or(u16::MAX),
            });
        }
    }
    for (slot, light) in lights[..usize::from(light_count)]
        .iter()
        .copied()
        .enumerate()
    {
        if !light.finite() {
            fail!(SnapshotError::NonFiniteInput {
                component: 0x100 + u16::try_from(slot).unwrap_or(u16::MAX),
            });
        }
        if light.kind != contract.light_kinds[slot] {
            fail!(SnapshotError::FrameContractMismatch {
                component: 0x100 + u16::try_from(slot).unwrap_or(u16::MAX),
            });
        }
    }

    let Some(forward) = camera.forward.normalized() else {
        fail!(SnapshotError::InvalidCamera);
    };
    let right_seed = camera
        .right
        .subtract(forward.scaled(camera.right.dot(forward)));
    let Some(right) = right_seed.normalized() else {
        fail!(SnapshotError::InvalidCamera);
    };
    let Some(up) = forward.cross(right).normalized() else {
        fail!(SnapshotError::InvalidCamera);
    };
    if up.dot(camera.up) <= 0.0 {
        fail!(SnapshotError::InvalidCamera);
    }

    let mut bytes = [0; MAX_SNAPSHOT_BYTES];
    let mut cursor = 0_usize;
    let mut write_f32 = |value: f32| {
        bytes[cursor..cursor + 4].copy_from_slice(&value.to_bits().to_le_bytes());
        cursor += 4;
    };
    for value in parameters[..usize::from(parameter_count)].iter().copied() {
        write_f32(value);
    }
    for value in [
        camera.eye.x,
        camera.eye.y,
        camera.eye.z,
        forward.x,
        forward.y,
        forward.z,
        right.x,
        right.y,
        right.z,
        up.x,
        up.y,
        up.z,
        exposure,
        environment.x,
        environment.y,
        environment.z,
    ] {
        write_f32(value);
    }
    for light in lights[..usize::from(light_count)].iter().copied() {
        for value in [
            light.position.x,
            light.position.y,
            light.position.z,
            light.direction.x,
            light.direction.y,
            light.direction.z,
            light.axis_u.x,
            light.axis_u.y,
            light.axis_u.z,
            light.axis_v.x,
            light.axis_v.y,
            light.axis_v.z,
            light.radiance.x,
            light.radiance.y,
            light.radiance.z,
        ] {
            write_f32(value);
        }
    }
    let byte_count = match u16::try_from(cursor) {
        Ok(value) => value,
        Err(_) => fail!(SnapshotError::CapacityExceeded),
    };
    let mut snapshot = CoeffSnapshot {
        bytes,
        byte_count,
        parameter_deltas: [0.0; MAX_PARAMETER_SLOTS],
        has_parameter_deltas: false,
        reuse_eligible: previous_presented.is_some(),
        frame_index,
    };
    if kinetic_reuse_requested {
        let Some(previous) = previous_presented else {
            snapshot.reuse_eligible = false;
            return Ok((params_owner, snapshot));
        };
        let mut deltas_complete = true;
        for (slot, value) in parameters[..usize::from(parameter_count)]
            .iter()
            .copied()
            .enumerate()
        {
            let Some(old) = previous.parameter(slot) else {
                snapshot.reuse_eligible = false;
                deltas_complete = false;
                break;
            };
            let delta = value - old;
            snapshot.parameter_deltas[slot] = delta;
            if !delta.is_finite()
                || delta.abs() > contract.parameters[slot].max_delta
                || (previous.has_parameter_deltas
                    && (delta - previous.parameter_deltas[slot]).abs()
                        > contract.parameters[slot].max_second_delta)
            {
                snapshot.reuse_eligible = false;
            }
        }
        snapshot.has_parameter_deltas = deltas_complete;
    } else {
        snapshot.reuse_eligible = false;
    }
    Ok((params_owner, snapshot))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn contract() -> FrameContract {
        let mut parameters = [ParameterContract::default(); MAX_PARAMETER_SLOTS];
        parameters[0] = ParameterContract {
            min: -1.0,
            max: 1.0,
            max_delta: 0.25,
            max_second_delta: 0.0625,
        };
        FrameContract {
            parameters,
            parameter_count: 1,
            near: 0.1,
            far: 128.0,
            output_mode: 1,
            light_kinds: [0; MAX_LIGHTS],
            light_count: 0,
            texture_ids: [0; MAX_LIGHTS],
            texture_count: 0,
            exposure_min: -1.0,
            exposure_max: 1.0,
            environment_min: Vec3::default(),
            environment_max: Vec3 {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        }
    }

    fn frame(owner: u32, parameter: f32) -> FrameInput<u32> {
        let mut parameters = [0.0; MAX_PARAMETER_SLOTS];
        parameters[0] = parameter;
        FrameInput {
            params_owner: owner,
            parameters,
            parameter_count: 1,
            camera: CameraInput {
                eye: Vec3::default(),
                forward: Vec3 {
                    x: 0.0,
                    y: 0.0,
                    z: 1.0,
                },
                right: Vec3 {
                    x: 1.0,
                    y: 0.0,
                    z: 0.0,
                },
                up: Vec3 {
                    x: 0.0,
                    y: 1.0,
                    z: 0.0,
                },
                near: 0.1,
                far: 128.0,
                output_mode: 1,
            },
            lights: [LightInput::default(); MAX_LIGHTS],
            light_count: 0,
            exposure: 0.0,
            environment: Vec3::default(),
            texture_ids: [0; MAX_LIGHTS],
            texture_count: 0,
            frame_index: 7,
        }
    }

    #[test]
    fn first_frame_snapshot_is_deterministic_and_needs_no_previous_state() {
        let first = validate_and_pack(frame(7, 0.5), &contract(), None, false)
            .unwrap()
            .1;
        let second = validate_and_pack(frame(8, 0.5), &contract(), None, false)
            .unwrap()
            .1;
        assert_eq!(first.bytes(), second.bytes());
        assert_eq!(first.digest(), second.digest());
        assert!(!first.reuse_eligible);
    }

    #[test]
    fn validation_returns_owned_parameters_on_every_error() {
        let (owner, error) =
            validate_and_pack(frame(17, f32::NAN), &contract(), None, false).unwrap_err();
        assert_eq!(owner, 17);
        assert!(matches!(error, SnapshotError::NonFiniteInput { .. }));

        let (owner, error) =
            validate_and_pack(frame(19, 2.0), &contract(), None, false).unwrap_err();
        assert_eq!(owner, 19);
        assert_eq!(error, SnapshotError::ParameterOutOfRange { slot: 0 });
    }

    #[test]
    fn out_of_rate_is_legal_for_from_scratch_and_only_disables_reuse() {
        let previous = validate_and_pack(frame(1, 0.0), &contract(), None, false)
            .unwrap()
            .1;
        let current = validate_and_pack(frame(2, 0.75), &contract(), Some(&previous), true)
            .unwrap()
            .1;
        assert!(!current.reuse_eligible);
    }

    #[test]
    fn second_delta_violation_disables_reuse_without_rejecting_frame() {
        let first = validate_and_pack(frame(1, 0.0), &contract(), None, true)
            .unwrap()
            .1;
        let second = validate_and_pack(frame(2, 0.1), &contract(), Some(&first), true)
            .unwrap()
            .1;
        assert!(second.reuse_eligible);
        let third = validate_and_pack(frame(3, 0.3), &contract(), Some(&second), true)
            .unwrap()
            .1;
        assert!(!third.reuse_eligible);
        assert_eq!(third.parameter(0), Some(0.3));
    }

    #[test]
    fn maximum_light_topology_fits_the_fixed_snapshot_exactly() {
        let mut contract = contract();
        contract.light_count = MAX_LIGHTS as u8;
        let mut input = frame(4, 0.0);
        input.light_count = MAX_LIGHTS as u8;
        let snapshot = validate_and_pack(input, &contract, None, false).unwrap().1;
        assert_eq!(
            snapshot.bytes().len(),
            (1 + CANONICAL_FRAME_SCALARS + MAX_LIGHTS * LIGHT_SCALARS) * 4
        );
        assert!(snapshot.bytes().len() <= MAX_SNAPSHOT_BYTES);
    }

    #[test]
    fn malformed_sealed_contract_fails_before_any_index_or_numeric_use() {
        let mut malformed = contract();
        malformed.light_count = u8::MAX;
        assert_eq!(
            validate_and_pack(frame(11, 0.0), &malformed, None, false),
            Err((11, SnapshotError::CapacityExceeded))
        );

        let mut malformed = contract();
        malformed.near = malformed.far;
        assert_eq!(
            validate_and_pack(frame(12, 0.0), &malformed, None, false),
            Err((12, SnapshotError::FrameContractMismatch { component: 0 }))
        );

        let mut malformed = contract();
        malformed.environment_min.x = f32::NAN;
        assert_eq!(
            validate_and_pack(frame(13, 0.0), &malformed, None, false),
            Err((13, SnapshotError::FrameContractMismatch { component: 0 }))
        );
    }

    #[test]
    fn snapshot_capacity_equals_the_compiler_machine_ceiling() {
        assert_eq!(
            MAX_PARAMETER_SLOTS,
            super::super::super::capacities::PixelsCeilings::MACHINE_V1.parameter_slots as usize
        );
        assert!(MAX_SNAPSHOT_BYTES <= usize::from(u16::MAX));
    }
}
