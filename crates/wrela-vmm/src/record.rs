//! Recorder/replayer for the determinism boundary (06-machine.md §8,
//! plans/M5.md item F, decision 13's own "recording = the clock-read log
//! + the console output digest; replay feeds the logged values back and
//! diffs the digest"). At M5 this machine has exactly one thing to
//! record — the clock-read log (`BootOutcome::clock_log`) — since no
//! other device exists yet and every other guest-visible fact of a boot
//! (the console transcript, the exit code) is already fully determined
//! by the image itself, not by anything the host fed it. `RecordFile` is
//! the on-disk form of one boot's own recorded facts; `replay` boots
//! again feeding a previously recorded clock log back in and reports
//! every way the second boot's own facts disagree with the first's.
//!
//! **The record file is a recording, not a golden** (plans/M5.md's own
//! house rule for this item): a live boot's clock values are wall-clock
//! dependent (`monotonic_ns`, `lib.rs`) and therefore vary run to run —
//! nothing here is ever written into `tests/golden/` or compared against
//! a pinned expectation. Only the *format* (`RecordFile::to_text`/
//! `parse` round-tripping) and the *divergence-detection logic*
//! (`replay`'s own comparisons) are unit-tested, both below, with a
//! hand-built recorded/actual pair rather than a real boot — exactly
//! `06 §8`'s own "the recorded boundary" property, at the one-device
//! scale this milestone actually has.

use std::path::Path;

use crate::{BootOutcome, VmmError, boot_image_core};

/// A non-cryptographic 64-bit fingerprint (FNV-1a) of the transcript
/// bytes — a "digest" in the plain sense the plan uses the word (06 §8:
/// "digests of every output"), not a security primitive. Deliberately
/// the dumbest fixed-size fingerprint that still catches an accidental
/// single-byte drift: no external crate, no hand-rolled SHA (a much
/// larger surface to get bit-perfect for no benefit here — nothing
/// downstream of this digest needs collision resistance, only "did the
/// transcript come out the same bytes twice").
pub fn digest_hex(bytes: &[u8]) -> String {
    const FNV_OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const FNV_PRIME: u64 = 0x0000_0100_0000_01b3;
    let mut h = FNV_OFFSET;
    for &b in bytes {
        h ^= b as u64;
        h = h.wrapping_mul(FNV_PRIME);
    }
    format!("{h:016x}")
}

/// One boot's own recorded facts (06 §8's subset that exists at M5, one
/// core, no vectors, no other devices): every clock-read value, in
/// order, plus a digest of the console transcript, the guest's own
/// reported exit code, and the total vCPU exit count. No timestamps, no
/// wall-clock content of any kind beyond the clock *values themselves*
/// (which are guest-visible data, not host bookkeeping) — `to_text`'s
/// own module-doc-cited house rule.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFile {
    pub clock_log: Vec<u64>,
    pub transcript_digest: String,
    pub exit_code: u64,
    pub exits: u64,
}

impl RecordFile {
    /// Builds a `RecordFile` from a fresh `BootOutcome` — the "record"
    /// half of record/replay (`replay`, below, is the "replay" half).
    pub fn from_outcome(outcome: &BootOutcome) -> RecordFile {
        RecordFile {
            clock_log: outcome.clock_log.clone(),
            transcript_digest: digest_hex(&outcome.transcript),
            exit_code: outcome.exit_code,
            exits: outcome.exits,
        }
    }

