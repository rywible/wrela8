//! Canonical inverse-view-depth camera algebra.

use super::config::RendererConfig;
use super::reference::interval::F64Interval;

/// Version 1 seals a 90 degree vertical field of view. Camera basis vectors
/// remain explicit runtime inputs; no per-pixel ray normalization is allowed.
pub const FOV_Y_RADIANS_V1: f64 = std::f64::consts::FRAC_PI_2;
pub const TAN_HALF_FOV_Y_V1: f64 = 1.0;
pub const BASIS_ORTHONORMAL_TOLERANCE_V1: f64 = 2.0e-4;

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ScreenPoint {
    pub u: f64,
    pub v: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraSample {
    pub eye: [f64; 3],
    pub forward: [f64; 3],
    pub right: [f64; 3],
    pub up: [f64; 3],
}

fn dot(a: [f64; 3], b: [f64; 3]) -> f64 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn cross(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [
        a[1] * b[2] - a[2] * b[1],
        a[2] * b[0] - a[0] * b[2],
        a[0] * b[1] - a[1] * b[0],
    ]
}

fn normalize(value: [f64; 3], failure: &str) -> Result<[f64; 3], String> {
    let length = dot(value, value).sqrt();
    if !length.is_finite() || length <= 0.0 {
        return Err(format!(
            "P016: projective denominator may reach zero inside the declared camera/world domain: {failure}"
        ));
    }
    Ok(value.map(|component| component / length))
}

pub fn camera_from_look_at(
    eye: [f64; 3],
    target: [f64; 3],
    authored_up: [f64; 3],
) -> Result<CameraSample, String> {
    let forward = normalize(
        std::array::from_fn(|component| target[component] - eye[component]),
        "camera eye and target coincide",
    )?;
    let right = normalize(
        cross(authored_up, forward),
        "camera up is zero or parallel to the view direction",
    )?;
    let camera = CameraSample {
        eye,
        forward,
        right,
        up: cross(forward, right),
    };
    validate_basis(camera)?;
    Ok(camera)
}

pub fn camera_from_quaternion(
    eye: [f64; 3],
    quaternion_xyzw: [f64; 4],
) -> Result<CameraSample, String> {
    let [x, y, z, w] = quaternion_xyzw;
    let norm = (x * x + y * y + z * z + w * w).sqrt();
    if !norm.is_finite() || norm <= 0.0 {
        return Err("P016: projective denominator may reach zero inside the declared camera/world domain: camera quaternion is zero or non-finite".to_string());
    }
    let [x, y, z, w] = [x / norm, y / norm, z / norm, w / norm];
    let camera = CameraSample {
        eye,
        right: [
            1.0 - 2.0 * (y * y + z * z),
            2.0 * (x * y + w * z),
            2.0 * (x * z - w * y),
        ],
        up: [
            2.0 * (x * y - w * z),
            1.0 - 2.0 * (x * x + z * z),
            2.0 * (y * z + w * x),
        ],
        forward: [
            2.0 * (x * z + w * y),
            2.0 * (y * z - w * x),
            1.0 - 2.0 * (x * x + y * y),
        ],
    };
    validate_basis(camera)?;
    Ok(camera)
}

pub fn camera_from_explicit_basis(
    eye: [f64; 3],
    authored_forward: [f64; 3],
    authored_right: [f64; 3],
    authored_up: [f64; 3],
) -> Result<CameraSample, String> {
    let forward = normalize(authored_forward, "camera forward is zero")?;
    let projection = dot(authored_right, forward);
    let right = normalize(
        std::array::from_fn(|component| {
            authored_right[component] - projection * forward[component]
        }),
        "camera right is zero or parallel to forward",
    )?;
    let up = cross(forward, right);
    let authored_up = normalize(authored_up, "camera up is zero")?;
    if dot(up, authored_up) <= 0.0 {
        return Err("P016: projective denominator may reach zero inside the declared camera/world domain: explicit camera basis is left-handed".to_string());
    }
    let camera = CameraSample {
        eye,
        forward,
        right,
        up,
    };
    validate_basis(camera)?;
    Ok(camera)
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CameraContract {
    pub width: u32,
    pub height: u32,
    pub aspect: f64,
    pub tan_half_fov_y: f64,
    pub q: F64Interval,
    pub eye_component: [F64Interval; 3],
    pub forward_component: [F64Interval; 3],
    pub right_component: [F64Interval; 3],
    pub up_component: [F64Interval; 3],
    pub eye_rate_component: [F64Interval; 3],
    pub forward_rate_component: [F64Interval; 3],
    pub right_rate_component: [F64Interval; 3],
    pub up_rate_component: [F64Interval; 3],
    pub max_frame_motion: f64,
}

impl CameraContract {
    pub fn derive(config: &RendererConfig) -> Result<Self, String> {
        if config.width == 0 || config.height == 0 {
            return Err("P016: projective denominator may reach zero inside the declared camera/world domain: zero output extent".to_string());
        }
        if !config.near.is_finite()
            || !config.far.is_finite()
            || config.near <= 0.0
            || config.near >= config.far
        {
            return Err(format!(
                "P016: projective denominator may reach zero inside the declared camera/world domain: require 0 < near < far, found near={} far={}",
                config.near, config.far
            ));
        }
        let q = F64Interval::new(
            super::reference::interval::next_down(1.0 / config.far),
            super::reference::interval::next_up(1.0 / config.near),
        )?;
        if q.lo <= 0.0 {
            return Err(format!(
                "P016: projective denominator may reach zero inside the declared camera/world domain: q={q:?}"
            ));
        }
        let motion = f64::from(config.camera_max_motion);
        let center_x = (f64::from(config.world_min.x) + f64::from(config.world_max.x)) * 0.5;
        let center_y = (f64::from(config.world_min.y) + f64::from(config.world_max.y)) * 0.5;
        // `CameraBounds.bounded` is the sealed v1 forward-facing camera box:
        // x/y move around the world-box center; z stays behind the minimum
        // world face by near+motion, so the complete scene domain cannot
        // silently cross the eye/near plane.
        let center_z = f64::from(config.world_min.z) - config.near - motion;
        let centers = [center_x, center_y, center_z];
        let eye_component = [
            F64Interval::new(
                super::reference::interval::next_down(centers[0] - motion),
                super::reference::interval::next_up(centers[0] + motion),
            )?,
            F64Interval::new(
                super::reference::interval::next_down(centers[1] - motion),
                super::reference::interval::next_up(centers[1] + motion),
            )?,
            F64Interval::new(
                super::reference::interval::next_down(centers[2] - motion),
                super::reference::interval::next_up(centers[2] + motion),
            )?,
        ];
        let basis_slack = BASIS_ORTHONORMAL_TOLERANCE_V1;
        let basis_radius = motion + basis_slack;
        let basis_component = |center: f64| {
            F64Interval::new(
                super::reference::interval::next_down((center - basis_radius).max(-1.0)),
                super::reference::interval::next_up((center + basis_radius).min(1.0)),
            )
        };
        // CameraBounds.bounded is centered on the canonical forward-facing
        // basis. `max_motion` bounds both absolute component displacement
        // from that pose and per-frame component rate. Larger authored turns
        // remain expressible by declaring a correspondingly larger bound;
        // runtime validation rejects an undeclared pose before it can make a
        // compiler projection exclusion unsound.
        let forward_component = [
            basis_component(0.0)?,
            basis_component(0.0)?,
            basis_component(1.0)?,
        ];
        let right_component = [
            basis_component(1.0)?,
            basis_component(0.0)?,
            basis_component(0.0)?,
        ];
        let up_component = [
            basis_component(0.0)?,
            basis_component(1.0)?,
            basis_component(0.0)?,
        ];
        // `camera_max_motion` is the sealed per-frame component delta for
        // both the eye and orthonormal basis.  Keeping the rate components
        // explicit lets temporal predicate programs account for camera
        // translation and rotation without sampling an undeclared pose.
        let rate = F64Interval::new(
            super::reference::interval::next_down(-motion),
            super::reference::interval::next_up(motion),
        )?;
        Ok(Self {
            width: config.width,
            height: config.height,
            aspect: f64::from(config.width) / f64::from(config.height),
            tan_half_fov_y: TAN_HALF_FOV_Y_V1,
            q,
            eye_component,
            forward_component,
            right_component,
            up_component,
            eye_rate_component: [rate; 3],
            forward_rate_component: [rate; 3],
            right_rate_component: [rate; 3],
            up_rate_component: [rate; 3],
            max_frame_motion: motion,
        })
    }

    pub fn pixel_center(&self, x: u32, y: u32) -> Result<ScreenPoint, String> {
        if x >= self.width || y >= self.height {
            return Err(format!(
                "pixels::camera: pixel ({x},{y}) is outside {}x{}",
                self.width, self.height
            ));
        }
        Ok(ScreenPoint {
            u: (((f64::from(x) + 0.5) / f64::from(self.width)) * 2.0 - 1.0)
                * self.aspect
                * self.tan_half_fov_y,
            v: (1.0 - ((f64::from(y) + 0.5) / f64::from(self.height)) * 2.0) * self.tan_half_fov_y,
        })
    }

    fn raw_ray(&self, camera: CameraSample, screen: ScreenPoint) -> [f64; 3] {
        std::array::from_fn(|component| {
            camera.forward[component]
                + screen.u * camera.right[component]
                + screen.v * camera.up[component]
        })
    }

    pub fn point(
        &self,
        camera: CameraSample,
        screen: ScreenPoint,
        q: f64,
    ) -> Result<[f64; 3], String> {
        validate_sample_against_contract(*self, camera)?;
        if !q.is_finite() || q <= 0.0 {
            return Err(format!(
                "P016: projective denominator may reach zero inside the declared camera/world domain: q={q}"
            ));
        }
        let ray = self.raw_ray(camera, screen);
        Ok(std::array::from_fn(|component| {
            camera.eye[component] + ray[component] / q
        }))
    }
}

pub fn validate_basis(camera: CameraSample) -> Result<(), String> {
    if camera
        .eye
        .iter()
        .chain(&camera.forward)
        .chain(&camera.right)
        .chain(&camera.up)
        .any(|value| !value.is_finite())
    {
        return Err("P016: projective denominator may reach zero inside the declared camera/world domain: camera contains a non-finite component".to_string());
    }
    let dot = |a: [f64; 3], b: [f64; 3]| a[0] * b[0] + a[1] * b[1] + a[2] * b[2];
    let norm = |value: [f64; 3]| dot(value, value).sqrt();
    for (name, vector) in [
        ("forward", camera.forward),
        ("right", camera.right),
        ("up", camera.up),
    ] {
        let length = norm(vector);
        if !length.is_finite() || (length - 1.0).abs() > BASIS_ORTHONORMAL_TOLERANCE_V1 {
            return Err(format!(
                "P016: projective denominator may reach zero inside the declared camera/world domain: camera {name} vector is zero or non-unit (length={length})"
            ));
        }
    }
    for (a_name, a, b_name, b) in [
        ("right", camera.right, "up", camera.up),
        ("right", camera.right, "forward", camera.forward),
        ("up", camera.up, "forward", camera.forward),
    ] {
        let product = dot(a, b);
        if product.abs() > BASIS_ORTHONORMAL_TOLERANCE_V1 {
            return Err(format!(
                "P016: projective denominator may reach zero inside the declared camera/world domain: camera {a_name}/{b_name} basis dot is {product}"
            ));
        }
    }
    let cross = [
        camera.right[1] * camera.up[2] - camera.right[2] * camera.up[1],
        camera.right[2] * camera.up[0] - camera.right[0] * camera.up[2],
        camera.right[0] * camera.up[1] - camera.right[1] * camera.up[0],
    ];
    // right × up = forward is the sealed right-handed convention.
    let handedness = dot(cross, camera.forward);
    if handedness < 1.0 - BASIS_ORTHONORMAL_TOLERANCE_V1 {
        return Err(format!(
            "P016: projective denominator may reach zero inside the declared camera/world domain: camera basis is not right-handed (orientation={handedness})"
        ));
    }
    Ok(())
}

pub fn validate_sample_against_contract(
    contract: CameraContract,
    camera: CameraSample,
) -> Result<(), String> {
    validate_basis(camera)?;
    for (name, values, intervals) in [
        ("eye", camera.eye, contract.eye_component),
        ("forward", camera.forward, contract.forward_component),
        ("right", camera.right, contract.right_component),
        ("up", camera.up, contract.up_component),
    ] {
        for component in 0..3 {
            if !intervals[component].contains(values[component]) {
                return Err(format!(
                    "P016: projective denominator may reach zero inside the declared camera/world domain: camera {name}[{component}]={} is outside [{},{}]",
                    values[component], intervals[component].lo, intervals[component].hi
                ));
            }
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::config::{RgbRangeConfig, ScalarRangeConfig, Vec3Config};
    use crate::sema::types::Type;

    fn config() -> RendererConfig {
        RendererConfig {
            declaration_index: 0,
            worker_count: 1,
            params_type: Type::Unit,
            field: String::new(),
            material: String::new(),
            material_type: Type::Unit,
            display_index: 0,
            display_doorbell_addr: wrela_machine::pixels::DOORBELL_ADDR,
            width: 4,
            height: 2,
            refresh_hz: 60,
            shade_hz: 60,
            profile: "AaaByteExact".to_string(),
            tone_curve: "Linear".to_string(),
            near: 0.25,
            far: 4.0,
            world_min: Vec3Config {
                x: -1.0,
                y: -1.0,
                z: -1.0,
            },
            world_max: Vec3Config {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
            camera_pose: None,
            camera_max_motion: 0.25,
            light_capacity: 0,
            light_kinds: Vec::new(),
            exposure: ScalarRangeConfig { min: 0.0, max: 0.0 },
            environment: RgbRangeConfig {
                min: [0.0; 3],
                max: [0.0; 3],
            },
            ao_enabled: false,
            probes_enabled: false,
            probe_initialization_worst_case_ms: 0,
            initialization_deadline_ms: 1,
            parameter_contracts: Vec::new(),
        }
    }

    #[test]
    fn screen_centers_pin_x_and_framebuffer_y_direction() {
        let camera = CameraContract::derive(&config()).unwrap();
        assert_eq!(
            camera.pixel_center(0, 0).unwrap(),
            ScreenPoint { u: -1.5, v: 0.5 }
        );
        assert_eq!(
            camera.pixel_center(3, 1).unwrap(),
            ScreenPoint { u: 1.5, v: -0.5 }
        );
        assert!(camera.pixel_center(0, 0).unwrap().v > camera.pixel_center(0, 1).unwrap().v);

        let mut odd_config = config();
        odd_config.width = 3;
        odd_config.height = 3;
        let odd_camera = CameraContract::derive(&odd_config).unwrap();
        assert_eq!(
            odd_camera.pixel_center(1, 1).unwrap(),
            ScreenPoint { u: 0.0, v: 0.0 }
        );
    }

    #[test]
    fn near_far_are_positive_inverse_depth() {
        let camera = CameraContract::derive(&config()).unwrap();
        assert!(camera.q.lo <= 0.25);
        assert!(camera.q.hi >= 4.0);
    }

    #[test]
    fn non_dyadic_near_far_inverse_depth_is_outward_enclosed() {
        let mut config = config();
        config.near = f64::from(0.1_f32);
        config.far = f64::from(3.7_f32);
        let camera = CameraContract::derive(&config).unwrap();
        let near_inverse = 1.0 / config.near;
        let far_inverse = 1.0 / config.far;
        assert!(camera.q.lo < far_inverse);
        assert!(camera.q.hi > near_inverse);
        assert!(camera.q.contains(far_inverse));
        assert!(camera.q.contains(near_inverse));
    }

    #[test]
    fn basis_validation_rejects_zero_degenerate_and_left_handed_inputs() {
        let valid = CameraSample {
            eye: [0.0; 3],
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        };
        validate_basis(valid).unwrap();
        assert!(
            validate_basis(CameraSample {
                up: [0.0; 3],
                ..valid
            })
            .unwrap_err()
            .contains("zero or non-unit")
        );
        assert!(
            validate_basis(CameraSample {
                up: [0.0, -1.0, 0.0],
                ..valid
            })
            .unwrap_err()
            .contains("right-handed")
        );
    }

    #[test]
    fn source_camera_surface_exposes_only_validating_authored_constructors() {
        let source = include_str!("../../../../stdlib/core/render.wr");
        let camera = source
            .split("pub struct Camera:")
            .nth(1)
            .and_then(|tail| tail.split("pub enum LightKind:").next())
            .expect("stdlib Camera declaration");
        for field in ["eye", "forward", "right", "up"] {
            assert!(
                camera.contains(&format!("    {field}: Vec3")),
                "camera field {field} must remain private"
            );
            assert!(!camera.contains(&format!("    pub {field}: Vec3")));
        }
        assert!(camera.contains("pub fn canonical(eye: Vec3) -> Camera:"));
        assert!(camera.contains("pub fn look_at("));
        assert!(camera.contains("pub fn quaternion("));
        assert!(camera.contains("pub fn explicit("));
        assert_eq!(camera.matches("-> Camera:").count(), 1);
        assert_eq!(camera.matches("-> Result[Camera, CameraError]:").count(), 3);
    }

    #[test]
    fn authored_camera_forms_canonicalize_and_reject_degeneracy() {
        let look_at = camera_from_look_at([0.0; 3], [0.0, 0.0, 1.0], [0.0, 1.0, 0.0]).unwrap();
        assert_eq!(look_at.forward, [0.0, 0.0, 1.0]);
        assert_eq!(look_at.right, [1.0, 0.0, 0.0]);
        assert_eq!(look_at.up, [0.0, 1.0, 0.0]);
        assert!(
            camera_from_look_at([0.0; 3], [0.0, 0.0, 1.0], [0.0; 3])
                .unwrap_err()
                .contains("up is zero")
        );

        let quarter_turn = camera_from_quaternion(
            [0.0; 3],
            [
                0.0,
                std::f64::consts::FRAC_1_SQRT_2,
                0.0,
                std::f64::consts::FRAC_1_SQRT_2,
            ],
        )
        .unwrap();
        assert!((quarter_turn.forward[0] - 1.0).abs() <= BASIS_ORTHONORMAL_TOLERANCE_V1);
        assert!(
            camera_from_quaternion([0.0; 3], [0.0; 4])
                .unwrap_err()
                .contains("quaternion")
        );
        let largest_f32 = f64::from(f32::MAX);
        let largest =
            camera_from_quaternion([0.0; 3], [largest_f32; 4]).expect("finite quaternion");
        validate_basis(largest).unwrap();

        let explicit = camera_from_explicit_basis(
            [0.0; 3],
            [0.0, 0.0, 2.0],
            [2.0, 0.25, 0.0],
            [0.0, 1.0, 0.0],
        )
        .unwrap();
        validate_basis(explicit).unwrap();
        assert!(
            camera_from_explicit_basis(
                [0.0; 3],
                [0.0, 0.0, 1.0],
                [1.0, 0.0, 0.0],
                [0.0, -1.0, 0.0],
            )
            .unwrap_err()
            .contains("left-handed")
        );
    }

    #[test]
    fn sealed_camera_sample_must_stay_in_the_declared_projection_box() {
        let contract = CameraContract::derive(&config()).unwrap();
        let center = contract.eye_component.map(|axis| (axis.lo + axis.hi) * 0.5);
        let valid = CameraSample {
            eye: center,
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        };
        validate_sample_against_contract(contract, valid).unwrap();
        let mut turning_config = config();
        turning_config.camera_max_motion = 1.0;
        validate_sample_against_contract(
            CameraContract::derive(&turning_config).unwrap(),
            camera_from_quaternion(
                center,
                [
                    0.0,
                    std::f64::consts::FRAC_1_SQRT_2,
                    0.0,
                    std::f64::consts::FRAC_1_SQRT_2,
                ],
            )
            .unwrap(),
        )
        .unwrap();
        assert!(
            validate_sample_against_contract(
                contract,
                camera_from_quaternion(
                    center,
                    [
                        0.0,
                        std::f64::consts::FRAC_1_SQRT_2,
                        0.0,
                        std::f64::consts::FRAC_1_SQRT_2,
                    ],
                )
                .unwrap(),
            )
            .unwrap_err()
            .contains("outside")
        );
        assert!(
            validate_sample_against_contract(
                contract,
                CameraSample {
                    eye: [100.0, center[1], center[2]],
                    ..valid
                }
            )
            .unwrap_err()
            .contains("outside")
        );
    }

    #[test]
    fn point_uses_unnormalized_ray_and_rejects_zero_q() {
        let contract = CameraContract::derive(&config()).unwrap();
        let eye = contract
            .eye_component
            .map(|interval| interval.lo + (interval.hi - interval.lo) * 0.5);
        let camera = CameraSample {
            eye,
            forward: [0.0, 0.0, 1.0],
            right: [1.0, 0.0, 0.0],
            up: [0.0, 1.0, 0.0],
        };
        assert_eq!(
            contract
                .point(camera, ScreenPoint { u: 2.0, v: -1.0 }, 0.5)
                .unwrap(),
            [eye[0] + 4.0, eye[1] - 2.0, eye[2] + 2.0]
        );
        assert!(
            contract
                .point(camera, ScreenPoint { u: 0.0, v: 0.0 }, 0.0)
                .unwrap_err()
                .starts_with("P016:")
        );
    }
}
