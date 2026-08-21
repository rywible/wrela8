//! Pixels compiler subsystem.
//!
//! Compile-time structural programs, sealed runtime data, and certified
//! visibility support for renderer images.

use std::collections::{BTreeMap, BTreeSet};

use crate::eval::image::ImageGraph;
use crate::sema::typed::{
    TypedCallArg, TypedClosureBody, TypedDeferBody, TypedExpr, TypedExprKind, TypedFn,
    TypedForIter, TypedProgram, TypedStmt, TypedStmtKind,
};
use crate::sema::types::{self, Type, TypeArg};

pub mod arena;
pub mod binary_verify;
pub mod bounds;
pub mod camera;
pub mod canonicalize;
pub mod capacities;
pub mod competition;
pub mod config;
pub mod csg;
pub mod decode;
pub mod deform;
pub mod derivative_bounds;
pub mod derivatives;
pub mod diagnostics;
pub mod dump;
pub mod encode;
pub mod event_kinds;
pub mod events;
pub mod exclusions;
pub mod features;
pub mod field_intrinsics;
pub mod glue;
pub mod graph;
pub mod hot_census;
pub mod ids;
pub mod index;
pub mod legality;
pub mod material;
pub mod material_graph;
pub mod material_intrinsics;
pub mod objects;
pub mod params;
pub mod polynomial;
pub mod primitive;
pub mod program;
pub mod projection_bounds;
pub mod projective;
pub mod quota;
pub mod reference;
pub mod repeat;
pub mod report;
pub mod scalar;
pub mod state;
pub mod support;
pub mod surface;
pub mod symbolic;
pub mod tables;
pub mod test_vectors;
pub mod texture;
pub mod verify;
pub mod version;
pub mod worker_errors;
pub mod world_bounds;

pub use dump::{
    PixelsDumpStage, dump_frame_program, dump_render_layout, dump_structural_graphs,
    dump_symbolic_graphs, dump_uncompiled_configs, dump_zero_renderers,
};

pub(crate) const DEBUG_VISIBILITY_PATH: &str = "certified-from-scratch";

