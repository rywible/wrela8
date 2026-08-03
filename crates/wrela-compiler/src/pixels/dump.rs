//! Stable textual dump boundaries for the Pixels compiler stages.

use super::PlaneSkeleton;
use super::config::{RendererConfig, RendererConfigs};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PixelsDumpStage {
    FieldGraph,
    FrameProgram,
    RenderLayout,
}

pub fn dump_zero_renderers(stage: PixelsDumpStage) -> String {
    let header = match stage {
        PixelsDumpStage::FieldGraph => "FieldGraph v1",
        PixelsDumpStage::FrameProgram => "FrameProgram v1",
        PixelsDumpStage::RenderLayout => "RenderLayout v1",
    };
    format!("{header}\nRenderers count=0\n")
}

fn stage_header(stage: PixelsDumpStage) -> &'static str {
    match stage {
        PixelsDumpStage::FieldGraph => "FieldGraph v1",
        PixelsDumpStage::FrameProgram => "FrameProgram v1",
        PixelsDumpStage::RenderLayout => "RenderLayout v1",
    }
}

fn dump_config(config: &RendererConfig, out: &mut String) {
    out.push_str(&format!(
        "  Renderer index={} params={} field={} material={} material_type={}\n",
        config.declaration_index,
        crate::sema::types::render_type(&config.params_type),
        config.field,
        config.material,
        crate::sema::types::render_type(&config.material_type),
    ));
    out.push_str("    Compilation status=not-run\n");
    out.push_str(&format!(
        "    Display ref=driver#{}\n    Mode width={} height={} refresh_hz={} shade_hz={}\n",
        config.display_index, config.width, config.height, config.refresh_hz, config.shade_hz,
    ));
    out.push_str(&format!(
        "    Profile value={} tone_curve={}\n",
        config.profile, config.tone_curve
    ));
    out.push_str(&format!(
        "    Depth near={} far={}\n    World min=[{},{},{}] max=[{},{},{}]\n",
        config.near,
        config.far,
        config.world_min.x,
        config.world_min.y,
        config.world_min.z,
        config.world_max.x,
        config.world_max.y,
        config.world_max.z,
    ));
    out.push_str(&format!(
        "    Contracts camera_max_motion={} light_capacity={} light_kinds=[{}] exposure=[{},{}] environment=[{},{},{}]-[{},{},{}] ao={} probes={} probe_initialization_worst_case_ms={} initialization_deadline_ms={}\n",
        config.camera_max_motion,
        config.light_capacity,
        config.light_kinds.join(","),
        config.exposure.min,
        config.exposure.max,
        config.environment.min[0],
        config.environment.min[1],
        config.environment.min[2],
        config.environment.max[0],
        config.environment.max[1],
        config.environment.max[2],
        config.ao_enabled,
        config.probes_enabled,
        config.probe_initialization_worst_case_ms,
        config.initialization_deadline_ms,
    ));
    for parameter in &config.parameter_contracts {
        let range = parameter
            .range
            .exact_integer
            .map(|(min, max)| format!("{min},{max}"))
            .unwrap_or_else(|| format!("{},{}", parameter.range.min, parameter.range.max));
        out.push_str(&format!(
            "    Parameter path={:?} type={} range=[{}]",
            parameter.path,
            crate::sema::types::render_type(&parameter.ty),
            range,
        ));
        if let Some(rate) = parameter.rate {
            out.push_str(&format!(
                " rate=[{},{}]",
                rate.max_delta, rate.max_second_delta
            ));
        } else {
            out.push_str(" rate=none");
        }
        out.push('\n');
    }
}

pub fn dump_uncompiled_configs(
    stage: PixelsDumpStage,
    configs: &RendererConfigs,
    renderer_index: Option<usize>,
) -> String {
    let mut out = format!(
        "{}\nRenderers count={}\n",
        stage_header(stage),
        configs.renderers.len()
    );
    if let Some(index) = renderer_index {
        if let Some(config) = configs.renderers.get(index) {
            dump_config(config, &mut out);
        }
    } else {
        for config in &configs.renderers {
            dump_config(config, &mut out);
        }
    }
    out
}

pub fn dump_field_graph(skeleton: &PlaneSkeleton) -> String {
    format!(
        "FieldGraph v1\n  Renderer index={} field={} material={}\n  Compilation status=plane-skeleton\n  Field id=f0 kind=Plane\n  Object id=o0 root=f0 material_type={}\n  Csg root=o0\n",
        skeleton.renderer_index, skeleton.field, skeleton.material, skeleton.material_type
    )
}

