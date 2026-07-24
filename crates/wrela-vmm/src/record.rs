//! Recorder/replayer for the determinism boundary (06-machine.md §8,
//! plans/M6.md item E, decision 9's own ROADMAP-constrained shape): "the
//! record file becomes a sequence of explicit choice points ... the FORMAT
//! (choice-tagged, enumerable — a later milestone can swap the chooser for
//! an enumerator) is the deliverable ROADMAP's constraint names." This
//! module extends M5's clock-log-only recorder (plans/M5.md item F) into
//! that shape: `ChoiceEntry` is one tagged nondeterministic decision the
//! machine made — a clock read, a deadline wake, a vector raise, or (the
//! format-only, never-yet-emitted) admission choice a future multi-core
//! milestone needs — and `Chooser::choose_next` is the single function
//! every one of those decisions flows through, in both record and replay
//! mode (and, later, a schedule enumerator's mode too, without this fn's
//! own call sites in `lib.rs` changing at all).
//!
//! **The record file is a recording, not a golden** (M5's own house rule,
//! carried forward unchanged): a live boot's clock/deadline values are
//! wall-clock dependent and vary run to run — nothing here is ever written
//! into `tests/golden/` or compared against a pinned expectation. Only the
//! *format* (`RecordFile::to_text`/`parse` round-tripping, pinned below
//! with a hand-built log — structure, not timestamps) and the *divergence-
//! detection logic* (`Chooser`'s own tag-checking, `replay`'s own digest/
//! exit comparisons) are unit-tested.
//!
//! **Admission at M6**: decision 9 names `Admission{mailbox, sender}` as a
//! real tag the FORMAT must support (a later, multi-core milestone's own
//! cross-core scheduling nondeterminism, 06 §8: "per-mailbox cross-core
//! admission order"). At M6 there is exactly one core: admission order is
//! deterministic program order, the VMM never runs guest-internal mailbox
//! code, and so it has no admission choice to observe at all — the tag
//! parses and round-trips (`choice_log_format_is_pinned_including_the_unused_admission_tag`,
//! below) but nothing in `lib.rs` ever constructs one. This is the honest
//! M6 form decision 9 itself calls for ("OPTIONAL/absent").

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

// --- the choice-tagged sequence (decision 9) -------------------------------

/// One nondeterministic choice the machine's own record/replay boundary
/// resolved, in order — the enumerable schedule representation the
/// ROADMAP's own recorded design constraint names (module doc above).
/// Every variant is a plain, named fact — never a raw "diverged" bit —
/// so a mismatch can always say *which* choice, and *what*, disagreed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceEntry {
    /// A guest read of `CLOCK_MMIO_ADDR` (M5's whole recorder, unchanged
    /// in spirit — one tag among several now).
    ClockRead { value: u64 },
    /// A parked core's own deadline wake (plans/M6.md item E, 06 §5): the
    /// VMM slept real wall time until `deadline_ns` (record mode) or
    /// skipped the sleep entirely (replay mode — decision 9: "sleep
    /// skipped under replay ... virtual time from the log").
    DeadlineWake { deadline_ns: u64 },
    /// The VMM raised vector `vector` (06 §4) — at M6 always `0`, the one
    /// deadline/cancel vector. A named field, not a hardcoded value, so a
    /// later milestone's additional vectors need no format change.
    VectorRaise { vector: u64 },
    /// Decision 9: "each admission event is recorded as (mailbox, chosen
    /// sender)". Format-only at M6 (module doc above) — never
    /// constructed by `lib.rs` today, but parseable and round-trippable
    /// so a later milestone's real cross-core admission choices need no
    /// format migration.
    Admission { mailbox: String, sender: String },
}

