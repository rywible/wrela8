//! Strict shared numeric-vector format for Rust/Wrela differential coverage.

use std::collections::{BTreeMap, BTreeSet};

use super::reference::{
    certificate, coverage, csg, display, fixed_q, iv32, kinetic, material, normal, order, poly,
    root, transfer,
};

pub const REQUIRED_KERNELS: &[&str] = &[
    "bernstein_compose",
    "bernstein_sign",
    "bernstein_subdivide",
    "byte_singleton",
    "coverage_line",
    "coverage_quadratic",
    "csg_first_transition",
    "fixed_q_step",
    "interval_add",
    "interval_intersect",
    "interval_mul",
    "interval_sub",
    "kinetic_slack",
    "normal_bound",
    "q_order",
    "quadratic_range",
    "root_bracket",
    "root_count",
    "run_certificate",
    "transfer_compose",
    "transparency_tail",
    "interval_negate",
    "interval_square",
    "interval_scale_pow2",
    "interval_hull",
    "interval_min",
    "interval_max",
    "interval_abs",
    "interval_affine",
    "interval_convert",
    "bernstein_average",
    "coverage_quadratic_split",
    "coverage_quadratic_graph",
    "bernstein_certificate",
    "krawczyk_certificate",
    "normal_safe",
    "normal_dot",
    "material_affine",
    "material_residual",
    "exposure_channel",
    "color_matrix_channel",
    "monotone_lut",
    "quantize_ties_even",
    "root_zero_corridor",
    "fixed_q_setup",
    "normal_cone_dot",
    "polynomial_sparse",
    "polynomial_taylor",
    "transfer_balanced",
    "transfer_replace",
    "material_affine_moments",
    "material_quadratic_moments",
    "q_insertion_sort",
    "q_adjacent_slack",
    "csg_stack_eval",
    "root_isolate",
    "csg_oriented_toggle",
    "csg_boundary_influence",
    "bernstein_certificate_subdivided",
    "fixed_q_nearer",
    "coverage_color_error",
    "material_footprint",
    "material_affine_vector",
    "normal_cone",
    "polynomial_horner",
    "polynomial_derivative",
    "polynomial_compose",
    "interval_mul_exp",
    "interval_from_f32_bits",
    "interval_contains_zero",
    "interval_strict_positive",
    "interval_strict_negative",
    "interval_add_restricted",
    "rgb_singleton",
    "bernstein_lerp_ratio",
    "fixed_domain",
    "certify_run",
    "root_config",
];

