//! Validation and typed extraction of sealed `Image.renderer` declarations.

use std::collections::{BTreeMap, BTreeSet};

use crate::eval::image::{DeclArg, DriverDecl, ImageDeclRef, ImageGraph, RendererDecl};
use crate::eval::value::Value;
use crate::sema::typed::TypedProgram;
#[cfg(test)]
use crate::sema::typed::TypedStruct;
use crate::sema::types::{self, Type, TypeArg};
use crate::syntax::ast::Span;

use super::params::ParameterContract;

const LIGHT_KIND_NAMES: &[&str] = &["Disabled", "Point", "Directional", "Rectangle", "Disk"];
pub(crate) const P7_MAX_RENDER_WORKERS: usize = 4;

fn p7_worker_count(cores: usize) -> Result<u32, &'static str> {
    if cores == 0 || cores > P7_MAX_RENDER_WORKERS {
        return Err(
            "renderer capacity `worker_count` must be in 1..=4 for the P7 coordinator group",
        );
    }
    u32::try_from(cores).map_err(|_| "renderer capacity `worker_count` exceeds the u32 encoding")
}

pub(crate) fn light_kind_tag(kind: &str) -> Option<u64> {
    LIGHT_KIND_NAMES
        .iter()
        .position(|candidate| *candidate == kind)
        .and_then(|tag| u64::try_from(tag).ok())
}

#[derive(Debug, Clone, PartialEq)]
struct ConfigFailure {
    code: &'static str,
    message: String,
    primary: Option<Span>,
}

impl ConfigFailure {
    fn at(mut self, span: Span) -> Self {
        self.primary = Some(span);
        self
    }

    fn from_prefixed(message: String) -> Self {
        let diagnostic =
            super::diagnostics::PixelsDiagnostic::from_prefixed(message, Span::default(), "P008");
        let mut message = diagnostic.message;
        for note in diagnostic.notes {
            message.push_str("\n  ");
            message.push_str(&note);
        }
        Self {
            code: diagnostic.code,
            message,
            primary: None,
        }
    }

    #[cfg(test)]
    fn contains(&self, pattern: &str) -> bool {
        self.message.contains(pattern)
    }

    #[cfg(test)]
    fn starts_with(&self, pattern: &str) -> bool {
        format!("{}: {}", self.code, self.message).starts_with(pattern)
    }
}

fn coded(code: &'static str, message: impl AsRef<str>) -> ConfigFailure {
    ConfigFailure {
        code,
        message: message.as_ref().to_string(),
        primary: None,
    }
}