#[derive(Debug, Clone, PartialEq)]
pub struct PixelsProgramSet {
    pub symbolic_graphs: Vec<symbolic::SymbolicGraph>,
    pub structural_programs: Vec<verify::VerifiedStructuralProgram>,
    pub projective_programs: Vec<verify::VerifiedProjectiveProgram>,
    pub compiled_renderers: Vec<CompiledRenderer>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CompiledRenderer {
    pub config: config::RendererConfig,
    pub symbolic: symbolic::SymbolicGraph,
    pub structural: verify::VerifiedStructuralProgram,
    pub projective: verify::VerifiedProjectiveProgram,
    pub program: program::VerifiedFrameProgram,
    pub encoded: Vec<u8>,
    pub mutable_layout: state::RendererStateLayout,
    pub generated: glue::GeneratedRenderer,
}

pub fn fuzz_seed_frame_program() -> Result<Vec<u8>, String> {
    encode::encode(&program::minimal_verified_frame_program()).map_err(|error| error.to_string())
}

fn compile_structural_renderer(
    graph: &symbolic::SymbolicGraph,
    config: &config::RendererConfig,
) -> Result<verify::VerifiedStructuralProgram, String> {
    verify::check_input_depth(graph)?;
    let params = params::derive_layout(graph, config)?;
    let values = bounds::propagate(graph, config)?;
    let derivatives = derivative_bounds::propagate(graph, &values)?;
    let support = support::propagate(graph, &values)?;
    let world_bounds = world_bounds::derive(graph, config, &values, &support)?;
    let objects = objects::partition(graph, &values, &world_bounds, &support)?;
    let csg = csg::compile(objects.csg.clone(), objects.objects.len())?;
    let features = features::decompose(graph, &objects, &values, &world_bounds, &support)?;
    let repeats = repeat::compile(graph, config, &objects)?;
    let deformations = deform::compile(graph, &values)?;
    let material_events = material::compile(graph, &values, &objects, &features)?;
    let capacities = capacities::derive(
        graph,
        config,
        &params,
        &values,
        &objects,
        &csg,
        &features,
        &repeats,
        &deformations,
        &material_events,
    )?;
    let report = report::build(&params, &capacities);
    verify::check(
        graph,
        config,
        verify::StructuralProgram {
            params,
            values,
            derivatives,
            world_bounds,
            support,
            objects,
            csg,
            features,
            repeats,
            deformations,
            material_events,
            capacities,
            report,
        },
    )
}

pub(crate) fn derive_projective_program(
    graph: &symbolic::SymbolicGraph,
    config: &config::RendererConfig,
    verified_structural: &verify::VerifiedStructuralProgram,
) -> Result<verify::ProjectiveProgram, String> {
    let structural = verified_structural.program();
    let mut equations = projective::compile(
        graph,
        config,
        &structural.values,
        &structural.objects,
        &structural.features,
    )?;
    let deformations = deform::compile_projective(
        &structural.deformations,
        &structural.features,
        &equations,
        &structural.values,
    )?;
    for deformation in &deformations {
        let feature = equations
            .features
            .get_mut(deformation.feature.index())
            .ok_or_else(|| {
                format!(
                    "pixels::deform: missing projective feature {}",
                    deformation.feature
                )
            })?;
        feature.max_root_count = deformation.maximum_root_count;
    }
    let derivatives = derivatives::compile(graph, &mut equations, structural)?;
    let spans = projection_bounds::derive(&structural.features, &equations)?;
    let mut events = events::compile(
        graph,
        structural,
        &mut equations,
        &derivatives,
        &spans,
        &deformations,
    )?;
    let competitions = competition::compile(
        &mut equations,
        structural,
        &derivatives,
        &spans,
        &mut events,
    )?;
    let coefficient_remap = projective::canonicalize_coefficient_ids(&mut equations)?;
    for event in &mut events.generators {
        if let events::EventRepresentation::DirectDepthCrossProduct {
            denominator_a,
            denominator_b,
            ..
        } = &mut event.representation
        {
            for denominator in [denominator_a, denominator_b] {
                denominator.coefficient = coefficient_remap
                    .get(denominator.coefficient.index())
                    .copied()
                    .flatten()
                    .ok_or_else(|| {
                        format!(
                            "pixels::events: live denominator coefficient {} was not canonicalized",
                            denominator.coefficient
                        )
                    })?;
            }
        }
    }
    let exclusions = exclusions::compile(&equations, &spans, &events, &competitions)?;
    let indexes = index::compile(
        graph,
        config,
        structural,
        &derivatives,
        &spans,
        &events,
        &competitions,
    )?;
    let capacities = capacities::derive_projective(
        &structural.capacities,
        &equations,
        &derivatives,
        &spans,
        &events,
        &competitions,
        &indexes,
    )?;
    Ok(verify::ProjectiveProgram {
        equations,
        deformations,
        derivatives,
        spans,
        events,
        competitions,
        exclusions,
        indexes,
        capacities,
    })
}

fn compile_projective_renderer(
    graph: &symbolic::SymbolicGraph,
    config: &config::RendererConfig,
    structural: &verify::VerifiedStructuralProgram,
) -> Result<verify::VerifiedProjectiveProgram, String> {
    let program = derive_projective_program(graph, config, structural)?;
    verify::check_projective(graph, config, structural, program)
}

fn compile_symbolic_renderer(
    programs: &BTreeMap<String, TypedProgram>,
    owner_module: &str,
    config: &config::RendererConfig,
    renderer_index: usize,
) -> Result<symbolic::SymbolicGraph, diagnostics::PixelsError> {
    let mut graph =
        symbolic::compile(programs, owner_module, config, renderer_index).map_err(|failure| {
            diagnostics::PixelsError::Diagnostic(diagnostics::PixelsDiagnostic::from_prefixed(
                failure.message,
                failure.primary,
                "P014",
            ))
        })?;
    canonicalize::run(&mut graph).map_err(|message| {
        diagnostics::PixelsError::Diagnostic(diagnostics::PixelsDiagnostic::from_prefixed(
            message,
            crate::syntax::ast::Span::default(),
            "P014",
        ))
    })?;
    Ok(graph)
}

/// Compile every renderer only after the generic sealed-image checks have
/// completed. This is the single P2 build gate used by dumps, builds, tests,
/// fuzzing, and determinism reproduction.
pub fn compile_all(
    programs: &BTreeMap<String, TypedProgram>,
    owner_module: &str,
    image: &ImageGraph,
    configs: &config::RendererConfigs,
) -> Result<PixelsProgramSet, diagnostics::PixelsError> {
    if image.renderers.len() != configs.renderers.len() {
        return Err(diagnostics::PixelsError::Diagnostic(
            diagnostics::PixelsDiagnostic::new(
                "P009",
                "sealed renderer configuration count does not match the image graph",
                crate::syntax::ast::Span::default(),
            ),
        ));
    }
    let symbolic_graphs = image
        .renderers
        .iter()
        .zip(&configs.renderers)
        .enumerate()
        .map(|(renderer_index, (renderer, config))| {
            compile_symbolic_renderer(programs, owner_module, config, renderer_index).map_err(
                |mut error| {
                    let diagnostics::PixelsError::Diagnostic(diagnostic) = &mut error;
                    if diagnostic.primary == crate::syntax::ast::Span::default() {
                        diagnostic.primary = renderer.span;
                    }
                    error
                },
            )
        })
        .collect::<Result<Vec<_>, _>>()?;
    let structural_programs = symbolic_graphs
        .iter()
        .zip(&configs.renderers)
        .zip(&image.renderers)
        .map(|((graph, config), renderer)| {
            compile_structural_renderer(graph, config).map_err(|message| {
                let prefixed = message.starts_with('P')
                    && message.get(1..4).is_some_and(|digits| {
                        digits.chars().all(|character| character.is_ascii_digit())
                    })
                    && message.get(4..6) == Some(": ");
                let diagnostic = if prefixed {
                    diagnostics::PixelsDiagnostic::from_prefixed(message, renderer.span, "P020")
                } else {
                    diagnostics::PixelsDiagnostic::internal(message)
                };
                diagnostics::PixelsError::Diagnostic(diagnostic)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let projective_programs = symbolic_graphs
        .iter()
        .zip(&configs.renderers)
        .zip(&structural_programs)
        .zip(&image.renderers)
        .map(|(((graph, config), structural), renderer)| {
            compile_projective_renderer(graph, config, structural).map_err(|message| {
                let prefixed = message.starts_with('P')
                    && message.get(1..4).is_some_and(|digits| {
                        digits.chars().all(|character| character.is_ascii_digit())
                    })
                    && message.get(4..6) == Some(": ");
                let diagnostic = if prefixed {
                    diagnostics::PixelsDiagnostic::from_prefixed(message, renderer.span, "P020")
                } else {
                    diagnostics::PixelsDiagnostic::internal(message)
                };
                diagnostics::PixelsError::Diagnostic(diagnostic)
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    let compiled_renderers = symbolic_graphs
        .iter()
        .zip(&structural_programs)
        .zip(&projective_programs)
        .zip(&configs.renderers)
        .zip(&image.renderers)
        .enumerate()
        .map(
            |(renderer_index, ((((graph, structural), projective), config), renderer))| {
                let compile = || -> Result<CompiledRenderer, String> {
                    let rich = program::finish_frame_program(
                        renderer_index,
                        graph,
                        structural,
                        projective,
                        config,
                    )?;
                    let verified = verify::check_program(rich)?;
                    let encoded = encode::encode(&verified)
                        .map_err(|error| error.diagnostic().message.clone())?;
                    // The builder already produced a verified program. Keep
                    // the independent round trip in debug/verification builds
                    // without taxing shipped release compilation.
                    #[cfg(debug_assertions)]
                    {
                        let decoded = decode::decode(&encoded).map_err(|error| {
                            format!("pixels::decode: compiler output failed round-trip: {error}")
                        })?;
                        if decoded.program() != verified.program() {
                            return Err(
                                "pixels::compile: frame-program encode/decode mismatch".to_string()
                            );
                        }
                    }
                    let mutable_layout = state::layout(structural, projective)?;
                    let generated = glue::generate(renderer_index, config, &verified)?;
                    Ok(CompiledRenderer {
                        config: config.clone(),
                        symbolic: graph.clone(),
                        structural: structural.clone(),
                        projective: projective.clone(),
                        program: verified,
                        encoded,
                        mutable_layout,
                        generated,
                    })
                };
                compile().map_err(|message| {
                    let prefixed = message.starts_with('P')
                        && message.get(1..4).is_some_and(|digits| {
                            digits.chars().all(|character| character.is_ascii_digit())
                        })
                        && message.get(4..6) == Some(": ");
                    let diagnostic = if prefixed {
                        diagnostics::PixelsDiagnostic::from_prefixed(message, renderer.span, "P020")
                    } else {
                        diagnostics::PixelsDiagnostic::internal(message)
                    };
                    diagnostics::PixelsError::Diagnostic(diagnostic)
                })
            },
        )
        .collect::<Result<Vec<_>, _>>()?;
    Ok(PixelsProgramSet {
        symbolic_graphs,
        structural_programs,
        projective_programs,
        compiled_renderers,
    })
}

pub const RENDERER_LABELS: &[&str] = &[
    "field",
    "material",
    "display",
    "width",
    "height",
    "refresh_hz",
    "shade_hz",
    "profile",
    "tone_curve",
    "near",
    "far",
    "world_min",
    "world_max",
    "camera_bounds",
    "light_config",
    "exposure_range",
    "environment_range",
    "ao",
    "probes",
    "initialization_deadline_ms",
];

/// Renderer labels that may be omitted. Everything in `RENDERER_LABELS` is
/// required; these are additions that must not break existing declarations.
///
/// `camera_pose` pins the frame camera to one exact pose. Declaring it turns
/// the analytic coverage tiers' admission from a per-frame runtime test into
/// a sealed fact: `validate_frame` rejects any frame whose camera differs, so
/// a renderer that declares it either always has those tiers or fails the
/// frame outright, and the compile-time cost model no longer has to reason
/// about a cliff it cannot see.
pub const OPTIONAL_RENDERER_LABELS: &[&str] = &["camera_pose"];

pub(crate) const FIELD_OPERATIONS: &[&str] = crate::sema::intrinsics::PIXELS_FIELD_SURFACE;

pub(crate) fn call_base(spelling: &str) -> &str {
    spelling
        .strip_prefix("fn:")
        .unwrap_or(spelling)
        .split('[')
        .next()
        .unwrap_or(spelling)
}

#[derive(Clone)]
pub(crate) struct LocatedFn {
    pub(crate) module: String,
    pub(crate) key: String,
    pub(crate) decl_name: String,
    pub(crate) function: TypedFn,
}

#[derive(Clone)]
pub(crate) struct LocatedConst {
    pub(crate) module: String,
    pub(crate) name: String,
    pub(crate) constant: crate::sema::typed::TypedConst,
}

pub(crate) fn located_constant(
    programs: &BTreeMap<String, TypedProgram>,
    current_module: &str,
    name: &str,
) -> Option<LocatedConst> {
    let visible = program_for_decl_module(programs, current_module)?;
    let declaration_module = visible
        .const_decl_modules
        .get(name)
        .cloned()
        .unwrap_or_else(|| current_module.to_string());
    let declaration_name = visible
        .const_decl_names
        .get(name)
        .map(String::as_str)
        .unwrap_or(name);
    if let Some(constant) = program_for_decl_module(programs, &declaration_module)
        .and_then(|program| program.consts.get(declaration_name))
        .cloned()
    {
        return Some(LocatedConst {
            module: declaration_module,
            name: declaration_name.to_string(),
            constant,
        });
    }
    visible
        .consts
        .get(name)
        .or_else(|| visible.imported.consts.get(name))
        .cloned()
        .map(|constant| LocatedConst {
            module: declaration_module,
            name: declaration_name.to_string(),
            constant,
        })
}

pub(crate) fn root_function(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    name: &str,
) -> Result<LocatedFn, String> {
    if let Some(function) = owner
        .fns
        .get(name)
        .or_else(|| {
            owner
                .imported
                .fns
                .get(name)
                .map(|function| function.as_ref())
        })
        .cloned()
    {
        let module = owner
            .fn_decl_modules
            .get(name)
            .cloned()
            .unwrap_or_else(|| "<image-owner>".to_string());
        let decl_name = owner
            .fn_decl_names
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_string());
        let function = program_for_decl_module(programs, &module)
            .and_then(|program| program.fns.get(&decl_name))
            .cloned()
            .unwrap_or(function);
        return Ok(LocatedFn {
            module,
            key: decl_name.clone(),
            decl_name,
            function,
        });
    }
    let mut local = programs.iter().filter_map(|(module, program)| {
        program.fns.get(name).cloned().map(|function| LocatedFn {
            module: module.clone(),
            key: name.to_string(),
            decl_name: name.to_string(),
            function,
        })
    });
    if let Some(found) = local.next() {
        if local.next().is_none() {
            return Ok(found);
        }
    }
    Err(format!(
        "pixels: renderer root `{name}` is unavailable or ambiguous"
    ))
}

pub(crate) fn called_function(
    programs: &BTreeMap<String, TypedProgram>,
    current_module: &str,
    spelling: &str,
) -> Option<LocatedFn> {
    let name = call_base(spelling);
    let local_instance = |program: &TypedProgram| {
        let candidates = [spelling, spelling.strip_prefix("fn:").unwrap_or(spelling)];
        candidates.into_iter().find_map(|key| {
            program
                .instantiations
                .get(key)
                .and_then(|instantiation| match instantiation {
                    crate::sema::typed::TypedInstantiation::Fn(function) => {
                        Some((key.to_string(), function.clone()))
                    }
                    _ => None,
                })
        })
    };
    let imported_instance = |program: &TypedProgram| {
        let candidates = [spelling, spelling.strip_prefix("fn:").unwrap_or(spelling)];
        candidates.into_iter().find_map(|key| {
            program
                .imported
                .instantiations
                .get(key)
                .and_then(|instantiation| match instantiation {
                    crate::sema::typed::TypedInstantiation::Fn(function) => {
                        Some((key.to_string(), function.clone()))
                    }
                    _ => None,
                })
        })
    };
    if let Some(program) = programs.get(current_module) {
        if let Some((visible_type, member)) = name.rsplit_once('.')
            && let Some((declaring_module, declaring_name)) = nominal_decl(program, visible_type)
            && let Some(function) = program_for_decl_module(programs, declaring_module)
                .and_then(|declaration| declaration.structs.get(declaring_name))
                .and_then(|strukt| {
                    strukt
                        .methods
                        .get(member)
                        .or_else(|| strukt.assoc_fns.get(member))
                })
                .cloned()
        {
            return Some(LocatedFn {
                module: declaring_module.to_string(),
                key: name.to_string(),
                decl_name: member.to_string(),
                function,
            });
        }
        if let Some((key, function)) = local_instance(program) {
            return Some(LocatedFn {
                module: program
                    .fn_decl_modules
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| current_module.to_string()),
                key,
                decl_name: program
                    .fn_decl_names
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| name.to_string()),
                function,
            });
        }
        if let Some(function) = program.fns.get(name).cloned() {
            return Some(LocatedFn {
                module: current_module.to_string(),
                key: name.to_string(),
                decl_name: name.to_string(),
                function,
            });
        }
        if let Some((key, function)) = imported_instance(program) {
            return Some(LocatedFn {
                module: program
                    .fn_decl_modules
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| current_module.to_string()),
                key,
                decl_name: program
                    .fn_decl_names
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| name.to_string()),
                function,
            });
        }
        if let Some(function) = program
            .imported
            .fns
            .get(name)
            .map(|function| function.as_ref().clone())
        {
            return Some(LocatedFn {
                module: program
                    .fn_decl_modules
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| current_module.to_string()),
                key: name.to_string(),
                decl_name: program
                    .fn_decl_names
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| name.to_string()),
                function,
            });
        }
    }
    let mut found = programs.iter().filter_map(|(module, program)| {
        if let Some((key, function)) = local_instance(program) {
            Some(LocatedFn {
                module: module.clone(),
                key,
                decl_name: program
                    .fn_decl_names
                    .get(name)
                    .cloned()
                    .unwrap_or_else(|| name.to_string()),
                function,
            })
        } else {
            program.fns.get(name).cloned().map(|function| LocatedFn {
                module: module.clone(),
                key: name.to_string(),
                decl_name: name.to_string(),
                function,
            })
        }
    });
    let first = found.next()?;
    found.next().is_none().then_some(first)
}

pub(crate) fn is_core_field_function(located: &LocatedFn) -> bool {
    is_core_field_function_identity(&located.module, &located.key, &located.decl_name)
}

pub(crate) fn is_core_scalar_function(located: &LocatedFn) -> bool {
    is_core_field_module(&located.module)
        && !call_base(&located.key).contains('.')
        && scalar::classify_intrinsic(&located.decl_name).is_some()
}

fn is_core_field_function_identity(module: &str, key: &str, decl_name: &str) -> bool {
    is_core_field_module(module)
        && !call_base(key).contains('.')
        && FIELD_OPERATIONS.contains(&decl_name)
}

pub(crate) fn is_core_field_vector_method(located: &LocatedFn) -> bool {
    is_core_field_vector_method_identity(&located.module, &located.key)
}

pub(crate) fn is_core_field_vector_method_identity(module: &str, key: &str) -> bool {
    is_core_field_module(module) && matches!(call_base(key), "Vec3.add" | "Vec3.subtract")
}

pub(crate) fn is_core_field_value_method(located: &LocatedFn) -> bool {
    is_core_field_module(&located.module) && call_base(&located.key) == "Field.union"
}

fn is_core_field_module(module: &str) -> bool {
    matches!(module, "field" | "core.field")
}

pub(crate) fn program_for_decl_module<'a>(
    programs: &'a BTreeMap<String, TypedProgram>,
    declaring_module: &str,
) -> Option<&'a TypedProgram> {
    programs.get(declaring_module).or_else(|| {
        let short = match declaring_module {
            "core.field" => "field",
            "core.render" => "render",
            "drivers.display" => "display",
            _ => return None,
        };
        programs.get(short)
    })
}