const ALLOWED_KEYS: &[&str] = &[
    "case", "family", "in0", "in1", "in2", "in3", "out0", "out1", "flags",
];

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NumericVector {
    pub kernel: String,
    pub case_id: u32,
    pub family: VectorFamily,
    pub fields: BTreeMap<String, i64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum VectorFamily {
    Edge,
    Generated,
}

impl NumericVector {
    pub fn line(&self) -> String {
        let mut line = format!(
            "{} case={} family={}",
            self.kernel,
            self.case_id,
            match self.family {
                VectorFamily::Edge => 0,
                VectorFamily::Generated => 1,
            }
        );
        for (key, value) in &self.fields {
            line.push(' ');
            line.push_str(key);
            line.push('=');
            line.push_str(&value.to_string());
        }
        line
    }
}

pub fn parse_line(line: &str) -> Result<NumericVector, String> {
    if line.is_empty() || line.trim() != line {
        return Err("pixels vector: empty line or surrounding whitespace".to_string());
    }
    let mut tokens = line.split(' ');
    let kernel = tokens
        .next()
        .ok_or_else(|| "pixels vector: missing kernel".to_string())?;
    if !REQUIRED_KERNELS.contains(&kernel) {
        return Err(format!("pixels vector: unknown kernel `{kernel}`"));
    }
    let mut values = BTreeMap::<String, i64>::new();
    for token in tokens {
        if token.is_empty() {
            return Err("pixels vector: repeated whitespace".to_string());
        }
        let (key, raw) = token
            .split_once('=')
            .ok_or_else(|| format!("pixels vector: malformed field `{token}`"))?;
        if !ALLOWED_KEYS.contains(&key) {
            return Err(format!("pixels vector: unknown key `{key}`"));
        }
        if values.contains_key(key) {
            return Err(format!("pixels vector: duplicate key `{key}`"));
        }
        let value = raw
            .parse::<i64>()
            .map_err(|_| format!("pixels vector: malformed or overflowing integer `{raw}`"))?;
        values.insert(key.to_string(), value);
    }
    let case_id = values
        .remove("case")
        .ok_or_else(|| "pixels vector: missing `case`".to_string())
        .and_then(|value| {
            u32::try_from(value).map_err(|_| "pixels vector: `case` outside u32".to_string())
        })?;
    let family = match values.remove("family") {
        Some(0) => VectorFamily::Edge,
        Some(1) => VectorFamily::Generated,
        Some(value) => {
            return Err(format!(
                "pixels vector: `family` must be 0 or 1, got {value}"
            ));
        }
        None => return Err("pixels vector: missing `family`".to_string()),
    };
    if !values.contains_key("in0")
        || !values.contains_key("out0")
        || !matches!(values.get("flags"), Some(0 | 1))
    {
        return Err(
            "pixels vector: every row requires `in0`, `out0`, and boolean `flags`".to_string(),
        );
    }
    Ok(NumericVector {
        kernel: kernel.to_string(),
        case_id,
        family,
        fields: values,
    })
}

pub fn deterministic_vectors() -> Vec<NumericVector> {
    let mut vectors = Vec::with_capacity(REQUIRED_KERNELS.len() * 6);
    for (kernel_index, kernel) in REQUIRED_KERNELS.iter().copied().enumerate() {
        let edge_inputs = match kernel {
            "bernstein_compose" => (0..9).map(|slot| [1, -2, 3, slot]).collect(),
            "bernstein_subdivide" => (0..18).map(|slot| [1, -2, 3, slot]).collect(),
            "polynomial_derivative" => (0..8).map(|slot| [1, -2, 3, slot]).collect(),
            "q_insertion_sort" | "q_adjacent_slack" => {
                vec![
                    [0, 0, 0, 0],
                    [8, -4, 4, 5],
                    [0, 0, 0, 61],
                    [1_000, 0, 0, 1],
                    [1_001, 0, 0, 1],
                ]
            }
            "transfer_balanced" | "transfer_replace" => {
                vec![[1, 2, 3, 0], [-1, 0, 1, 5], [0, 0, 0, 61]]
            }
            "transfer_compose" => vec![
                [-8_i32, 8, -7, 3],
                [-1_i32, 0, 0, 1],
                [i32::MAX - 1, i32::MAX, 255, 256],
            ],
            "interval_scale_pow2" => vec![
                [0, 1, -96, 0],
                [0, 1, -97, 0],
                [0, 1, -159, 0],
                [0, 1, -63, 0],
                [0, 1, 30, 0],
                [0, 1, 31, 0],
                [0, 0, 63, 0],
                [0, 0, 97, 0],
                [0, 0, 127, 0],
                [0, 0, 128, 0],
                [0, 0, 159, 0],
                [-1, -1, 31, 0],
            ],
            "interval_mul_exp" => vec![
                [-1, 0, 31, 1],
                [-1, -1, 31, 1],
                [0, i32::MAX, 63, 0],
                [1, 1, 63, 1],
                [-1, -1, -96, 1],
            ],
            "interval_from_f32_bits" => vec![
                [0, 0, -8, 0],
                [1, 0, -96, 0],
                [1_065_353_216, 0, -8, 0],
                [-1_082_130_432, 1, -8, 0],
                [2_139_095_040, 0, -8, 0],
                [2_143_289_344, 0, -8, 0],
                [0, 0, 100, 0],
                [1_065_353_216, 0, 63, 0],
                [1_065_353_216, i32::MAX, -8, 0],
            ],
            "fixed_q_setup" => vec![
                [0, i32::MAX, i32::MAX, 1],
                [0, 1, 1, 32],
                [i32::MAX - 1, 1, 0, 2],
            ],
            "material_residual" => vec![
                [i32::MIN, 0, 0, i32::MAX],
                [0, i32::MAX, i32::MAX, i32::MAX],
                [0, 1, -1, 2],
                [0, 1, 1, -1],
            ],
            "bernstein_certificate" => vec![[-1, -1, i32::MIN, 1], [-4, -1, 1, 4]],
            "krawczyk_certificate" => {
                vec![[i32::MIN, i32::MAX, 1, 0], [-8, -4, 4, 8], [0, 0, 0, 0]]
            }
            "normal_bound" => vec![[i32::MAX, 0, 0, 2], [i32::MIN, 0, 0, -1], [0, 0, 0, 0]],
            "coverage_line" => vec![[1_000_001, 1, -1, 0], [1, 1, -1, 0], [0, 1, 0, 0]],
            "byte_singleton" => vec![[-9, -9, 0, 0], [24, 24, 0, 0], [-8, -8, 0, 0]],
            "quantize_ties_even" => {
                vec![[-1, 0, 0, 0], [1024, 0, 0, 0], [10, 0, 0, 0]]
            }
            "csg_first_transition" | "csg_stack_eval" | "csg_boundary_influence" => {
                vec![[i32::MIN, 0, 0, 0], [-1, 0, 0, 1], [0, 0, 0, 0]]
            }
            "coverage_quadratic_graph" => vec![
                [i32::MAX, i32::MAX, i32::MAX, 0],
                [-64, 512, -256, 0],
                [320, -512, 256, 1],
                [0, 0, 0, 0],
                [256, 0, 0, 1],
            ],
            "root_isolate" => (0..14)
                .flat_map(|shape| (0..49).map(move |selector| [shape, selector, 0, 0]))
                .collect(),
            "interval_contains_zero" | "interval_strict_positive" | "interval_strict_negative" => {
                vec![[-1, 1, 0, 0], [0, 0, 0, 0], [1, 2, 0, 0], [-2, -1, 0, 0]]
            }
            "interval_add_restricted" => vec![
                [100, 1, -100, 100],
                [99, 1, -100, 100],
                [-100, -1, -100, 100],
                [-99, -1, -100, 100],
                [0, 0, 1, -1],
            ],
            "rgb_singleton" => vec![
                [5, 6, 6, 2],
                [6, 9, 6, 2],
                [-1, 0, 0, 2],
                [1024, 1024, 0, 2],
                [0, 0, 0, 31],
            ],
            "bernstein_lerp_ratio" => vec![[-2, 3, 1, 3], [-1, 1, 2, 3], [0, 1, 1, 2]],
            "fixed_domain" => vec![
                [0, -100, 100, 100],
                [64, -100, 100, 0],
                [0, 1, -1, 0],
                [-96, i32::MIN, i32::MAX, 0],
            ],
            "interval_convert" => vec![[10, 10, 30, 100], [-8, -4, 4, 8], [0, 0, 0, 0]],
            "bernstein_certificate_subdivided" => vec![[-4, 4, 1, 9], [-4, 4, 1, 8], [0, 0, 0, 0]],
            "quadratic_range" => vec![
                [0, 513, 1, 1],
                [0, -8, -8, 16],
                [0, -1, 2, 1],
                [4, 5, -7, 3],
            ],
            "certify_run" => vec![
                [1, -4, 4, 2],
                [2, -4, 4, 2],
                [3, -4, 4, 2],
                [0, -4, 4, 2],
                [4, -4, 4, 2],
                [0, 1, 2, 1],
                [0, 1, 2, 0],
            ],
            "root_config" => vec![
                [0, 1, 12, 16],
                [0, 0, 12, 16],
                [0, 1, 16, 16],
                [0, 1, 12, 17],
                [99, 99, 12, 16],
            ],
            _ => vec![[-8_i32, -4, 4, 8], [-1_i32, 0, 0, 1], [0_i32, 0, 0, 0]],
        };
        for (case_id, inputs) in edge_inputs.into_iter().enumerate() {
            vectors.push(vector(kernel, case_id as u32, VectorFamily::Edge, inputs));
        }
        let mut state = 0x6a09_e667_f3bc_c909_u64 ^ kernel_index as u64;
        let generated_base = if kernel == "root_isolate" {
            10_000
        } else {
            100
        };
        for generated in 0..3_u32 {
            let mut inputs = [0_i32; 4];
            for input in &mut inputs {
                state = state
                    .wrapping_mul(6_364_136_223_846_793_005)
                    .wrapping_add(1_442_695_040_888_963_407);
                *input = ((state >> 32) as i32).rem_euclid(17) - 8;
            }
            vectors.push(vector(
                kernel,
                generated_base + generated,
                VectorFamily::Generated,
                inputs,
            ));
        }
    }
    vectors.sort_by(|a, b| (&a.kernel, a.case_id, a.family).cmp(&(&b.kernel, b.case_id, b.family)));
    vectors
}

fn vector(kernel: &str, case_id: u32, family: VectorFamily, mut inputs: [i32; 4]) -> NumericVector {
    normalize_inputs(kernel, &mut inputs);
    let mut fields = BTreeMap::new();
    for (index, input) in inputs.into_iter().enumerate() {
        fields.insert(format!("in{index}"), i64::from(input));
    }
    let result = evaluate_kernel(kernel, inputs);
    fields.insert("flags".to_string(), i64::from(result.is_err()));
    fields.insert("out0".to_string(), result.unwrap_or(0));
    NumericVector {
        kernel: kernel.to_string(),
        case_id,
        family,
        fields,
    }
}

fn normalize_inputs(kernel: &str, inputs: &mut [i32; 4]) {
    match kernel {
        "byte_singleton" => {
            inputs[0] = (inputs[0] + 8) * 32;
            inputs[1] = (inputs[1] + 8) * 32;
        }
        "coverage_quadratic" => {
            inputs[3] = inputs[3].unsigned_abs().min(16) as i32;
        }
        "fixed_q_step" => {
            inputs[0] *= 1024;
            inputs[1] *= 64;
            inputs[2] *= 4;
            inputs[3] = inputs[3].unsigned_abs() as i32;
        }
        "kinetic_slack" => {
            inputs[0] = inputs[0].unsigned_abs() as i32;
            inputs[1] = inputs[1].unsigned_abs() as i32;
        }
        "transparency_tail" => {
            for input in inputs.iter_mut() {
                *input = input.unsigned_abs() as i32;
            }
        }
        "root_isolate" => {
            inputs[0] = inputs[0].rem_euclid(14);
            inputs[1] = inputs[1].unsigned_abs().min(48) as i32;
            inputs[2] = inputs[2].unsigned_abs().min(7) as i32;
            inputs[3] = 0;
        }
        _ => {}
    }
    for pair in [[0, 1], [2, 3]] {
        if matches!(
            kernel,
            "interval_add"
                | "interval_intersect"
                | "interval_mul"
                | "interval_sub"
                | "q_order"
                | "byte_singleton"
                | "transfer_compose"
                | "interval_negate"
                | "interval_square"
                | "interval_scale_pow2"
                | "interval_hull"
                | "interval_min"
                | "interval_max"
                | "interval_abs"
                | "interval_convert"
                | "bernstein_average"
                | "coverage_color_error"
                | "material_footprint"
                | "interval_contains_zero"
                | "interval_strict_positive"
                | "interval_strict_negative"
        ) && (pair[0] == 0
            || matches!(
                kernel,
                "interval_add"
                    | "interval_intersect"
                    | "interval_mul"
                    | "interval_sub"
                    | "q_order"
                    | "transfer_compose"
                    | "interval_hull"
                    | "interval_min"
                    | "interval_max"
                    | "bernstein_average"
                    | "coverage_color_error"
                    | "material_footprint"
            ))
            && inputs[pair[0]] > inputs[pair[1]]
        {
            inputs.swap(pair[0], pair[1]);
        }
    }
    if kernel == "root_bracket" {
        inputs[0] = inputs[0].min(inputs[1]);
        inputs[1] = inputs[0].max(inputs[1]).max(inputs[0] + 2);
    }
    if kernel == "material_residual" && inputs[0] > inputs[1] {
        inputs.swap(0, 1);
    }
    if kernel == "rgb_singleton" && inputs[0] > inputs[1] {
        inputs.swap(0, 1);
    }
    if kernel == "coverage_line" && inputs[0] == 0 && inputs[1] == 0 {
        inputs[0] = 1;
    }
}

fn fold(values: &[i32]) -> i64 {
    values
        .iter()
        .fold(0_i64, |digest, value| digest * 257 + i64::from(*value))
}

fn fold_small(values: &[i32]) -> i64 {
    values
        .iter()
        .fold(0_i64, |digest, value| digest * 17 + i64::from(*value))
}

fn certificate_method_tag(method: certificate::RootCertMethod) -> i32 {
    match method {
        certificate::RootCertMethod::BernsteinFaces => 0,
        certificate::RootCertMethod::MonotoneTube => 1,
        certificate::RootCertMethod::Krawczyk => 2,
    }
}

fn composition_shape_tag(shape: Option<certificate::CompositionShape>) -> i32 {
    match shape {
        None => 0,
        Some(certificate::CompositionShape::Plane) => 1,
        Some(certificate::CompositionShape::Sphere) => 2,
        Some(certificate::CompositionShape::Torus) => 3,
    }
}

fn root_vector_program(
    shape: i32,
    domain: iv32::FixedDomain,
) -> Result<(root::RootEvaluator, iv32::Iv32), iv32::NumericError> {
    let shape = shape.rem_euclid(14);
    if shape >= 11 {
        let mut coefficients = [iv32::Iv32::point(0); poly::MAX_COEFFICIENTS];
        if shape == 11 {
            coefficients[..3].copy_from_slice(&[
                iv32::Iv32::point(3),
                iv32::Iv32::point(-5),
                iv32::Iv32::point(3),
            ]);
        } else if shape == 12 {
            coefficients[..3].copy_from_slice(&[
                iv32::Iv32::point(0),
                iv32::Iv32::point(-1),
                iv32::Iv32::point(1),
            ]);
        } else {
            coefficients[..2].copy_from_slice(&[iv32::Iv32::point(-2), iv32::Iv32::point(3)]);
        }
        return Ok((
            root::RootEvaluator::Bernstein(poly::Bernstein {
                coefficients,
                degree: if shape == 13 { 1 } else { 2 },
            }),
            iv32::Iv32::new(0, if shape == 13 { 3 } else { 1 })?,
        ));
    }
    if shape >= 8 {
        let (coefficients, remainder, q_domain) = match shape {
            8 => (
                vec![iv32::Iv32::point(-1), iv32::Iv32::point(1)],
                iv32::Iv32::point(0),
                iv32::Iv32::new(0, 2)?,
            ),
            9 => (
                vec![iv32::Iv32::point(5)],
                iv32::Iv32::new(-1, 1)?,
                iv32::Iv32::new(0, 2)?,
            ),
            _ => (
                vec![iv32::Iv32::point(0), iv32::Iv32::point(i32::MAX)],
                iv32::Iv32::point(0),
                iv32::Iv32::new(i32::MAX - 1, i32::MAX)?,
            ),
        };
        return Ok((
            root::RootEvaluator::IntervalTaylor {
                polynomial: poly::Polynomial::new(&coefficients)?,
                remainder,
            },
            q_domain,
        ));
    }
    let coefficients: &[i32] = match shape {
        0 => &[-1, 2],
        1 => &[1, -2, 1],
        2 => &[-1, 1, 0, 1],
        3 => &[24, -250, 875, -1250, 625],
        4 => &[-1, 2, 0, 0, 0, 1],
        5 => &[24, -250, 875, -1250, 625, 0, 1],
        6 => &[1, -2, 1, 0, 0, 0, 0, 1],
        _ => &[24, -250, 875, -1250, 625, 0, 0, 0, 1],
    };
    let polynomial = poly::Polynomial::new(
        &coefficients
            .iter()
            .copied()
            .map(iv32::Iv32::point)
            .collect::<Vec<_>>(),
    )?;
    Ok((
        root::RootEvaluator::Bernstein(polynomial.to_bernstein(domain)?),
        iv32::Iv32::new(0, 256)?,
    ))
}

fn root_outcome_field(outcome: root::RootOutcome, selector: i32) -> i64 {
    let selector = selector.unsigned_abs().min(48) as usize;
    match outcome {
        root::RootOutcome::CertifiedNone => -100,
        root::RootOutcome::Unresolved(reason) => -i64::from(match reason {
            root::RootReason::DepthExhausted => 1,
            root::RootReason::NoProgress => 2,
            root::RootReason::Numeric => 3,
            root::RootReason::CapacityRoots => 4,
            root::RootReason::CapacitySheets => 5,
            root::RootReason::DegenerateZero => 6,
        }),
        root::RootOutcome::Roots { roots, count } => {
            if selector == 0 {
                return i64::from(count);
            }
            let root_index = (selector - 1) / 3;
            if root_index >= usize::from(count) {
                return 0;
            }
            match (selector - 1) % 3 {
                0 => i64::from(roots[root_index].interval.lo),
                1 => i64::from(roots[root_index].interval.hi),
                _ => i64::from(roots[root_index].unique),
            }
        }
    }
}

fn evaluate_kernel(kernel: &str, input: [i32; 4]) -> Result<i64, iv32::NumericError> {
    let d0 = iv32::FixedDomain::full(0);
    let interval = |lo: i32, hi: i32| iv32::Iv32::new(lo, hi);
    Ok(match kernel {
        "bernstein_compose" => {
            let coefficients = [
                input[0], input[1], input[2], input[0], input[1], input[2], input[0], input[1],
                input[2],
            ]
            .map(iv32::Iv32::point);
            let polynomial = poly::Polynomial::new(&coefficients)?;
            let coefficients = polynomial.to_bernstein(d0)?.coefficients;
            let slot = input[3].unsigned_abs().min(8) as usize;
            fold(&[coefficients[slot].lo, coefficients[slot].hi])
        }
        "bernstein_sign" => {
            let mut coefficients = [iv32::Iv32::point(0); poly::MAX_COEFFICIENTS];
            coefficients[0] = iv32::Iv32::point(input[0]);
            coefficients[1] = iv32::Iv32::point(input[1]);
            coefficients[2] = iv32::Iv32::point(input[2]);
            match (poly::Bernstein {
                coefficients,
                degree: 2,
            })
            .sign()
            {
                poly::CoefficientSign::StrictNegative => -1,
                poly::CoefficientSign::Mixed => 0,
                poly::CoefficientSign::StrictPositive => 1,
            }
        }
        "bernstein_subdivide" => {
            let coefficients = [
                input[0], input[1], input[2], input[0], input[1], input[2], input[0], input[1],
                input[2],
            ]
            .map(iv32::Iv32::point);
            let polynomial = poly::Polynomial::new(&coefficients)?;
            let bernstein = poly::Bernstein {
                coefficients: polynomial.coefficients,
                degree: 8,
            };
            let (left, right) = bernstein.subdivide_half(d0)?;
            let selector = input[3].unsigned_abs().min(17) as usize;
            let value = if selector < 9 {
                left.coefficients[selector]
            } else {
                right.coefficients[selector - 9]
            };
            fold(&[value.lo, value.hi])
        }
        "byte_singleton" => i64::from(
            display::endpoint_singleton(interval(input[0], input[1])?, 2)?.map_or(-1, i32::from),
        ),
        "coverage_line" => {
            let area = coverage::half_plane_area(
                coverage::HalfPlane {
                    a: i64::from(input[0]),
                    b: i64::from(input[1]),
                    c: i64::from(input[2]),
                },
                iv32::FixedDomain::full(-9),
                coverage::BoundaryOwner::LowerOrLeft,
            )?;
            fold(&[area.lo, area.hi])
        }
        "coverage_quadratic" => {
            let area = coverage::quadratic_strip_area(
                coverage::HalfPlane {
                    a: i64::from(input[0]),
                    b: i64::from(input[1]),
                    c: i64::from(input[2]),
                },
                input[3].unsigned_abs(),
                iv32::FixedDomain::full(-9),
                coverage::BoundaryOwner::LowerOrLeft,
            )?;
            fold(&[area.lo, area.hi])
        }
        "csg_first_transition" => {
            if input[0] == i32::MIN {
                return csg::first_transition(&[], 0, &[]).map(|_| 0);
            }
            let program = [
                csg::CsgInstruction::Object(0),
                csg::CsgInstruction::Object(1),
                csg::CsgInstruction::Union,
            ];
            let mut bits = (input[0] as u64) & 3;
            let objects = [
                input[1].unsigned_abs().min(2) as u8,
                input[2].unsigned_abs().min(2) as u8,
            ];
            let mut crossings = [(0_u8, csg::Orientation::Enter); 2];
            for (slot, object) in crossings.iter_mut().zip(objects) {
                let mask = 1_u64 << object;
                let orientation = if bits & mask == 0 {
                    csg::Orientation::Enter
                } else {
                    csg::Orientation::Exit
                };
                *slot = (object, orientation);
                bits ^= mask;
            }
            let count = input[3].unsigned_abs().min(2) as usize;
            csg::first_transition(&program, (input[0] as u64) & 3, &crossings[..count])?
                .map_or(-1, |index| index as i64)
        }
        "fixed_q_step" => {
            let mut run = fixed_q::setup(
                iv32::Iv32::point(input[0]),
                iv32::Iv32::point(input[1]),
                iv32::Iv32::point(input[2]),
                d0,
                1,
                input[3],
            )?;
            let next = run.advance()?;
            fold(&[next, run.dq])
        }
        "interval_add" | "interval_sub" | "interval_mul" | "interval_intersect" => {
            let a = interval(input[0], input[1])?;
            let b = interval(input[2], input[3])?;
            let result = match kernel {
                "interval_add" => Some(a.add(b, d0)?),
                "interval_sub" => Some(a.subtract(b, d0)?),
                "interval_mul" => Some(a.multiply(b, d0)?),
                _ => a.intersect(b, d0)?,
            };
            result.map_or(-1, |value| fold(&[value.lo, value.hi]))
        }
        "kinetic_slack" => i64::from(kinetic::slack_preserves(input[0], input[1])?),
        "normal_bound" => {
            let value = normal::reconstruct(
                iv32::Iv32::point(input[0]),
                iv32::Iv32::point(input[1]),
                iv32::Iv32::point(input[2]),
                iv32::Iv32::point(input[3]),
                iv32::Iv32::point(0),
                d0,
            )?;
            fold(&[
                value.x.lo, value.x.hi, value.y.lo, value.y.hi, value.z.lo, value.z.hi,
            ])
        }
        "q_order" => i64::from(matches!(
            order::compare(interval(input[0], input[1])?, interval(input[2], input[3])?),
            order::OrderRelation::Nearer
        )),
        "quadratic_range" => {
            let range = poly::exact_quadratic_range(
                poly::Quadratic2 {
                    c: input[0],
                    bu: input[1],
                    bv: input[2],
                    buu: input[3],
                    buv: input[2],
                    bvv: input[3],
                },
                d0,
            )?;
            fold(&[range.lo, range.hi])
        }
        "root_bracket" => {
            let value = root::bracket_step(input[0], input[1], input[2] < 0, input[3] < 0)?;
            fold(&[value.interval.lo, value.interval.hi])
        }
        "root_count" => {
            let bernstein = poly::Bernstein {
                coefficients: {
                    let mut coefficients = [iv32::Iv32::point(0); poly::MAX_COEFFICIENTS];
                    coefficients[0] = iv32::Iv32::point(input[0]);
                    coefficients[1] = iv32::Iv32::point(input[1]);
                    coefficients[2] = iv32::Iv32::point(input[2]);
                    coefficients
                },
                degree: 2,
            };
            match bernstein.sign_variations() {
                poly::SignVariation::Exact(count) => i64::from(count),
                poly::SignVariation::Inconclusive => -1,
            }
        }
        "run_certificate" => i64::from(
            certificate::monotone_tube(
                iv32::Iv32::point(input[0]),
                iv32::Iv32::point(input[1]),
                iv32::Iv32::point(input[2]),
            )
            .is_some(),
        ),
        "transfer_compose" => {
            let front = transfer::Transfer {
                rgb: [interval(input[0], input[1])?; 3],
                transmittance: interval(input[2], input[3])?,
            };
            let back = transfer::Transfer {
                rgb: [interval(input[2], input[3])?; 3],
                transmittance: interval(input[0], input[1])?,
            };
            let result = transfer::compose(front, back, iv32::FixedDomain::full(-8), 0)?;
            fold(&[
                result.rgb[0].lo,
                result.rgb[0].hi,
                result.transmittance.lo,
                result.transmittance.hi,
            ])
        }
        "transparency_tail" => i64::from(transfer::tail_can_stop(
            iv32::Iv32::point(input[0]),
            iv32::Iv32::point(input[1]),
            input[2],
            iv32::FixedDomain::full(-8),
        )?),
        "interval_negate" => {
            let value = interval(input[0], input[1])?.negate(d0)?;
            fold(&[value.lo, value.hi])
        }
        "interval_square" => {
            let value = interval(input[0], input[1])?.square(d0)?;
            fold(&[value.lo, value.hi])
        }
        "interval_scale_pow2" => {
            let value = interval(input[0], input[1])?.scale_pow2(
                i16::try_from(input[2]).map_err(|_| iv32::NumericError::Overflow)?,
                d0,
            )?;
            fold(&[value.lo, value.hi])
        }
        "interval_contains_zero" => i64::from(interval(input[0], input[1])?.contains_zero()),
        "interval_strict_positive" => i64::from(interval(input[0], input[1])?.strict_positive()),
        "interval_strict_negative" => i64::from(interval(input[0], input[1])?.strict_negative()),
        "interval_add_restricted" => {
            let domain = iv32::FixedDomain::new(0, input[2], input[3])?;
            let value = iv32::Iv32::point(input[0]).add(iv32::Iv32::point(input[1]), domain)?;
            fold(&[value.lo, value.hi])
        }
        "rgb_singleton" => {
            let color = [
                interval(input[0], input[1])?,
                iv32::Iv32::point(input[2]),
                iv32::Iv32::point(input[2]),
            ];
            let fraction_bits = u8::try_from(input[3]).map_err(|_| iv32::NumericError::Overflow)?;
            display::rgb_singleton(color, fraction_bits)?
                .map_or(-1, |codes| fold(&codes.map(i32::from)))
        }
        "bernstein_lerp_ratio" => {
            let value = poly::bernstein_lerp_ratio(
                iv32::Iv32::point(input[0]),
                iv32::Iv32::point(input[1]),
                u64::try_from(input[2]).map_err(|_| iv32::NumericError::InvalidDomain)?,
                u64::try_from(input[3]).map_err(|_| iv32::NumericError::InvalidDomain)?,
                d0,
            )?;
            fold(&[value.lo, value.hi])
        }
        "fixed_domain" => {
            let domain = iv32::FixedDomain::new(
                i16::try_from(input[0]).map_err(|_| iv32::NumericError::InvalidDomain)?,
                input[1],
                input[2],
            )?;
            i64::from(domain.min_raw <= input[3] && input[3] <= domain.max_raw)
        }
        "interval_hull" | "interval_min" | "interval_max" => {
            let a = interval(input[0], input[1])?;
            let b = interval(input[2], input[3])?;
            let value = match kernel {
                "interval_hull" => a.hull(b, d0)?,
                "interval_min" => a.min(b, d0)?,
                _ => a.max(b, d0)?,
            };
            fold(&[value.lo, value.hi])
        }
        "interval_abs" => {
            let value = interval(input[0], input[1])?.abs(d0)?;
            fold(&[value.lo, value.hi])
        }
        "interval_affine" => {
            let value = iv32::Iv32::affine(
                iv32::Iv32::point(input[0]),
                iv32::Iv32::point(input[1]),
                iv32::Iv32::point(input[2]),
                d0,
            )?;
            fold(&[value.lo, value.hi])
        }
        "interval_convert" => {
            let source = iv32::FixedDomain::full(0);
            let destination = if input == [10, 10, 30, 100] {
                iv32::FixedDomain::new(-2, 30, 100)?
            } else {
                iv32::FixedDomain::full(
                    i16::try_from(input[2].clamp(-8, 8))
                        .map_err(|_| iv32::NumericError::Overflow)?,
                )
            };
            let value = interval(input[0], input[1])?.convert(source, destination)?;
            fold(&[value.lo, value.hi])
        }
        "bernstein_average" => {
            let a = interval(input[0], input[1])?;
            let b = interval(input[2], input[3])?;
            let value = poly::bernstein_average(a, b, d0)?;
            fold(&[value.lo, value.hi])
        }
        "coverage_quadratic_split" => {
            let value = coverage::quadratic_monotone_split(
                coverage::QuadraticGraph {
                    c: input[0],
                    b: input[1],
                    a: input[2],
                },
                iv32::FixedDomain::full(-8),
            )?
            .unwrap_or(iv32::Iv32::point(-1));
            fold(&[value.lo, value.hi])
        }
        "coverage_quadratic_graph" => {
            let value = coverage::quadratic_graph_area(
                coverage::QuadraticGraph {
                    c: input[0],
                    b: input[1],
                    a: input[2],
                },
                input[3].unsigned_abs().min(8),
                iv32::FixedDomain::full(-8),
                true,
                if input[3] & 1 == 0 {
                    coverage::BoundaryOwner::LowerOrLeft
                } else {
                    coverage::BoundaryOwner::UpperOrRight
                },
            )?;
            fold(&[value.lo, value.hi])
        }
        "bernstein_certificate" => {
            let make = |values: [i32; 3]| {
                let mut coefficients = [iv32::Iv32::point(0); poly::MAX_COEFFICIENTS];
                for (slot, value) in coefficients.iter_mut().zip(values) {
                    *slot = iv32::Iv32::point(value);
                }
                poly::Bernstein {
                    coefficients,
                    degree: 2,
                }
            };
            certificate::bernstein_faces(
                make([input[0], input[0], input[1]]),
                make([input[3], input[3], input[3]]),
                iv32::Iv32::point(input[2]),
            )
            .map_or(0, |value| {
                fold_small(&[
                    certificate_method_tag(value.method),
                    value.derivative_margin.lo,
                    value.face_margin.lo,
                    value.contraction_margin.lo,
                    0,
                    0,
                ])
            })
        }
        "krawczyk_certificate" => {
            let (reciprocal, residual, derivative, correction) =
                if input == [i32::MIN, i32::MAX, 1, 0] {
                    (
                        iv32::Iv32::point(1),
                        iv32::Iv32::point(0),
                        iv32::Iv32::point(1),
                        iv32::Iv32::new(i32::MIN, i32::MAX)?,
                    )
                } else {
                    (
                        iv32::Iv32::point(input[0]),
                        iv32::Iv32::point(input[1]),
                        iv32::Iv32::point(input[2]),
                        interval(input[0].min(input[3]), input[0].max(input[3]))?,
                    )
                };
            certificate::krawczyk(reciprocal, residual, derivative, correction, d0)?.map_or(
                0,
                |value| {
                    fold_small(&[
                        certificate_method_tag(value.method),
                        value.derivative_margin.lo,
                        value.face_margin.lo,
                        value.contraction_margin.lo,
                        0,
                        0,
                    ])
                },
            )
        }
        "normal_safe" | "normal_dot" => {
            let first = normal::reconstruct(
                iv32::Iv32::point(input[0]),
                iv32::Iv32::point(input[1]),
                iv32::Iv32::point(input[2]),
                iv32::Iv32::point(input[3]),
                iv32::Iv32::point(0),
                d0,
            )?;
            if kernel == "normal_safe" {
                i64::from(normal::normalization_is_safe(first, d0)?)
            } else {
                let value = normal::dot(first, first, d0)?;
                fold(&[value.lo, value.hi])
            }
        }
        "material_affine" => {
            let value = material::evaluate_affine_scalar(
                iv32::Iv32::point(input[0]),
                iv32::Iv32::point(input[1]),
                iv32::Iv32::point(input[2]),
                iv32::Iv32::point(input[3]),
                iv32::Iv32::point(input[3]),
                d0,
            )?;
            fold(&[value.lo, value.hi])
        }
        "material_residual" => i64::from(material::low_rank_residual_accepts(
            interval(input[0], input[1])?,
            input[2],
            input[3],
            d0,
        )?),
        "exposure_channel" => {
            let value = display::exposure(
                [iv32::Iv32::point(input[0]); 3],
                iv32::Iv32::point(input[1]),
                d0,
            )?[0];
            fold(&[value.lo, value.hi])
        }
        "color_matrix_channel" => {
            let zero = iv32::Iv32::point(0);
            let matrix = [
                [
                    iv32::Iv32::point(input[1]),
                    iv32::Iv32::point(input[2]),
                    iv32::Iv32::point(input[3]),
                ],
                [zero; 3],
                [zero; 3],
            ];
            let value = display::color_matrix([iv32::Iv32::point(input[0]); 3], matrix, d0)?[0];
            fold(&[value.lo, value.hi])
        }
        "monotone_lut" => {
            let raw = input[0].unsigned_abs().min(64) as i32;
            let value =
                display::monotone_lut(iv32::Iv32::point(raw), display::srgb_transfer_lut(), 2)?;
            fold(&[value.lo, value.hi])
        }
        "quantize_ties_even" => i64::from(display::quantize_u8_ties_even(input[0], 2)?),
        "root_zero_corridor" => {
            let mut coefficients = [iv32::Iv32::point(0); poly::MAX_COEFFICIENTS];
            coefficients[0] = iv32::Iv32::point(input[0]);
            coefficients[1] = iv32::Iv32::point(input[1]);
            coefficients[2] = iv32::Iv32::point(input[2]);
            i64::from(root::bernstein_is_identically_zero(poly::Bernstein {
                coefficients,
                degree: 2,
            }))
        }
        "fixed_q_setup" => {
            let run = fixed_q::setup_from_integer_coefficients(
                input[0],
                input[1],
                input[2],
                input[3].unsigned_abs().min(64) as u8,
                0,
            )?;
            fold_small(&[
                run.q,
                run.dq,
                run.ddq,
                i32::from(run.microtile_width),
                i32::from(run.domain.exponent),
                run.error_radius,
            ])
        }
        "normal_cone_dot" => {
            let a = normal::Iv3 {
                x: iv32::Iv32::point(input[0]),
                y: iv32::Iv32::point(input[1]),
                z: iv32::Iv32::point(input[2]),
            };
            let b = normal::Iv3 {
                x: iv32::Iv32::point(
                    input[0]
                        .checked_mul(input[3])
                        .ok_or(iv32::NumericError::Overflow)?,
                ),
                y: iv32::Iv32::point(
                    input[1]
                        .checked_mul(input[3])
                        .ok_or(iv32::NumericError::Overflow)?,
                ),
                z: iv32::Iv32::point(
                    input[2]
                        .checked_mul(input[3])
                        .ok_or(iv32::NumericError::Overflow)?,
                ),
            };
            let value = normal::normalized_dot_q8(a, b, d0)?;
            fold(&[value.lo, value.hi])
        }
        "polynomial_sparse" => {
            let mut terms = [poly::SparseTerm {
                coefficient: iv32::Iv32::point(0),
                powers: [0; 4],
            }; 32];
            for (index, term) in terms.iter_mut().enumerate() {
                term.coefficient =
                    iv32::Iv32::point(if index & 1 == 0 { input[0] } else { input[1] });
                term.powers = [
                    (index & 1) as u8,
                    ((index >> 1) & 1) as u8,
                    ((index >> 2) & 1) as u8,
                    ((index >> 3) & 1) as u8,
                ];
            }
            let value = poly::evaluate_sparse(
                &terms,
                [
                    iv32::Iv32::point(input[2]),
                    iv32::Iv32::point(1),
                    iv32::Iv32::point(-1),
                    iv32::Iv32::point(input[3]),
                ],
                d0,
            )?;
            fold(&[value.lo, value.hi])
        }
        "polynomial_taylor" => {
            let polynomial =
                poly::Polynomial::new(&[iv32::Iv32::point(input[0]), iv32::Iv32::point(input[1])])?;
            let radius = input[3].unsigned_abs().min(i32::MAX as u32) as i32;
            let value = poly::taylor_with_remainder(
                polynomial,
                iv32::Iv32::point(input[2]),
                iv32::Iv32::new(-radius, radius)?,
                d0,
            )?;
            fold(&[value.lo, value.hi])
        }
        "transfer_balanced" | "transfer_replace" => {
            let mut layers = [transfer::Transfer {
                rgb: [iv32::Iv32::point(0); 3],
                transmittance: iv32::Iv32::point(256),
            }; 64];
            for (index, layer) in layers.iter_mut().enumerate() {
                layer.rgb = [iv32::Iv32::point(match index % 3 {
                    0 => input[0],
                    1 => input[1],
                    _ => input[2],
                }); 3];
                layer.transmittance = iv32::Iv32::point(128 + (index & 1) as i32);
            }
            let one = iv32::Iv32::point(256);
            let count = input[3].unsigned_abs().min(61) as usize + 3;
            let result = if kernel == "transfer_balanced" {
                transfer::balanced_summary(&layers[..count], iv32::FixedDomain::full(-8), one, 0)
            } else {
                let replacement = transfer::Transfer {
                    rgb: [iv32::Iv32::point(input[2]); 3],
                    transmittance: iv32::Iv32::point(192),
                };
                transfer::replace_and_summarize(
                    &mut layers[..count],
                    input[0].unsigned_abs() as usize % count,
                    replacement,
                    iv32::FixedDomain::full(-8),
                    one,
                    0,
                )
            };
            let value = result?;
            fold(&[
                value.rgb[0].lo,
                value.rgb[0].hi,
                value.transmittance.lo,
                value.transmittance.hi,
            ])
        }
        "material_affine_moments" | "material_quadratic_moments" => {
            let value = if kernel == "material_affine_moments" {
                material::affine_moments(
                    iv32::Iv32::point(input[0]),
                    iv32::Iv32::point(input[1]),
                    iv32::Iv32::point(input[2]),
                    iv32::Iv32::point(input[3]),
                    d0,
                )?
            } else {
                material::quadratic_moments(
                    iv32::Iv32::point(input[0]),
                    iv32::Iv32::point(input[1]),
                    iv32::Iv32::point(input[1]),
                    iv32::Iv32::point(input[2]),
                    iv32::Iv32::point(input[3]),
                    d0,
                )?
            };
            fold(&[
                value.mean.lo,
                value.mean.hi,
                value.first.lo,
                value.first.hi,
                value.second.lo,
                value.second.hi,
            ])
        }
        "q_insertion_sort" | "q_adjacent_slack" => {
            let mut roots = [order::OrderedRoot {
                q: iv32::Iv32::point(0),
                feature_id: 0,
                orientation: 1,
            }; order::MAX_ORDERED_ROOTS];
            let count = input[3].unsigned_abs().min(61) as usize + 3;
            if matches!(input[0], 1_000 | 1_001) {
                let chain = [
                    order::OrderedRoot {
                        q: iv32::Iv32::new(0, 2)?,
                        feature_id: 0,
                        orientation: 1,
                    },
                    order::OrderedRoot {
                        q: iv32::Iv32::new(1, 10)?,
                        feature_id: 1,
                        orientation: -1,
                    },
                    order::OrderedRoot {
                        q: iv32::Iv32::new(9, 11)?,
                        feature_id: 2,
                        orientation: 1,
                    },
                    order::OrderedRoot {
                        q: iv32::Iv32::new(-4, -3)?,
                        feature_id: 3,
                        orientation: 1,
                    },
                ];
                for (root, source) in roots[..4].iter_mut().zip(if input[0] == 1_000 {
                    chain
                } else {
                    [chain[3], chain[2], chain[1], chain[0]]
                }) {
                    *root = source;
                }
            } else {
                for (index, root) in roots.iter_mut().take(count).enumerate() {
                    root.q = iv32::Iv32::point(
                        input[0]
                            .checked_sub((index / 2) as i32 * 4)
                            .ok_or(iv32::NumericError::Overflow)?,
                    );
                    root.feature_id = (count - index) as u32;
                    root.orientation = if index & 1 == 0 { 1 } else { -1 };
                }
            }
            order::insertion_sort(&mut roots[..count])?;
            if kernel == "q_adjacent_slack" {
                i64::from(order::adjacent_slack(&roots[..count])?)
            } else {
                fold_small(&[
                    roots[0].q.lo,
                    roots[0].q.hi,
                    roots[0].feature_id as i32,
                    i32::from(roots[0].orientation),
                    roots[1].q.lo,
                    roots[1].q.hi,
                    roots[1].feature_id as i32,
                    i32::from(roots[1].orientation),
                    roots[2].q.lo,
                    roots[2].q.hi,
                    roots[2].feature_id as i32,
                    i32::from(roots[2].orientation),
                ])
            }
        }
        "csg_stack_eval" => {
            if input[0] == i32::MIN {
                return csg::evaluate(&[], 0).map(i64::from);
            }
            let operation = match input[2].rem_euclid(3) {
                0 => csg::CsgInstruction::Union,
                1 => csg::CsgInstruction::Intersection,
                _ => csg::CsgInstruction::Subtract,
            };
            let program = [
                csg::CsgInstruction::Object(0),
                csg::CsgInstruction::Object(1),
                operation,
            ];
            let bits = u64::from(input[0] & 1 != 0) | (u64::from(input[1] & 1 != 0) << 1);
            i64::from(csg::evaluate(&program, bits)?)
        }
        "root_isolate" => {
            let (evaluator, q_domain) = root_vector_program(input[0], d0)?;
            root_outcome_field(
                root::isolate_roots(
                    evaluator,
                    q_domain,
                    d0,
                    input[2].unsigned_abs().saturating_add(1),
                    12,
                    16,
                )?,
                input[1],
            )
        }
        "csg_oriented_toggle" => {
            let bits = (input[0] as u64) & 7;
            let object = input[1].unsigned_abs().min(2) as u8;
            let orientation = if input[2] & 1 == 0 {
                csg::Orientation::Enter
            } else {
                csg::Orientation::Exit
            };
            csg::oriented_toggle(bits, object, orientation).map(|value| value as i64)?
        }
        "csg_boundary_influence" => {
            if input[0] == i32::MIN {
                return csg::boundary_influences(&[], 0, 0).map(i64::from);
            }
            let program = [
                csg::CsgInstruction::Object(0),
                csg::CsgInstruction::Object(1),
                csg::CsgInstruction::Union,
            ];
            i64::from(csg::boundary_influences(
                &program,
                (input[0] as u64) & 3,
                input[1].unsigned_abs().min(2) as u8,
            )?)
        }
        "bernstein_certificate_subdivided" => {
            let make = |values: [i32; 3]| {
                let mut coefficients = [iv32::Iv32::point(0); poly::MAX_COEFFICIENTS];
                for (slot, value) in coefficients.iter_mut().zip(values) {
                    *slot = iv32::Iv32::point(value);
                }
                poly::Bernstein {
                    coefficients,
                    degree: 2,
                }
            };
            certificate::bernstein_faces_subdivided(
                make([-4, input[0], -4]),
                make([4, input[1], 4]),
                iv32::Iv32::point(input[2]),
                d0,
                u8::try_from(input[3]).map_err(|_| iv32::NumericError::InvalidDomain)?,
            )?
            .map_or(0, |value| {
                fold_small(&[
                    certificate_method_tag(value.method),
                    value.derivative_margin.lo,
                    value.face_margin.lo,
                    value.contraction_margin.lo,
                    0,
                    0,
                ])
            })
        }
        "fixed_q_nearer" => i64::from(fixed_q::quantized_nearer_raw(
            input[0], input[1], input[2], input[3],
        )),
        "coverage_color_error" => {
            let value = coverage::color_error_bound(
                interval(input[0], input[1])?,
                interval(input[2], input[3])?,
                d0,
            )?;
            fold(&[value.lo, value.hi])
        }
        "material_footprint" => {
            let value = material::footprint_bound(
                interval(input[0], input[1])?,
                interval(input[2], input[3])?,
                d0,
            )?;
            fold(&[value.lo, value.hi])
        }
        "material_affine_vector" => {
            let vector = |value| normal::Iv3 {
                x: iv32::Iv32::point(value),
                y: iv32::Iv32::point(value),
                z: iv32::Iv32::point(value),
            };
            let value = material::evaluate_affine_vector(
                vector(input[0]),
                vector(input[1]),
                vector(input[2]),
                iv32::Iv32::point(input[3]),
                iv32::Iv32::point(0),
                d0,
            )?;
            fold(&[
                value.x.lo, value.x.hi, value.y.lo, value.y.hi, value.z.lo, value.z.hi,
            ])
        }
        "normal_cone" => {
            match normal::normal_cone(
                iv32::Iv32::point(input[0]),
                iv32::Iv32::point(0),
                iv32::Iv32::point(input[1]),
                iv32::Iv32::point(input[2]),
                iv32::Iv32::point(input[3]),
                d0,
            )? {
                Some(value) => fold_small(&[
                    value.direction.x.lo,
                    value.direction.x.hi,
                    value.direction.y.lo,
                    value.direction.y.hi,
                    value.direction.z.lo,
                    value.direction.z.hi,
                    value.norm_squared.lo,
                    value.norm_squared.hi,
                    0,
                    0,
                    0,
                    0,
                ]),
                None => -1,
            }
        }
        "polynomial_horner" => {
            let coefficients = [
                input[0], input[1], input[2], input[0], input[1], input[2], input[0], input[1],
                input[2],
            ]
            .map(iv32::Iv32::point);
            let value =
                poly::Polynomial::new(&coefficients)?.evaluate(iv32::Iv32::point(input[3]), d0)?;
            fold(&[value.lo, value.hi])
        }
        "polynomial_derivative" => {
            let coefficients = [
                input[0], input[1], input[2], input[0], input[1], input[2], input[0], input[1],
                input[2],
            ]
            .map(iv32::Iv32::point);
            let derivative = poly::Polynomial::new(&coefficients)?.derivative(d0)?;
            let slot = input[3].unsigned_abs().min(7) as usize;
            fold(&[
                derivative.coefficients[slot].lo,
                derivative.coefficients[slot].hi,
            ])
        }
        "polynomial_compose" => {
            let outer = poly::Polynomial::new(&[
                iv32::Iv32::point(input[0]),
                iv32::Iv32::point(input[1]),
                iv32::Iv32::point(input[2]),
            ])?;
            let candidate = poly::Polynomial::new(&[
                iv32::Iv32::point(input[3]),
                iv32::Iv32::point(1),
                iv32::Iv32::point(1),
            ])?;
            let composed = poly::compose_univariate(outer, candidate, d0)?;
            fold(&[
                composed.coefficients[0].lo,
                composed.coefficients[0].hi,
                composed.coefficients[1].lo,
                composed.coefficients[1].hi,
                composed.coefficients[2].lo,
                composed.coefficients[2].hi,
            ])
        }
        "interval_mul_exp" => {
            let domain = iv32::FixedDomain::full(
                i16::try_from(input[2]).map_err(|_| iv32::NumericError::Overflow)?,
            );
            let value =
                interval(input[0], input[1])?.multiply(iv32::Iv32::point(input[3]), domain)?;
            fold(&[value.lo, value.hi])
        }
        "interval_from_f32_bits" => {
            let value = iv32::Iv32::from_f32_bits(
                input[0] as u32,
                input[1].unsigned_abs(),
                iv32::FixedDomain::full(
                    i16::try_from(input[2]).map_err(|_| iv32::NumericError::Overflow)?,
                ),
            )?;
            fold(&[value.lo, value.hi])
        }
        "certify_run" => {
            let shape = match input[0] {
                0 => None,
                1 | 4 => Some(certificate::CompositionShape::Plane),
                2 => Some(certificate::CompositionShape::Sphere),
                3 => Some(certificate::CompositionShape::Torus),
                _ => return Err(iv32::NumericError::UnsupportedShape),
            };
            let make = |middle: iv32::Iv32, endpoint: i32| {
                let mut coefficients = [iv32::Iv32::point(0); poly::MAX_COEFFICIENTS];
                coefficients[..3].copy_from_slice(&[
                    iv32::Iv32::point(endpoint),
                    middle,
                    iv32::Iv32::point(endpoint),
                ]);
                poly::Bernstein {
                    coefficients,
                    degree: 2,
                }
            };
            let lower_middle = if input[0] == 4 {
                iv32::Iv32::new(-1, 1)?
            } else {
                iv32::Iv32::point(input[1])
            };
            certificate::certify_run(certificate::RunCertificateInput {
                composition_shape: shape,
                lower_coefficients: make(lower_middle, input[1]),
                upper_coefficients: make(iv32::Iv32::point(input[2]), input[2]),
                lower_face: iv32::Iv32::point(input[1]),
                upper_face: iv32::Iv32::point(input[2]),
                derivative: iv32::Iv32::point(input[3]),
                reciprocal_derivative: iv32::Iv32::point(1),
                residual: iv32::Iv32::point(0),
                correction: iv32::Iv32::new(-2, 2)?,
                domain: d0,
            })?
            .map_or(-1, |value| {
                fold_small(&[
                    certificate_method_tag(value.certificate.method),
                    composition_shape_tag(value.composition_shape),
                    value.certificate.derivative_margin.lo,
                    value.certificate.face_margin.lo,
                    value.certificate.contraction_margin.lo,
                    0,
                ])
            })
        }
        "root_config" => {
            let degree = if input[0] == 99 && input[1] == 99 {
                0
            } else {
                1
            };
            let mut coefficients = [iv32::Iv32::point(0); poly::MAX_COEFFICIENTS];
            coefficients[0] = iv32::Iv32::point(if degree == 0 { 1 } else { -1 });
            coefficients[1] = iv32::Iv32::point(1);
            let q_domain = if degree == 0 {
                iv32::Iv32::new(0, 1)?
            } else {
                iv32::Iv32::new(input[0], input[1])?
            };
            root_outcome_field(
                root::isolate_roots(
                    root::RootEvaluator::Bernstein(poly::Bernstein {
                        coefficients,
                        degree,
                    }),
                    q_domain,
                    d0,
                    1,
                    u8::try_from(input[2]).map_err(|_| iv32::NumericError::InvalidDomain)?,
                    u16::try_from(input[3]).map_err(|_| iv32::NumericError::InvalidDomain)?,
                )?,
                0,
            )
        }
        _ => return Err(iv32::NumericError::UnsupportedShape),
    })
}

pub fn vector_text() -> String {
    let mut output = deterministic_vectors()
        .iter()
        .map(NumericVector::line)
        .collect::<Vec<_>>()
        .join("\n");
    output.push('\n');
    output
}

pub fn vector_digest() -> u64 {
    vector_text()
        .bytes()
        .fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
            (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3)
        })
}