type ConfigResult<T> = Result<T, ConfigFailure>;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Vec3Config {
    pub x: f32,
    pub y: f32,
    pub z: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ScalarRangeConfig {
    pub min: f32,
    pub max: f32,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RgbRangeConfig {
    pub min: [f32; 3],
    pub max: [f32; 3],
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LightRangeConfig {
    pub position_min: Vec3Config,
    pub position_max: Vec3Config,
    pub axis_component_max: f32,
    pub radiance_max: [f32; 3],
    pub max_delta: f32,
}

pub fn default_light_ranges() -> Vec<LightRangeConfig> {
    vec![
        LightRangeConfig {
            position_min: Vec3Config {
                x: -65504.0,
                y: -65504.0,
                z: -65504.0,
            },
            position_max: Vec3Config {
                x: 65504.0,
                y: 65504.0,
                z: 65504.0,
            },
            axis_component_max: 65504.0,
            radiance_max: [65504.0; 3],
            max_delta: 65504.0,
        };
        8
    ]
}

#[derive(Debug, Clone, PartialEq)]
pub struct RendererConfig {
    pub declaration_index: usize,
    pub worker_count: u32,
    pub params_type: Type,
    pub field: String,
    pub material: String,
    pub material_type: Type,
    pub display_index: usize,
    /// Sealed machine-v1 MMIO address of the bound display device doorbell.
    pub display_doorbell_addr: u64,
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
    pub shade_hz: u32,
    pub profile: String,
    pub tone_curve: String,
    pub near: f64,
    pub far: f64,
    pub world_min: Vec3Config,
    pub world_max: Vec3Config,
    pub camera_max_motion: f32,
    /// A sealed absolute camera pose, in the order
    /// `eye, forward, right, up` (twelve f32). Present only when the
    /// declaration pins one with `camera_pose=`, and enforced by frame
    /// validation, which rejects any frame whose camera differs. That is what
    /// turns the analytic coverage tiers' pose precondition from a per-frame
    /// runtime test into a compile-time fact.
    pub camera_pose: Option<[f32; 12]>,
    pub light_capacity: u32,
    pub light_kinds: Vec<String>,
    pub light_ranges: Vec<LightRangeConfig>,
    pub exposure: ScalarRangeConfig,
    pub environment: RgbRangeConfig,
    pub ao_enabled: bool,
    pub ao_radius: f32,
    pub ao_strength: f32,
    pub probes_enabled: bool,
    pub probe_initialization_worst_case_ms: u32,
    pub initialization_deadline_ms: u32,
    pub parameter_contracts: Vec<ParameterContract>,
}

#[derive(Debug, Clone, PartialEq, Default)]
pub struct RendererConfigs {
    pub renderers: Vec<RendererConfig>,
}

fn validate_labels(renderer: &RendererDecl) -> ConfigResult<()> {
    let mut found = BTreeSet::new();
    for argument in &renderer.args {
        if !crate::pixels::RENDERER_LABELS.contains(&argument.label.as_str())
            && !crate::pixels::OPTIONAL_RENDERER_LABELS.contains(&argument.label.as_str())
        {
            return Err(coded(
                "P008",
                format!(
                    "renderer declaration has unknown or duplicate argument `{}`: \
                     the label is not part of the sealed renderer declaration",
                    argument.label
                ),
            )
            .at(argument.span));
        }
        if !found.insert(argument.label.as_str()) {
            return Err(coded(
                "P008",
                format!(
                    "renderer declaration has unknown or duplicate argument `{}`: \
                     the label is bound more than once",
                    argument.label
                ),
            )
            .at(argument.span));
        }
    }
    for required in crate::pixels::RENDERER_LABELS {
        if !found.contains(required) {
            return Err(coded(
                "P008",
                format!(
                    "renderer declaration has unknown or duplicate argument `{required}`: \
                     the required argument is missing"
                ),
            )
            .at(renderer.span));
        }
    }
    Ok(())
}

fn arg<'a>(renderer: &'a RendererDecl, label: &str) -> ConfigResult<&'a DeclArg> {
    renderer
        .args
        .iter()
        .find(|arg| arg.label == label)
        .ok_or_else(|| {
            coded(
                "P008",
                format!(
                    "renderer declaration has unknown or duplicate argument `{label}`: \
                     the required argument is missing"
                ),
            )
            .at(renderer.span)
        })
}

const CANONICAL_CONFIG_TYPES: &[(&str, &str, &str, &str)] = &[
    ("profile", "RenderProfile", "core.render", "P022"),
    ("tone_curve", "ToneCurve", "core.render", "P022"),
    ("world_min", "Vec3", "core.field", "P007"),
    ("world_max", "Vec3", "core.field", "P007"),
    ("camera_bounds", "CameraBounds", "core.render", "P007"),
    ("light_config", "LightConfig", "core.render", "P007"),
    ("exposure_range", "ScalarRange", "core.render", "P007"),
    ("environment_range", "RgbRange", "core.render", "P007"),
    ("ao", "AoConfig", "core.render", "P007"),
    ("probes", "ProbeConfig", "core.render", "P007"),
];

fn validate_canonical_config_types(
    owner: &TypedProgram,
    renderer: &RendererDecl,
) -> ConfigResult<()> {
    for &(label, expected_type, expected_module, code) in CANONICAL_CONFIG_TYPES {
        let argument = arg(renderer, label)?;
        let Type::Named(name, args) = &argument.ty else {
            return Err(coded(
                code,
                format!(
                    "renderer bound `{label}` must have canonical type \
                     `{expected_module}::{expected_type}`"
                ),
            )
            .at(argument.span));
        };
        let declaration = super::nominal_decl(owner, name);
        let canonical_module = declaration.is_some_and(|(declaring_module, _)| {
            declaring_module == expected_module
                || expected_module
                    .strip_prefix("core.")
                    .is_some_and(|short| declaring_module == short)
        });
        let canonical_name =
            declaration.is_some_and(|(_, declaring_name)| declaring_name == expected_type);
        if !canonical_name || !args.is_empty() || !canonical_module {
            return Err(coded(
                code,
                format!(
                    "renderer bound `{label}` must have canonical type \
                     `{expected_module}::{expected_type}`, found `{}` declared by `{}`",
                    types::render_type(&argument.ty),
                    declaration
                        .map(|(module, name)| format!("{module}::{name}"))
                        .unwrap_or_else(|| "<unknown>".to_string())
                ),
            )
            .at(argument.span));
        }
    }
    Ok(())
}

fn function(renderer: &RendererDecl, label: &str) -> ConfigResult<String> {
    let argument = arg(renderer, label)?;
    match &argument.value {
        Value::Fn(key) => Ok(key.spelling()),
        _ => Err(coded(
            "P009",
            format!("renderer `{label}=` must be a checked function reference"),
        )
        .at(argument.span)),
    }
}

fn integer(renderer: &RendererDecl, label: &str) -> ConfigResult<u32> {
    let argument = arg(renderer, label)?;
    let value = crate::eval::value::as_i128(&argument.value).ok_or_else(|| {
        coded("P010", format!("renderer `{label}=` must be an integer")).at(argument.span)
    })?;
    u32::try_from(value).map_err(|_| {
        coded(
            "P010",
            format!("renderer `{label}={value}` is out of range"),
        )
        .at(argument.span)
    })
}

fn oversized_mode_dimension(width: u32, height: u32) -> Option<(&'static str, u32, u32)> {
    [
        ("width", width, wrela_machine::pixels::MAX_MODE_WIDTH),
        ("height", height, wrela_machine::pixels::MAX_MODE_HEIGHT),
    ]
    .into_iter()
    .find(|(_, value, maximum)| value > maximum)
}

fn display_integer(
    programs: &BTreeMap<String, TypedProgram>,
    display: &DriverDecl,
    label: &str,
    fallback_span: Span,
) -> ConfigResult<u32> {
    let argument = display.args.iter().find(|argument| argument.label == label);
    let (value, span) = if let Some(argument) = argument {
        let value = crate::eval::value::as_i128(&argument.value).ok_or_else(|| {
            coded(
                "P010",
                format!(
                    "renderer display mode disagrees with the bound display driver: display \
                     `{label}=` must be an integer"
                ),
            )
            .at(argument.span)
        })?;
        (value, argument.span)
    } else {
        let value = super::program_for_decl_module(programs, "drivers.display")
            .and_then(|program| program.structs.get("Display"))
            .and_then(|display| display.init.as_ref())
            .and_then(|init| init.params.iter().find(|param| param.name == label))
            .and_then(|param| param.default.as_ref())
            .and_then(|default| {
                super::legality::constant_integer(programs, "drivers.display", default)
            })
            .ok_or_else(|| {
                coded(
                    "P010",
                    format!(
                        "renderer display mode disagrees with the bound display driver: display \
                         declaration has no `{label}=` component or comptime default"
                    ),
                )
                .at(fallback_span)
            })?;
        (value, fallback_span)
    };
    u32::try_from(value).map_err(|_| {
        coded(
            "P010",
            format!(
                "renderer display mode disagrees with the bound display driver: display \
                 `{label}={value}` is out of range"
            ),
        )
        .at(span)
    })
}

fn float_value(value: &Value, label: &str) -> ConfigResult<f64> {
    let value = match value {
        Value::F32(value) => f64::from(*value),
        Value::F64(value) => *value,
        _ => {
            return Err(coded(
                "P007",
                format!("renderer bound `{label}` must be a floating-point scalar"),
            ));
        }
    };
    if !value.is_finite() {
        return Err(coded(
            "P007",
            format!("renderer bound `{label}` must be finite"),
        ));
    }
    Ok(value)
}

fn float(renderer: &RendererDecl, label: &str) -> ConfigResult<f64> {
    let argument = arg(renderer, label)?;
    float_value(&argument.value, label).map_err(|error| error.at(argument.span))
}

fn f32_value(value: &Value, label: &str) -> ConfigResult<f32> {
    let Value::F32(value) = value else {
        return Err(coded(
            "P007",
            format!("renderer bound `{label}` must use canonical `f32` storage"),
        ));
    };
    if !value.is_finite() {
        return Err(coded(
            "P007",
            format!("renderer bound `{label}` must be finite"),
        ));
    }
    Ok(*value)
}

fn structure<'a>(
    renderer: &'a RendererDecl,
    label: &str,
    expected_type: &str,
    expected_fields: usize,
) -> ConfigResult<&'a [Value]> {
    let argument = arg(renderer, label)?;
    // `extract` validates the declaration identity of every structured
    // renderer argument before decoding its comptime storage. Do not repeat
    // that check by spelling here: imported aliases are deliberately valid.
    let Value::Struct(fields) = &argument.value else {
        return Err(coded(
            "P007",
            format!("renderer bound `{label}` must be a comptime `{expected_type}` value"),
        )
        .at(argument.span));
    };
    if fields.len() != expected_fields {
        return Err(coded(
            "P007",
            format!(
                "renderer bound `{label}` `{expected_type}` layout has {} fields, expected {expected_fields}",
                fields.len()
            ),
        )
        .at(argument.span));
    }
    Ok(fields)
}

fn vec3(renderer: &RendererDecl, label: &str) -> ConfigResult<Vec3Config> {
    let span = arg(renderer, label)?.span;
    let fields = structure(renderer, label, "Vec3", 3)?;
    Ok(Vec3Config {
        x: f32_value(&fields[0], label).map_err(|error| error.at(span))?,
        y: f32_value(&fields[1], label).map_err(|error| error.at(span))?,
        z: f32_value(&fields[2], label).map_err(|error| error.at(span))?,
    })
}

