use wrela_machine::pixels::PresentedFrame;

pub fn frame_log_bytes(frames: &[PresentedFrame]) -> Vec<u8> {
    let mut out = Vec::new();
    for frame in frames {
        out.extend_from_slice(
            format!("display frame={} digest={}\n", frame.sequence, frame.digest).as_bytes(),
        );
    }
    out
}

pub fn verify_frame_replay(
    recorded: &[PresentedFrame],
    replayed: &[PresentedFrame],
) -> Result<(), String> {
    let expected = frame_log_bytes(recorded);
    let actual = frame_log_bytes(replayed);
    if expected == actual {
        Ok(())
    } else {
        Err(format!(
            "display replay differs: expected {} byte(s), got {} byte(s)",
            expected.len(),
            actual.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(sequence: u64, digest: &str) -> PresentedFrame {
        PresentedFrame {
            sequence,
            digest: digest.to_string(),
            bgra: Vec::new(),
        }
    }

    #[test]
    fn replay_compares_frame_order_and_digest_bytes() {
        let recorded = vec![frame(0, "aa"), frame(1, "bb")];
        assert!(verify_frame_replay(&recorded, &recorded).is_ok());
        assert!(verify_frame_replay(&recorded, &[frame(1, "bb"), frame(0, "aa")]).is_err());
        assert!(verify_frame_replay(&recorded, &[frame(0, "ab"), frame(1, "bb")]).is_err());
    }
}