pub fn guest_module_text() -> String {
    let srgb_lut_prefix = super::reference::display::srgb_transfer_lut()
        .iter()
        .take(17)
        .map(u16::to_string)
        .collect::<Vec<_>>()
        .join(", ");
    let mut output = format!(
        r#"module render_test_vectors

# @generated by crates/wrela-compiler/src/pixels/test_vectors.rs.
from core.render_interval import FixedDomain, FixedDomainOutcome, Iv32, NumericOutcome, fixed_domain, interval_abs, interval_add, interval_affine, interval_ceil_div, interval_contains_zero, interval_convert, interval_floor_div, interval_from_f32_bits, interval_hull, interval_intersect, interval_max, interval_min, interval_mul, interval_narrow, interval_negate, interval_scale_pow2, interval_square, interval_strict_negative, interval_strict_positive, interval_sub, interval_valid
from core.render_program import BernsteinOutcome3, BernsteinSplit9Outcome, BernsteinSplitOutcome3, Polynomial9, Polynomial9Outcome, QuadraticRangeOutcome, QuadraticRangeWideOutcome, SparseTerm, bernstein_average, bernstein_compose, bernstein_from_power9, bernstein_lerp_ratio, bernstein_sign, bernstein_subdivide, bernstein_subdivide9, polynomial_compose9, polynomial_derivative9, polynomial_horner9, polynomial_multiply9, polynomial_sparse, polynomial_sparse32, polynomial_taylor, program_binomial, program_ceil_div, program_floor_div, program_gcd, quadratic_range, quadratic_range2, quadratic_range_wide
from core.render_events import RootIsolationConfig, RootOutcome8, RootSet8, root_bracket_step, root_count, root_is_identically_zero, root_isolate8, root_isolate_program8, root_isolate_taylor8
from core.render_sweep import CertificateRequest, CertifiedRunOutcome, CsgBitsOutcome, CsgEvaluateOutcome, CsgInfluenceOutcome, CsgTransitionOutcome, OrderedRoot, OrderedRootSet64, OrderedRootSetOutcome, OrderedSlackOutcome, RootCertificateOutcome, bernstein_certificate, bernstein_certificate_subdivided, bernstein_face_margin3, certify_run, csg_boundary_influences64, csg_evaluate64, csg_first_transition64, csg_oriented_toggle64, csg_transition, kinetic_slack, krawczyk_certificate, monotone_certificate, q_adjacent_slack3, q_adjacent_slack64, q_feature_before, q_order, q_sort3, q_sort64, q_storage_before, run_certificate, strict_margin
from core.render_certificate import certify_monotone_root
from core.render_raster import QRunScalar, QSetup, QStep, fixed_q_nearer, fixed_q_quantize_integer, fixed_q_recurrence_fits, fixed_q_select_integer_domain, fixed_q_setup, fixed_q_step
from core.render_coverage import CoverageOutcome, coverage_color_error, coverage_line_twice_area, coverage_quadratic, coverage_quadratic_graph, coverage_quadratic_split, coverage_quadratic_strip
from core.render_material import CenteredMoments, Iv3Outcome, MaterialPredicateOutcome, MomentOutcome, NormalCone, NormalConeOutcome, NormalRawRange, material_affine, material_affine_moments, material_affine_vector, material_footprint, material_quadratic_moments, material_residual, normal_bound, normal_cone, normal_cone_dot, normal_dot, normal_floor_sqrt, normal_norm_squared, normal_raw_add, normal_raw_dot, normal_raw_norm_squared, normal_raw_product, normal_raw_square, normal_safe, normal_vector
from core.render_transfer import Transfer, TransferOutcome, transfer_balanced2, transfer_balanced64, transfer_channel, transfer_compose, transfer_replace2, transfer_replace64, transparency_tail
from core.render_display import ByteSingletonOutcome, QuantizeOutcome, RgbSingletonOutcome, byte_singleton, color_matrix_channel, exposure_channel, monotone_lut17, quantize_ties_even, rgb_singleton

pub const VECTOR_COUNT: u64 = {}
pub const VECTOR_DIGEST: u64 = {}

pub fn fold2(a: i32, b: i32) -> i64:
    return a.to[i64]() * 257 + b.to[i64]()

pub fn fold4(a: i32, b: i32, c: i32, d: i32) -> i64:
    return ((a.to[i64]() * 257 + b.to[i64]()) * 257 + c.to[i64]()) * 257 + d.to[i64]()

pub fn fold6(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> i64:
    return ((((a.to[i64]() * 257 + b.to[i64]()) * 257 + c.to[i64]()) * 257 + d.to[i64]()) * 257 + e.to[i64]()) * 257 + f.to[i64]()

pub fn fold6_small(a: i32, b: i32, c: i32, d: i32, e: i32, f: i32) -> i64:
    return ((((a.to[i64]() * 17 + b.to[i64]()) * 17 + c.to[i64]()) * 17 + d.to[i64]()) * 17 + e.to[i64]()) * 17 + f.to[i64]()

pub fn fold12(values: [i32; 12]) -> i64:
    result: i64 = 0
    @budget(bound=12)
    for value in values:
        result = result * 17 + value.to[i64]()
    return result

pub fn bool_result(value: bool) -> i64:
    if value:
        return 1
    return 0

pub fn numeric_result(value: NumericOutcome) -> i64:
    match value:
        case .Value(interval):
            return fold2(interval.lower(), interval.upper())
        case .Empty:
            return -1
        case .Error:
            return -9223372036854775807

pub enum VectorResult:
    Value(i64)
    Error

pub fn vector_transfer_balanced(in0: i32, in1: i32, in2: i32, in3: i32) -> i64:
    identity = Transfer.make(Iv32.point(0), Iv32.point(0), Iv32.point(0), Iv32.point(256))
    layers: [Transfer; 64] = [identity; 64]
    layer_index: i32 = 0
    @budget(bound=64)
    while layer_index < 64:
        color = in0
        if layer_index % 3 == 1:
            color = in1
        elif layer_index % 3 == 2:
            color = in2
        layers[layer_index.to[usize]()] = Transfer.make(Iv32.point(color), Iv32.point(color), Iv32.point(color), Iv32.point(128 + (layer_index & 1)))
        layer_index = layer_index + 1
    count = in3
    if count < 0:
        count = -count
    if count > 61:
        count = 61
    count = count + 3
    match transfer_balanced64(take layers, count.to[u8](), 8, 0):
        case .Value(value):
            return fold4(value.red_lower(), value.red_upper(), value.transmittance_lower(), value.transmittance_upper())
        case .Error:
            return -9223372036854775807

pub fn vector_transfer_replace(in0: i32, in1: i32, in2: i32, in3: i32) -> i64:
    identity = Transfer.make(Iv32.point(0), Iv32.point(0), Iv32.point(0), Iv32.point(256))
    layers: [Transfer; 64] = [identity; 64]
    layer_index: i32 = 0
    @budget(bound=64)
    while layer_index < 64:
        color = in0
        if layer_index % 3 == 1:
            color = in1
        elif layer_index % 3 == 2:
            color = in2
        layers[layer_index.to[usize]()] = Transfer.make(Iv32.point(color), Iv32.point(color), Iv32.point(color), Iv32.point(128 + (layer_index & 1)))
        layer_index = layer_index + 1
    count = in3
    if count < 0:
        count = -count
    if count > 61:
        count = 61
    count = count + 3
    replacement = Transfer.make(Iv32.point(in2), Iv32.point(in2), Iv32.point(in2), Iv32.point(192))
    replace_index = in0
    if replace_index < 0:
        replace_index = -replace_index
    replace_index = replace_index % count
    match transfer_replace64(take layers, count.to[u8](), replace_index.to[u8](), replacement, 8, 0):
        case .Value(value):
            return fold4(value.red_lower(), value.red_upper(), value.transmittance_lower(), value.transmittance_upper())
        case .Error:
            return -9223372036854775807

pub fn vector_order_roots(in0: i32, count_input: i32) -> [OrderedRoot; 64]:
    roots: [OrderedRoot; 64] = [OrderedRoot.make(Iv32.point(0), 0, 1); 64]
    count = count_input
    if count < 0:
        count = -count
    if count > 61:
        count = 61
    count = count + 3
    if in0 == 1000 or in0 == 1001:
        a = OrderedRoot.make(Iv32.range(0, 2), 0, 1)
        b = OrderedRoot.make(Iv32.range(1, 10), 1, -1)
        c = OrderedRoot.make(Iv32.range(9, 11), 2, 1)
        far = OrderedRoot.make(Iv32.range(-4, -3), 3, 1)
        if in0 == 1000:
            roots[0] = a
            roots[1] = b
            roots[2] = c
            roots[3] = far
        else:
            roots[0] = far
            roots[1] = c
            roots[2] = b
            roots[3] = a
        return roots
    root_index: i32 = 0
    @budget(bound=64)
    while root_index < count:
        orientation: i32 = 1
        if root_index & 1 != 0:
            orientation = -1
        roots[root_index.to[usize]()] = OrderedRoot.make(
            Iv32.point(in0 - (root_index / 2) * 4),
            (count - root_index).to[u32](),
            orientation,
        )
        root_index = root_index + 1
    return roots

pub fn vector_q_sort(in0: i32, in3: i32) -> i64:
    count = in3
    if count < 0:
        count = -count
    if count > 61:
        count = 61
    count = count + 3
    roots = vector_order_roots(in0, in3)
    match q_sort64(roots, count.to[u8]()):
        case .Value(values):
            first = values.root(0)
            second = values.root(1)
            third = values.root(2)
            return fold12([
                first.q_value().lower(), first.q_value().upper(), first.feature().to[i32](), first.orientation_value(),
                second.q_value().lower(), second.q_value().upper(), second.feature().to[i32](), second.orientation_value(),
                third.q_value().lower(), third.q_value().upper(), third.feature().to[i32](), third.orientation_value(),
            ])
        case .Error:
            return -9223372036854775807

pub fn vector_q_slack(in0: i32, in3: i32) -> i64:
    count = in3
    if count < 0:
        count = -count
    if count > 61:
        count = 61
    count = count + 3
    roots = vector_order_roots(in0, in3)
    match q_sort64(roots, count.to[u8]()):
        case .Value(values):
            sorted: [OrderedRoot; 64] = roots
            sorted_index: i32 = 0
            @budget(bound=64)
            while sorted_index < count:
                sorted[sorted_index.to[usize]()] = values.root(sorted_index.to[usize]())
                sorted_index = sorted_index + 1
            match q_adjacent_slack64(sorted, count.to[u8]()):
                case .Value(slack):
                    return slack.to[i64]()
                case .Error:
                    return -9223372036854775807
        case .Error:
            return -9223372036854775807

pub fn vector_root_outcome_field(outcome: RootOutcome8, requested_selector: i32) -> i64:
    selector = requested_selector
    if selector < 0:
        selector = -selector
    if selector > 48:
        selector = 48
    match outcome:
        case .CertifiedNone:
            return -100
        case .Unresolved(reason):
            return -reason.to[i64]()
        case .Error:
            return -9223372036854775807
        case .Roots(roots):
            count = roots.root_count_value().to[i32]()
            if selector == 0:
                return count.to[i64]()
            root_index = (selector - 1) / 3
            if root_index >= count:
                return 0
            field = (selector - 1) % 3
            if field == 0:
                return roots.root_low(root_index.to[usize]()).to[i64]()
            if field == 1:
                return roots.root_high(root_index.to[usize]()).to[i64]()
            return bool_result(roots.root_unique(root_index.to[usize]()))

pub fn vector_result_raw_low(kernel: i32, in0: i32, in1: i32, in2: i32, in3: i32) -> i64:
    if kernel == 0:
        coefficients = [Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in0), Iv32.point(in1), Iv32.point(in2)]
        slot = in3
        if slot < 0:
            slot = -slot
        if slot > 8:
            slot = 8
        match bernstein_from_power9(coefficients, 8, FixedDomain.full(0)):
            case .Value(value):
                coefficient = value.coefficient(slot.to[usize]())
                return fold2(coefficient.lower(), coefficient.upper())
            case .Error:
                return -9223372036854775807
    if kernel == 1:
        return bernstein_sign([Iv32.point(in0), Iv32.point(in1), Iv32.point(in2)]).to[i64]()
    if kernel == 2:
        coefficients = [Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in0), Iv32.point(in1), Iv32.point(in2)]
        selector = in3
        if selector < 0:
            selector = -selector
        if selector > 17:
            selector = 17
        match bernstein_subdivide9(coefficients, 8, FixedDomain.full(0)):
            case .Value(value):
                if selector < 9:
                    coefficient = value.left_coefficient(selector.to[usize]())
                    return fold2(coefficient.lower(), coefficient.upper())
                coefficient = value.right_coefficient((selector - 9).to[usize]())
                return fold2(coefficient.lower(), coefficient.upper())
            case .Error:
                return -9223372036854775807
    if kernel == 3:
        match byte_singleton(Iv32.range(in0, in1), 2):
            case .Value(code):
                return code.to[i64]()
            case .NotSingleton:
                return -1
            case .Error:
                return -9223372036854775807
    if kernel == 4:
        match coverage_line_twice_area(in0, in1, in2):
            case .Value(value):
                return fold2(value.lower(), value.upper())
            case .Error:
                return -9223372036854775807
    if kernel == 5:
        match coverage_quadratic_strip(in0, in1, in2, in3):
            case .Value(value):
                return fold2(value.lower(), value.upper())
            case .Error:
                return -9223372036854775807
    if kernel == 6:
        program: [i32; 64] = [0; 64]
        program[0] = 0
        program[1] = 1
        program[2] = 64
        bits_raw = in0 % 4
        if bits_raw < 0:
            bits_raw = bits_raw + 4
        bits = bits_raw.to[u64]()
        objects: [i32; 64] = [0; 64]
        orientations: [i32; 64] = [0; 64]
        object0 = in1
        if object0 < 0:
            object0 = -object0
        if object0 > 2:
            object0 = 2
        object1 = in2
        if object1 < 0:
            object1 = -object1
        if object1 > 2:
            object1 = 2
        objects[0] = object0
        objects[1] = object1
        mask0 = 1.to[u64]() << object0.to[u64]()
        if bits & mask0 != 0:
            orientations[0] = 1
        next_bits = bits ^ mask0
        mask1 = 1.to[u64]() << object1.to[u64]()
        if next_bits & mask1 != 0:
            orientations[1] = 1
        count = in3
        if count < 0:
            count = -count
        if count > 2:
            count = 2
        program_count: u8 = 3
        if in0 == -2147483648:
            program_count = 0
        match csg_first_transition64(program, program_count, bits, objects, orientations, count.to[u8]()):
            case .Value(index):
                return index.to[i64]()
            case .None:
                return -1
            case .Error:
                return -9223372036854775807
    if kernel == 7:
        match fixed_q_step(QRunScalar.make(in0, in1, in2, in3)):
            case .Value(value):
                return fold2(value.value(), value.delta())
            case .Error:
                return -9223372036854775807
    domain = FixedDomain.full(0)
    if kernel == 8:
        match interval_add(Iv32.range(in0, in1), Iv32.range(in2, in3), domain):
            case .Value(value):
                return fold2(value.lower(), value.upper())
            case _:
                return -9223372036854775807
    if kernel == 9:
        match interval_intersect(Iv32.range(in0, in1), Iv32.range(in2, in3), domain):
            case .Value(value):
                return fold2(value.lower(), value.upper())
            case .Empty:
                return -1
            case .Error:
                return -9223372036854775807
    if kernel == 10:
        match interval_mul(Iv32.range(in0, in1), Iv32.range(in2, in3), domain):
            case .Value(value):
                return fold2(value.lower(), value.upper())
            case _:
                return -9223372036854775807
    if kernel == 11:
        match interval_sub(Iv32.range(in0, in1), Iv32.range(in2, in3), domain):
            case .Value(value):
                return fold2(value.lower(), value.upper())
            case _:
                return -9223372036854775807
    if kernel == 12:
        return bool_result(kinetic_slack(in0, in1))
    if kernel == 13:
        match interval_mul(Iv32.point(in0), Iv32.point(in3), domain):
            case .Value(uqu):
                match normal_bound(Iv32.point(in2), Iv32.point(in3), Iv32.point(0), uqu, Iv32.point(0)):
                    case .Value(value):
                        return fold6(value.x_lower(), value.x_upper(), value.y_lower(), value.y_upper(), value.z_lower(), value.z_upper())
                    case .Error:
                        return -9223372036854775807
            case _:
                return -9223372036854775807
    if kernel == 14:
        return bool_result(q_order(Iv32.range(in0, in1), Iv32.range(in2, in3)))
    if kernel == 15:
        match quadratic_range2(in0, in1, in2, in3, in2, in3):
            case .Value(value):
                return fold2(value[0], value[1])
            case .Error:
                return -9223372036854775807
    if kernel == 16:
        value = root_bracket_step(in0, in1, in2 < 0, in3 < 0)
        return fold2(value.lower(), value.upper())
    if kernel == 17:
        return root_count(in0, in1, in2).to[i64]()
    if kernel == 18:
        return bool_result(run_certificate(Iv32.point(in0), Iv32.point(in1), Iv32.point(in2)))
    if kernel == 19:
        front = Transfer.make(Iv32.range(in0, in1), Iv32.range(in0, in1), Iv32.range(in0, in1), Iv32.range(in2, in3))
        back = Transfer.make(Iv32.range(in2, in3), Iv32.range(in2, in3), Iv32.range(in2, in3), Iv32.range(in0, in1))
        match transfer_compose(front, back, 8, 0):
            case .Value(value):
                return fold4(value.red_lower(), value.red_upper(), value.transmittance_lower(), value.transmittance_upper())
            case .Error:
                return -9223372036854775807
    if kernel == 20:
        return bool_result(transparency_tail(in0, in1, in2))
    if kernel == 21:
        return numeric_result(interval_negate(Iv32.range(in0, in1), domain))
    if kernel == 22:
        return numeric_result(interval_square(Iv32.range(in0, in1), domain))
    return -9223372036854775807

pub fn vector_result_raw_mid(kernel: i32, in0: i32, in1: i32, in2: i32, in3: i32) -> i64:
    domain = FixedDomain.full(0)
    if kernel == 23:
        return numeric_result(interval_scale_pow2(Iv32.range(in0, in1), in2, domain))
    if kernel == 24:
        return numeric_result(interval_hull(Iv32.range(in0, in1), Iv32.range(in2, in3), domain))
    if kernel == 25:
        return numeric_result(interval_min(Iv32.range(in0, in1), Iv32.range(in2, in3), domain))
    if kernel == 26:
        return numeric_result(interval_max(Iv32.range(in0, in1), Iv32.range(in2, in3), domain))
    if kernel == 27:
        return numeric_result(interval_abs(Iv32.range(in0, in1), domain))
    if kernel == 28:
        return numeric_result(interval_affine(Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), domain))
    if kernel == 29:
        if in0 == 10 and in1 == 10 and in2 == 30 and in3 == 100:
            match fixed_domain(-2, 30, 100):
                case .Value(destination):
                    return numeric_result(interval_convert(Iv32.range(in0, in1), domain, destination))
                case .Error:
                    return -9223372036854775807
        exponent = in2
        if exponent < -8:
            exponent = -8
        if exponent > 8:
            exponent = 8
        return numeric_result(interval_convert(Iv32.range(in0, in1), domain, FixedDomain.full(exponent)))
    if kernel == 30:
        return numeric_result(bernstein_average(Iv32.range(in0, in1), Iv32.range(in2, in3), domain))
    if kernel == 31:
        value = coverage_quadratic_split(in0, in1, in2)
        return fold2(value.lower(), value.upper())
    if kernel == 32:
        strip = in3
        if strip < 0:
            strip = -strip
        if strip > 8:
            strip = 8
        match coverage_quadratic_graph(in0, in1, in2, strip, true, in3 & 1 == 0):
            case .Value(value):
                return fold2(value.lower(), value.upper())
            case .Error:
                return -9223372036854775807
    if kernel == 33:
        match bernstein_certificate(
            [Iv32.point(in0), Iv32.point(in0), Iv32.point(in1)],
            [Iv32.point(in3), Iv32.point(in3), Iv32.point(in3)],
            Iv32.point(in2),
        ):
            case .Value(value):
                return fold6_small(value.method_tag(), value.derivative_margin_value(), value.face_margin_value(), value.contraction_margin_value(), 0, 0)
            case .Rejected:
                return 0
            case .Error:
                return -9223372036854775807
    if kernel == 34:
        reciprocal = Iv32.point(in0)
        residual = Iv32.point(in1)
        derivative = Iv32.point(in2)
        correction_lo = in0
        correction_hi = in3
        if in0 == -2147483648 and in1 == 2147483647 and in2 == 1 and in3 == 0:
            reciprocal = Iv32.point(1)
            residual = Iv32.point(0)
            derivative = Iv32.point(1)
            correction_lo = -2147483648
            correction_hi = 2147483647
        if correction_lo > correction_hi:
            old = correction_lo
            correction_lo = correction_hi
            correction_hi = old
        match krawczyk_certificate(reciprocal, residual, derivative, Iv32.range(correction_lo, correction_hi), Iv32.point(1), domain):
            case .Value(value):
                return fold6_small(value.method_tag(), value.derivative_margin_value(), value.face_margin_value(), value.contraction_margin_value(), 0, 0)
            case .Rejected:
                return 0
            case .Error:
                return -9223372036854775807
    if kernel == 35:
        match interval_mul(Iv32.point(in0), Iv32.point(in3), domain):
            case .Value(uqu):
                match normal_bound(Iv32.point(in2), Iv32.point(in3), Iv32.point(0), uqu, Iv32.point(0)):
                    case .Value(value):
                        return bool_result(normal_safe(value, domain))
                    case .Error:
                        return -9223372036854775807
            case _:
                return -9223372036854775807
    if kernel == 36:
        match interval_mul(Iv32.point(in0), Iv32.point(in3), domain):
            case .Value(uqu):
                match normal_bound(Iv32.point(in2), Iv32.point(in3), Iv32.point(0), uqu, Iv32.point(0)):
                    case .Value(value):
                        return numeric_result(normal_dot(value, value, domain))
                    case .Error:
                        return -9223372036854775807
            case _:
                return -9223372036854775807
    if kernel == 37:
        return numeric_result(material_affine(Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in3), Iv32.point(in3), domain))
    if kernel == 38:
        match material_residual(Iv32.range(in0, in1), in2, in3, domain):
            case .Value(value):
                return bool_result(value)
            case .Error:
                return -9223372036854775807
    if kernel == 39:
        return numeric_result(exposure_channel(Iv32.point(in0), Iv32.point(in1), domain))
    if kernel == 40:
        return numeric_result(color_matrix_channel(Iv32.point(in0), Iv32.point(in0), Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in3), domain))
    if kernel == 41:
        raw = in0
        if raw < 0:
            raw = -raw
        if raw > 64:
            raw = 64
        return numeric_result(monotone_lut17(Iv32.point(raw), [{}], 2))
    if kernel == 42:
        match quantize_ties_even(in0, 2):
            case .Value(code):
                return code.to[i64]()
            case .Error:
                return -9223372036854775807
    if kernel == 43:
        return bool_result(root_is_identically_zero([Iv32.point(in0), Iv32.point(in1), Iv32.point(in2)]))
    if kernel == 44:
        requested = in3
        if requested < 0:
            requested = -requested
        if requested > 64:
            requested = 64
        match fixed_q_setup(in0, in1, in2, requested.to[u8](), 0):
            case .Value(run):
                return fold6_small(run.value(), run.delta(), run.second_delta(), run.width().to[i32](), run.exponent(), run.error())
            case .Error:
                return -9223372036854775807
    if kernel == 45:
        a = normal_vector(Iv32.point(in0), Iv32.point(in1), Iv32.point(in2))
        b = normal_vector(Iv32.point(in0 * in3), Iv32.point(in1 * in3), Iv32.point(in2 * in3))
        return numeric_result(normal_cone_dot(a, b, domain))
    return -9223372036854775807