/// Parse an optional pinned camera pose. `Camera`'s authored constructors are
/// closed and always produce the same canonical `eye/forward/right/up` shape,
/// so a comptime `Camera` value is exactly twelve floats in that order.
fn camera_pose(renderer: &RendererDecl) -> ConfigResult<Option<[f32; 12]>> {
    let Some(argument) = renderer.args.iter().find(|arg| arg.label == "camera_pose") else {
        return Ok(None);
    };
    let span = argument.span;
    let fields = structure(renderer, "camera_pose", "Camera", 4)?;
    let mut pose = [0.0_f32; 12];
    for (index, field) in fields.iter().enumerate() {
        let Value::Struct(components) = field else {
            return Err(coded(
                "P007",
                "renderer bound `camera_pose` `Camera` members must be `Vec3` values".to_string(),
            )
            .at(span));
        };
        if components.len() != 3 {
            return Err(coded(
                "P007",
                "renderer bound `camera_pose` `Camera` members must be `Vec3` values".to_string(),
            )
            .at(span));
        }
        for (axis, component) in components.iter().enumerate() {
            pose[index * 3 + axis] =
                f32_value(component, "camera_pose").map_err(|error| error.at(span))?;
        }
    }
    if pose.iter().any(|value| !value.is_finite()) {
        return Err(coded(
            "P007",
            "renderer bound `camera_pose` must be finite".to_string(),
        )
        .at(span));
    }
    Ok(Some(pose))
}

fn rgb(value: &Value, label: &str, span: Span) -> ConfigResult<[f32; 3]> {
    let Value::Struct(fields) = value else {
        return Err(coded(
            "P007",
            format!("renderer bound `{label}` must contain comptime `Rgb` values"),
        )
        .at(span));
    };
    if fields.len() != 3 {
        return Err(coded(
            "P007",
            format!("renderer bound `{label}` contains malformed `Rgb` storage"),
        )
        .at(span));
    }
    Ok([
        f32_value(&fields[0], label).map_err(|error| error.at(span))?,
        f32_value(&fields[1], label).map_err(|error| error.at(span))?,
        f32_value(&fields[2], label).map_err(|error| error.at(span))?,
    ])
}

fn scalar_range(renderer: &RendererDecl, label: &str) -> ConfigResult<ScalarRangeConfig> {
    let span = arg(renderer, label)?.span;
    let fields = structure(renderer, label, "ScalarRange", 2)?;
    let range = ScalarRangeConfig {
        min: f32_value(&fields[0], label).map_err(|error| error.at(span))?,
        max: f32_value(&fields[1], label).map_err(|error| error.at(span))?,
    };
    if range.min > range.max {
        return Err(coded(
            "P007",
            format!(
                "renderer range `{label}` has min={} greater than max={}",
                range.min, range.max
            ),
        )
        .at(span));
    }
    Ok(range)
}

fn rgb_range(renderer: &RendererDecl, label: &str) -> ConfigResult<RgbRangeConfig> {
    let span = arg(renderer, label)?.span;
    let fields = structure(renderer, label, "RgbRange", 2)?;
    let range = RgbRangeConfig {
        min: rgb(&fields[0], label, span)?,
        max: rgb(&fields[1], label, span)?,
    };
    if range.min.iter().zip(range.max).any(|(min, max)| *min > max) {
        return Err(coded(
            "P007",
            format!("renderer range `{label}` has a component minimum greater than its maximum"),
        )
        .at(span));
    }
    Ok(range)
}

fn one_f32_struct(renderer: &RendererDecl, label: &str, ty: &str) -> ConfigResult<f32> {
    let span = arg(renderer, label)?.span;
    let fields = structure(renderer, label, ty, 1)?;
    f32_value(&fields[0], label).map_err(|error| error.at(span))
}

fn light_config(
    renderer: &RendererDecl,
) -> ConfigResult<(u32, Vec<String>, Vec<LightRangeConfig>)> {
    const LIGHT_CAPACITY: usize = 8;

    let span = arg(renderer, "light_config")?.span;
    let fields = structure(renderer, "light_config", "LightConfig", 3)?;
    let value = crate::eval::value::as_i128(&fields[0]).ok_or_else(|| {
        coded(
            "P007",
            "renderer bound `light_config` `LightConfig` capacity must be an integer",
        )
        .at(span)
    })?;
    let capacity = u32::try_from(value).map_err(|_| {
        coded(
            "P007",
            "renderer bound `light_config` capacity is out of range",
        )
        .at(span)
    })?;
    if usize::try_from(capacity).unwrap_or(usize::MAX) > LIGHT_CAPACITY {
        return Err(coded(
            "P015",
            format!(
                "renderer capacity `lights` exceeds the machine-v1 sealed ceiling: \
                 requested {capacity}, ceiling {LIGHT_CAPACITY}"
            ),
        )
        .at(span));
    }
    let Value::Array(kinds) = &fields[1] else {
        return Err(coded(
            "P007",
            "renderer bound `light_config` must contain a fixed light-kind array",
        )
        .at(span));
    };
    if kinds.len() != LIGHT_CAPACITY {
        return Err(coded(
            "P007",
            format!(
                "renderer bound `light_config` has {} topology slots, expected {LIGHT_CAPACITY}",
                kinds.len()
            ),
        )
        .at(span));
    }
    let kinds = kinds
        .iter()
        .take(capacity as usize)
        .map(|kind| {
            let Value::Enum(index, payload) = kind else {
                return Err(coded(
                    "P007",
                    "renderer bound `light_config` topology must contain `LightKind` values",
                )
                .at(span));
            };
            if !payload.is_empty() {
                return Err(coded(
                    "P007",
                    "renderer bound `light_config` light kinds may not carry payload",
                )
                .at(span));
            }
            LIGHT_KIND_NAMES
                .get(*index)
                .map(|kind| (*kind).to_string())
                .ok_or_else(|| {
                    coded(
                        "P007",
                        format!(
                            "renderer bound `light_config` has unknown `LightKind` variant index \
                             {index}"
                        ),
                    )
                    .at(span)
                })
        })
        .collect::<ConfigResult<Vec<_>>>()?;
    let Value::Array(ranges) = &fields[2] else {
        return Err(coded(
            "P007",
            "renderer bound `light_config` must contain a fixed light-range array",
        )
        .at(span));
    };
    if ranges.len() != LIGHT_CAPACITY {
        return Err(coded(
            "P007",
            "renderer bound `light_config` light-range array must contain exactly eight entries",
        )
        .at(span));
    }
    let parse_vec3 = |value: &Value, slot: usize, field: &str| -> ConfigResult<Vec3Config> {
        let Value::Struct(components) = value else {
            return Err(coded(
                "P007",
                format!("renderer light range slot {slot} `{field}` must be `Vec3`"),
            )
            .at(span));
        };
        if components.len() != 3 {
            return Err(coded(
                "P007",
                format!("renderer light range slot {slot} `{field}` is malformed"),
            )
            .at(span));
        }
        Ok(Vec3Config {
            x: f32_value(&components[0], "light_config").map_err(|error| error.at(span))?,
            y: f32_value(&components[1], "light_config").map_err(|error| error.at(span))?,
            z: f32_value(&components[2], "light_config").map_err(|error| error.at(span))?,
        })
    };
    let mut decoded_ranges = Vec::with_capacity(LIGHT_CAPACITY);
    for (slot, value) in ranges.iter().enumerate() {
        let Value::Struct(range_fields) = value else {
            return Err(coded(
                "P007",
                format!("renderer light range slot {slot} must be `LightRange`"),
            )
            .at(span));
        };
        if range_fields.len() != 5 {
            return Err(coded(
                "P007",
                format!("renderer light range slot {slot} has the wrong field count"),
            )
            .at(span));
        }
        let position_min = parse_vec3(&range_fields[0], slot, "position_min")?;
        let position_max = parse_vec3(&range_fields[1], slot, "position_max")?;
        let axis_component_max =
            f32_value(&range_fields[2], "light_config").map_err(|error| error.at(span))?;
        let radiance_max = rgb(&range_fields[3], "light_config", span)?;
        let max_delta =
            f32_value(&range_fields[4], "light_config").map_err(|error| error.at(span))?;
        if position_min.x > position_max.x
            || position_min.y > position_max.y
            || position_min.z > position_max.z
            || axis_component_max <= 0.0
            || radiance_max.into_iter().any(|component| component < 0.0)
            || max_delta < 0.0
        {
            return Err(coded(
                "P007",
                format!("renderer light range slot {slot} is not finite and ordered"),
            )
            .at(span));
        }
        decoded_ranges.push(LightRangeConfig {
            position_min,
            position_max,
            axis_component_max,
            radiance_max,
            max_delta,
        });
    }
    Ok((capacity, kinds, decoded_ranges))
}

