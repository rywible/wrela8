//! Finite-repeat instance templates and wrap-boundary obligations.

use std::collections::{BTreeMap, BTreeSet};

use super::graph::Axis;
use super::ids::{FieldId, ObjectId};
use super::objects::ObjectPartition;
use super::reference::interval::F64Interval;
use super::scalar::ScalarOp;

#[derive(Clone, Debug, PartialEq)]
pub struct AffineTranslationProgram {
    pub repeat_field: FieldId,
    pub axis: Axis,
    pub first: i32,
    pub index: i32,
    pub period: F64Interval,
    pub translation: F64Interval,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepeatInstanceProgram {
    pub object: ObjectId,
    pub translations: Vec<AffineTranslationProgram>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct WrapBoundaryEventFamily {
    pub repeat_field: FieldId,
    pub axis: Axis,
    pub left_index: i32,
    pub right_index: i32,
    pub boundary: F64Interval,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RepeatTemplate {
    pub object: ObjectId,
    pub source_root: FieldId,
    pub instance_count: u32,
    pub affine_translation_count: u32,
    pub wrap_event_families: u32,
    pub instances: Vec<RepeatInstanceProgram>,
    pub wrap_events: Vec<WrapBoundaryEventFamily>,
    pub certificate_must_fix_instance: bool,
}

fn scaled_index(first: i32, index: i32, period: F64Interval) -> Result<F64Interval, String> {
    super::world_bounds::repeat_translation_interval(first, index, period)
}

fn transform_scalars(
    transform: &super::graph::TransformProgram,
    output: &mut Vec<super::ids::ScalarId>,
) {
    use super::graph::TransformProgram;
    match transform {
        TransformProgram::Translate { by } => output.extend(*by),
        TransformProgram::Rotate {
            row_x,
            row_y,
            row_z,
        } => output.extend(row_x.iter().chain(row_y).chain(row_z).copied()),
        TransformProgram::Rigid {
            translation,
            row_x,
            row_y,
            row_z,
        } => output.extend(
            translation
                .iter()
                .chain(row_x)
                .chain(row_y)
                .chain(row_z)
                .copied(),
        ),
        TransformProgram::UniformScale { scale } => output.push(*scale),
        TransformProgram::SourceRigidSequence { steps, .. }
        | TransformProgram::RigidSequence { steps, .. } => {
            for step in steps {
                transform_scalars(step, output);
            }
        }
    }
}

fn scalar_may_move(
    graph: &super::symbolic::SymbolicGraph,
    layout: &super::params::ParameterLayout,
    root: super::ids::ScalarId,
) -> Result<bool, String> {
    let mut stack = vec![root];
    let mut seen = BTreeSet::new();
    while let Some(id) = stack.pop() {
        if !seen.insert(id) {
            continue;
        }
        let node = graph.scalar.get(id)?;
        if let ScalarOp::Param(parameter) = node.op {
            let slot = layout
                .slots
                .iter()
                .find(|slot| slot.id == parameter)
                .ok_or_else(|| {
                    format!("pixels::repeat: missing dependency slot for parameter {parameter}")
                })?;
            if !slot.immutable {
                return Ok(true);
            }
        }
        stack.extend(super::params::scalar_children(&node.op));
    }
    Ok(false)
}

fn family_may_cross_wrap(
    graph: &super::symbolic::SymbolicGraph,
    config: &super::config::RendererConfig,
    layout: &super::params::ParameterLayout,
    objects: &[&super::objects::SmoothObject],
    repeat_field: FieldId,
) -> Result<bool, String> {
    if config.camera_max_motion > 0.0 {
        return Ok(true);
    }
    let super::graph::FieldKind::FiniteRepeat { period, .. } =
        &graph.fields.get(repeat_field)?.kind
    else {
        return Err(format!(
            "pixels::repeat: wrap family root {repeat_field} is not a finite repeat"
        ));
    };
    let mut moving_scalars = vec![*period];
    for object in objects {
        for path in &object.primitive_occurrences {
            let Some(repeat_position) = path
                .iter()
                .position(|occurrence| occurrence.field == repeat_field)
            else {
                continue;
            };
            for occurrence in path.iter().skip(repeat_position + 1) {
                match &graph.fields.get(occurrence.field)?.kind {
                    super::graph::FieldKind::Transform { transform, .. } => {
                        transform_scalars(transform, &mut moving_scalars);
                    }
                    super::graph::FieldKind::FiniteRepeat { period, .. } => {
                        moving_scalars.push(*period);
                    }
                    _ => {}
                }
            }
        }
    }
    for scalar in moving_scalars {
        if scalar_may_move(graph, layout, scalar)? {
            return Ok(true);
        }
    }
    Ok(false)
}

pub fn compile(
    graph: &super::symbolic::SymbolicGraph,
    config: &super::config::RendererConfig,
    objects: &ObjectPartition,
) -> Result<Vec<RepeatTemplate>, String> {
    let parameter_layout = super::params::derive_layout(graph, config)?;
    let mut grouped = BTreeMap::<FieldId, Vec<&super::objects::SmoothObject>>::new();
    for object in &objects.objects {
        if object.repeat_instances.is_empty() {
            continue;
        }
        grouped.entry(object.source_root).or_default().push(object);
    }
    grouped
        .into_iter()
        .map(|(source_root, objects)| {
            let object = objects[0].id;
            let instances = objects
                .iter()
                .map(|object| {
                    let translations = object
                        .repeat_instances
                        .iter()
                        .map(|instance| {
                            Ok(AffineTranslationProgram {
                                repeat_field: instance.repeat_field,
                                axis: instance.axis,
                                first: instance.first,
                                index: instance.index,
                                period: instance.period,
                                translation: scaled_index(
                                    instance.first,
                                    instance.index,
                                    instance.period,
                                )?,
                            })
                        })
                        .collect::<Result<Vec<_>, String>>()?;
                    Ok(RepeatInstanceProgram {
                        object: object.id,
                        translations,
                    })
                })
                .collect::<Result<Vec<_>, String>>()?;
            let mut families =
                BTreeMap::<(FieldId, Axis, u64, u64), BTreeMap<i32, F64Interval>>::new();
            for instance in &instances {
                for translation in &instance.translations {
                    families
                        .entry((
                            translation.repeat_field,
                            translation.axis,
                            translation.period.lo.to_bits(),
                            translation.period.hi.to_bits(),
                        ))
                        .or_default()
                        .insert(translation.index, translation.translation);
                }
            }
            let mut wrap_events = Vec::new();
            for ((repeat_field, axis, _, _), translations) in families {
                if family_may_cross_wrap(graph, config, &parameter_layout, &objects, repeat_field)?
                {
                    let indices = translations.keys().copied().collect::<Vec<_>>();
                    for pair in indices.windows(2) {
                        if pair[0].checked_add(1) != Some(pair[1]) {
                            continue;
                        }
                        let left = translations[&pair[0]];
                        let right = translations[&pair[1]];
                        let boundary = left
                            .add_outward(right)?
                            .mul_outward(F64Interval::point(0.5)?)?;
                        wrap_events.push(WrapBoundaryEventFamily {
                            repeat_field,
                            axis,
                            left_index: pair[0],
                            right_index: pair[1],
                            boundary,
                        });
                    }
                }
            }
            let instance_count = u32::try_from(instances.len())
                .map_err(|_| "pixels::repeat: instance count overflow".to_string())?;
            let affine_translation_count =
                instances.iter().try_fold(0_u32, |count, instance| {
                    count
                        .checked_add(u32::try_from(instance.translations.len()).map_err(|_| {
                            "pixels::repeat: translation count overflow".to_string()
                        })?)
                        .ok_or_else(|| "pixels::repeat: translation count overflow".to_string())
                })?;
            let wrap_event_families = u32::try_from(wrap_events.len())
                .map_err(|_| "pixels::repeat: wrap-event count overflow".to_string())?;
            Ok(RepeatTemplate {
                object,
                source_root,
                instance_count,
                affine_translation_count,
                wrap_event_families,
                instances,
                wrap_events,
                certificate_must_fix_instance: true,
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn negative_translation_and_boundary_intervals_follow_source_f32_instances() {
        let period = F64Interval::new(2.0, 3.0).unwrap();
        let translation = scaled_index(-2, -2, period).unwrap();
        assert!(translation.lo <= -6.0);
        assert!(translation.hi >= -4.0);
        let right = scaled_index(-2, -1, period).unwrap();
        let boundary = translation
            .add_outward(right)
            .unwrap()
            .mul_outward(F64Interval::point(0.5).unwrap())
            .unwrap();
        assert!(boundary.lo <= -4.5);
        assert!(boundary.hi >= -3.0);
    }
}