pub fn vector_result_raw_high(kernel: i32, in0: i32, in1: i32, in2: i32, in3: i32) -> i64:
    domain = FixedDomain.full(0)
    if kernel == 46:
        terms: [SparseTerm; 32] = [SparseTerm.make(Iv32.point(0), [0; 4]); 32]
        term_index: i32 = 0
        @budget(bound=32)
        while term_index < 32:
            coefficient = in0
            if term_index % 2 != 0:
                coefficient = in1
            terms[term_index.to[usize]()] = SparseTerm.make(
                Iv32.point(coefficient),
                [
                    (term_index & 1).to[u8](),
                    ((term_index >> 1) & 1).to[u8](),
                    ((term_index >> 2) & 1).to[u8](),
                    ((term_index >> 3) & 1).to[u8](),
                ],
            )
            term_index = term_index + 1
        return numeric_result(polynomial_sparse32(terms, 32, [Iv32.point(in2), Iv32.point(1), Iv32.point(-1), Iv32.point(in3)], domain))
    if kernel == 47:
        radius = in3
        if radius < 0:
            radius = -radius
        return numeric_result(polynomial_taylor(in0, in1, in2, radius, domain))
    if kernel == 48:
        return vector_transfer_balanced(in0, in1, in2, in3)
    if kernel == 49:
        return vector_transfer_replace(in0, in1, in2, in3)
    if kernel == 50:
        match material_affine_moments(Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in3), domain):
            case .Value(value):
                mean = value.mean_value()
                first = value.first_value()
                second = value.second_value()
                return fold6(mean.lower(), mean.upper(), first.lower(), first.upper(), second.lower(), second.upper())
            case .Error:
                return -9223372036854775807
    if kernel == 51:
        match material_quadratic_moments(Iv32.point(in0), Iv32.point(in1), Iv32.point(in1), Iv32.point(in2), Iv32.point(in3), Iv32.range(-1, 2), domain):
            case .Value(value):
                mean = value.mean_value()
                first = value.first_value()
                second = value.second_value()
                return fold6(mean.lower(), mean.upper(), first.lower(), first.upper(), second.lower(), second.upper())
            case .Error:
                return -9223372036854775807
    if kernel == 52:
        return vector_q_sort(in0, in3)
    if kernel == 53:
        return vector_q_slack(in0, in3)
    if kernel == 54:
        program: [i32; 64] = [0; 64]
        program[0] = 0
        program[1] = 1
        operation = in2 % 3
        if operation < 0:
            operation = operation + 3
        program[2] = 64 + operation
        bits: u64 = 0
        if in0 % 2 != 0:
            bits = bits | 1
        if in1 % 2 != 0:
            bits = bits | 2
        count: u8 = 3
        if in0 == -2147483648:
            count = 0
        match csg_evaluate64(program, count, bits):
            case .Value(value):
                return bool_result(value)
            case .Error:
                return -9223372036854775807
    if kernel == 55:
        shape = in0 % 14
        if shape < 0:
            shape = shape + 14
        tolerance = in2
        if tolerance < 0:
            tolerance = -tolerance
        tolerance = tolerance + 1
        power: [Iv32; 9] = [Iv32.point(0); 9]
        degree: u8 = 1
        if shape == 0:
            power[0] = Iv32.point(-1)
            power[1] = Iv32.point(2)
        elif shape == 1:
            degree = 2
            power[0] = Iv32.point(1)
            power[1] = Iv32.point(-2)
            power[2] = Iv32.point(1)
        elif shape == 2:
            degree = 3
            power[0] = Iv32.point(-1)
            power[1] = Iv32.point(1)
            power[3] = Iv32.point(1)
        elif shape == 3:
            degree = 4
            power[0] = Iv32.point(24)
            power[1] = Iv32.point(-250)
            power[2] = Iv32.point(875)
            power[3] = Iv32.point(-1250)
            power[4] = Iv32.point(625)
        elif shape == 4:
            degree = 5
            power[0] = Iv32.point(-1)
            power[1] = Iv32.point(2)
            power[5] = Iv32.point(1)
        elif shape == 5:
            degree = 6
            power[0] = Iv32.point(24)
            power[1] = Iv32.point(-250)
            power[2] = Iv32.point(875)
            power[3] = Iv32.point(-1250)
            power[4] = Iv32.point(625)
            power[6] = Iv32.point(1)
        elif shape == 6:
            degree = 7
            power[0] = Iv32.point(1)
            power[1] = Iv32.point(-2)
            power[2] = Iv32.point(1)
            power[7] = Iv32.point(1)
        elif shape == 7:
            degree = 8
            power[0] = Iv32.point(24)
            power[1] = Iv32.point(-250)
            power[2] = Iv32.point(875)
            power[3] = Iv32.point(-1250)
            power[4] = Iv32.point(625)
            power[8] = Iv32.point(1)
        elif shape == 8:
            power[0] = Iv32.point(-1)
            power[1] = Iv32.point(1)
            return vector_root_outcome_field(root_isolate_taylor8(power, 1, Iv32.point(0), Iv32.range(0, 2), domain, tolerance.to[u32](), 12, 16), in1)
        elif shape == 9:
            power[0] = Iv32.point(5)
            return vector_root_outcome_field(root_isolate_taylor8(power, 0, Iv32.range(-1, 1), Iv32.range(0, 2), domain, tolerance.to[u32](), 12, 16), in1)
        elif shape == 10:
            power[0] = Iv32.point(0)
            power[1] = Iv32.point(2147483647)
            return vector_root_outcome_field(root_isolate_taylor8(power, 1, Iv32.point(0), Iv32.range(2147483646, 2147483647), domain, tolerance.to[u32](), 12, 16), in1)
        elif shape == 11:
            coefficients = [Iv32.point(0); 9]
            coefficients[0] = Iv32.point(3)
            coefficients[1] = Iv32.point(-5)
            coefficients[2] = Iv32.point(3)
            return vector_root_outcome_field(root_isolate8(coefficients, 2, Iv32.range(0, 1), domain, tolerance.to[u32](), 12, 16), in1)
        elif shape == 12:
            coefficients = [Iv32.point(0); 9]
            coefficients[0] = Iv32.point(0)
            coefficients[1] = Iv32.point(-1)
            coefficients[2] = Iv32.point(1)
            return vector_root_outcome_field(root_isolate8(coefficients, 2, Iv32.range(0, 1), domain, tolerance.to[u32](), 12, 16), in1)
        else:
            coefficients = [Iv32.point(0); 9]
            coefficients[0] = Iv32.point(-2)
            coefficients[1] = Iv32.point(3)
            return vector_root_outcome_field(root_isolate8(coefficients, 1, Iv32.range(0, 3), domain, tolerance.to[u32](), 12, 16), in1)
        match bernstein_from_power9(power, degree, domain):
            case .Value(polynomial):
                coefficients: [Iv32; 9] = [Iv32.point(0); 9]
                coefficient_index: i32 = 0
                @budget(bound=9)
                while coefficient_index <= degree.to[i32]():
                    coefficients[coefficient_index.to[usize]()] = polynomial.coefficient(coefficient_index.to[usize]())
                    coefficient_index = coefficient_index + 1
                return vector_root_outcome_field(root_isolate8(coefficients, degree, Iv32.range(0, 256), domain, tolerance.to[u32](), 12, 16), in1)
            case .Error:
                return -9223372036854775807
    if kernel == 56:
        bits_raw = in0 % 8
        if bits_raw < 0:
            bits_raw = bits_raw + 8
        bits = bits_raw.to[u64]()
        object = in1
        if object < 0:
            object = -object
        if object > 2:
            object = 2
        orientation = in2 & 1
        match csg_oriented_toggle64(bits, object, orientation):
            case .Value(value):
                return value.to[i64]()
            case .Error:
                return -9223372036854775807
    if kernel == 57:
        program: [i32; 64] = [0; 64]
        program[0] = 0
        program[1] = 1
        program[2] = 64
        object = in1
        if object < 0:
            object = -object
        if object > 2:
            object = 2
        bits_raw = in0 % 4
        if bits_raw < 0:
            bits_raw = bits_raw + 4
        count: u8 = 3
        if in0 == -2147483648:
            count = 0
        match csg_boundary_influences64(program, count, bits_raw.to[u64](), object):
            case .Value(value):
                return bool_result(value)
            case .Error:
                return -9223372036854775807
    if kernel == 58:
        depth = in3
        if depth < 0 or depth > 255:
            return -9223372036854775807
        match bernstein_certificate_subdivided(
            [Iv32.point(-4), Iv32.point(in0), Iv32.point(-4)],
            [Iv32.point(4), Iv32.point(in1), Iv32.point(4)],
            Iv32.point(in2),
            domain,
            depth.to[u8](),
        ):
            case .Value(value):
                return fold6_small(value.method_tag(), value.derivative_margin_value(), value.face_margin_value(), value.contraction_margin_value(), 0, 0)
            case .Rejected:
                return 0
            case .Error:
                return -9223372036854775807
    if kernel == 59:
        return bool_result(fixed_q_nearer(in0, in1, in2, in3))
    if kernel == 60:
        return numeric_result(coverage_color_error(Iv32.range(in0, in1), Iv32.range(in2, in3), domain))
    if kernel == 61:
        return numeric_result(material_footprint(Iv32.range(in0, in1), Iv32.range(in2, in3), domain))
    if kernel == 62:
        center = normal_vector(Iv32.point(in0), Iv32.point(in0), Iv32.point(in0))
        du = normal_vector(Iv32.point(in1), Iv32.point(in1), Iv32.point(in1))
        dv = normal_vector(Iv32.point(in2), Iv32.point(in2), Iv32.point(in2))
        match material_affine_vector(center, du, dv, Iv32.point(in3), Iv32.point(0), domain):
            case .Value(value):
                return fold6(value.x_lower(), value.x_upper(), value.y_lower(), value.y_upper(), value.z_lower(), value.z_upper())
            case .Error:
                return -9223372036854775807
    if kernel == 63:
        match normal_cone(Iv32.point(in0), Iv32.point(0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in3), domain):
            case .Value(value):
                direction = value.direction_value()
                norm = value.norm_value()
                return fold12([direction.x_lower(), direction.x_upper(), direction.y_lower(), direction.y_upper(), direction.z_lower(), direction.z_upper(), norm.lower(), norm.upper(), 0, 0, 0, 0])
            case .Rejected:
                return -1
            case .Error:
                return -9223372036854775807
    if kernel == 64:
        coefficients = [Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in0), Iv32.point(in1), Iv32.point(in2)]
        return numeric_result(polynomial_horner9(coefficients, 8, Iv32.point(in3), domain))
    if kernel == 65:
        coefficients = [Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(in0), Iv32.point(in1), Iv32.point(in2)]
        slot = in3
        if slot < 0:
            slot = -slot
        if slot > 7:
            slot = 7
        match polynomial_derivative9(coefficients, 8, domain):
            case .Value(value):
                coefficient = value.coefficient(slot.to[usize]())
                return fold2(coefficient.lower(), coefficient.upper())
            case .Error:
                return -9223372036854775807
    if kernel == 66:
        outer = [Iv32.point(in0), Iv32.point(in1), Iv32.point(in2), Iv32.point(0), Iv32.point(0), Iv32.point(0), Iv32.point(0), Iv32.point(0), Iv32.point(0)]
        candidate = [Iv32.point(in3), Iv32.point(1), Iv32.point(1), Iv32.point(0), Iv32.point(0), Iv32.point(0), Iv32.point(0), Iv32.point(0), Iv32.point(0)]
        match polynomial_compose9(outer, 2, candidate, 2, domain):
            case .Value(value):
                first = value.coefficient(0)
                second = value.coefficient(1)
                third = value.coefficient(2)
                return fold6(first.lower(), first.upper(), second.lower(), second.upper(), third.lower(), third.upper())
            case .Error:
                return -9223372036854775807
    if kernel == 67:
        return numeric_result(interval_mul(Iv32.range(in0, in1), Iv32.point(in3), FixedDomain.full(in2)))
    if kernel == 68:
        radius = in1
        if radius < 0:
            radius = -radius
        bits = in0.to[i64]()
        if bits < 0:
            bits = bits + 4294967296
        return numeric_result(interval_from_f32_bits(bits.to[u32](), radius.to[u32](), FixedDomain.full(in2)))
    if kernel == 69:
        return bool_result(interval_contains_zero(Iv32.range(in0, in1)))
    if kernel == 70:
        return bool_result(interval_strict_positive(Iv32.range(in0, in1)))
    if kernel == 71:
        return bool_result(interval_strict_negative(Iv32.range(in0, in1)))
    if kernel == 72:
        match fixed_domain(0, in2, in3):
            case .Value(restricted):
                return numeric_result(interval_add(Iv32.point(in0), Iv32.point(in1), restricted))
            case .Error:
                return -9223372036854775807
    if kernel == 73:
        color = [Iv32.range(in0, in1), Iv32.point(in2), Iv32.point(in2)]
        match rgb_singleton(color, in3):
            case .Value(codes):
                return (codes[0].to[i64]() * 257 + codes[1].to[i64]()) * 257 + codes[2].to[i64]()
            case .NotSingleton:
                return -1
            case .Error:
                return -9223372036854775807
    if kernel == 74:
        return numeric_result(bernstein_lerp_ratio(Iv32.point(in0), Iv32.point(in1), in2.to[i64](), in3.to[i64](), FixedDomain.full(0)))
    if kernel == 75:
        match fixed_domain(in0, in1, in2):
            case .Value(value):
                return bool_result(value.contains_raw(in3))
            case .Error:
                return -9223372036854775807
    if kernel == 76:
        shape = in0
        lower_middle = Iv32.point(in1)
        if shape == 4:
            shape = 1
            lower_middle = Iv32.range(-1, 1)
        lower = [Iv32.point(in1), lower_middle, Iv32.point(in1)]
        upper = [Iv32.point(in2), Iv32.point(in2), Iv32.point(in2)]
        request = CertificateRequest(composition_shape=shape, lower_coefficients=lower, upper_coefficients=upper, lower_face=Iv32.point(in1), upper_face=Iv32.point(in2), derivative=Iv32.point(in3), reciprocal_derivative=Iv32.point(1), residual=Iv32.point(0), correction=Iv32.range(-2, 2), one=Iv32.point(1), domain=domain)
        match certify_run(request):
            case .Value(value):
                return fold6_small(value.method_tag(), value.composition_shape_tag(), value.derivative_margin_value(), value.face_margin_value(), value.contraction_margin_value(), 0)
            case .Rejected:
                return -1
            case .Error:
                return -9223372036854775807
    if kernel == 77:
        coefficients: [Iv32; 9] = [Iv32.point(0); 9]
        degree: u8 = 1
        q_lo = in0
        q_hi = in1
        if in0 == 99 and in1 == 99:
            degree = 0
            q_lo = 0
            q_hi = 1
            coefficients[0] = Iv32.point(1)
        else:
            coefficients[0] = Iv32.point(-1)
            coefficients[1] = Iv32.point(1)
        if in2 < 0 or in2 > 255 or in3 < 0 or in3 > 255:
            return -9223372036854775807
        return vector_root_outcome_field(root_isolate8(coefficients, degree, Iv32.range(q_lo, q_hi), domain, 1, in2.to[u8](), in3.to[u8]()), 0)
    return -9223372036854775807