fn ao_config(renderer: &RendererDecl) -> ConfigResult<(bool, f32, f32)> {
    let span = arg(renderer, "ao")?.span;
    let fields = structure(renderer, "ao", "AoConfig", 3)?;
    let enabled = match fields[0] {
        Value::Bool(value) => value,
        _ => {
            return Err(
                coded("P007", "renderer bound `ao` enabled value must be boolean").at(span),
            );
        }
    };
    let radius = f32_value(&fields[1], "ao.radius").map_err(|error| error.at(span))?;
    let strength = f32_value(&fields[2], "ao.strength").map_err(|error| error.at(span))?;
    if radius <= 0.0 || !(0.0..=1.0).contains(&strength) {
        return Err(coded(
            "P007",
            "renderer bound `ao` requires radius > 0 and strength in [0,1]",
        )
        .at(span));
    }
    Ok((enabled, radius, strength))
}

fn probe_config(renderer: &RendererDecl) -> ConfigResult<(bool, u32)> {
    let span = arg(renderer, "probes")?.span;
    let fields = structure(renderer, "probes", "ProbeConfig", 2)?;
    let enabled = match fields[0] {
        Value::Bool(value) => value,
        _ => {
            return Err(coded(
                "P007",
                "renderer bound `probes` enabled value must be boolean",
            )
            .at(span));
        }
    };
    let deadline = crate::eval::value::as_i128(&fields[1])
        .and_then(|value| u32::try_from(value).ok())
        .ok_or_else(|| {
            coded(
                "P007",
                "renderer bound `probes` initialization must be a nonnegative u32",
            )
            .at(span)
        })?;
    if !enabled && deadline != 0 {
        return Err(coded(
            "P007",
            "renderer bound `probes` disabled configuration must have zero initialization work",
        )
        .at(span));
    }
    if enabled && deadline == 0 {
        return Err(coded(
            "P007",
            "renderer bound `probes` enabled configuration must declare initialization work",
        )
        .at(span));
    }
    Ok((enabled, deadline))
}

fn enum_variant(
    renderer: &RendererDecl,
    label: &str,
    expected_type: &str,
    variants: &[&str],
) -> ConfigResult<String> {
    let argument = arg(renderer, label)?;
    let Value::Enum(index, payload) = &argument.value else {
        return Err(coded(
            "P022",
            format!("render profile `{label}` must be a comptime `{expected_type}` variant"),
        )
        .at(argument.span));
    };
    if !payload.is_empty() {
        return Err(coded(
            "P022",
            format!("render profile `{label}` `{expected_type}` variant may not carry payload"),
        )
        .at(argument.span));
    }
    variants
        .get(*index)
        .map(|name| (*name).to_string())
        .ok_or_else(|| {
            coded(
                "P022",
                format!(
                    "render profile `{label}` has unknown `{expected_type}` variant index {index}"
                ),
            )
            .at(argument.span)
        })
}

fn is_canonical_display_type(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    ty: &Type,
    expected_name: &str,
    require_driver: bool,
) -> bool {
    let Type::Named(visible_name, args) = ty else {
        return false;
    };
    if !args.is_empty() {
        return false;
    }
    let Some((declaring_module, declaring_name)) = super::nominal_decl(owner, visible_name) else {
        return false;
    };
    if declaring_name != expected_name || !matches!(declaring_module, "display" | "drivers.display")
    {
        return false;
    }
    super::program_for_decl_module(programs, declaring_module)
        .and_then(|program| program.structs.get(declaring_name))
        .is_some_and(|definition| !require_driver || definition.is_driver)
}

fn finite_data(
    ty: &Type,
    context: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    active: &mut BTreeSet<String>,
) -> bool {
    match ty {
        Type::Bool
        | Type::U8
        | Type::U16
        | Type::U32
        | Type::U64
        | Type::Usize
        | Type::I8
        | Type::I16
        | Type::I32
        | Type::I64
        | Type::Isize
        | Type::F32
        | Type::F64
        | Type::Char => true,
        Type::Array(element, _) | Type::Option(element) => {
            finite_data(element, context, programs, active)
        }
        Type::Tuple(items) => items
            .iter()
            .all(|item| finite_data(item, context, programs, active)),
        Type::Result(ok, error) => {
            finite_data(ok, context, programs, active)
                && finite_data(error, context, programs, active)
        }
        Type::Named(name, args) => {
            if args.iter().any(|arg| match arg {
                TypeArg::Type(ty) => !finite_data(ty, context, programs, active),
                TypeArg::Const(_) | TypeArg::Bound(_) | TypeArg::Pool(_) => false,
            }) {
                return false;
            }
            let Some((declaring_module, declaring_name)) = super::nominal_decl(context, name)
            else {
                return false;
            };
            let Some(declaration_program) =
                super::program_for_decl_module(programs, declaring_module)
            else {
                return false;
            };
            let concrete_struct = if args.is_empty() {
                declaration_program
                    .structs
                    .get(declaring_name)
                    .map(|strukt| (strukt, declaration_program))
            } else {
                super::instantiated_struct(context, name, args)
                    .map(|strukt| (strukt, context))
                    .or_else(|| {
                        super::instantiated_struct(declaration_program, declaring_name, args)
                            .map(|strukt| (strukt, declaration_program))
                    })
            };
            let Some((strukt, struct_context)) = concrete_struct else {
                let enumeration = if args.is_empty() {
                    declaration_program.enums.get(declaring_name)
                } else {
                    super::instantiated_enum(context, name, args)
                        .or_else(|| {
                            super::instantiated_enum(declaration_program, declaring_name, args)
                        })
                        .or_else(|| declaration_program.enums.get(declaring_name))
                };
                let Some(enumeration) = enumeration else {
                    return false;
                };
                let identity = format!("enum:{declaring_module}::{declaring_name}");
                if !active.insert(identity.clone()) {
                    return false;
                }
                // Generated frame validation has no payload-pattern lowering
                // yet. Fieldless enums are finite; payload-bearing enums are
                // rejected at admission so their contents cannot bypass the
                // per-frame numeric/range checks.
                let finite = enumeration.variant_payload_types.iter().all(Vec::is_empty);
                active.remove(&identity);
                return finite;
            };
            let identity = format!("struct:{declaring_module}::{declaring_name}");
            if strukt.is_resource
                || strukt.is_actor
                || strukt.is_driver
                || !active.insert(identity.clone())
            {
                return false;
            }
            let finite = strukt
                .field_types
                .values()
                .all(|field| finite_data(field, struct_context, programs, active));
            active.remove(&identity);
            finite
        }
        Type::Unit => true,
        Type::Own(_, _)
        | Type::Static(_)
        | Type::Fn(_, _)
        | Type::Str
        | Type::String(_)
        | Type::Bytes(_)
        | Type::Generic(_)
        | Type::Never => false,
    }
}