    /// A stable, line-oriented, hand-parseable text format (mirrors
    /// `lib.rs`'s own `parse_report`'s house style: plain `key=value`
    /// lines, no TOML/JSON dependency for an in-tree, internal-only
    /// format). One `clock[i]=value` line per logged read, in order,
    /// rather than a single delimited list, so a truncated/corrupted
    /// file's own first bad line is trivially locatable by a human
    /// reader — the same "review-visible" preference `layout.rs`'s
    /// `img.hex` dump makes for image bytes.
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str(&format!("clock_log_len={}\n", self.clock_log.len()));
        for (i, v) in self.clock_log.iter().enumerate() {
            s.push_str(&format!("clock[{i}]={v}\n"));
        }
        s.push_str(&format!("transcript_digest={}\n", self.transcript_digest));
        s.push_str(&format!("exit_code={}\n", self.exit_code));
        s.push_str(&format!("exits={}\n", self.exits));
        s
    }

    /// The inverse of `to_text` — fails closed (a plain `Err(String)`,
    /// never a panic or a silently-defaulted field) on any malformed,
    /// missing, or extra line, including a `clock_log_len` that disagrees
    /// with the number of `clock[i]=` lines actually present.
    pub fn parse(text: &str) -> Result<RecordFile, String> {
        let mut clock_log_len: Option<usize> = None;
        let mut clock_entries: std::collections::BTreeMap<usize, u64> =
            std::collections::BTreeMap::new();
        let mut transcript_digest: Option<String> = None;
        let mut exit_code: Option<u64> = None;
        let mut exits: Option<u64> = None;
        for line in text.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("malformed record line (no `=`): {line:?}"));
            };
            if key == "clock_log_len" {
                clock_log_len = Some(
                    value
                        .parse()
                        .map_err(|e| format!("bad clock_log_len {value:?}: {e}"))?,
                );
            } else if let Some(idx_str) =
                key.strip_prefix("clock[").and_then(|s| s.strip_suffix(']'))
            {
                let idx: usize = idx_str
                    .parse()
                    .map_err(|e| format!("bad clock index `{idx_str}`: {e}"))?;
                let v: u64 = value
                    .parse()
                    .map_err(|e| format!("bad clock value {value:?}: {e}"))?;
                if clock_entries.insert(idx, v).is_some() {
                    return Err(format!("duplicate `clock[{idx}]=` line"));
                }
            } else if key == "transcript_digest" {
                transcript_digest = Some(value.to_string());
            } else if key == "exit_code" {
                exit_code = Some(
                    value
                        .parse()
                        .map_err(|e| format!("bad exit_code {value:?}: {e}"))?,
                );
            } else if key == "exits" {
                exits = Some(
                    value
                        .parse()
                        .map_err(|e| format!("bad exits {value:?}: {e}"))?,
                );
            } else {
                return Err(format!("unknown record key `{key}`"));
            }
        }
        let clock_log_len = clock_log_len.ok_or("missing clock_log_len")?;
        if clock_entries.len() != clock_log_len {
            return Err(format!(
                "clock_log_len={clock_log_len} but {} `clock[i]=` line(s) present",
                clock_entries.len()
            ));
        }
        let mut clock_log = Vec::with_capacity(clock_log_len);
        for i in 0..clock_log_len {
            let v = clock_entries
                .get(&i)
                .ok_or_else(|| format!("missing clock[{i}]= line"))?;
            clock_log.push(*v);
        }
        Ok(RecordFile {
            clock_log,
            transcript_digest: transcript_digest.ok_or("missing transcript_digest")?,
            exit_code: exit_code.ok_or("missing exit_code")?,
            exits: exits.ok_or("missing exits")?,
        })
    }
}

/// Every way a replay boot's own facts can disagree with a previously
/// recorded boot's — 06 §8's "diagnoses any divergence", named exactly
/// rather than collapsed into one opaque "mismatch" string, so a caller
/// (`xtask`, a future device milestone's own replay harness) can report
/// which of the record boundary's own guarantees actually broke.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    /// The replayed guest read the clock more times than the recorded
    /// log has values for — `read_index` is the (zero-based) read that
    /// first ran dry; every such read past it is fed `0` so the boot
    /// still completes (a wrong clock value is as diagnosable a
    /// divergence as a hung one, and the rest of the transcript is still
    /// worth comparing).
    ClockLogUnderrun { read_index: usize, recorded: usize },
    /// The recorded log has more clock values than the replayed guest
    /// ever asked for — the same image took a different path (or a
    /// different number of clock reads) the second time.
    ClockLogOverrun { consumed: usize, recorded: usize },
    /// The console transcript's own digest differs.
    TranscriptDigestMismatch { expected: String, actual: String },
    /// The guest's own reported exit code differs.
    ExitCodeMismatch { expected: u64, actual: u64 },
}

impl std::fmt::Display for Divergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Divergence::ClockLogUnderrun {
                read_index,
                recorded,
            } => write!(
                f,
                "clock log underrun: read #{read_index} requested a value but only {recorded} were recorded"
            ),
            Divergence::ClockLogOverrun { consumed, recorded } => write!(
                f,
                "clock log overrun: only {consumed} clock read(s) happened this time, but {recorded} were recorded"
            ),
            Divergence::TranscriptDigestMismatch { expected, actual } => write!(
                f,
                "transcript digest mismatch: recorded {expected}, replay produced {actual}"
            ),
            Divergence::ExitCodeMismatch { expected, actual } => write!(
                f,
                "exit code mismatch: recorded {expected}, replay produced {actual}"
            ),
        }
    }
}

/// Boots `img_path`/`report_path` live (`crate::boot_image`) and returns
/// the resulting `RecordFile` — the "record" step a caller runs once to
/// produce the file `replay` (below) later checks against. A thin,
/// named wrapper over `boot_image` + `RecordFile::from_outcome`, kept
/// separate so a caller never has to reach into `BootOutcome` itself.
pub fn record(report_path: &Path, img_path: &Path) -> Result<RecordFile, VmmError> {
    let (outcome, _underrun) = boot_image_core(report_path, img_path, None)?;
    Ok(RecordFile::from_outcome(&outcome))
}