impl ChoiceEntry {
    fn tag(&self) -> &'static str {
        match self {
            ChoiceEntry::ClockRead { .. } => "ClockRead",
            ChoiceEntry::DeadlineWake { .. } => "DeadlineWake",
            ChoiceEntry::VectorRaise { .. } => "VectorRaise",
            ChoiceEntry::Admission { .. } => "Admission",
        }
    }

    /// One `choice[i]=` line's own right-hand side: `<Tag> key=value ...`.
    fn to_text_fields(&self) -> String {
        match self {
            ChoiceEntry::ClockRead { value } => format!("ClockRead value={value}"),
            ChoiceEntry::DeadlineWake { deadline_ns } => {
                format!("DeadlineWake deadline_ns={deadline_ns}")
            }
            ChoiceEntry::VectorRaise { vector } => format!("VectorRaise vector={vector}"),
            ChoiceEntry::Admission { mailbox, sender } => {
                format!("Admission mailbox={mailbox} sender={sender}")
            }
        }
    }

    fn parse_fields(text: &str) -> Result<ChoiceEntry, String> {
        let mut parts = text.split_whitespace();
        let tag = parts
            .next()
            .ok_or_else(|| "empty choice entry".to_string())?;
        let mut fields: std::collections::BTreeMap<&str, &str> = Default::default();
        for part in parts {
            let (k, v) = part
                .split_once('=')
                .ok_or_else(|| format!("malformed choice field {part:?} (no `=`)"))?;
            fields.insert(k, v);
        }
        let field = |k: &str| -> Result<&str, String> {
            fields
                .get(k)
                .copied()
                .ok_or_else(|| format!("choice entry `{tag}` missing field `{k}`"))
        };
        match tag {
            "ClockRead" => Ok(ChoiceEntry::ClockRead {
                value: field("value")?
                    .parse()
                    .map_err(|e| format!("bad ClockRead value: {e}"))?,
            }),
            "DeadlineWake" => Ok(ChoiceEntry::DeadlineWake {
                deadline_ns: field("deadline_ns")?
                    .parse()
                    .map_err(|e| format!("bad DeadlineWake deadline_ns: {e}"))?,
            }),
            "VectorRaise" => Ok(ChoiceEntry::VectorRaise {
                vector: field("vector")?
                    .parse()
                    .map_err(|e| format!("bad VectorRaise vector: {e}"))?,
            }),
            "Admission" => Ok(ChoiceEntry::Admission {
                mailbox: field("mailbox")?.to_string(),
                sender: field("sender")?.to_string(),
            }),
            other => Err(format!("unknown choice tag `{other}`")),
        }
    }
}

/// What `boot_image_core`'s exit loop is asking `Chooser::choose_next` to
/// resolve — carries only what's needed to check the *tag* the log (or a
/// future enumerator) actually produces against what this boot expected,
/// plus enough to build a safe fallback on a divergence (`fallback`,
/// below) without ever fabricating a value that looks like real data.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceRequest {
    ClockRead,
    DeadlineWake { deadline_ns: u64 },
    VectorRaise { vector: u64 },
}

impl ChoiceRequest {
    fn tag(&self) -> &'static str {
        match self {
            ChoiceRequest::ClockRead => "ClockRead",
            ChoiceRequest::DeadlineWake { .. } => "DeadlineWake",
            ChoiceRequest::VectorRaise { .. } => "VectorRaise",
        }
    }

    /// The safe recovery value fed back on an underrun or tag mismatch —
    /// keeps the boot running rather than aborting mid-boot (mirrors M5's
    /// own "answered with 0, the boot completes" clock-underrun
    /// precedent): a divergence is diagnosed via `Chooser`'s own
    /// accumulated list, never by silently corrupting an otherwise
    /// perfectly good post-mortem transcript.
    fn fallback(&self) -> ChoiceEntry {
        match self {
            ChoiceRequest::ClockRead => ChoiceEntry::ClockRead { value: 0 },
            ChoiceRequest::DeadlineWake { deadline_ns } => ChoiceEntry::DeadlineWake {
                deadline_ns: *deadline_ns,
            },
            ChoiceRequest::VectorRaise { vector } => ChoiceEntry::VectorRaise { vector: *vector },
        }
    }
}

