//! Closed P9 diffuse/GGX material and direct-light reference.

use std::f64::consts::PI;

pub const POINT_RADIUS_MIN_V1: f64 = 1.0 / 4096.0;
pub const AREA_AXIS_DOT_MAX_V1: f64 = 1.0e-4;
pub const UNIT_DIRECTION_LENGTH_SQUARED_MIN_V1: f64 = 0.9999;
pub const UNIT_DIRECTION_LENGTH_SQUARED_MAX_V1: f64 = 1.0001;
/// Allowed deterministic quadrature error in the v1 white-furnace host oracle.
pub const WHITE_FURNACE_NUMERIC_RADIUS_V1: f64 = 0.02;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Vec3 {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl Vec3 {
    pub fn dot(self, other: Self) -> f64 {
        self.x * other.x + self.y * other.y + self.z * other.z
    }

    pub fn length(self) -> f64 {
        self.dot(self).sqrt()
    }

    pub fn normalize(self) -> Result<Self, ShadeError> {
        let length = self.length();
        if !length.is_finite() || length <= 0.0 {
            return Err(ShadeError::InvalidInput);
        }
        Ok(Self {
            x: self.x / length,
            y: self.y / length,
            z: self.z / length,
        })
    }

    pub fn subtract(self, other: Self) -> Self {
        Self {
            x: self.x - other.x,
            y: self.y - other.y,
            z: self.z - other.z,
        }
    }

    pub fn add(self, other: Self) -> Self {
        Self {
            x: self.x + other.x,
            y: self.y + other.y,
            z: self.z + other.z,
        }
    }

    pub fn scale(self, factor: f64) -> Self {
        Self {
            x: self.x * factor,
            y: self.y * factor,
            z: self.z * factor,
        }
    }

    pub fn cross(self, other: Self) -> Self {
        Self {
            x: self.y * other.z - self.z * other.y,
            y: self.z * other.x - self.x * other.z,
            z: self.x * other.y - self.y * other.x,
        }
    }
}

pub type Rgb = [f64; 3];

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct StandardMaterial {
    pub base_color: Rgb,
    pub metallic: f64,
    pub roughness: f64,
    pub specular: f64,
    pub emissive: Rgb,
    pub opacity: f64,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Light {
    Directional {
        direction_to_light: Vec3,
        radiance: Rgb,
    },
    Point {
        position: Vec3,
        intensity: Rgb,
        radius: f64,
    },
    Rectangle {
        center: Vec3,
        half_axis_u: Vec3,
        half_axis_v: Vec3,
        radiance: Rgb,
    },
    Disk {
        center: Vec3,
        normal: Vec3,
        radius: f64,
        radiance: Rgb,
    },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadeError {
    InvalidInput,
    UnsupportedLobe,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct LightCoefficientBound {
    pub maximum_incident_radiance: Rgb,
    pub influence_min: Vec3,
    pub influence_max: Vec3,
}

/// Deterministically lower the source-level disk normal and radius to the
/// equal-length half axes used by the sealed runtime record.
pub fn disk_half_axes(normal: Vec3, radius: f64) -> Result<(Vec3, Vec3), ShadeError> {
    if !radius.is_finite() || radius <= 0.0 {
        return Err(ShadeError::InvalidInput);
    }
    let normal = normal.normalize()?;
    let absolute = [normal.x.abs(), normal.y.abs(), normal.z.abs()];
    let helper = if absolute[0] <= absolute[1] && absolute[0] <= absolute[2] {
        Vec3 {
            x: 1.0,
            ..Vec3::default()
        }
    } else if absolute[1] <= absolute[2] {
        Vec3 {
            y: 1.0,
            ..Vec3::default()
        }
    } else {
        Vec3 {
            z: 1.0,
            ..Vec3::default()
        }
    };
    let axis_u = helper.cross(normal).normalize()?;
    let axis_v = normal.cross(axis_u).normalize()?;
    Ok((axis_u.scale(radius), axis_v.scale(radius)))
}

pub fn validate_light(
    light: Light,
    world_min: Vec3,
    world_max: Vec3,
) -> Result<LightCoefficientBound, ShadeError> {
    if ![
        world_min.x,
        world_min.y,
        world_min.z,
        world_max.x,
        world_max.y,
        world_max.z,
    ]
    .into_iter()
    .all(f64::is_finite)
        || world_min.x > world_max.x
        || world_min.y > world_max.y
        || world_min.z > world_max.z
    {
        return Err(ShadeError::InvalidInput);
    }
    let full_world = LightCoefficientBound {
        maximum_incident_radiance: [f64::INFINITY; 3],
        influence_min: world_min,
        influence_max: world_max,
    };
    match light {
        Light::Directional {
            direction_to_light,
            radiance,
        } => {
            direction_to_light.normalize()?;
            if !finite_rgb(radiance) || radiance.into_iter().any(|v| v < 0.0) {
                return Err(ShadeError::InvalidInput);
            }
            Ok(LightCoefficientBound {
                maximum_incident_radiance: radiance,
                ..full_world
            })
        }
        Light::Point {
            position,
            intensity,
            radius,
        } => {
            if ![position.x, position.y, position.z, radius]
                .into_iter()
                .all(f64::is_finite)
                || radius < POINT_RADIUS_MIN_V1
                || !finite_rgb(intensity)
                || intensity.into_iter().any(|v| v < 0.0)
            {
                return Err(ShadeError::InvalidInput);
            }
            Ok(LightCoefficientBound {
                maximum_incident_radiance: intensity.map(|v| v / (radius * radius)),
                ..full_world
            })
        }
        Light::Rectangle {
            center,
            half_axis_u,
            half_axis_v,
            radiance,
        } => {
            let u2 = half_axis_u.dot(half_axis_u);
            let v2 = half_axis_v.dot(half_axis_v);
            if ![center.x, center.y, center.z, u2, v2]
                .into_iter()
                .all(f64::is_finite)
                || u2 <= 0.0
                || v2 <= 0.0
                || half_axis_u.normalize()?.dot(half_axis_v.normalize()?).abs()
                    > AREA_AXIS_DOT_MAX_V1
                || !finite_rgb(radiance)
                || radiance.into_iter().any(|v| v < 0.0)
            {
                return Err(ShadeError::InvalidInput);
            }
            Ok(LightCoefficientBound {
                maximum_incident_radiance: radiance,
                // V1 has no finite light cutoff. The emitter bounds are a
                // source-integration domain, not a safe receiver-influence
                // bound, so every non-disabled area light retains the full
                // sealed world just like point and directional lights.
                ..full_world
            })
        }
        Light::Disk {
            center,
            normal,
            radius,
            radiance,
        } => {
            if ![center.x, center.y, center.z]
                .into_iter()
                .all(f64::is_finite)
                || !finite_rgb(radiance)
                || radiance.into_iter().any(|v| v < 0.0)
            {
                return Err(ShadeError::InvalidInput);
            }
            disk_half_axes(normal, radius)?;
            Ok(LightCoefficientBound {
                maximum_incident_radiance: radiance,
                // V1 has no finite light cutoff. The emitter bounds are a
                // source-integration domain, not a safe receiver-influence
                // bound, so every non-disabled area light retains the full
                // sealed world just like point and directional lights.
                ..full_world
            })
        }
    }
}

fn finite_rgb(value: Rgb) -> bool {
    value.into_iter().all(f64::is_finite)
}

impl StandardMaterial {
    pub fn validate(self) -> Result<Self, ShadeError> {
        if !finite_rgb(self.base_color)
            || !finite_rgb(self.emissive)
            || self
                .base_color
                .into_iter()
                .any(|value| !(0.0..=1.0).contains(&value))
            || self.emissive.into_iter().any(|value| value < 0.0)
            || !(0.0..=1.0).contains(&self.metallic)
            || !(0.02..=1.0).contains(&self.roughness)
            || !(0.0..=1.0).contains(&self.specular)
            || !(0.0..=1.0).contains(&self.opacity)
        {
            return Err(ShadeError::InvalidInput);
        }
        Ok(self)
    }
}

fn schlick(f0: f64, view_half: f64) -> f64 {
    f0 + (1.0 - f0) * (1.0 - view_half.clamp(0.0, 1.0)).powi(5)
}

/// Isotropic GGX with height-correlated Smith visibility and Burley diffuse.
pub fn brdf(
    material: StandardMaterial,
    normal: Vec3,
    view: Vec3,
    light: Vec3,
) -> Result<Rgb, ShadeError> {
    let material = material.validate()?;
    let (normal, view, light) = (normal.normalize()?, view.normalize()?, light.normalize()?);
    let no_v = normal.dot(view).clamp(0.0, 1.0);
    let no_l = normal.dot(light).clamp(0.0, 1.0);
    if no_v == 0.0 || no_l == 0.0 {
        return Ok([0.0; 3]);
    }
    let half = view.add(light).normalize()?;
    let no_h = normal.dot(half).clamp(0.0, 1.0);
    let vo_h = view.dot(half).clamp(0.0, 1.0);
    let alpha = (material.roughness * material.roughness).max(0.0004);
    let alpha2 = alpha * alpha;
    let denominator = no_h * no_h * (alpha2 - 1.0) + 1.0;
    let distribution = alpha2 / (PI * denominator.mul_add(denominator, 1.0e-12));
    let lambda = |no_x: f64| {
        let no_x2 = no_x * no_x;
        ((alpha2 * (1.0 - no_x2) / no_x2.max(1.0e-12) + 1.0).sqrt() - 1.0) * 0.5
    };
    let visibility = 1.0 / (1.0 + lambda(no_v) + lambda(no_l)).max(1.0e-12);
    // Burley's grazing retroreflection term, with energy removed by Fresnel
    // and metallic response below on a per-channel basis.
    let fd90 = 0.5 + 2.0 * material.roughness * vo_h * vo_h;
    let diffuse_shape =
        (1.0 + (fd90 - 1.0) * (1.0 - no_l).powi(5)) * (1.0 + (fd90 - 1.0) * (1.0 - no_v).powi(5));
    let mut result = [0.0; 3];
    for channel in 0..3 {
        let f0 = (0.08 * material.specular) * (1.0 - material.metallic)
            + material.base_color[channel] * material.metallic;
        let fresnel = schlick(f0, vo_h);
        let diffuse = material.base_color[channel]
            * (1.0 - material.metallic)
            * (1.0 - fresnel)
            * diffuse_shape
            / PI;
        let specular = distribution * visibility * fresnel / (4.0 * no_v * no_l).max(1.0e-12);
        result[channel] = (diffuse + specular) * no_l;
    }
    Ok(result)
}

pub fn direct(
    material: StandardMaterial,
    position: Vec3,
    normal: Vec3,
    view: Vec3,
    light: Light,
    visibility: f64,
) -> Result<Rgb, ShadeError> {
    if !visibility.is_finite() || !(0.0..=1.0).contains(&visibility) {
        return Err(ShadeError::InvalidInput);
    }
    let (direction, radiance) = match light {
        Light::Directional {
            direction_to_light,
            radiance,
        } => (direction_to_light.normalize()?, radiance),
        Light::Point {
            position: source,
            intensity,
            radius,
        } => {
            if !radius.is_finite() || radius < POINT_RADIUS_MIN_V1 {
                return Err(ShadeError::InvalidInput);
            }
            let displacement = source.subtract(position);
            let distance_squared = displacement.dot(displacement).max(radius * radius);
            (
                displacement.normalize()?,
                intensity.map(|value| value / distance_squared),
            )
        }
        Light::Rectangle { .. } | Light::Disk { .. } => {
            // Area emitters require full-cell visibility/integration through
            // `area_light::integrate`; a center sample is never substituted.
            return Err(ShadeError::UnsupportedLobe);
        }
    };
    if !finite_rgb(radiance) || radiance.into_iter().any(|value| value < 0.0) {
        return Err(ShadeError::InvalidInput);
    }
    let response = brdf(material, normal, view, direction)?;
    Ok(std::array::from_fn(|channel| {
        response[channel] * radiance[channel] * visibility
    }))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn material() -> StandardMaterial {
        StandardMaterial {
            base_color: [0.8, 0.4, 0.2],
            metallic: 0.0,
            roughness: 0.5,
            specular: 0.5,
            emissive: [0.0; 3],
            opacity: 1.0,
        }
    }

    #[test]
    fn material_contract_rejects_every_out_of_range_field() {
        assert!(material().validate().is_ok());
        assert_eq!(
            StandardMaterial {
                roughness: 0.019,
                ..material()
            }
            .validate(),
            Err(ShadeError::InvalidInput)
        );
        assert_eq!(
            StandardMaterial {
                metallic: f64::NAN,
                ..material()
            }
            .validate(),
            Err(ShadeError::InvalidInput)
        );
        assert_eq!(
            StandardMaterial {
                emissive: [-1.0, 0.0, 0.0],
                ..material()
            }
            .validate(),
            Err(ShadeError::InvalidInput)
        );
    }

    #[test]
    fn point_radius_removes_the_source_singularity() {
        let value = direct(
            material(),
            Vec3::default(),
            Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            Light::Point {
                position: Vec3::default(),
                intensity: [1.0; 3],
                radius: 0.25,
            },
            1.0,
        );
        assert_eq!(
            value,
            Err(ShadeError::InvalidInput),
            "direction at the exact source is unresolved, never guessed"
        );
    }

    #[test]
    fn point_radius_contract_matches_the_sealed_nonsingular_boundary() {
        let radius = POINT_RADIUS_MIN_V1;
        let bound = validate_light(
            Light::Point {
                position: Vec3::default(),
                intensity: [1.0; 3],
                radius,
            },
            Vec3 {
                x: -1.0,
                y: -1.0,
                z: -1.0,
            },
            Vec3 {
                x: 1.0,
                y: 1.0,
                z: 1.0,
            },
        )
        .unwrap();
        assert_eq!(
            bound.maximum_incident_radiance,
            [1.0 / (radius * radius); 3]
        );
        assert_eq!(
            validate_light(
                Light::Point {
                    position: Vec3::default(),
                    intensity: [1.0; 3],
                    radius: radius * 0.5,
                },
                Vec3 {
                    x: -1.0,
                    y: -1.0,
                    z: -1.0,
                },
                Vec3 {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
            ),
            Err(ShadeError::InvalidInput)
        );
    }

    #[test]
    fn permanent_normal_incidence_vector_matches_the_wrela_selftest() {
        let value = brdf(
            StandardMaterial {
                base_color: [0.5; 3],
                ..material()
            },
            Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
        )
        .unwrap();
        for channel in value {
            assert!((channel - 0.203_718_327_144_588_04).abs() < 1.0e-12);
        }
    }

    #[test]
    fn permanent_grazing_vector_matches_the_wrela_denominator_floor() {
        let value = brdf(
            StandardMaterial {
                base_color: [0.5; 3],
                ..material()
            },
            Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
            Vec3 {
                x: 1.0,
                z: 1.0e-8,
                ..Vec3::default()
            },
            Vec3 {
                z: 1.0,
                ..Vec3::default()
            },
        )
        .unwrap();
        for channel in value {
            assert!((channel - 0.745_556_143_999_673_7).abs() < 1.0e-9);
        }
    }

    #[test]
    fn white_furnace_grid_is_finite_and_bounded() {
        let normal = Vec3 {
            z: 1.0,
            ..Vec3::default()
        };
        for metallic in [0.0, 0.5, 1.0] {
            for roughness in [0.02, 0.1, 0.5, 1.0] {
                let material = StandardMaterial {
                    base_color: [1.0; 3],
                    metallic,
                    roughness,
                    specular: 1.0,
                    emissive: [0.0; 3],
                    opacity: 1.0,
                };
                let mut integral = [0.0; 3];
                let steps = 512;
                for i in 0..steps {
                    // Integrate the isotropic hemisphere as 2*pi*d(cosine).
                    // cosine=1-t^4 concentrates deterministic midpoint nodes
                    // around the narrow minimum-roughness GGX peak.
                    let t = (i as f64 + 0.5) / steps as f64;
                    let cosine = 1.0 - t.powi(4);
                    let jacobian = 4.0 * t.powi(3);
                    let sine = (1.0 - cosine * cosine).sqrt();
                    let value = brdf(
                        material,
                        normal,
                        normal,
                        Vec3 {
                            x: sine,
                            y: 0.0,
                            z: cosine,
                        },
                    )
                    .unwrap();
                    for channel in 0..3 {
                        integral[channel] += value[channel] * 2.0 * PI * jacobian / steps as f64;
                    }
                }
                assert!(integral.into_iter().all(
                    |value| value.is_finite() && value <= 1.0 + WHITE_FURNACE_NUMERIC_RADIUS_V1
                ));
            }
        }
    }

    #[test]
    fn every_v1_light_kind_has_a_finite_compiler_contract() {
        let world_min = Vec3 {
            x: -10.0,
            y: -10.0,
            z: -10.0,
        };
        let world_max = Vec3 {
            x: 10.0,
            y: 10.0,
            z: 10.0,
        };
        let axis_u = Vec3 {
            x: 1.0,
            y: 0.0,
            z: 0.0,
        };
        let axis_v = Vec3 {
            x: 0.0,
            y: 1.0,
            z: 0.0,
        };
        for light in [
            Light::Directional {
                direction_to_light: Vec3 {
                    x: 0.0,
                    y: 0.0,
                    z: 1.0,
                },
                radiance: [2.0; 3],
            },
            Light::Point {
                position: Vec3::default(),
                intensity: [2.0; 3],
                radius: 0.25,
            },
            Light::Rectangle {
                center: Vec3::default(),
                half_axis_u: axis_u,
                half_axis_v: axis_v,
                radiance: [2.0; 3],
            },
            Light::Disk {
                center: Vec3::default(),
                normal: Vec3 {
                    z: 2.0,
                    ..Vec3::default()
                },
                radius: 1.0,
                radiance: [2.0; 3],
            },
        ] {
            let bound = validate_light(light, world_min, world_max).unwrap();
            assert!(
                bound
                    .maximum_incident_radiance
                    .into_iter()
                    .all(|v| v.is_finite())
            );
            assert_eq!(bound.influence_min, world_min);
            assert_eq!(bound.influence_max, world_max);
        }
    }

    #[test]
    fn area_axes_reject_parallel_vectors_at_disparate_scales() {
        let world_min = Vec3 {
            x: -2.0,
            y: -2.0,
            z: -2.0,
        };
        let world_max = Vec3 {
            x: 2.0,
            y: 2.0,
            z: 2.0,
        };
        let parallel = Light::Rectangle {
            center: Vec3::default(),
            half_axis_u: Vec3 {
                x: 1.0,
                ..Vec3::default()
            },
            half_axis_v: Vec3 {
                x: 1.0e-6,
                ..Vec3::default()
            },
            radiance: [1.0; 3],
        };
        assert_eq!(
            validate_light(parallel, world_min, world_max),
            Err(ShadeError::InvalidInput),
        );
    }

    #[test]
    fn disk_source_contract_lowers_normal_and_radius_to_equal_axes() {
        let (axis_u, axis_v) = disk_half_axes(
            Vec3 {
                z: 2.0,
                ..Vec3::default()
            },
            3.0,
        )
        .unwrap();
        assert_eq!(
            axis_u,
            Vec3 {
                y: -3.0,
                ..Vec3::default()
            }
        );
        assert_eq!(
            axis_v,
            Vec3 {
                x: 3.0,
                ..Vec3::default()
            }
        );
        assert_eq!(
            disk_half_axes(Vec3::default(), 1.0),
            Err(ShadeError::InvalidInput)
        );
        assert_eq!(
            disk_half_axes(
                Vec3 {
                    z: 1.0,
                    ..Vec3::default()
                },
                0.0,
            ),
            Err(ShadeError::InvalidInput)
        );
    }
}