struct SceneAnalysis<'a> {
    programs: &'a BTreeMap<String, TypedProgram>,
    active: BTreeSet<String>,
    visited: BTreeSet<String>,
    functions: BTreeMap<String, TypedFn>,
    field_operations: Vec<String>,
    mark_types: Vec<NominalType>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct NominalType {
    ty: Type,
    key: String,
}

pub(crate) fn nominal_decl<'a>(
    program: &'a TypedProgram,
    visible_name: &str,
) -> Option<(&'a str, &'a str)> {
    Some((
        program.type_decl_modules.get(visible_name)?.as_str(),
        program.type_decl_names.get(visible_name)?.as_str(),
    ))
}

pub(crate) fn instantiated_struct<'a>(
    program: &'a TypedProgram,
    name: &str,
    args: &[TypeArg],
) -> Option<&'a crate::sema::typed::TypedStruct> {
    let key =
        crate::sema::generics::canonical_key(crate::sema::bodies::InstKind::Struct, name, args);
    program
        .instantiations
        .get(&key)
        .or_else(|| program.imported.instantiations.get(&key))
        .and_then(|instantiation| match instantiation {
            crate::sema::typed::TypedInstantiation::Struct(strukt) => Some(strukt),
            crate::sema::typed::TypedInstantiation::Fn(_)
            | crate::sema::typed::TypedInstantiation::Enum(_) => None,
        })
}