/// Every way a replay boot's own facts can disagree with a previously
/// recorded boot's — named exactly rather than collapsed into one opaque
/// "mismatch" string (06 §8: "diagnoses any divergence"), so a caller
/// (`xtask`, a future device milestone's own replay harness) can report
/// which of the record boundary's own guarantees actually broke. Grown
/// from M5's clock-log-specific pair into the general choice-sequence
/// shape (decision 9's own "the M5 Divergence variants grow tags").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    /// The replayed guest asked for a choice past the end of the
    /// recorded sequence — `index` is the (zero-based) choice that first
    /// ran dry; it (and every one after it) is answered with a safe
    /// fallback so the boot still completes.
    ChoiceLogUnderrun { index: usize, recorded: usize },
    /// The recorded sequence has more choices than the replayed guest
    /// ever asked for — the same image took a different (shorter) path
    /// the second time.
    ChoiceLogOverrun { consumed: usize, recorded: usize },
    /// The replayed guest asked for a different *kind* of choice than the
    /// recording has at that position (e.g. it parked and asked for a
    /// `DeadlineWake` where the recording has a plain `ClockRead`) — a
    /// real control-flow divergence, not merely a different value.
    ChoiceTagMismatch {
        index: usize,
        expected: String,
        actual: String,
    },
    /// The console transcript's own digest differs.
    TranscriptDigestMismatch { expected: String, actual: String },
    /// The guest's own reported exit code differs.
    ExitCodeMismatch { expected: u64, actual: u64 },
}

