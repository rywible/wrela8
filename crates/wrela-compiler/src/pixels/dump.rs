//! Stable textual dump boundaries for the Pixels compiler stages.

use super::PlaneSkeleton;

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

pub fn dump_field_graph(skeleton: &PlaneSkeleton) -> String {
    format!(
        "FieldGraph v1\n  Renderer index={} field={} material={}\n  Compilation status=plane-skeleton\n  Field id=f0 kind=Plane\n  Object id=o0 root=f0 material_type={}\n  Csg root=o0\n",
        skeleton.renderer_index, skeleton.field, skeleton.material, skeleton.material_type
    )
}

pub fn dump_frame_program(skeleton: &PlaneSkeleton) -> String {
    format!(
        "FrameProgram v1 renderer={} digest={}\n  Header magic=WRELAPX\\0 version=1 bytes=80 flags=[plane-skeleton] total_bytes=80\n  Directory count=0 offset=80\n  Record kind=Plane id=f0 semantic_digest={}\n",
        skeleton.renderer_index, skeleton.frame_program_digest, skeleton.semantic_digest
    )
}

pub fn dump_render_layout(skeleton: &PlaneSkeleton) -> String {
    let renderer = crate::codegen::emit_pixels_plane_renderer(&skeleton.frame_program);
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
}