pub(crate) fn typed_struct_decl<'a>(
    programs: &'a BTreeMap<String, TypedProgram>,
    module: &str,
    ty: &Type,
) -> Option<(&'a str, &'a crate::sema::typed::TypedStruct)> {
    let Type::Named(name, args) = ty else {
        return None;
    };
    let context = program_for_decl_module(programs, module)?;
    let (declaring_module, declaration_name) = nominal_decl(context, name)?;
    let declaration = program_for_decl_module(programs, declaring_module)?;
    let strukt = if args.is_empty() {
        declaration.structs.get(declaration_name)?
    } else {
        instantiated_struct(context, name, args)
            .or_else(|| instantiated_struct(declaration, declaration_name, args))?
    };
    Some((declaring_module, strukt))
}

pub(crate) fn instantiated_enum<'a>(
    program: &'a TypedProgram,
    name: &str,
    args: &[TypeArg],
) -> Option<&'a crate::sema::typed::TypedEnum> {
    let key = crate::sema::generics::canonical_key(crate::sema::bodies::InstKind::Enum, name, args);
    program
        .instantiations
        .get(&key)
        .or_else(|| program.imported.instantiations.get(&key))
        .and_then(|instantiation| match instantiation {
            crate::sema::typed::TypedInstantiation::Enum(enumeration) => Some(enumeration),
            crate::sema::typed::TypedInstantiation::Fn(_)
            | crate::sema::typed::TypedInstantiation::Struct(_) => None,
        })
}