impl std::fmt::Display for Divergence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Divergence::ChoiceLogUnderrun { index, recorded } => write!(
                f,
                "choice log underrun: choice #{index} was requested but only {recorded} were recorded"
            ),
            Divergence::ChoiceLogOverrun { consumed, recorded } => write!(
                f,
                "choice log overrun: only {consumed} choice(s) were consumed this time, but {recorded} were recorded"
            ),
            Divergence::ChoiceTagMismatch {
                index,
                expected,
                actual,
            } => write!(
                f,
                "choice #{index} tag mismatch: recorded `{actual}`, this boot asked for `{expected}`"
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

enum ChooserMode {
    Record,
    Replay { log: Vec<ChoiceEntry>, idx: usize },
}

/// The single-point-of-choice structure decision 9 explicitly buys
/// (CLAUDE.md's "the one piece of architecture the ROADMAP explicitly
/// bought — build it plainly"): every nondeterministic decision
/// `boot_image_core`'s exit loop makes — what a clock read returns,
/// whether/how long a deadline park sleeps, which vector a wake raises —
/// flows through `choose_next`, whether this boot is recording (produce a
/// fresh live value, log it) or replaying (consume the next tagged entry,
/// fail loudly — via an accumulated `Divergence`, never a panic — on a
/// tag mismatch). A future milestone's schedule enumerator is a *third*
/// implementation of this identical interface (drive a chosen branch of
/// the search instead of replaying a fixed log); `lib.rs`'s own call
/// sites never need to change to add it.
pub struct Chooser {
    mode: ChooserMode,
    /// Record mode: every entry produced, in order (this boot's own new
    /// recording). Replay mode: every entry actually consumed, in order
    /// — so a caller can inspect "what this boot actually did" without
    /// re-deriving it from the original log.
    log: Vec<ChoiceEntry>,
    divergences: Vec<Divergence>,
}

impl Chooser {
    pub fn recorder() -> Chooser {
        Chooser {
            mode: ChooserMode::Record,
            log: Vec::new(),
            divergences: Vec::new(),
        }
    }

    pub fn replayer(recorded: Vec<ChoiceEntry>) -> Chooser {
        Chooser {
            mode: ChooserMode::Replay {
                log: recorded,
                idx: 0,
            },
            log: Vec::new(),
            divergences: Vec::new(),
        }
    }

    /// THE single point of choice (module doc above). `live` is called
    /// exactly once, and only when this `Chooser` is recording: it
    /// performs whatever real, nondeterministic work the choice needs (a
    /// real clock read, a real host-thread sleep) and returns the entry
    /// actually observed. Replay mode never calls `live` at all — it
    /// consumes the next tagged entry from the recorded log instead,
    /// which is exactly why a deadline wake's own sleep is skipped under
    /// replay (decision 9): the sleep lives inside `live`, structurally
    /// unreachable from the replay arm below.
    pub fn choose_next(
        &mut self,
        request: ChoiceRequest,
        live: impl FnOnce() -> ChoiceEntry,
    ) -> ChoiceEntry {
        match &mut self.mode {
            ChooserMode::Record => {
                let entry = live();
                debug_assert_eq!(
                    entry.tag(),
                    request.tag(),
                    "a live choice must answer the shape it was asked for"
                );
                self.log.push(entry.clone());
                entry
            }
            ChooserMode::Replay { log, idx } => {
                let Some(entry) = log.get(*idx).cloned() else {
                    self.divergences.push(Divergence::ChoiceLogUnderrun {
                        index: *idx,
                        recorded: log.len(),
                    });
                    let fallback = request.fallback();
                    self.log.push(fallback.clone());
                    return fallback;
                };
                *idx += 1;
                if entry.tag() != request.tag() {
                    self.divergences.push(Divergence::ChoiceTagMismatch {
                        index: *idx - 1,
                        expected: request.tag().to_string(),
                        actual: entry.tag().to_string(),
                    });
                    let fallback = request.fallback();
                    self.log.push(fallback.clone());
                    return fallback;
                }
                self.log.push(entry.clone());
                entry
            }
        }
    }

    /// Finalizes this chooser: in replay mode, any recorded choices never
    /// consumed are an overrun (the guest took a different, shorter path
    /// the second time). Returns the choice sequence actually produced or
    /// consumed (`BootOutcome::choices`'s own source) plus every
    /// divergence found along the way.
    fn finish(self) -> (Vec<ChoiceEntry>, Vec<Divergence>) {
        let mut divergences = self.divergences;
        if let ChooserMode::Replay { log, idx } = &self.mode {
            if *idx < log.len() {
                divergences.push(Divergence::ChoiceLogOverrun {
                    consumed: *idx,
                    recorded: log.len(),
                });
            }
        }
        (self.log, divergences)
    }
}

/// `boot_image_core`'s own internal convenience: builds the `(choices,
/// divergences)` pair `finish` returns — a free fn (not a method) purely
/// so `lib.rs` never needs `pub(crate)` visibility into `Chooser`'s own
/// private `finish`.
pub(crate) fn finish_chooser(chooser: Chooser) -> (Vec<ChoiceEntry>, Vec<Divergence>) {
    chooser.finish()
}

// --- the record file itself -------------------------------------------------

/// Versioned header line every record file starts with — decision 9's own
/// "versioned header line `ChoiceLog v1`". A future format revision bumps
/// this and `parse` refuses an old/foreign header outright (fail closed,
/// never a best-effort guess at an unversioned format).
const FORMAT_HEADER: &str = "ChoiceLog v1";

/// One boot's own recorded facts (06 §8, decision 9): the ordered choice
/// sequence (`ChoiceEntry`, above) plus a digest of the console
/// transcript, the guest's own reported exit code, and the total vCPU
/// exit count. No timestamps, no wall-clock content of any kind beyond
/// the choice *values themselves* (which are guest-visible/machine-
/// observable data, not host bookkeeping) — `to_text`'s own house rule,
/// carried forward from M5 unchanged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFile {
    pub choices: Vec<ChoiceEntry>,
    pub transcript_digest: String,
    pub exit_code: u64,
    pub exits: u64,
}

impl RecordFile {
    /// Builds a `RecordFile` from a fresh `BootOutcome` — the "record"
    /// half of record/replay (`replay`, below, is the "replay" half).
    pub fn from_outcome(outcome: &BootOutcome) -> RecordFile {
        RecordFile {
            choices: outcome.choices.clone(),
            transcript_digest: digest_hex(&outcome.transcript),
            exit_code: outcome.exit_code,
            exits: outcome.exits,
        }
    }