pub fn vector_result_raw(kernel: i32, in0: i32, in1: i32, in2: i32, in3: i32) -> i64:
    if kernel <= 22:
        return vector_result_raw_low(kernel, in0, in1, in2, in3)
    if kernel <= 45:
        return vector_result_raw_mid(kernel, in0, in1, in2, in3)
    return vector_result_raw_high(kernel, in0, in1, in2, in3)

pub fn vector_result(kernel: i32, in0: i32, in1: i32, in2: i32, in3: i32) -> VectorResult:
    raw = vector_result_raw(kernel, in0, in1, in2, in3)
    if raw == -9223372036854775807:
        return .Error
    return .Value(raw)

pub fn vector_case_matches(kernel: i32, in0: i32, in1: i32, in2: i32, in3: i32, expected: i64, expect_error: bool) -> bool:
    match vector_result(kernel, in0, in1, in2, in3):
        case .Value(actual):
            return not expect_error and actual == expected
        case .Error:
            return expect_error
"#,
        deterministic_vectors().len(),
        vector_digest(),
        srgb_lut_prefix,
    );
    let vectors = deterministic_vectors();
    for (chunk_index, chunk) in vectors.chunks(96).enumerate() {
        output.push_str(&format!("\npub fn run_vector_chunk_{chunk_index}():\n"));
        for vector in chunk {
            let kernel = REQUIRED_KERNELS
                .iter()
                .position(|kernel| *kernel == vector.kernel)
                .expect("required kernel");
            output.push_str(&format!(
                "    assert vector_case_matches({kernel}, {}, {}, {}, {}, {}, {}), \"{} case {}\"\n",
                vector.fields["in0"],
                vector.fields["in1"],
                vector.fields["in2"],
                vector.fields["in3"],
                vector.fields["out0"],
                if vector.fields["flags"] == 1 {
                    "true"
                } else {
                    "false"
                },
                vector.kernel,
                vector.case_id,
            ));
        }
    }
    output.push_str("\npub fn run_all_vectors():\n");
    for chunk_index in 0..vectors.len().div_ceil(96) {
        output.push_str(&format!("    run_vector_chunk_{chunk_index}()\n"));
    }
    output
}