fn is_nominal_type(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    ty: &Type,
    declaring_modules: &[&str],
    declaration: &str,
) -> bool {
    let Type::Named(name, args) = ty else {
        return false;
    };
    args.is_empty()
        && program_for_decl_module(programs, module).is_some_and(|program| {
            nominal_decl(program, name).is_some_and(|(found_module, found_name)| {
                found_name == declaration && declaring_modules.contains(&found_module)
            })
        })
}

pub(crate) fn is_core_material_constructor(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    spelling: &str,
) -> bool {
    let Some((receiver, method)) = call_base(spelling).rsplit_once('.') else {
        return false;
    };
    matches!(method, "standard" | "clay" | "porcelain" | "textured")
        && program_for_decl_module(programs, module).is_some_and(|program| {
            nominal_decl(program, receiver).is_some_and(|(declaring_module, declaration)| {
                declaration == "MaterialSample"
                    && matches!(declaring_module, "render" | "core.render")
            })
        })
}

fn nominal_type(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    ty: Type,
) -> Result<NominalType, String> {
    let Type::Named(name, _) = &ty else {
        return Err(format!(
            "pixels: material identity must be nominal, found `{}`",
            types::render_type(&ty)
        ));
    };
    let name = name.clone();
    let (declaring_module, declaring_name) = programs
        .get(module)
        .and_then(|program| nominal_decl(program, &name))
        .ok_or_else(|| {
            format!("pixels: cannot resolve nominal type `{name}` from module `{module}`")
        })?;
    Ok(NominalType {
        ty,
        key: format!("{declaring_module}::{declaring_name}"),
    })
}

fn same_canonical_type(
    programs: &BTreeMap<String, TypedProgram>,
    left_module: &str,
    left: &Type,
    right_module: &str,
    right: &Type,
) -> Result<bool, String> {
    match (left, right) {
        (Type::Named(left_name, left_args), Type::Named(right_name, right_args)) => {
            let left_decl = programs
                .get(left_module)
                .and_then(|program| nominal_decl(program, left_name));
            let right_decl = programs
                .get(right_module)
                .and_then(|program| nominal_decl(program, right_name));
            if left_decl.is_none() || left_decl != right_decl || left_args.len() != right_args.len()
            {
                return Ok(false);
            }
            for (left_arg, right_arg) in left_args.iter().zip(right_args) {
                match (left_arg, right_arg) {
                    (TypeArg::Type(left), TypeArg::Type(right)) => {
                        if !same_canonical_type(programs, left_module, left, right_module, right)? {
                            return Ok(false);
                        }
                    }
                    _ if left_arg == right_arg => {}
                    _ => return Ok(false),
                }
            }
            Ok(true)
        }
        (Type::Array(left, left_len), Type::Array(right, right_len)) => Ok(left_len == right_len
            && same_canonical_type(programs, left_module, left, right_module, right)?),
        (Type::Tuple(left), Type::Tuple(right)) => {
            if left.len() != right.len() {
                return Ok(false);
            }
            for (left, right) in left.iter().zip(right) {
                if !same_canonical_type(programs, left_module, left, right_module, right)? {
                    return Ok(false);
                }
            }
            Ok(true)
        }
        (Type::Option(left), Type::Option(right)) | (Type::Static(left), Type::Static(right)) => {
            same_canonical_type(programs, left_module, left, right_module, right)
        }
        (Type::Result(left_ok, left_err), Type::Result(right_ok, right_err)) => Ok(
            same_canonical_type(programs, left_module, left_ok, right_module, right_ok)?
                && same_canonical_type(programs, left_module, left_err, right_module, right_err)?,
        ),
        (Type::Own(left_pool, left), Type::Own(right_pool, right)) => Ok(left_pool == right_pool
            && same_canonical_type(programs, left_module, left, right_module, right)?),
        (Type::Fn(left_params, left_ret), Type::Fn(right_params, right_ret)) => {
            if left_params.len() != right_params.len() {
                return Ok(false);
            }
            for ((left_mode, left), (right_mode, right)) in left_params.iter().zip(right_params) {
                if left_mode != right_mode
                    || !same_canonical_type(programs, left_module, left, right_module, right)?
                {
                    return Ok(false);
                }
            }
            same_canonical_type(programs, left_module, left_ret, right_module, right_ret)
        }
        _ => Ok(left == right),
    }
}