pub fn dump_frame_program(skeleton: &PlaneSkeleton) -> String {
    format!(
        "FrameProgram v1 renderer={} digest={}\n  Header magic=WRELAPX\\0 version=1 bytes=80 flags=[] total_bytes=80\n  Directory count=0 offset=80\n  WalkingSkeleton version=P-1 semantic_seed={} storage=generated-actor\n",
        skeleton.renderer_index, skeleton.frame_program_digest, skeleton.semantic_digest
    )
}

pub fn dump_render_layout(skeleton: &PlaneSkeleton) -> String {
    let renderer = crate::codegen::emit_pixels_plane_renderer(
        &skeleton.frame_program,
        &skeleton.semantic_seed,
    );
    let code_bytes = renderer.code.len() * 4;
    let memory_bytes = wrela_machine::pixels::FRAME_BYTES
        + wrela_machine::pixels::CONTROL_BYTES
        + wrela_machine::pixels::QUEUE_CAPACITY as usize * wrela_machine::pixels::TILE_BYTES
        + skeleton.frame_program.len();
    format!(
        "RenderLayout v1\n  Renderer index={}\n    FrameProgram base={:#010x} size=80\n    GeneratedActor type=Renderer entry={} worker_count=0\n    Display ref={}\n    Mode width={} height={} refresh_hz={} shade_hz={}\n    Tile owner=renderer#0 range=[0,1)\n    Buffer base={:#010x} bytes={} format=BGRA8\n    Baseline code_bytes={} memory_bytes={} frame_cost_instructions={}\n",
        skeleton.renderer_index,
        wrela_machine::pixels::FRAME_PROGRAM_BASE,
        crate::codegen::PIXELS_RENDERER_SYMBOL,
        skeleton.display,
        skeleton.width,
        skeleton.height,
        skeleton.refresh_hz,
        skeleton.shade_hz,
        wrela_machine::pixels::FRAMEBUFFER_BASE,
        wrela_machine::pixels::FRAME_BYTES,
        code_bytes,
        memory_bytes,
        renderer.code.len()
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zero_renderer_dumps_are_complete_and_byte_stable() {
        assert_eq!(
            dump_zero_renderers(PixelsDumpStage::FieldGraph),
            "FieldGraph v1\nRenderers count=0\n"
        );
        assert_eq!(
            dump_zero_renderers(PixelsDumpStage::FrameProgram),
            "FrameProgram v1\nRenderers count=0\n"
        );
        assert_eq!(
            dump_zero_renderers(PixelsDumpStage::RenderLayout),
            "RenderLayout v1\nRenderers count=0\n"
        );
    }

    #[test]
    fn uncompiled_dump_keeps_stage_boundary_explicit() {
        let configs = RendererConfigs {
            renderers: vec![RendererConfig {
                declaration_index: 0,
                params_type: crate::sema::types::Type::U32,
                field: "world".to_string(),
                material: "shade".to_string(),
                material_type: crate::sema::types::Type::U8,
                display_index: 0,
                width: 64,
                height: 32,
                refresh_hz: 60,
                shade_hz: 30,
                profile: "AaaByteExact".to_string(),
                tone_curve: "Linear".to_string(),
                near: 0.1,
                far: 10.0,
                world_min: crate::pixels::config::Vec3Config {
                    x: -1.0,
                    y: -1.0,
                    z: -1.0,
                },
                world_max: crate::pixels::config::Vec3Config {
                    x: 1.0,
                    y: 1.0,
                    z: 1.0,
                },
                camera_max_motion: 0.0,
                light_capacity: 0,
                light_kinds: vec![],
                exposure: crate::pixels::config::ScalarRangeConfig { min: 0.0, max: 1.0 },
                environment: crate::pixels::config::RgbRangeConfig {
                    min: [0.0; 3],
                    max: [1.0; 3],
                },
                ao_enabled: false,
                probes_enabled: false,
                probe_initialization_worst_case_ms: 0,
                initialization_deadline_ms: 1,
                parameter_contracts: vec![],
            }],
        };
        let dump = dump_uncompiled_configs(PixelsDumpStage::FieldGraph, &configs, None);
        assert!(dump.contains("Compilation status=not-run"));
        assert!(!dump.contains("Field id="));
    }
}
