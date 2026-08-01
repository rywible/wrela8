use wrela_machine::layout::DRAM_BASE;

pub const LANE2_BASE: u64 = 0x4000_8800;

pub const LANE2_POOL_COUNT: usize = 3072;

pub type HitVec = Vec<(u32, u64)>;

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

pub const TRUNCATED_MARKER: &str = " truncated=";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Lane2Line {
    pub hits: HitVec,
    pub truncated: Option<u64>,
}

pub fn parse_lane2_line(line: &str) -> Result<Lane2Line, String> {
    let rest = line
        .strip_prefix("lane2 hits=")
        .ok_or_else(|| format!("not a lane2 hits line: {line:?}"))?;
    let (body, truncated) = match rest.find(TRUNCATED_MARKER) {
        Some(at) => {
            let n_text = &rest[at + TRUNCATED_MARKER.len()..];
            let n: u64 = n_text
                .trim()
                .parse()
                .map_err(|e| format!("lane2 hits: bad truncated count {n_text:?}: {e}"))?;
            (&rest[..at], Some(n))
        }
        None => (rest, None),
    };
    Ok(Lane2Line {
        hits: parse_lane2_hits_line(&format!("lane2 hits={body}"))?,
        truncated,
    })
}

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
        let (id_s, count_s) = part
            .split_once(':')
            .ok_or_else(|| format!("lane2 hits: malformed pair {part:?} (want id:count)"))?;
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
                return Err(format!("lane2 hits: ids must ascend (saw {id} after {p})"));
            }
        }
        prev_id = Some(id);
        out.push((id, count));
    }
    Ok(out)
}

pub fn format_hits(hits: &[(u32, u64)]) -> String {
    hits.iter()
        .map(|(id, c)| format!("{id}:{c}"))
        .collect::<Vec<_>>()
        .join(",")
}

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
    let parsed = parse_lane2_line(lane2_line)?;
    let guest = parsed.hits;
    let Some(truncated) = parsed.truncated else {
        return Err(format!(
            "diff-block-count: Lane 2 line carries no `{}` marker — decision 1610 makes the \
             marker unconditional, so this is a pre-1610 guest dump and the line cannot be \
             distinguished from a complete one (fail closed): {lane2_line:?}",
            TRUNCATED_MARKER.trim()
        ));
    };
    if guest.is_empty() {
        return Err(
            "diff-block-count: Lane 2 vector is empty — control case must \
             exercise block counters (fail closed; empty==empty is not agreement)"
                .to_string(),
        );
    }
    if host_hits.is_empty() {
        return Err("diff-block-count: Lane 3 host DRAM hit map is empty — VMM \
             snapshot saw LANE2.enabled=0 or all-zero hits (fail closed)"
            .to_string());
    }
    if host_hits.len() < guest.len() {
        return Err(format!(
            "diff-block-count: Lane 2 / Lane 3 DISAGREEMENT: the transcript carries {} pair(s) \
             but the host snapshot only has {}",
            guest.len(),
            host_hits.len()
        ));
    }
    if guest.as_slice() != &host_hits[..guest.len()] {
        return Err(format!(
            "diff-block-count: Lane 2 / Lane 3 DISAGREEMENT on control case:\n  \
             lane2 (guest dump): {}\n  \
             lane3 (host DRAM):  {}",
            format_hits(&guest),
            format_hits(&host_hits[..guest.len()])
        ));
    }
    let host_dropped = (host_hits.len() - guest.len()) as u64;
    if truncated != host_dropped {
        return Err(format!(
            "diff-block-count: Lane 2 / Lane 3 DISAGREEMENT on the truncation count: the \
             transcript says `truncated={truncated}` but the host snapshot carries \
             {host_dropped} pair(s) the transcript did not print (a truncating line must not \
             pass by being compared to itself)"
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
    fn parse_lane2_line_reads_the_truncation_marker() {
        let p = parse_lane2_line("lane2 hits=0:3,2:1 truncated=481").expect("parses");
        assert_eq!(p.hits, vec![(0, 3), (2, 1)]);
        assert_eq!(p.truncated, Some(481));
        let none = parse_lane2_line("lane2 hits=0:3").expect("parses");
        assert_eq!(none.truncated, None);
        let zero = parse_lane2_line("lane2 hits=0:3 truncated=0").expect("parses");
        assert_eq!(zero.truncated, Some(0));
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
    fn agree_fails_closed_when_the_truncation_marker_is_absent() {
        let err = agree_lane2_vs_host("lane2 hits=0:1\n", &[(0, 1)]).expect_err("no marker");
        assert!(err.contains("truncated"), "{err}");
    }

    #[test]
    fn agree_fails_closed_on_empty_guest() {
        let err =
            agree_lane2_vs_host("lane2 hits= truncated=0\n", &[(0, 1)]).expect_err("empty guest");
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn agree_fails_closed_on_empty_host() {
        let err = agree_lane2_vs_host("lane2 hits=0:1 truncated=0\n", &[]).expect_err("empty host");
        assert!(err.contains("empty"), "{err}");
    }

    #[test]
    fn agree_fails_closed_on_mismatch() {
        let err = agree_lane2_vs_host("lane2 hits=0:3,1:1 truncated=0\n", &[(0, 3), (1, 2)])
            .expect_err("mismatch");
        assert!(err.contains("DISAGREEMENT"), "{err}");
    }

    #[test]
    fn agree_passes_when_vectors_match() {
        let hits = vec![(0, 3), (2, 1), (168, 17)];
        agree_lane2_vs_host(
            &format!(
                "test x: ok\nlane2 hits={} truncated=0\n",
                format_hits(&hits)
            ),
            &hits,
        )
        .expect("agree");
    }

    #[test]
    fn agree_accepts_a_truncated_line_whose_marker_accounts_for_the_tail() {
        let host = vec![(0, 3), (2, 1), (168, 17), (300, 4)];
        let printed = &host[..2];
        agree_lane2_vs_host(
            &format!("lane2 hits={} truncated=2\n", format_hits(printed)),
            &host,
        )
        .expect("agree over the pairs the transcript carries");
    }

    #[test]
    fn agree_fails_closed_when_the_truncation_count_does_not_match_the_host_tail() {
        let host = vec![(0, 3), (2, 1), (168, 17), (300, 4)];
        let printed = &host[..2];
        let err = agree_lane2_vs_host(
            &format!("lane2 hits={} truncated=0\n", format_hits(printed)),
            &host,
        )
        .expect_err("a silent drop must not agree");
        assert!(err.contains("truncation count"), "{err}");

        let err = agree_lane2_vs_host(
            &format!("lane2 hits={} truncated=7\n", format_hits(printed)),
            &host,
        )
        .expect_err("an over-count must not agree");
        assert!(err.contains("truncation count"), "{err}");
    }

    #[test]
    fn agree_fails_closed_when_the_host_carries_fewer_pairs_than_the_transcript() {
        let err = agree_lane2_vs_host("lane2 hits=0:1,2:1 truncated=0\n", &[(0, 1)])
            .expect_err("short host");
        assert!(err.contains("DISAGREEMENT"), "{err}");
    }
}