pub(crate) fn validate_parameter_identity(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    params_type: &Type,
    field: &str,
    material: &str,
) -> Result<(), String> {
    let field = root_function(owner, programs, field)?;
    let material = root_function(owner, programs, material)?;
    for (kind, root) in [("field", field), ("material", material)] {
        let root_params = root
            .function
            .params
            .get(1)
            .map(|param| &param.ty)
            .unwrap_or(&Type::Unit);
        if !same_canonical_type(
            programs,
            &owner.module_path,
            params_type,
            &root.module,
            root_params,
        )? {
            return Err(format!(
                "pixels: renderer parameter type `{}` is not the same nominal type as \
                 {kind} root `{}` parameter `{}`",
                types::render_type(params_type),
                root.key,
                types::render_type(root_params)
            ));
        }
    }
    Ok(())
}

impl<'a> SceneAnalysis<'a> {
    fn new(programs: &'a BTreeMap<String, TypedProgram>) -> Self {
        Self {
            programs,
            active: BTreeSet::new(),
            visited: BTreeSet::new(),
            functions: BTreeMap::new(),
            field_operations: Vec::new(),
            mark_types: Vec::new(),
        }
    }

    fn visit_function(&mut self, located: LocatedFn, field_root: bool) -> Result<(), String> {
        let identity = format!("{}::{}", located.module, located.key);
        if self.visited.contains(&identity) {
            return Ok(());
        }
        if !self.active.insert(identity.clone()) {
            return Err(format!(
                "pixels: P-1 field helper recursion reaches `{}`",
                located.key
            ));
        }
        self.functions
            .insert(identity.clone(), located.function.clone());
        self.visit_stmts(&located.function.body, &located.module, field_root)?;
        self.active.remove(&identity);
        self.visited.insert(identity);
        Ok(())
    }

    fn visit_args(
        &mut self,
        args: &[TypedCallArg],
        module: &str,
        field_root: bool,
    ) -> Result<(), String> {
        for arg in args {
            if let Some(value) = &arg.value {
                self.visit_expr(value, module, field_root)?;
            }
        }
        Ok(())
    }

