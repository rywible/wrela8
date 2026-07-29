//! Integrity Phase 2 Item N — Lane 3 agreement oracle.
//!
//! Lane 2 dumps an in-guest `lane2 hits=` transcript line from the
//! placed `LANE2` counter page. Lane 3 is the host-side cross-check:
//! after halt the VMM reads that same page out of guest DRAM and
//! compares the non-zero hit vector to the transcript line (diff-eval
//! shape: two independent observations of the same run must agree).
//!
//! Fail closed: missing line, empty vectors, or any id/count mismatch
//! is disagreement. Empty==empty is **not** agreement — the named
//! control case must exercise counters.

use wrela_machine::layout::DRAM_BASE;

/// Must match `stdlib/core/runtime.wr` `@placed(0x40008800)` `LANE2`.
pub const LANE2_BASE: u64 = 0x4000_8800;

/// Must match `wrela_compiler::rtconfig::BLOCK_POOL_COUNT` / runtime import.
pub const LANE2_POOL_COUNT: usize = 1024;

/// Compact hit vector: non-zero `(id, count)` pairs in ascending id order.
pub type HitVec = Vec<(u32, u64)>;

/// Read Lane 2 counters from host-mapped guest DRAM after halt.
///
/// Layout (`Lane2Counters`): `enabled: u64` at offset 0, then
/// `hits: [u64; LANE2_POOL_COUNT]`. Returns only non-zero hits when
/// `enabled != 0`; otherwise empty (Lane 2 emission was off).
pub fn read_lane2_hits(host_ram: *const u8) -> HitVec {
    let base_off = (LANE2_BASE - DRAM_BASE) as usize;
    let enabled = unsafe { read_u64(host_ram, base_off) };
    if enabled == 0 {
        return Vec::new();
    }
    let mut out = Vec::new();
    for i in 0..LANE2_POOL_COUNT {
        let c = unsafe { read_u64(host_ram, base_off + 8 + i * 8) };
        if c != 0 {
            out.push((i as u32, c));
        }
    }
    out
}

unsafe fn read_u64(host_ram: *const u8, off: usize) -> u64 {
    unsafe { std::ptr::read_unaligned(host_ram.add(off) as *const u64) }
}

/// Parse a guest `lane2 hits=<id>:<count>,…` line into a hit vector.
pub fn parse_lane2_hits_line(line: &str) -> Result<HitVec, String> {
    let rest = line
        .strip_prefix("lane2 hits=")
        .ok_or_else(|| format!("not a lane2 hits line: {line:?}"))?;
    if rest.is_empty() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    let mut prev_id: Option<u32> = None;
    for part in rest.split(',') {
        let (id_s, count_s) = part.split_once(':').ok_or_else(|| {
            format!("lane2 hits: malformed pair {part:?} (want id:count)")
        })?;
        let id: u32 = id_s
            .parse()
            .map_err(|e| format!("lane2 hits: bad id {id_s:?}: {e}"))?;
        let count: u64 = count_s
            .parse()
            .map_err(|e| format!("lane2 hits: bad count {count_s:?}: {e}"))?;
        if count == 0 {
            return Err(format!(
                "lane2 hits: zero count for id={id} (dump emits non-zero only)"
            ));
        }
        if let Some(p) = prev_id {
            if id <= p {
                return Err(format!(
                    "lane2 hits: ids must ascend (saw {id} after {p})"
                ));
            }
        }
        prev_id = Some(id);
        out.push((id, count));
    }
    Ok(out)
}

/// Format a hit vector as the guest dump body (`id:count,id:count,…`).
pub fn format_hits(hits: &[(u32, u64)]) -> String {
    hits.iter()
        .map(|(id, c)| format!("{id}:{c}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// Lane 2 (guest transcript dump) vs Lane 3 (host DRAM snapshot).
///
/// Fail closed on missing line, empty either side, or any mismatch.
pub fn agree_lane2_vs_host(transcript: &str, host_hits: &[(u32, u64)]) -> Result<(), String> {
    let lane2_line = transcript
        .lines()
        .find(|l| l.starts_with("lane2 hits="))
        .ok_or_else(|| {
            "diff-block-count: Lane 2 transcript line missing \
             (guest `__wrela_lane2_dump` did not print `lane2 hits=` — \
             was `--block-count` emission enabled?)"
                .to_string()
        })?;
    let guest = parse_lane2_hits_line(lane2_line)?;
    if guest.is_empty() {
        return Err(
            "diff-block-count: Lane 2 vector is empty — control case must \
             exercise block counters (fail closed; empty==empty is not agreement)"
                .to_string(),
        );
    }
    if host_hits.is_empty() {
        return Err(
            "diff-block-count: Lane 3 host DRAM hit map is empty — VMM \
             snapshot saw LANE2.enabled=0 or all-zero hits (fail closed)"
                .to_string(),
        );
    }
    if guest.as_slice() != host_hits {
        return Err(format!(
            "diff-block-count: Lane 2 / Lane 3 DISAGREEMENT on control case:\n  \
             lane2 (guest dump): {}\n  \
             lane3 (host DRAM):  {}",
            format_hits(&guest),
            format_hits(host_hits)
        ));
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_lane2_hits_line_round_trips() {
        let line = "lane2 hits=0:3,2:1,168:17";
        let v = parse_lane2_hits_line(line).expect("parses");
        assert_eq!(v, vec![(0, 3), (2, 1), (168, 17)]);
        assert_eq!(format!("lane2 hits={}", format_hits(&v)), line);
    }

    #[test]
    fn parse_rejects_descending_ids() {
        assert!(parse_lane2_hits_line("lane2 hits=2:1,1:1").is_err());
    }

    #[test]
    fn agree_fails_closed_on_missing_line() {
        let err = agree_lane2_vs_host("test turns: ok\n1 passed, 0 failed\n", &[(0, 1)])
            .expect_err("missing line");
        assert!(err.contains("missing"), "{err}");
    }

    #[test]
    fn agree_fails_closed_on_empty_guest() {
        let err = agree_lane2_vs_host("lane2 hits=\n", &[(0, 1)]).expect_err("empty guest");
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn agree_fails_closed_on_empty_host() {
        let err = agree_lane2_vs_host("lane2 hits=0:1\n", &[]).expect_err("empty host");
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn agree_fails_closed_on_mismatch() {
        let err = agree_lane2_vs_host("lane2 hits=0:3,1:1\n", &[(0, 3), (1, 2)])
            .expect_err("mismatch");
        assert!(err.contains("DISAGREEMENT"), "{err}");
    }

    #[test]
    fn agree_passes_when_vectors_match() {
        let hits = vec![(0, 3), (2, 1), (168, 17)];
        agree_lane2_vs_host(
            &format!("test x: ok\nlane2 hits={}\n", format_hits(&hits)),
            &hits,
        )
        .expect("agree");
    }
}