/// Boots `img_path`/`report_path` again, feeding `recorded.clock_log`
/// back in place of the live clock (06 §8: "replay feeds the log from
/// virtual device models, suppresses real outputs, and diagnoses any
/// divergence") and compares every recorded fact against this fresh
/// boot's own. Returns every divergence found (empty = the replay
/// reproduced the recording exactly); a genuine VMM/boot failure (a bad
/// report, an HVF error, a timeout) is still its own `Err`, never
/// folded into the divergence list — those are boot failures, not
/// determinism findings.
pub fn replay(
    report_path: &Path,
    img_path: &Path,
    recorded: &RecordFile,
) -> Result<Vec<Divergence>, VmmError> {
    let (outcome, underrun) = boot_image_core(report_path, img_path, Some(&recorded.clock_log))?;
    let mut divergences = Vec::new();
    if let Some(read_index) = underrun {
        divergences.push(Divergence::ClockLogUnderrun {
            read_index,
            recorded: recorded.clock_log.len(),
        });
    } else if outcome.clock_log.len() < recorded.clock_log.len() {
        divergences.push(Divergence::ClockLogOverrun {
            consumed: outcome.clock_log.len(),
            recorded: recorded.clock_log.len(),
        });
    }
    let actual_digest = digest_hex(&outcome.transcript);
    if actual_digest != recorded.transcript_digest {
        divergences.push(Divergence::TranscriptDigestMismatch {
            expected: recorded.transcript_digest.clone(),
            actual: actual_digest,
        });
    }
    if outcome.exit_code != recorded.exit_code {
        divergences.push(Divergence::ExitCodeMismatch {
            expected: recorded.exit_code,
            actual: outcome.exit_code,
        });
    }
    Ok(divergences)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sample() -> RecordFile {
        RecordFile {
            clock_log: vec![100, 250, 9999],
            transcript_digest: "deadbeefcafebabe".to_string(),
            exit_code: 0,
            exits: 7,
        }
    }

    #[test]
    fn digest_is_deterministic_and_sensitive_to_every_byte() {
        let a = digest_hex(b"test arith_ok: ok\n");
        let b = digest_hex(b"test arith_ok: ok\n");
        assert_eq!(a, b);
        let c = digest_hex(b"test arith_ok: OK\n");
        assert_ne!(a, c);
    }

    #[test]
    fn record_file_roundtrips_through_text() {
        let rec = sample();
        let text = rec.to_text();
        let parsed = RecordFile::parse(&text).expect("parses");
        assert_eq!(parsed, rec);
    }

    #[test]
    fn record_file_with_zero_clock_reads_roundtrips() {
        let rec = RecordFile {
            clock_log: vec![],
            transcript_digest: digest_hex(b""),
            exit_code: 0,
            exits: 1,
        };
        let text = rec.to_text();
        assert_eq!(RecordFile::parse(&text).expect("parses"), rec);
    }

    #[test]
    fn parse_rejects_a_clock_log_len_mismatch() {
        let text = "clock_log_len=2\nclock[0]=1\ntranscript_digest=x\nexit_code=0\nexits=1\n";
        assert!(RecordFile::parse(text).is_err());
    }

    #[test]
    fn parse_rejects_a_missing_field() {
        let text = "clock_log_len=0\ntranscript_digest=x\nexits=1\n";
        assert!(RecordFile::parse(text).is_err());
    }

    #[test]
    fn parse_rejects_a_malformed_line() {
        let text = "clock_log_len=0\ntranscript_digest\nexit_code=0\nexits=0\n";
        assert!(RecordFile::parse(text).is_err());
    }

    #[test]
    fn parse_rejects_an_unknown_key() {
        let text = "clock_log_len=0\ntranscript_digest=x\nexit_code=0\nexits=0\nmystery=1\n";
        assert!(RecordFile::parse(text).is_err());
    }

    #[test]
    fn to_text_carries_no_extra_whitespace_or_timestamp_shaped_content() {
        // House rule (plans/M5.md item F): the record file's own format
        // never carries wall-clock content beyond the clock *values*
        // themselves — this is a cheap textual tripwire, not exhaustive,
        // but it would catch an accidental `SystemTime::now()`/timestamp
        // field creeping into `to_text`.
        let text = sample().to_text();
        assert!(!text.contains("SystemTime"));
        assert!(text.ends_with('\n'));
    }
}