    fn visit_expr(
        &mut self,
        expr: &TypedExpr,
        module: &str,
        field_root: bool,
    ) -> Result<(), String> {
        match &expr.kind {
            TypedExprKind::Call {
                callee,
                receiver,
                args,
            } => {
                let spelling = callee.spelling();
                let visible_name = call_base(&spelling);
                let resolved = called_function(self.programs, module, &spelling);
                let canonical_field_name = resolved
                    .as_ref()
                    .filter(|located| is_core_field_function(located))
                    .map(|located| located.decl_name.as_str());
                if field_root && let Some(canonical_field_name) = canonical_field_name {
                    if canonical_field_name == "mark" {
                        let material = args
                            .get(2)
                            .and_then(|arg| arg.value.as_ref())
                            .map(|value| value.ty.clone())
                            .ok_or_else(|| {
                                "pixels: `mark` lacks its material value after sema".to_string()
                            })?;
                        self.mark_types
                            .push(nominal_type(self.programs, module, material)?);
                    } else {
                        self.field_operations.push(canonical_field_name.to_string());
                    }
                } else if let Some(function) = resolved {
                    self.visit_function(function, field_root)?;
                } else if field_root
                    && is_nominal_type(
                        self.programs,
                        module,
                        &expr.ty,
                        &["field", "core.field"],
                        "Field",
                    )
                {
                    return Err(format!(
                        "pixels: P-1 cannot resolve field-producing helper `{visible_name}`"
                    ));
                }
                if let Some(receiver) = receiver {
                    self.visit_expr(receiver, module, field_root)?;
                }
                self.visit_args(args, module, field_root)?;
            }
            TypedExprKind::CallValue(callee, args) => {
                if field_root {
                    return Err(
                        "pixels: P-1 field roots may not call a closure or function value"
                            .to_string(),
                    );
                }
                self.visit_expr(callee, module, field_root)?;
                self.visit_args(args, module, field_root)?;
            }
            TypedExprKind::Field(base, _)
            | TypedExprKind::ToScalar(base)
            | TypedExprKind::Neg(base)
            | TypedExprKind::BitNot(base)
            | TypedExprKind::Take(base)
            | TypedExprKind::Not(base)
            | TypedExprKind::Panic(base)
            | TypedExprKind::Await(base)
            | TypedExprKind::Send(base) => self.visit_expr(base, module, field_root)?,
            TypedExprKind::Try(base, conversion) => {
                self.visit_expr(base, module, field_root)?;
                if let Some(conversion) = conversion {
                    let spelling = conversion.spelling();
                    let Some(function) = called_function(self.programs, module, &spelling) else {
                        return Err(format!(
                            "pixels: P-1 cannot resolve `?` conversion `{spelling}`"
                        ));
                    };
                    self.visit_function(function, field_root)?;
                }
            }
            TypedExprKind::Index(a, b)
            | TypedExprKind::Binary(_, a, b)
            | TypedExprKind::And(a, b)
            | TypedExprKind::Or(a, b) => {
                self.visit_expr(a, module, field_root)?;
                self.visit_expr(b, module, field_root)?;
            }
            TypedExprKind::OpCall(callee, receiver, argument) => {
                self.visit_expr(receiver, module, field_root)?;
                self.visit_expr(argument, module, field_root)?;
                let spelling = callee.spelling();
                let Some(function) = called_function(self.programs, module, &spelling) else {
                    return Err(format!(
                        "pixels: P-1 cannot resolve overloaded operator `{spelling}`"
                    ));
                };
                self.visit_function(function, field_root)?;
            }
            TypedExprKind::Is(value, _) => self.visit_expr(value, module, field_root)?,
            TypedExprKind::EnumConstruct { args, .. } => {
                self.visit_args(args, module, field_root)?
            }
            TypedExprKind::Tuple(items) | TypedExprKind::List(items) => {
                for item in items {
                    self.visit_expr(item, module, field_root)?;
                }
            }
            TypedExprKind::StructLiteral { fields, .. } => {
                for (_, value) in fields {
                    self.visit_expr(value, module, field_root)?;
                }
            }
            TypedExprKind::Intrinsic { receiver, args, .. } => {
                if let Some(receiver) = receiver {
                    self.visit_expr(receiver, module, field_root)?;
                }
                for (_, value) in args {
                    self.visit_expr(value, module, field_root)?;
                }
            }
            TypedExprKind::Closure { body, .. } => {
                if field_root {
                    return Err("pixels: P-1 field roots may not contain closures".to_string());
                }
                match body {
                    TypedClosureBody::Expr(value) => self.visit_expr(value, module, field_root)?,
                    TypedClosureBody::Suite(body) => self.visit_stmts(body, module, field_root)?,
                }
            }
            TypedExprKind::Int(_)
            | TypedExprKind::Float(_)
            | TypedExprKind::Str(_)
            | TypedExprKind::BStr(_)
            | TypedExprKind::Char(_)
            | TypedExprKind::Bool(_)
            | TypedExprKind::Unit
            | TypedExprKind::Local(_)
            | TypedExprKind::Const(_)
            | TypedExprKind::Static(_)
            | TypedExprKind::FnRef(_)
            | TypedExprKind::PoolName(_)
            | TypedExprKind::GroupChild(_) => {}
        }
        Ok(())
    }

    fn visit_stmts(
        &mut self,
        stmts: &[TypedStmt],
        module: &str,
        field_root: bool,
    ) -> Result<(), String> {
        for stmt in stmts {
            match &stmt.kind {
                TypedStmtKind::Let { value, .. } => self.visit_expr(value, module, field_root)?,
                TypedStmtKind::Assign { target, value } => {
                    self.visit_expr(target, module, field_root)?;
                    self.visit_expr(value, module, field_root)?;
                }
                TypedStmtKind::If {
                    cond,
                    then_branch,
                    elifs,
                    else_branch,
                } => {
                    self.visit_expr(cond, module, field_root)?;
                    self.visit_stmts(then_branch, module, field_root)?;
                    for elif in elifs {
                        self.visit_expr(&elif.cond, module, field_root)?;
                        self.visit_stmts(&elif.body, module, field_root)?;
                    }
                    if let Some(branch) = else_branch {
                        self.visit_stmts(branch, module, field_root)?;
                    }
                }
                TypedStmtKind::Match { scrutinee, arms } => {
                    self.visit_expr(scrutinee, module, field_root)?;
                    for arm in arms {
                        if let Some(guard) = &arm.guard {
                            self.visit_expr(guard, module, field_root)?;
                        }
                        self.visit_stmts(&arm.body, module, field_root)?;
                    }
                }
                TypedStmtKind::For { iter, body, .. } => {
                    match iter {
                        TypedForIter::Range(a, b, _) => {
                            self.visit_expr(a, module, field_root)?;
                            self.visit_expr(b, module, field_root)?;
                        }
                        TypedForIter::Expr(value) => self.visit_expr(value, module, field_root)?,
                    }
                    self.visit_stmts(body, module, field_root)?;
                }
                TypedStmtKind::While { cond, body, .. } => {
                    self.visit_expr(cond, module, field_root)?;
                    self.visit_stmts(body, module, field_root)?;
                }
                TypedStmtKind::Return(Some(value))
                | TypedStmtKind::ExprStmt(value)
                | TypedStmtKind::BareSend { expr: value, .. } => {
                    self.visit_expr(value, module, field_root)?
                }
                TypedStmtKind::Assert { cond, message }
                | TypedStmtKind::ComptimeAssert { cond, message, .. } => {
                    self.visit_expr(cond, module, field_root)?;
                    if let Some(message) = message {
                        self.visit_expr(message, module, field_root)?;
                    }
                }
                TypedStmtKind::Defer(TypedDeferBody::Expr(value)) => {
                    self.visit_expr(value, module, field_root)?
                }
                TypedStmtKind::Defer(TypedDeferBody::Suite(body))
                | TypedStmtKind::WithGroup { body, .. } => {
                    self.visit_stmts(body, module, field_root)?
                }
                TypedStmtKind::Break
                | TypedStmtKind::Continue
                | TypedStmtKind::Pass
                | TypedStmtKind::Return(None) => {}
            }
        }
        Ok(())
    }
}