fn validate_capacity(
    renderer_count: usize,
    _cores: usize,
    actor_count: usize,
    driver_count: usize,
) -> ConfigResult<()> {
    if renderer_count > usize::from(u16::MAX) + 1 {
        return Err(coded(
            "P015",
            format!(
                "renderer capacity `declarations` needs {renderer_count}, which exceeds the \
                 machine-v1 ceiling {}",
                usize::from(u16::MAX) + 1
            ),
        ));
    }
    let generated_actor_count = renderer_count
        .checked_mul(P7_MAX_RENDER_WORKERS.checked_add(1).ok_or_else(|| {
            coded(
                "P015",
                "renderer capacity `generated_actors` overflows machine-v1",
            )
        })?)
        .and_then(|count| count.checked_add(actor_count))
        .and_then(|count| count.checked_add(driver_count))
        .ok_or_else(|| {
            coded(
                "P015",
                "renderer capacity `generated_actors` overflows machine-v1",
            )
        })?;
    if generated_actor_count >= u32::MAX as usize {
        return Err(coded(
            "P015",
            format!(
                "renderer capacity `generated_actors` needs {generated_actor_count}, which exceeds \
                 the existing u32 turn-id ceiling {}",
                u32::MAX - 1
            ),
        ));
    }
    Ok(())
}