    /// A stable, line-oriented, hand-parseable text format (mirrors
    /// `lib.rs`'s own `parse_report`'s house style: plain `key=value`
    /// lines, no TOML/JSON dependency for an in-tree, internal-only
    /// format). One `choice[i]=<Tag> field=value ...` line per resolved
    /// choice, in order, rather than one delimited list, so a truncated/
    /// corrupted file's own first bad line is trivially locatable by a
    /// human reader (the same "review-visible" preference `layout.rs`'s
    /// `img.hex` dump makes for image bytes).
    pub fn to_text(&self) -> String {
        let mut s = String::new();
        s.push_str(FORMAT_HEADER);
        s.push('\n');
        s.push_str(&format!("choice_count={}\n", self.choices.len()));
        for (i, c) in self.choices.iter().enumerate() {
            s.push_str(&format!("choice[{i}]={}\n", c.to_text_fields()));
        }
        s.push_str(&format!("transcript_digest={}\n", self.transcript_digest));
        s.push_str(&format!("exit_code={}\n", self.exit_code));
        s.push_str(&format!("exits={}\n", self.exits));
        s
    }

    /// The inverse of `to_text` — fails closed (a plain `Err(String)`,
    /// never a panic or a silently-defaulted field) on any malformed,
    /// missing, or extra line, an unrecognized version header, or a
    /// `choice_count` that disagrees with the number of `choice[i]=`
    /// lines actually present.
    pub fn parse(text: &str) -> Result<RecordFile, String> {
        let mut lines = text.lines();
        let header = lines
            .next()
            .ok_or_else(|| "empty record file".to_string())?;
        if header.trim() != FORMAT_HEADER {
            return Err(format!(
                "unrecognized record format header {header:?} (expected {FORMAT_HEADER:?})"
            ));
        }
        let mut choice_count: Option<usize> = None;
        let mut entries: std::collections::BTreeMap<usize, ChoiceEntry> = Default::default();
        let mut transcript_digest: Option<String> = None;
        let mut exit_code: Option<u64> = None;
        let mut exits: Option<u64> = None;
        for line in lines {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            let Some((key, value)) = line.split_once('=') else {
                return Err(format!("malformed record line (no `=`): {line:?}"));
            };
            if key == "choice_count" {
                choice_count = Some(
                    value
                        .parse()
                        .map_err(|e| format!("bad choice_count {value:?}: {e}"))?,
                );
            } else if let Some(idx_str) = key
                .strip_prefix("choice[")
                .and_then(|s| s.strip_suffix(']'))
            {
                let idx: usize = idx_str
                    .parse()
                    .map_err(|e| format!("bad choice index `{idx_str}`: {e}"))?;
                let entry = ChoiceEntry::parse_fields(value)?;
                if entries.insert(idx, entry).is_some() {
                    return Err(format!("duplicate `choice[{idx}]=` line"));
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
        let choice_count = choice_count.ok_or("missing choice_count")?;
        if entries.len() != choice_count {
            return Err(format!(
                "choice_count={choice_count} but {} `choice[i]=` line(s) present",
                entries.len()
            ));
        }
        let mut choices = Vec::with_capacity(choice_count);
        for i in 0..choice_count {
            let e = entries
                .remove(&i)
                .ok_or_else(|| format!("missing choice[{i}]= line"))?;
            choices.push(e);
        }
        Ok(RecordFile {
            choices,
            transcript_digest: transcript_digest.ok_or("missing transcript_digest")?,
            exit_code: exit_code.ok_or("missing exit_code")?,
            exits: exits.ok_or("missing exits")?,
        })
    }
}

/// Boots `img_path`/`report_path` live (`crate::boot_image_core`) and
/// returns the resulting `RecordFile` — the "record" step a caller runs
/// once to produce the file `replay` (below) later checks against. A
/// live recording can never diverge from itself (`Chooser::recorder`
/// never accumulates a `Divergence`), asserted defensively.
pub fn record(report_path: &Path, img_path: &Path) -> Result<RecordFile, VmmError> {
    let (outcome, divergences) = boot_image_core(report_path, img_path, None, None)?;
    debug_assert!(
        divergences.is_empty(),
        "a live recording boot cannot diverge from itself: {divergences:?}"
    );
    Ok(RecordFile::from_outcome(&outcome))
}

/// Boots `img_path`/`report_path` again, feeding `recorded.choices` back
/// through the identical `Chooser::choose_next` a live boot uses (06 §8's
/// replay half: "replay feeds the log from virtual device models") and
/// compares every recorded fact against this fresh boot's own. Returns
/// every divergence found (empty = the replay reproduced the recording
/// exactly); a genuine VMM/boot failure (a bad report, an HVF error, a
/// timeout) is still its own `Err`, never folded into the divergence list
/// — those are boot failures, not determinism findings.
pub fn replay(
    report_path: &Path,
    img_path: &Path,
    recorded: &RecordFile,
) -> Result<Vec<Divergence>, VmmError> {
    let (outcome, mut divergences) =
        boot_image_core(report_path, img_path, Some(recorded.choices.clone()), None)?;
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
            choices: vec![
                ChoiceEntry::ClockRead { value: 100 },
                ChoiceEntry::ClockRead { value: 250 },
                ChoiceEntry::DeadlineWake { deadline_ns: 9999 },
                ChoiceEntry::VectorRaise { vector: 0 },
            ],
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
    fn record_file_with_zero_choices_roundtrips() {
        let rec = RecordFile {
            choices: vec![],
            transcript_digest: digest_hex(b""),
            exit_code: 0,
            exits: 1,
        };
        let text = rec.to_text();
        assert_eq!(RecordFile::parse(&text).expect("parses"), rec);
    }

    /// Decision 9's own pinned FORMAT (the deliverable, "structure, not
    /// timestamps"): a hand-built log exercising every tag the format
    /// supports — including `Admission`, which M6 never emits but the
    /// FORMAT must still carry (module doc above) — compared against an
    /// exact, hand-written expected string. This is the golden `to_text`
    /// itself never needs a `tests/golden/` entry for: the format is
    /// small and stable enough to pin as a plain Rust string literal,
    /// exactly like M5's own record.rs already did for the clock-only
    /// shape.
    #[test]
    fn choice_log_format_is_pinned_including_the_unused_admission_tag() {
        let rec = RecordFile {
            choices: vec![
                ChoiceEntry::ClockRead { value: 12345 },
                ChoiceEntry::DeadlineWake {
                    deadline_ns: 500_000,
                },
                ChoiceEntry::VectorRaise { vector: 0 },
                ChoiceEntry::Admission {
                    mailbox: "Store".to_string(),
                    sender: "root".to_string(),
                },
            ],
            transcript_digest: "0123456789abcdef".to_string(),
            exit_code: 0,
            exits: 3,
        };
        let expected = "ChoiceLog v1\n\
             choice_count=4\n\
             choice[0]=ClockRead value=12345\n\
             choice[1]=DeadlineWake deadline_ns=500000\n\
             choice[2]=VectorRaise vector=0\n\
             choice[3]=Admission mailbox=Store sender=root\n\
             transcript_digest=0123456789abcdef\n\
             exit_code=0\n\
             exits=3\n";
        assert_eq!(rec.to_text(), expected);
        assert_eq!(RecordFile::parse(expected).expect("parses"), rec);
    }

    #[test]
    fn parse_rejects_an_unversioned_or_foreign_header() {
        let text = "clock_log_len=0\ntranscript_digest=x\nexit_code=0\nexits=1\n";
        assert!(RecordFile::parse(text).is_err());
        let text2 = "ChoiceLog v2\nchoice_count=0\ntranscript_digest=x\nexit_code=0\nexits=1\n";
        assert!(RecordFile::parse(text2).is_err());
    }

    #[test]
    fn parse_rejects_a_choice_count_mismatch() {
        let text = "ChoiceLog v1\nchoice_count=2\nchoice[0]=ClockRead value=1\ntranscript_digest=x\nexit_code=0\nexits=1\n";
        assert!(RecordFile::parse(text).is_err());
    }

    #[test]
    fn parse_rejects_a_missing_field() {
        let text = "ChoiceLog v1\nchoice_count=0\ntranscript_digest=x\nexits=1\n";
        assert!(RecordFile::parse(text).is_err());
    }

    #[test]
    fn parse_rejects_an_unknown_choice_tag() {
        let text = "ChoiceLog v1\nchoice_count=1\nchoice[0]=Mystery foo=1\ntranscript_digest=x\nexit_code=0\nexits=0\n";
        assert!(RecordFile::parse(text).is_err());
    }

    #[test]
    fn parse_rejects_an_unknown_key() {
        let text =
            "ChoiceLog v1\nchoice_count=0\ntranscript_digest=x\nexit_code=0\nexits=0\nmystery=1\n";
        assert!(RecordFile::parse(text).is_err());
    }

    #[test]
    fn to_text_carries_no_extra_whitespace_or_timestamp_shaped_content() {
        // House rule (carried from M5): the record file's own format
        // never carries wall-clock content beyond the choice *values*
        // themselves — a cheap textual tripwire, not exhaustive, but it
        // would catch an accidental `SystemTime::now()`/timestamp field
        // creeping into `to_text`.
        let text = sample().to_text();
        assert!(!text.contains("SystemTime"));
        assert!(text.ends_with('\n'));
    }

    // --- Chooser: the single-point-of-choice structure itself ------------

    #[test]
    fn recorder_logs_every_live_value_in_order() {
        let mut c = Chooser::recorder();
        let a = c.choose_next(ChoiceRequest::ClockRead, || ChoiceEntry::ClockRead {
            value: 111,
        });
        let b = c.choose_next(ChoiceRequest::DeadlineWake { deadline_ns: 222 }, || {
            ChoiceEntry::DeadlineWake { deadline_ns: 222 }
        });
        assert_eq!(a, ChoiceEntry::ClockRead { value: 111 });
        assert_eq!(b, ChoiceEntry::DeadlineWake { deadline_ns: 222 });
        let (log, divergences) = finish_chooser(c);
        assert_eq!(log, vec![a, b]);
        assert!(divergences.is_empty());
    }

    #[test]
    fn replayer_never_invokes_live_and_feeds_the_recorded_value_back() {
        let mut c = Chooser::replayer(vec![ChoiceEntry::ClockRead { value: 42 }]);
        let got = c.choose_next(ChoiceRequest::ClockRead, || {
            panic!("replay must never call `live`")
        });
        assert_eq!(got, ChoiceEntry::ClockRead { value: 42 });
        let (_, divergences) = finish_chooser(c);
        assert!(divergences.is_empty());
    }

    #[test]
    fn replayer_diverges_loudly_on_underrun_but_still_completes() {
        let mut c = Chooser::replayer(vec![]);
        let got = c.choose_next(ChoiceRequest::ClockRead, || {
            panic!("replay must never call `live`")
        });
        assert_eq!(got, ChoiceEntry::ClockRead { value: 0 }); // safe fallback
        let (_, divergences) = finish_chooser(c);
        assert_eq!(
            divergences,
            vec![Divergence::ChoiceLogUnderrun {
                index: 0,
                recorded: 0
            }]
        );
    }

    #[test]
    fn replayer_diverges_loudly_on_a_tag_mismatch() {
        let mut c = Chooser::replayer(vec![ChoiceEntry::ClockRead { value: 1 }]);
        let got = c.choose_next(ChoiceRequest::DeadlineWake { deadline_ns: 5 }, || {
            panic!("replay must never call `live`")
        });
        assert_eq!(got, ChoiceEntry::DeadlineWake { deadline_ns: 5 }); // safe fallback
        let (_, divergences) = finish_chooser(c);
        assert_eq!(
            divergences,
            vec![Divergence::ChoiceTagMismatch {
                index: 0,
                expected: "DeadlineWake".to_string(),
                actual: "ClockRead".to_string(),
            }]
        );
    }

    #[test]
    fn replayer_diverges_loudly_on_overrun() {
        let mut c = Chooser::replayer(vec![
            ChoiceEntry::ClockRead { value: 1 },
            ChoiceEntry::ClockRead { value: 2 },
        ]);
        let _ = c.choose_next(ChoiceRequest::ClockRead, || {
            panic!("replay must never call `live`")
        });
        let (_, divergences) = finish_chooser(c);
        assert_eq!(
            divergences,
            vec![Divergence::ChoiceLogOverrun {
                consumed: 1,
                recorded: 2
            }]
        );
    }
}