fn material_type(
    programs: &BTreeMap<String, TypedProgram>,
    module: &str,
    function: &TypedFn,
) -> Option<Type> {
    let Type::Named(name, args) = &function.params.first()?.ty else {
        return None;
    };
    let program = program_for_decl_module(programs, module)?;
    if !nominal_decl(program, name).is_some_and(|(declaring_module, declaration)| {
        declaration == "SurfaceContext" && matches!(declaring_module, "render" | "core.render")
    }) {
        return None;
    }
    match args.as_slice() {
        [TypeArg::Type(material)] => Some(material.clone()),
        _ => None,
    }
}

pub(crate) fn validate_material_identity(
    owner: &TypedProgram,
    programs: &BTreeMap<String, TypedProgram>,
    field: &str,
    material: &str,
) -> Result<Type, String> {
    let field_fn = root_function(owner, programs, field)?;
    let material_fn = root_function(owner, programs, material)?;
    let expected = material_type(programs, &material_fn.module, &material_fn.function)
        .ok_or_else(|| "pixels: material root lacks `SurfaceContext[M]`".to_string())?;
    let expected_nominal = nominal_type(programs, &material_fn.module, expected.clone())?;
    let mut analysis = SceneAnalysis::new(programs);
    analysis.visit_function(field_fn, true)?;
    let mut nominal: Vec<NominalType> = Vec::new();
    for material in analysis.mark_types {
        if !nominal.iter().any(|prior| prior.key == material.key) {
            nominal.push(material);
        }
    }
    if nominal.is_empty() {
        return Err(
            "pixels: renderer `@field` must reach at least one `mark(..., material=M.*)`"
                .to_string(),
        );
    }
    if nominal.len() != 1 {
        return Err(format!(
            "pixels: `@field` reaches more than one nominal material type: {}",
            nominal
                .iter()
                .map(|material| material.key.as_str())
                .collect::<Vec<_>>()
                .join(", ")
        ));
    }
    if nominal[0].key != expected_nominal.key {
        return Err(format!(
            "pixels: renderer material mismatch: field marks use `{}`, `SurfaceContext` uses `{}`",
            nominal[0].key, expected_nominal.key
        ));
    }
    Ok(expected)
}

pub fn smooth_min(a: f64, b: f64, k: f64) -> f64 {
    if a <= b - k {
        a
    } else if b <= a - k {
        b
    } else {
        let h = 0.5 + 0.5 * (b - a) / k;
        b + (a - b) * h - k * h * (1.0 - h)
    }
}

pub fn sinusoidal_contract(amplitude: f64, frequency: f64) -> [f64; 4] {
    [
        amplitude.abs(),
        (amplitude * frequency).abs(),
        (amplitude * frequency * frequency).abs(),
        (amplitude * frequency * frequency * frequency).abs(),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn smooth_interior_root_is_not_a_leaf_root() {
        let k = 4.0;
        let a = k / 4.0;
        assert_ne!(a, 0.0);
        assert_eq!(smooth_min(a, a, k), 0.0);
        assert!(a.abs() <= k / 4.0);
    }

    #[test]
    fn saturated_branch_returns_selected_operand_bits() {
        let a = -3.25f64;
        let b = 2.0f64;
        assert_eq!(smooth_min(a, b, 0.5).to_bits(), a.to_bits());
    }

    #[test]
    fn nested_smooth_composition_keeps_interior_candidate() {
        let inner = smooth_min(1.0, 1.0, 4.0);
        assert_eq!(smooth_min(inner, 2.0, 0.5), 0.0);
    }

    #[test]
    fn deformation_contract_is_derived_and_rejects_false_bound() {
        let contract = sinusoidal_contract(2.0, 3.0);
        assert_eq!(contract, [2.0, 6.0, 18.0, 54.0]);
        let author_claim = [2.0, 5.9, 18.0, 54.0];
        assert!(
            author_claim
                .iter()
                .zip(contract)
                .any(|(claimed, derived)| *claimed < derived)
        );
    }

    #[test]
    fn field_operation_census_equals_public_producer_sites() {
        let source = include_str!("../../../../stdlib/core/field.wr");
        let public_functions: BTreeSet<&str> = source
            .lines()
            .filter_map(|line| {
                line.strip_prefix("pub fn ")
                    .and_then(|tail| tail.split(['(', '[']).next())
            })
            .collect();
        let field_census: BTreeSet<&str> = FIELD_OPERATIONS.iter().copied().collect();
        let scalar_census: BTreeSet<&str> = [
            "sqrt_scalar",
            "rsqrt_scalar",
            "sin_scalar",
            "cos_scalar",
            "dot3",
            "cross3",
            "length2",
            "length3",
            "normalize3",
        ]
        .into_iter()
        .collect();
        assert!(field_census.is_disjoint(&scalar_census));
        assert_eq!(
            field_census
                .union(&scalar_census)
                .copied()
                .collect::<BTreeSet<_>>(),
            public_functions
        );
        assert!(field_census.contains("mark"));
    }

    #[test]
    fn canonical_field_module_check_rejects_suffix_counterfeits() {
        assert!(is_core_field_module("field"));
        assert!(is_core_field_module("core.field"));
        assert!(!is_core_field_module("lib.field"));
        assert!(!is_core_field_module("examples.field"));
    }

    #[test]
    fn canonical_field_function_check_excludes_methods_with_producer_basenames() {
        assert!(is_core_field_function_identity(
            "core.field",
            "aliased_union",
            "union"
        ));
        assert!(is_core_field_function_identity(
            "field",
            "fn:mark[ObjectId,MaterialId]",
            "mark"
        ));
        assert!(!is_core_field_function_identity(
            "core.field",
            "Field.union",
            "union"
        ));
        assert!(!is_core_field_function_identity(
            "core.field",
            "Vec3.subtract",
            "subtract"
        ));
    }
}