fn validate_renderers_inner(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    graph: &ImageGraph,
) -> ConfigResult<RendererConfigs> {
    let fallback_span = graph
        .renderers
        .first()
        .map(|renderer| renderer.span)
        .unwrap_or_default();
    super::legality::check_field_storage(programs)
        .map_err(|message| ConfigFailure::from_prefixed(message).at(fallback_span))?;
    let renderer_count = graph.renderers.len();
    validate_capacity(
        renderer_count,
        graph.cores,
        graph.actors.len(),
        graph.drivers.len(),
    )
    .map_err(|error| error.at(fallback_span))?;
    let mut claimed_displays = BTreeMap::new();
    let mut claimed_display_doorbells = BTreeMap::new();
    let mut configs = Vec::with_capacity(graph.renderers.len());
    for (declaration_index, renderer) in graph.renderers.iter().enumerate() {
        let actor_type_is_canonical = matches!(
            &renderer.actor_type,
            Type::Named(name, args)
                if name == "Renderer"
                    && matches!(
                        args.as_slice(),
                        [TypeArg::Type(params)]
                            if crate::sema::bodies::types_eq(params, &renderer.params_type)
                    )
        );
        if !actor_type_is_canonical {
            return Err(coded(
                "P009",
                "renderer declaration handle type is not the sealed canonical `Renderer[P]`",
            )
            .at(renderer.span));
        }
        validate_labels(renderer)?;
        validate_canonical_config_types(owner, renderer)?;
        if !finite_data(&renderer.params_type, owner, programs, &mut BTreeSet::new()) {
            return Err(coded(
                "P009",
                format!(
                    "renderer field/material parameter types disagree: `{}` is not finite data",
                    types::render_type(&renderer.params_type)
                ),
            )
            .at(renderer.span));
        }
        let field = function(renderer, "field")?;
        let material = function(renderer, "material")?;
        super::legality::check_renderer_roots(owner, programs, &field, &material).map_err(
            |error| {
                let diagnostic = error.diagnostic();
                let mut message = diagnostic.message.clone();
                for note in &diagnostic.notes {
                    message.push_str("\n  ");
                    message.push_str(note);
                }
                ConfigFailure {
                    code: diagnostic.code,
                    message,
                    primary: Some(if diagnostic.primary == Span::default() {
                        renderer.span
                    } else {
                        diagnostic.primary
                    }),
                }
            },
        )?;
        super::validate_parameter_identity(
            owner,
            programs,
            &renderer.params_type,
            &field,
            &material,
        )
        .map_err(|message| {
            coded(
                "P009",
                format!("renderer field/material parameter types disagree: {message}"),
            )
            .at(renderer.span)
        })?;
        let material_type = super::validate_material_identity(owner, programs, &field, &material)
            .map_err(|message| {
            coded(
                "P009",
                format!("renderer field/material parameter types disagree: {message}"),
            )
            .at(renderer.span)
        })?;

        let display_argument = arg(renderer, "display")?;
        let display_index = match &display_argument.value {
            Value::ImageDecl(ImageDeclRef::Driver(index)) => *index,
            Value::ImageDecl(reference) => {
                return Err(coded(
                    "P010",
                    format!(
                        "renderer display mode disagrees with the bound display driver: found {}",
                        reference.render()
                    ),
                )
                .at(display_argument.span));
            }
            _ => {
                return Err(coded(
                    "P010",
                    "renderer display mode disagrees with the bound display driver: `display` is not an image declaration",
                )
                .at(display_argument.span));
            }
        };
        let display = graph.drivers.get(display_index).ok_or_else(|| {
            coded(
                "P010",
                format!(
                    "renderer display mode disagrees with the bound display driver: driver#{display_index} does not exist"
                ),
            )
            .at(display_argument.span)
        })?;
        if !is_canonical_display_type(owner, programs, &display.actor_type, "Display", true) {
            return Err(coded(
                "P010",
                format!(
                    "renderer display mode disagrees with the bound display driver: expected `Display`, found `{}`",
                    types::render_type(&display.actor_type)
                ),
            )
            .at(display_argument.span));
        }
        let device_binding = display
            .args
            .iter()
            .find(|argument| argument.label == "device")
            .ok_or_else(|| {
                coded(
                    "P010",
                    format!("renderer display driver#{display_index} has no `device=` binding"),
                )
                .at(display_argument.span)
            })?;
        let display_device_index = match &device_binding.value {
            Value::ImageDecl(ImageDeclRef::Device(index)) => *index,
            value => {
                return Err(coded(
                    "P010",
                    format!(
                        "renderer display driver#{display_index} has invalid `device=` value {value:?}"
                    ),
                )
                .at(device_binding.span));
            }
        };
        let display_device = graph.devices.get(display_device_index).ok_or_else(|| {
            coded(
                "P010",
                format!("renderer display device#{display_device_index} does not exist"),
            )
            .at(device_binding.span)
        })?;
        if !is_canonical_display_type(
            owner,
            programs,
            &display_device.device_type,
            "DisplayDevice",
            false,
        ) {
            return Err(coded(
                "P010",
                format!(
                    "renderer display driver#{display_index} must bind a machine-v1 `DisplayDevice`, found `{}`",
                    types::render_type(&display_device.device_type)
                ),
            )
            .at(device_binding.span));
        }
        let transport = display_device
            .args
            .iter()
            .find(|argument| argument.label == "transport")
            .ok_or_else(|| {
                coded(
                    "P010",
                    format!(
                        "renderer display device#{display_device_index} has no `transport=` binding"
                    ),
                )
                .at(device_binding.span)
            })?;
        let transport_offset = match &transport.value {
            Value::U64(value) | Value::Usize(value) => *value,
            Value::U32(value) => u64::from(*value),
            Value::I64(value) | Value::Isize(value) if *value >= 0 => *value as u64,
            Value::I32(value) if *value >= 0 => u64::from(*value as u32),
            Value::Enum(0, payload) if payload.is_empty() => 0x6000,
            value => {
                return Err(coded(
                    "P010",
                    format!(
                        "renderer display device#{display_device_index} has invalid `transport=` value {value:?}"
                    ),
                )
                .at(transport.span));
            }
        };
        let display_doorbell_addr = wrela_machine::mmio::MMIO_BASE
            .checked_add(transport_offset)
            .ok_or_else(|| {
                coded("P010", "renderer display transport address overflows u64").at(transport.span)
            })?;
        if !wrela_machine::pixels::is_display_doorbell_addr(display_doorbell_addr) {
            return Err(coded(
                "P010",
                format!(
                    "renderer display transport {transport_offset:#x} is not a machine-v1 display doorbell slot"
                ),
            )
            .at(transport.span));
        }
        if let Some(prior) =
            claimed_display_doorbells.insert(display_doorbell_addr, declaration_index)
        {
            return Err(coded(
                "P021",
                format!(
                    "more than one renderer claims display doorbell {display_doorbell_addr:#x} (renderer#{prior} and renderer#{declaration_index})"
                ),
            )
            .at(transport.span));
        }
        if let Some(prior) = claimed_displays.insert(display_index, declaration_index) {
            return Err(coded(
                "P021",
                format!(
                    "more than one renderer claims display declaration `driver#{display_index}` (renderer#{prior} and renderer#{declaration_index})"
                ),
            )
            .at(display_argument.span));
        }

        let width = integer(renderer, "width")?;
        let height = integer(renderer, "height")?;
        let refresh_hz = integer(renderer, "refresh_hz")?;
        let shade_hz = integer(renderer, "shade_hz")?;
        let display_width = display_integer(programs, display, "width", display_argument.span)?;
        let display_height = display_integer(programs, display, "height", display_argument.span)?;
        let display_refresh_hz =
            display_integer(programs, display, "refresh_hz", display_argument.span)?;
        if width == 0 || height == 0 || refresh_hz == 0 || shade_hz == 0 {
            let label = [
                ("width", width),
                ("height", height),
                ("refresh_hz", refresh_hz),
                ("shade_hz", shade_hz),
            ]
            .into_iter()
            .find_map(|(label, value)| (value == 0).then_some(label))
            .expect("zero mode component exists");
            return Err(coded(
                "P010",
                "renderer display mode disagrees with the bound display driver: dimensions and rates must be positive",
            )
                .at(arg(renderer, label)?.span));
        }
        if let Some((label, value, maximum)) = oversized_mode_dimension(width, height) {
            return Err(coded(
                "P010",
                format!(
                    "renderer display mode exceeds the machine-v1 {label} ceiling: found {value}, maximum is {maximum}"
                ),
            )
            .at(arg(renderer, label)?.span));
        }
        let machine_mode = wrela_machine::pixels::DisplayModeV1 {
            width,
            height,
            refresh_hz,
        };
        if machine_mode.validate().is_err() {
            return Err(coded(
                "P010",
                format!(
                    "renderer display mode {width}x{height}@{refresh_hz} exceeds the machine-v1 \
                     complete guest-memory budget (maximum product is {} pixels)",
                    wrela_machine::pixels::MAX_MODE_PIXELS,
                ),
            )
            .at(display_argument.span));
        }
        if refresh_hz % shade_hz != 0 {
            return Err(coded(
                "P010",
                format!(
                    "renderer display mode disagrees with the bound display driver: shade_hz={shade_hz} must divide refresh_hz={refresh_hz}"
                ),
            )
            .at(arg(renderer, "shade_hz")?.span));
        }
        // P8 scanout is tile-local: every 64x32 allocation starts on its own
        // aligned boundary and a tile has exactly one worker owner. Partial
        // rows therefore never share an update word across workers, so mode
        // width is no longer constrained by the pre-P8 linear framebuffer.
        if (width, height, refresh_hz) != (display_width, display_height, display_refresh_hz) {
            return Err(coded(
                "P010",
                format!(
                    "renderer display mode disagrees with the bound display driver: renderer \
                     mode is {width}x{height}@{refresh_hz}, display mode is \
                     {display_width}x{display_height}@{display_refresh_hz}"
                ),
            )
            .at(display_argument.span));
        }
        let profile = enum_variant(renderer, "profile", "RenderProfile", &["AaaByteExact"])?;
        let tone_curve = enum_variant(
            renderer,
            "tone_curve",
            "ToneCurve",
            &["Linear", "FilmicV1", "__wrela_NonMonotoneFixture"],
        )?;
        let near = float(renderer, "near")?;
        let far = float(renderer, "far")?;
        if near <= 0.0 || near >= far {
            return Err(coded(
                "P016",
                format!(
                    "projective denominator may reach zero inside the declared camera/world domain: \
                     renderer depth range requires 0 < near < far, found near={near} far={far}"
                ),
            )
            .at(arg(renderer, "near")?.span));
        }
        let world_min = vec3(renderer, "world_min")?;
        let world_max = vec3(renderer, "world_max")?;
        if world_min.x > world_max.x || world_min.y > world_max.y || world_min.z > world_max.z {
            return Err(coded(
                "P007",
                "renderer world range has a component minimum greater than its maximum",
            )
            .at(arg(renderer, "world_min")?.span));
        }
        let camera_max_motion = one_f32_struct(renderer, "camera_bounds", "CameraBounds")?;
        if camera_max_motion < 0.0 {
            return Err(coded(
                "P007",
                "renderer camera range max_motion must be nonnegative",
            )
            .at(arg(renderer, "camera_bounds")?.span));
        }
        let (light_capacity, light_kinds, light_ranges) = light_config(renderer)?;
        let exposure = scalar_range(renderer, "exposure_range")?;
        let environment = rgb_range(renderer, "environment_range")?;
        let (ao_enabled, ao_radius, ao_strength) = ao_config(renderer)?;
        let (probes_enabled, probe_initialization_worst_case_ms) = probe_config(renderer)?;
        let initialization_deadline_ms = integer(renderer, "initialization_deadline_ms")?;
        if initialization_deadline_ms == 0 {
            return Err(coded(
                "P007",
                "renderer initialization range deadline must be positive",
            )
            .at(arg(renderer, "initialization_deadline_ms")?.span));
        }
        if initialization_deadline_ms < probe_initialization_worst_case_ms {
            return Err(coded(
                "P007",
                format!(
                    "renderer initialization range deadline {initialization_deadline_ms} is below \
                     the deterministic probe bound {probe_initialization_worst_case_ms}"
                ),
            )
            .at(arg(renderer, "initialization_deadline_ms")?.span));
        }
        let parameter_contracts = super::params::collect_parameter_contracts(
            owner,
            programs,
            &renderer.params_type,
            &field,
            &material,
        )
        .map_err(|message| ConfigFailure::from_prefixed(message).at(renderer.span))?;
        configs.push(RendererConfig {
            declaration_index,
            worker_count: p7_worker_count(graph.cores)
                .map_err(|message| coded("P015", message).at(renderer.span))?,
            params_type: renderer.params_type.clone(),
            field,
            material,
            material_type,
            display_index,
            display_doorbell_addr,
            width,
            height,
            refresh_hz,
            shade_hz,
            profile,
            tone_curve,
            near,
            far,
            world_min,
            world_max,
            camera_max_motion,
            camera_pose: camera_pose(renderer)?,
            light_capacity,
            light_kinds,
            light_ranges,
            exposure,
            environment,
            ao_enabled,
            ao_radius,
            ao_strength,
            probes_enabled,
            probe_initialization_worst_case_ms,
            initialization_deadline_ms,
            parameter_contracts,
        });
    }
    Ok(RendererConfigs { renderers: configs })
}