pub fn validate_vectors(vectors: &[NumericVector]) -> Result<(), String> {
    let mut previous: Option<(&str, u32)> = None;
    let mut families = BTreeMap::<&str, BTreeSet<VectorFamily>>::new();
    for vector in vectors {
        let key = (vector.kernel.as_str(), vector.case_id);
        if previous.is_some_and(|previous| previous >= key) {
            return Err("pixels vector: order must be stable by kernel then case".to_string());
        }
        previous = Some(key);
        families
            .entry(vector.kernel.as_str())
            .or_default()
            .insert(vector.family);
    }
    for kernel in REQUIRED_KERNELS {
        let present = families
            .get(kernel)
            .ok_or_else(|| format!("pixels vector: missing kernel `{kernel}`"))?;
        if !present.contains(&VectorFamily::Edge) || !present.contains(&VectorFamily::Generated) {
            return Err(format!(
                "pixels vector: `{kernel}` requires edge and generated families"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parser_is_strict_and_fails_closed() {
        for (line, fragment) in [
            ("unknown case=0 family=0 in0=0 out0=0", "unknown kernel"),
            (
                "interval_add case=0 family=0 in0=0 out0=0 nope=1",
                "unknown key",
            ),
            (
                "interval_add case=0 family=0 in0=0 in0=1 out0=0",
                "duplicate key",
            ),
            ("interval_add case=0 family=0 in0=x out0=0", "malformed"),
            (
                "interval_add case=0 family=0 in0=999999999999999999999 out0=0",
                "overflow",
            ),
        ] {
            assert!(parse_line(line).unwrap_err().contains(fragment));
        }
    }

    #[test]
    fn generation_is_stable_complete_and_round_trips() {
        let vectors = deterministic_vectors();
        validate_vectors(&vectors).unwrap();
        let parsed = vector_text()
            .lines()
            .map(parse_line)
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        assert_eq!(parsed, vectors);
        assert_eq!(vector_digest(), vector_digest());
        assert_eq!(
            guest_module_text()
                .lines()
                .filter(|line| { line.trim_start().starts_with("assert vector_case_matches(") })
                .count(),
            vectors.len()
        );
    }

    #[test]
    fn no_float_text_is_present() {
        assert!(!vector_text().contains('.'));
        assert!(!vector_text().contains("e+"));
    }

    #[test]
    fn boundary_vectors_pin_outward_rounding_and_explicit_failure() {
        let vectors = deterministic_vectors();
        let field = |kernel: &str, case_id: u32, name: &str| {
            vectors
                .iter()
                .find(|vector| {
                    vector.kernel == kernel
                        && vector.case_id == case_id
                        && vector.family == VectorFamily::Edge
                })
                .and_then(|vector| vector.fields.get(name))
                .copied()
                .unwrap_or_else(|| panic!("missing {name} for {kernel} edge case {case_id}"))
        };
        assert_ne!(
            field("bernstein_compose", 1, "out0"),
            fold(&[-1, -1, 0, 0, -1, -1]),
            "odd signed averages must not be silently truncated"
        );
        assert_ne!(
            field("bernstein_subdivide", 1, "out0"),
            0,
            "odd signed subdivision endpoints must retain outward width"
        );
        assert_eq!(
            field("transfer_compose", 2, "flags"),
            1,
            "machine overflow must be an explicit guest-visible error"
        );
        assert_eq!(
            field("transfer_compose", 2, "out0"),
            0,
            "error payload must be canonical"
        );
        assert_eq!(
            field("transfer_compose", 0, "flags"),
            0,
            "signed, zero-crossing interval products must be supported"
        );
        assert_eq!(
            field("interval_from_f32_bits", 6, "flags"),
            1,
            "an invalid fixed domain must fail before converting even zero"
        );
        assert_eq!(
            field("interval_from_f32_bits", 1, "flags"),
            0,
            "the smallest subnormal must be outward-enclosed"
        );
        assert_eq!(
            field("root_isolate", 539, "out0"),
            -2,
            "a tolerance cell with two variations must stay unresolved"
        );
        assert_eq!(
            field("root_isolate", 588, "out0"),
            -2,
            "an endpoint root plus an interior variation must stay unresolved"
        );
        assert_eq!(
            (
                field("root_isolate", 638, "out0"),
                field("root_isolate", 639, "out0"),
            ),
            (1, 2),
            "an odd-width q split must retain a corridor containing the root"
        );
        assert_eq!(
            field("interval_add_restricted", 0, "flags"),
            1,
            "restricted domains must reject a result beyond the sealed raw maximum"
        );
        assert_eq!(
            field("interval_add_restricted", 1, "flags"),
            0,
            "restricted domains must accept an in-range boundary result"
        );
        assert_eq!(
            field("interval_scale_pow2", 2, "flags"),
            0,
            "large negative shifts must remain conservative"
        );
        assert_eq!(
            field("interval_scale_pow2", 8, "flags"),
            0,
            "zero may be scaled by every representable i128 power"
        );
        assert_eq!(
            field("interval_scale_pow2", 9, "flags"),
            1,
            "a power outside the shared exact intermediate range must fail explicitly"
        );
        assert_eq!(
            field("rgb_singleton", 0, "out0"),
            -1,
            "an RGB interval crossing a byte boundary must not be accepted"
        );
        assert_eq!(
            field("rgb_singleton", 1, "flags"),
            0,
            "three fixed RGB channels must produce one exact code triplet"
        );
        assert_eq!(
            field("bernstein_lerp_ratio", 0, "out0"),
            fold(&[-1, 0]),
            "the one-third Bernstein split must round outward"
        );
        assert_eq!(
            field("fixed_domain", 0, "out0"),
            1,
            "the checked domain constructor must preserve its restricted bounds"
        );
        assert_eq!(
            field("fixed_domain", 1, "flags"),
            1,
            "the checked domain constructor must reject an invalid exponent"
        );
        assert_eq!(
            field("fixed_q_setup", 0, "flags"),
            1,
            "setup must reject a first-step dq overflow"
        );
        assert_eq!(
            field("material_residual", 0, "flags"),
            1,
            "an unrepresentable absolute residual must be a tagged error"
        );
        assert_eq!(
            field("bernstein_certificate", 0, "flags"),
            0,
            "the i32 minimum strict margin must be handled without guest arithmetic failure"
        );
        assert_eq!(
            field("q_insertion_sort", 3, "out0"),
            field("q_insertion_sort", 4, "out0"),
            "corridor component ordering must be permutation invariant"
        );
        assert_eq!(
            (
                field("interval_mul_exp", 0, "out0"),
                field("interval_mul_exp", 0, "flags"),
            ),
            (fold(&[i32::MIN, 0]), 0),
            "non-point exponent-31 products must retain every fitting endpoint"
        );
        assert_eq!(
            (
                field("interval_convert", 0, "out0"),
                field("interval_convert", 0, "flags"),
            ),
            (fold(&[40, 40]), 0),
            "restricted destinations must validate the input in the source domain before scaling"
        );
        assert_eq!(
            field("bernstein_certificate", 1, "out0"),
            fold_small(&[0, 1, 1, 0, 0, 0]),
            "certificate vectors must compare the real derivative and face margins"
        );
        assert_eq!(
            field("normal_bound", 0, "flags"),
            1,
            "normal reconstruction overflow must be a tagged error"
        );
        assert_eq!(
            field("krawczyk_certificate", 0, "out0"),
            fold_small(&[2, 1, 0, i32::MAX, 0, 0]),
            "Krawczyk margin intermediates must widen before the checked i32 result"
        );
        for kernel in [
            "byte_singleton",
            "coverage_line",
            "csg_first_transition",
            "csg_stack_eval",
            "csg_boundary_influence",
            "quantize_ties_even",
            "quadratic_range",
            "bernstein_certificate_subdivided",
        ] {
            assert_eq!(
                field(kernel, 0, "flags"),
                1,
                "{kernel} must expose its sealed failure path as a tagged error"
            );
        }
        for (case, expected) in [
            (0, fold_small(&[0, 1, 2, 4, 0, 0])),
            (1, fold_small(&[0, 2, 2, 4, 0, 0])),
            (2, fold_small(&[0, 3, 2, 4, 0, 0])),
            (3, fold_small(&[1, 0, 2, 2, 0, 0])),
            (4, fold_small(&[1, 1, 2, 2, 0, 0])),
            (5, fold_small(&[2, 0, 1, 0, 2, 0])),
        ] {
            assert_eq!(
                field("certify_run", case, "out0"),
                expected,
                "ordered certificate selection and composition-shape reporting drifted"
            );
        }
        assert_eq!(
            field("root_config", 1, "flags"),
            1,
            "point root domains must fail consistently"
        );
        assert_eq!(
            field("root_config", 2, "flags"),
            1,
            "root depth outside the shared fixed-capacity schedule must be an error"
        );
        assert_eq!(
            field("root_config", 4, "flags"),
            1,
            "degree-zero Bernstein programs must be rejected consistently"
        );
    }
}
