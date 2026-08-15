use std::fmt::Write as _;

use wrela_machine::pixels::PresentedFrame;

use crate::display::DisplayEvent;

pub const DISPLAY_REPLAY_REVISION: u16 = 1;

pub(crate) fn frame_divergence_class(
    expected: &PresentedFrame,
    actual: &PresentedFrame,
) -> Option<&'static str> {
    [
        (
            "frame-order",
            expected.renderer_index != actual.renderer_index
                || expected.sequence != actual.sequence
                || expected.generation != actual.generation
                || expected.released_generation != actual.released_generation,
        ),
        (
            "mode-format",
            expected.mode != actual.mode || expected.format != actual.format,
        ),
        (
            "tile-descriptor",
            expected.tile_descriptor_digest != actual.tile_descriptor_digest,
        ),
        ("visible", expected.visible_digest != actual.visible_digest),
        (
            "raw-tile",
            expected.raw_tile_digest != actual.raw_tile_digest,
        ),
        (
            "vsync-checkpoint",
            expected.vsync_id != actual.vsync_id || expected.checkpoint != actual.checkpoint,
        ),
    ]
    .into_iter()
    .find_map(|(class, differs)| differs.then_some(class))
}

fn hex(bytes: &[u8; 32]) -> String {
    let mut out = String::with_capacity(64);
    for byte in bytes {
        write!(out, "{byte:02x}").expect("String writes cannot fail");
    }
    out
}

pub fn frame_log_bytes(frames: &[PresentedFrame]) -> Vec<u8> {
    let mut out = String::new();
    for frame in frames {
        writeln!(
            out,
            "display v{} renderer={} frame={} generation={} released={} mode={}x{}@{} format={} descriptor={} visible={} raw={} vsync={} checkpoint={}",
            DISPLAY_REPLAY_REVISION,
            frame.renderer_index,
            frame.sequence,
            frame.generation,
            frame
                .released_generation
                .map_or_else(|| "none".to_string(), |value| value.to_string()),
            frame.mode.width,
            frame.mode.height,
            frame.mode.refresh_hz,
            frame.format,
            hex(&frame.tile_descriptor_digest),
            hex(&frame.visible_digest),
            hex(&frame.raw_tile_digest),
            frame.vsync_id,
            frame.checkpoint,
        )
        .expect("String writes cannot fail");
    }
    out.into_bytes()
}

pub fn display_event_log_bytes(events: &[DisplayEvent]) -> Vec<u8> {
    let mut out = String::new();
    for event in events {
        match event {
            DisplayEvent::Presented {
                renderer_index,
                sequence,
                generation,
                visible_digest,
                raw_tile_digest,
            } => {
                writeln!(
                    out,
                    "display-event presented renderer={renderer_index} frame={sequence} generation={generation} visible={} raw={}",
                    hex(visible_digest),
                    hex(raw_tile_digest),
                )
                .expect("String writes cannot fail");
            }
            DisplayEvent::Rejected { sequence, error } => {
                writeln!(
                    out,
                    "display-event rejected frame={} error={error}",
                    sequence.map_or_else(|| "unknown".to_string(), |value| value.to_string()),
                )
                .expect("String writes cannot fail");
            }
        }
    }
    out.into_bytes()
}

/// Rejected submissions are deterministic diagnostics, not frame outputs.
/// Keep them in the boot transcript so record/replay covers malformed display
/// input without duplicating successful frames already emitted by
/// `frame_log_bytes`.
pub fn rejected_display_event_log_bytes(events: &[DisplayEvent]) -> Vec<u8> {
    let rejected = events
        .iter()
        .filter(|event| matches!(event, DisplayEvent::Rejected { .. }))
        .cloned()
        .collect::<Vec<_>>();
    display_event_log_bytes(&rejected)
}

pub fn verify_frame_replay(
    recorded: &[PresentedFrame],
    replayed: &[PresentedFrame],
) -> Result<(), String> {
    if recorded.len() != replayed.len() {
        return Err(format!(
            "display replay differs: expected {} successful frame(s), got {}",
            recorded.len(),
            replayed.len()
        ));
    }
    for (index, (expected, actual)) in recorded.iter().zip(replayed).enumerate() {
        let frame = expected.sequence;
        if let Some(class) = frame_divergence_class(expected, actual) {
            return Err(format!(
                "display replay diverged at output index {index}, frame {frame}, class {class}"
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(sequence: u64) -> PresentedFrame {
        PresentedFrame {
            renderer_index: 2,
            sequence,
            generation: (sequence & 1) as u8,
            released_generation: sequence.checked_sub(1).map(|value| (value & 1) as u8),
            mode: wrela_machine::pixels::DisplayModeV1::SEED,
            format: wrela_machine::pixels::FORMAT_BGRA8_SRGB,
            tile_descriptor_digest: [1; 32],
            visible_digest: [2; 32],
            raw_tile_digest: [3; 32],
            digest: String::new(),
            vsync_id: sequence,
            checkpoint: sequence + 10,
            bgra: Vec::new(),
        }
    }

    #[test]
    fn replay_compares_all_exact_output_classes_and_names_first_divergence() {
        let recorded = vec![frame(0), frame(1)];
        assert_eq!(verify_frame_replay(&recorded, &recorded), Ok(()));
        let mut changed = recorded.clone();
        changed[1].visible_digest[7] ^= 1;
        assert!(
            verify_frame_replay(&recorded, &changed)
                .unwrap_err()
                .contains("frame 1, class visible")
        );
        changed = recorded.clone();
        changed[0].raw_tile_digest[7] ^= 1;
        assert!(
            verify_frame_replay(&recorded, &changed)
                .unwrap_err()
                .contains("frame 0, class raw-tile")
        );
    }

    #[test]
    fn successful_and_rejected_events_are_distinct() {
        let events = [
            DisplayEvent::Rejected {
                sequence: Some(4),
                error: "bad descriptor".into(),
            },
            DisplayEvent::Presented {
                renderer_index: 1,
                sequence: 4,
                generation: 0,
                visible_digest: [1; 32],
                raw_tile_digest: [2; 32],
            },
        ];
        let log = String::from_utf8(display_event_log_bytes(&events)).unwrap();
        assert!(log.contains("rejected frame=4"));
        assert!(log.contains("presented renderer=1 frame=4"));
        let rejected = String::from_utf8(rejected_display_event_log_bytes(&events)).unwrap();
        assert!(rejected.contains("rejected frame=4"));
        assert!(!rejected.contains("presented"));
    }
}