pub fn validate_renderers(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    graph: &ImageGraph,
) -> Result<RendererConfigs, super::diagnostics::PixelsError> {
    validate_renderers_inner(owner, programs, graph).map_err(|failure| {
        super::diagnostics::PixelsError::Diagnostic(
            super::diagnostics::PixelsDiagnostic::from_code(
                failure.code,
                &failure.message,
                failure.primary.unwrap_or_default(),
            ),
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sema::typed::TypedEnum;

    #[test]
    fn canonical_config_type_table_covers_every_nominal_renderer_argument() {
        let labels: BTreeSet<&str> = CANONICAL_CONFIG_TYPES
            .iter()
            .map(|(label, _, _, _)| *label)
            .collect();
        assert_eq!(
            labels,
            BTreeSet::from([
                "ao",
                "camera_bounds",
                "environment_range",
                "exposure_range",
                "light_config",
                "probes",
                "profile",
                "tone_curve",
                "world_max",
                "world_min",
            ])
        );
        assert!(
            CANONICAL_CONFIG_TYPES
                .iter()
                .all(|(_, _, module, _)| matches!(*module, "core.field" | "core.render"))
        );
    }

    fn renderer_arg(label: &str, ty: Type, value: Value) -> RendererDecl {
        RendererDecl {
            params_type: Type::Unit,
            actor_type: Type::Named("Renderer".to_string(), vec![]),
            args: vec![DeclArg {
                label: label.to_string(),
                ty,
                value,
                span: Default::default(),
            }],
            span: Default::default(),
        }
    }

    #[test]
    fn scalar_extraction_rejects_non_finite_values() {
        let mut renderer = renderer_arg("near", Type::F32, Value::F32(f32::NAN));
        renderer.args[0].span = Span {
            line: 42,
            col: 17,
            ..Span::default()
        };
        let error = float(&renderer, "near").unwrap_err();
        assert_eq!(error.code, "P007");
        assert_eq!(error.message, "renderer bound `near` must be finite");
        assert_eq!(error.primary, Some(renderer.args[0].span));
        let renderer = renderer_arg("near", Type::F64, Value::F64(f64::MAX));
        assert_eq!(float(&renderer, "near").unwrap(), f64::MAX);
        let renderer = renderer_arg("near", Type::F64, Value::F64(f64::MIN_POSITIVE));
        assert_eq!(float(&renderer, "near").unwrap(), f64::MIN_POSITIVE);
    }

    #[test]
    fn machine_mode_dimension_ceiling_is_shared_exactly() {
        assert_eq!(
            oversized_mode_dimension(
                wrela_machine::pixels::MAX_MODE_WIDTH,
                wrela_machine::pixels::MAX_MODE_HEIGHT,
            ),
            None,
        );
        assert_eq!(
            oversized_mode_dimension(wrela_machine::pixels::MAX_MODE_WIDTH + 1, 1),
            Some((
                "width",
                wrela_machine::pixels::MAX_MODE_WIDTH + 1,
                wrela_machine::pixels::MAX_MODE_WIDTH,
            )),
        );
        assert_eq!(
            oversized_mode_dimension(1, wrela_machine::pixels::MAX_MODE_HEIGHT + 1),
            Some((
                "height",
                wrela_machine::pixels::MAX_MODE_HEIGHT + 1,
                wrela_machine::pixels::MAX_MODE_HEIGHT,
            )),
        );
    }

    #[test]
    fn range_extraction_rejects_reversed_and_non_finite_components() {
        let renderer = renderer_arg(
            "exposure_range",
            Type::Named("ScalarRange".to_string(), vec![]),
            Value::Struct(vec![Value::F32(2.0), Value::F32(1.0)]),
        );
        assert!(
            scalar_range(&renderer, "exposure_range")
                .unwrap_err()
                .contains("greater than")
        );
        let renderer = renderer_arg(
            "environment_range",
            Type::Named("RgbRange".to_string(), vec![]),
            Value::Struct(vec![
                Value::Struct(vec![Value::F32(0.0), Value::F32(0.0), Value::F32(0.0)]),
                Value::Struct(vec![
                    Value::F32(f32::INFINITY),
                    Value::F32(1.0),
                    Value::F32(1.0),
                ]),
            ]),
        );
        assert!(
            rgb_range(&renderer, "environment_range")
                .unwrap_err()
                .contains("finite")
        );
    }

    #[test]
    fn enum_extraction_rejects_unknown_variants() {
        let renderer = renderer_arg(
            "profile",
            Type::Named("RenderProfile".to_string(), vec![]),
            Value::Enum(1, vec![]),
        );
        assert!(
            enum_variant(&renderer, "profile", "RenderProfile", &["AaaByteExact"])
                .unwrap_err()
                .contains("unknown")
        );
    }

    #[test]
    fn display_nominals_cannot_be_forged_by_name() {
        let display_type = Type::Named("Screen".to_string(), vec![]);
        let mut owner = TypedProgram::default();
        owner
            .type_decl_modules
            .insert("Screen".to_string(), "drivers.display".to_string());
        owner
            .type_decl_names
            .insert("Screen".to_string(), "Display".to_string());
        let mut display_module = TypedProgram::default();
        display_module.structs.insert(
            "Display".to_string(),
            TypedStruct {
                is_driver: true,
                ..TypedStruct::default()
            },
        );
        let mut unrelated = TypedProgram::default();
        unrelated
            .structs
            .insert("Display".to_string(), TypedStruct::default());
        let programs = BTreeMap::from([
            ("drivers.display".to_string(), display_module),
            ("examples.counterfeit".to_string(), unrelated),
        ]);
        assert!(is_canonical_display_type(
            &owner,
            &programs,
            &display_type,
            "Display",
            true
        ));

        owner
            .type_decl_modules
            .insert("Screen".to_string(), "examples.counterfeit".to_string());
        assert!(!is_canonical_display_type(
            &owner,
            &programs,
            &display_type,
            "Display",
            true
        ));
    }

    #[test]
    fn renderer_label_validation_is_closed_and_duplicate_safe() {
        let mut renderer = renderer_arg("mystery", Type::U32, Value::U32(1));
        assert!(validate_labels(&renderer).unwrap_err().contains("unknown"));
        renderer.args[0].label = "field".to_string();
        renderer.args.push(renderer.args[0].clone());
        assert!(
            validate_labels(&renderer)
                .unwrap_err()
                .contains("duplicate")
        );
        renderer.args.truncate(1);
        assert!(validate_labels(&renderer).unwrap_err().contains("missing"));
    }

    #[test]
    fn renderer_parameter_type_rejects_resources_statics_and_all_enum_payloads() {
        let mut program = TypedProgram::default();
        program.module_path = "examples.params".to_string();
        for name in ["ResourceParam", "BadChoice", "FiniteChoice"] {
            program
                .type_decl_modules
                .insert(name.to_string(), program.module_path.clone());
            program
                .type_decl_names
                .insert(name.to_string(), name.to_string());
        }
        program.structs.insert(
            "ResourceParam".to_string(),
            TypedStruct {
                is_resource: true,
                ..TypedStruct::default()
            },
        );
        program.enums.insert(
            "BadChoice".to_string(),
            TypedEnum {
                variants: vec!["Static".to_string()],
                variant_payload_types: vec![vec![Type::Static(Box::new(Type::U32))]],
                generic_type_params: Vec::new(),
                methods: BTreeMap::new(),
                assoc_fns: BTreeMap::new(),
            },
        );
        program.enums.insert(
            "FiniteChoice".to_string(),
            TypedEnum {
                variants: vec!["Value".to_string()],
                variant_payload_types: vec![vec![Type::U32]],
                generic_type_params: Vec::new(),
                methods: BTreeMap::new(),
                assoc_fns: BTreeMap::new(),
            },
        );
        let programs = BTreeMap::from([("examples.params".to_string(), program.clone())]);
        assert!(!finite_data(
            &Type::Named("ResourceParam".to_string(), vec![]),
            &program,
            &programs,
            &mut BTreeSet::new()
        ));
        assert!(!finite_data(
            &Type::Static(Box::new(Type::U32)),
            &program,
            &programs,
            &mut BTreeSet::new()
        ));
        assert!(!finite_data(
            &Type::Named("BadChoice".to_string(), vec![]),
            &program,
            &programs,
            &mut BTreeSet::new()
        ));
        assert!(!finite_data(
            &Type::Named("FiniteChoice".to_string(), vec![]),
            &program,
            &programs,
            &mut BTreeSet::new()
        ));
    }

    #[test]
    fn renderer_capacity_is_bounded() {
        assert!(validate_capacity(usize::from(u16::MAX) + 1, 4, 2, 1).is_ok());
        assert!(
            validate_capacity(usize::from(u16::MAX) + 2, 4, 2, 1)
                .unwrap_err()
                .starts_with("P015:")
        );
        assert!(
            validate_capacity(1, 4, usize::MAX, 0)
                .unwrap_err()
                .starts_with("P015:")
        );
    }

    #[test]
    fn light_config_extracts_fixed_topology_and_enforces_eight_slot_ceiling() {
        let kinds = Value::Array(vec![
            Value::Enum(3, vec![]),
            Value::Enum(1, vec![]),
            Value::Enum(0, vec![]),
            Value::Enum(0, vec![]),
            Value::Enum(0, vec![]),
            Value::Enum(0, vec![]),
            Value::Enum(0, vec![]),
            Value::Enum(0, vec![]),
        ]);
        let vec3 = |value: f32| Value::Struct(vec![Value::F32(value); 3]);
        let range = Value::Struct(vec![
            vec3(-4.0),
            vec3(4.0),
            Value::F32(2.0),
            Value::Struct(vec![Value::F32(8.0); 3]),
            Value::F32(0.25),
        ]);
        let ranges = Value::Array(vec![range; 8]);
        let renderer = renderer_arg(
            "light_config",
            Type::Named("LightConfig".to_string(), vec![]),
            Value::Struct(vec![Value::U32(2), kinds.clone(), ranges.clone()]),
        );
        let decoded = light_config(&renderer).unwrap();
        assert_eq!(decoded.0, 2);
        assert_eq!(
            decoded.1,
            vec!["Rectangle".to_string(), "Point".to_string()]
        );
        assert_eq!(decoded.2.len(), 8);
        assert_eq!(decoded.2[0].max_delta, 0.25);

        let oversized = renderer_arg(
            "light_config",
            Type::Named("LightConfig".to_string(), vec![]),
            Value::Struct(vec![Value::U32(9), kinds, ranges]),
        );
        let error = light_config(&oversized).unwrap_err();
        assert_eq!(error.code, "P015");
        assert!(
            error
                .message
                .starts_with("renderer capacity `lights` exceeds the machine-v1 sealed ceiling")
        );
    }

    #[test]
    fn light_kind_index_decoder_is_pinned_to_the_stdlib_declaration_order() {
        let render_source = include_str!("../../../../stdlib/core/render.wr");
        let declaration = format!(
            "pub enum LightKind:\n{}",
            LIGHT_KIND_NAMES
                .iter()
                .map(|name| format!("    {name}\n"))
                .collect::<String>()
        );
        assert!(
            render_source.contains(&declaration),
            "update the typed light-kind decoder deliberately when the stdlib enum changes"
        );
    }

    #[test]
    fn p7_renderer_worker_group_has_an_explicit_fail_closed_ceiling() {
        assert_eq!(p7_worker_count(1), Ok(1));
        assert_eq!(p7_worker_count(4), Ok(4));
        assert_eq!(
            p7_worker_count(5),
            Err("renderer capacity `worker_count` must be in 1..=4 for the P7 coordinator group")
        );
    }
}
