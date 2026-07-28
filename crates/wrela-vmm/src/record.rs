//! Recorder/replayer for the determinism boundary (06-machine.md §8,
//! plans/M6.md item E, decision 9's own ROADMAP-constrained shape): "the
//! record file becomes a sequence of explicit choice points ... the FORMAT
//! (choice-tagged, enumerable — a later milestone can swap the chooser for
//! an enumerator) is the deliverable ROADMAP's constraint names." This
//! module extends M5's clock-log-only recorder (plans/M5.md item F) into
//! that shape: `ChoiceEntry` is one tagged nondeterministic decision the
//! machine made — a clock read, a deadline wake, a vector raise, a device
//! completion, or a cross-core mailbox admission — and
//! `Chooser::choose_next` is the single function
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
//! **Admission, M6 → M8 item C3.** Decision 9 named
//! `Admission{mailbox, sender}` as a real tag the FORMAT must support (06
//! §8: "per-mailbox cross-core admission order"), and at M6 there was
//! exactly one core, so nothing in `lib.rs` ever constructed one. M8 item
//! C2 built the cross-core rings, so an admission finally exists to
//! record, and item C3 (decision 42) records it:
//! `crate::witness_admissions` reads each request ring's occupancy word
//! out of guest memory at every vCPU exit and pushes one `Admission` per
//! message the owning core's drain just handed to a mailbox root.
//!
//! **It is a witness, not an injection, and the distinction is the whole
//! honest claim of the item.** The drain is guest code and the mailbox is
//! guest memory; the VMM never writes either, so replay does not *feed*
//! the recorded order back — under plans/M15.md item I / decision 7,
//! record overlaps `hv_vcpu_run` while replay re-serializes enter from
//! Yield-`Progress` only. That serial hand-off does not reproduce
//! concurrent drain interleaving, so Admission replay checks a **multiset
//! bag** of `(mailbox, sender)` pairs (decision 8: retire positional /
//! exact-one-core Δcount assumptions under overlap). Progress / Clock /
//! Device / Deadline stay strictly positional on the choice tape.
//! Divergence detection is therefore still the entire enforcement
//! (`xtask repro`'s own cross-core admission lane is its oracle), and this
//! comment says so rather than implying the stronger property.

use std::collections::BTreeMap;
use std::path::Path;

use crate::{BootOutcome, VmmError, boot_image_core};

/// SHA-256 hex of `bytes` (06 §8: "digests of every output"). Same
/// in-tree FIPS 180-4 implementation the compiler's report digests use
/// (`wrela_machine::sha256`) — collision-resistant enough that a forged
/// choice-log cannot quietly match a different transcript.
pub fn digest_hex(bytes: &[u8]) -> String {
    wrela_machine::sha256::sha256_hex(bytes)
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
    /// sender)". **Real since plans/M8.md item C3** (decision 42): one
    /// message handed to a mailbox root by another core's cross-core
    /// request ring, in the order the owning core's drain admitted it.
    /// `mailbox` is the target mailbox root's own name (the ring feeds
    /// exactly one — decision 28), and `sender` is the *producing core*
    /// spelled `core<N>`, because decision 28 settled that the producer of
    /// a cross-core ring is a core and not an actor. The field set is M6's
    /// unchanged: no timestamp, no address, no interleaving trace — the
    /// order is the position in the sequence (06 §8's enumerable choice
    /// sequence).
    Admission { mailbox: String, sender: String },
    /// One device completion (plans/M7.md decision 7: "device completions
    /// join the choice sequence"; 06 §8: the VMM "logs every device
    /// completion and DMA-written byte range ... and digests of every
    /// output"). `head` is the operation's own identity — the descriptor
    /// chain head, which is also the `id` the used ring reports — `status`
    /// and `len` are the completion the guest observes, and `digest`
    /// fingerprints every byte the operation wrote (guest memory first,
    /// then the disk: `devices::Completion::digest`'s own rule).
    ///
    /// Under replay the used-ring entry is written from *this* record
    /// rather than from the model's own fresh answer, and a model that
    /// disagrees is reported as `Divergence::DeviceCompletionMismatch` —
    /// never silently applied (`crate::service_blk`'s own doc has the
    /// whole shape).
    DeviceCompletion {
        device: String,
        queue: u32,
        head: u32,
        status: u32,
        len: u32,
        digest: String,
    },
    /// plans/M15.md item I / decision 7: a guest Yield that moves the
    /// schedule — release or park. `core` is the next core to run (the
    /// hand-off target). Recorded under Progress-serialized execution;
    /// under replay that core is forced.
    Progress { core: u32 },
}

impl ChoiceEntry {
    fn tag(&self) -> &'static str {
        match self {
            ChoiceEntry::ClockRead { .. } => "ClockRead",
            ChoiceEntry::DeadlineWake { .. } => "DeadlineWake",
            ChoiceEntry::VectorRaise { .. } => "VectorRaise",
            ChoiceEntry::Admission { .. } => "Admission",
            ChoiceEntry::DeviceCompletion { .. } => "DeviceCompletion",
            ChoiceEntry::Progress { .. } => "Progress",
        }
    }

    /// One `choice[i]=` line's own right-hand side: `<Tag> key=value ...`.
    /// Public because `Divergence::DeviceCompletionMismatch` reports two
    /// whole entries by name, and this is already the one canonical way
    /// this format spells one.
    pub fn to_text_fields(&self) -> String {
        match self {
            ChoiceEntry::ClockRead { value } => format!("ClockRead value={value}"),
            ChoiceEntry::DeadlineWake { deadline_ns } => {
                format!("DeadlineWake deadline_ns={deadline_ns}")
            }
            ChoiceEntry::VectorRaise { vector } => format!("VectorRaise vector={vector}"),
            ChoiceEntry::Admission { mailbox, sender } => {
                format!("Admission mailbox={mailbox} sender={sender}")
            }
            ChoiceEntry::DeviceCompletion {
                device,
                queue,
                head,
                status,
                len,
                digest,
            } => format!(
                "DeviceCompletion device={device} queue={queue} head={head} status={status} len={len} digest={digest}"
            ),
            ChoiceEntry::Progress { core } => format!("Progress core={core}"),
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
            // Same strictness as `wrela_machine::report::parse_report_fields`:
            // a repeated key in a file replay trusts is refuse-closed, never
            // a silent last-wins overwrite (adversarial audit, 2026-07-27).
            if fields.insert(k, v).is_some() {
                return Err(format!("choice entry `{tag}` repeats field `{k}`"));
            }
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
            "DeviceCompletion" => {
                let num = |k: &str| -> Result<u32, String> {
                    field(k)?
                        .parse::<u32>()
                        .map_err(|e| format!("bad DeviceCompletion {k}: {e}"))
                };
                Ok(ChoiceEntry::DeviceCompletion {
                    device: field("device")?.to_string(),
                    queue: num("queue")?,
                    head: num("head")?,
                    status: num("status")?,
                    len: num("len")?,
                    digest: field("digest")?.to_string(),
                })
            }
            "Progress" => Ok(ChoiceEntry::Progress {
                core: field("core")?
                    .parse()
                    .map_err(|e| format!("bad Progress core: {e}"))?,
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
    DeadlineWake {
        deadline_ns: u64,
    },
    VectorRaise {
        vector: u64,
    },
    /// plans/M7.md decision 7. Unlike every other request shape, this one
    /// carries the *whole* answer the device model already computed: a
    /// completion is not a value the VMM invents at choice time (the model
    /// derives it deterministically from the guest's own ring and the
    /// disk), it is a value whose *timing and ordering* are the machine's
    /// nondeterminism. Carrying it here makes the fallback exactly "what
    /// the model said" and lets `crate::service_blk` compare the model's
    /// answer against the log's, entry for entry.
    DeviceCompletion {
        device: String,
        queue: u32,
        head: u32,
        status: u32,
        len: u32,
        digest: String,
    },
    /// plans/M8.md item C3, decision 42. Like `DeviceCompletion` above and
    /// for the same reason, this request carries the *whole* answer the
    /// VMM already observed in guest memory: an admission is not a value
    /// the VMM invents — the guest's own drain performed it — so the
    /// recorder's job is to witness it, and replay's job is to check that
    /// the witnessed one is the recorded one
    /// (`Divergence::AdmissionMismatch`).
    Admission {
        mailbox: String,
        sender: String,
    },
    /// plans/M15.md item I: Yield hand-off. Under record the live closure
    /// supplies `next_core`; under replay the logged core is injected.
    Progress {
        core: u32,
    },
}

impl ChoiceRequest {
    fn tag(&self) -> &'static str {
        match self {
            ChoiceRequest::ClockRead => "ClockRead",
            ChoiceRequest::DeadlineWake { .. } => "DeadlineWake",
            ChoiceRequest::VectorRaise { .. } => "VectorRaise",
            ChoiceRequest::DeviceCompletion { .. } => "DeviceCompletion",
            ChoiceRequest::Admission { .. } => "Admission",
            ChoiceRequest::Progress { .. } => "Progress",
        }
    }

    /// The safe recovery value fed back on an underrun or tag mismatch —
    /// keeps the boot running rather than aborting mid-boot (mirrors M5's
    /// own "answered with 0, the boot completes" clock-underrun
    /// precedent): a divergence is diagnosed via `Chooser`'s own
    /// accumulated list, never by silently corrupting an otherwise
    /// perfectly good post-mortem transcript.
    pub fn fallback(&self) -> ChoiceEntry {
        match self {
            ChoiceRequest::ClockRead => ChoiceEntry::ClockRead { value: 0 },
            ChoiceRequest::DeadlineWake { deadline_ns } => ChoiceEntry::DeadlineWake {
                deadline_ns: *deadline_ns,
            },
            ChoiceRequest::VectorRaise { vector } => ChoiceEntry::VectorRaise { vector: *vector },
            ChoiceRequest::DeviceCompletion {
                device,
                queue,
                head,
                status,
                len,
                digest,
            } => ChoiceEntry::DeviceCompletion {
                device: device.clone(),
                queue: *queue,
                head: *head,
                status: *status,
                len: *len,
                digest: digest.clone(),
            },
            ChoiceRequest::Admission { mailbox, sender } => ChoiceEntry::Admission {
                mailbox: mailbox.clone(),
                sender: sender.clone(),
            },
            ChoiceRequest::Progress { core } => ChoiceEntry::Progress { core: *core },
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
    /// plans/M7.md decision 7: the tag matched (a device completion was
    /// expected and one happened) but the device model's own freshly
    /// computed completion disagrees with the recorded one — a different
    /// status, length, head, or a different set of DMA-written bytes. The
    /// *recorded* completion is what gets published in the used ring, so
    /// the replayed guest still sees the recording; this names the fact
    /// that the model would have said something else.
    DeviceCompletionMismatch {
        index: usize,
        recorded: String,
        actual: String,
    },
    /// plans/M8.md item C3, decision 42: the tag matched (an admission was
    /// expected and one happened) but the mailbox or the producing core
    /// disagrees with the recorded one — this boot's guest admitted a
    /// different message, at a different mailbox, or from a different
    /// core, than the recording says it did. Witness-only, exactly like
    /// `DeviceCompletionMismatch`: nothing about the recorded entry is fed
    /// back into the guest (the drain is guest code and the VMM never
    /// writes a mailbox), so this divergence *is* the whole enforcement.
    AdmissionMismatch {
        index: usize,
        recorded: String,
        actual: String,
    },
    /// plans/M8.md item H Target B: the recording and this boot disagree on
    /// *how many* admissions happened — an `Admission` was deleted from the
    /// log, a spurious one was inserted, or the guest admitted fewer/more
    /// than recorded. Distinct from a generic choice-log underrun/overrun
    /// so a count tamper is caught by name (06 §8: the choice sequence is
    /// enumerable and replay is exact).
    AdmissionCountMismatch { index: usize, detail: String },
    /// plans/M15.md item I: a `Progress` Yield hand-off named a different
    /// next core than this boot's guest-visible `next_core` would have
    /// picked (or a Progress tamper changed the forced core).
    ProgressMismatch {
        index: usize,
        recorded: u32,
        actual: u32,
    },
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
            Divergence::DeviceCompletionMismatch {
                index,
                recorded,
                actual,
            } => write!(
                f,
                "choice #{index} device completion mismatch: recorded `{recorded}`, the device model produced `{actual}`"
            ),
            Divergence::AdmissionMismatch {
                index,
                recorded,
                actual,
            } => {
                // Name the field(s) that disagree — a tamper that only
                // swaps senders must say `sender`, not a generic mismatch.
                let field = admission_mismatch_fields(recorded, actual);
                write!(
                    f,
                    "choice #{index} admission mismatch ({field}): recorded `{recorded}`, this \
                     boot's guest admitted `{actual}`"
                )
            }
            Divergence::AdmissionCountMismatch { index, detail } => {
                write!(f, "choice #{index} admission count mismatch: {detail}")
            }
            Divergence::ProgressMismatch {
                index,
                recorded,
                actual,
            } => write!(
                f,
                "choice #{index} progress mismatch: recorded next core {recorded}, this boot's \
                 Yield hand-off would have run core {actual}"
            ),
        }
    }
}

/// Which `Admission` fields disagree between a recorded entry and what this
/// boot witnessed. Empty only if the two strings are equal (caller should
/// not reach Display in that case).
fn admission_mismatch_fields(recorded: &str, actual: &str) -> String {
    fn pick<'a>(s: &'a str, key: &str) -> Option<&'a str> {
        s.split_whitespace().find_map(|p| p.strip_prefix(key))
    }
    let rm = pick(recorded, "mailbox=");
    let am = pick(actual, "mailbox=");
    let rs = pick(recorded, "sender=");
    let as_ = pick(actual, "sender=");
    let mut fields = Vec::new();
    if rm != am {
        fields.push("mailbox");
    }
    if rs != as_ {
        fields.push("sender");
    }
    if fields.is_empty() {
        "entry".to_string()
    } else {
        fields.join("+")
    }
}

enum ChooserMode {
    Record,
    /// Replay walks non-Admission choices positionally (`idx`). Admissions
    /// are consumed from `admission_bag` (plans/M15.md decision 8):
    /// overlapping record and Progress-serial replay need not agree on
    /// drain order, only on the `(mailbox, sender)` multiset.
    Replay {
        log: Vec<ChoiceEntry>,
        idx: usize,
        admission_bag: BTreeMap<(String, String), usize>,
    },
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
    /// When true (live `boot_image_core` replay), the first divergence
    /// aborts the boot instead of inventing a fallback and continuing.
    /// Unit tests of the chooser itself leave this false so they can
    /// collect every divergence shape.
    strict: bool,
}

impl Chooser {
    pub fn recorder() -> Chooser {
        Chooser {
            mode: ChooserMode::Record,
            log: Vec::new(),
            divergences: Vec::new(),
            strict: false,
        }
    }

    pub fn replayer(recorded: Vec<ChoiceEntry>) -> Chooser {
        let mut admission_bag: BTreeMap<(String, String), usize> = BTreeMap::new();
        for e in &recorded {
            if let ChoiceEntry::Admission { mailbox, sender } = e {
                *admission_bag
                    .entry((mailbox.clone(), sender.clone()))
                    .or_insert(0) += 1;
            }
        }
        Chooser {
            mode: ChooserMode::Replay {
                log: recorded,
                idx: 0,
                admission_bag,
            },
            log: Vec::new(),
            divergences: Vec::new(),
            // Live boots opt in via [`Chooser::strict`].
            strict: false,
        }
    }

    /// `true` in record mode — the arm that actually calls `live` (and so
    /// the only mode that must perform a real host-side park sleep).
    pub fn is_recording(&self) -> bool {
        matches!(self.mode, ChooserMode::Record)
    }

    /// `true` in replay mode — serial Yield hand-off from `Progress`.
    pub fn is_replaying(&self) -> bool {
        matches!(self.mode, ChooserMode::Replay { .. })
    }

    /// Peek at the next recorded non-Admission choice without consuming it
    /// (replay only). Admissions live in the bag, not the positional cursor.
    pub fn peek_replay(&self) -> Option<&ChoiceEntry> {
        match &self.mode {
            ChooserMode::Replay { log, idx, .. } => {
                let mut i = *idx;
                while i < log.len() && matches!(log[i], ChoiceEntry::Admission { .. }) {
                    i += 1;
                }
                log.get(i)
            }
            ChooserMode::Record => None,
        }
    }

    /// Fail closed on the first divergence (live replay boots).
    pub fn strict(mut self) -> Chooser {
        self.strict = true;
        self
    }

    /// `Some` when `strict` and at least one divergence has been noted.
    pub fn fatal_divergence(&self) -> Option<&Divergence> {
        if self.strict {
            self.divergences.first()
        } else {
            None
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
            ChooserMode::Replay {
                log,
                idx,
                admission_bag,
            } => {
                // Admissions: multiset bag (overlap record vs Progress-serial
                // replay). Everything else stays positional on the tape.
                if let ChoiceRequest::Admission { mailbox, sender } = &request {
                    let key = (mailbox.clone(), sender.clone());
                    match admission_bag.get_mut(&key) {
                        Some(n) if *n > 0 => {
                            *n -= 1;
                            if *n == 0 {
                                admission_bag.remove(&key);
                            }
                            let entry = ChoiceEntry::Admission {
                                mailbox: mailbox.clone(),
                                sender: sender.clone(),
                            };
                            self.log.push(entry.clone());
                            return entry;
                        }
                        _ => {
                            let index = self.log.len();
                            // Prefer a same-mailbox leftover (sender field),
                            // else same-sender (mailbox field), else any bag
                            // entry — so identity tampers stay named.
                            let alt = admission_bag
                                .keys()
                                .find(|(m, _)| m == mailbox)
                                .or_else(|| admission_bag.keys().find(|(_, s)| s == sender))
                                .or_else(|| admission_bag.keys().next())
                                .cloned();
                            if let Some((m, s)) = alt {
                                self.divergences.push(Divergence::AdmissionMismatch {
                                    index,
                                    recorded: format!("Admission mailbox={m} sender={s}"),
                                    actual: format!(
                                        "Admission mailbox={mailbox} sender={sender}"
                                    ),
                                });
                            } else {
                                self.divergences.push(Divergence::AdmissionCountMismatch {
                                    index,
                                    detail: format!(
                                        "this boot admitted an entry the recording does not have \
                                         (Admission mailbox={mailbox} sender={sender})"
                                    ),
                                });
                            }
                            let fallback = request.fallback();
                            self.log.push(fallback.clone());
                            return fallback;
                        }
                    }
                }

                // Skip recorded Admissions on the positional cursor — they
                // were already loaded into the bag at replayer construction.
                while *idx < log.len() && matches!(log[*idx], ChoiceEntry::Admission { .. }) {
                    *idx += 1;
                }
                let Some(entry) = log.get(*idx).cloned() else {
                    let index = *idx;
                    let recorded = log.len();
                    // Overlap park-count drift: extra Progress the guest asks
                    // for beyond the recording is answered from the request
                    // (live next_core) without diverging. Any other underrun
                    // is still a real control-flow break.
                    if matches!(request, ChoiceRequest::Progress { .. }) {
                        let fallback = request.fallback();
                        self.log.push(fallback.clone());
                        return fallback;
                    }
                    self.divergences
                        .push(Divergence::ChoiceLogUnderrun { index, recorded });
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

    /// Like [`Chooser::choose_next`], but aborts with `VmmError` when this
    /// chooser is [`Chooser::strict`] and a divergence was just recorded.
    pub fn choose_checked(
        &mut self,
        request: ChoiceRequest,
        live: impl FnOnce() -> ChoiceEntry,
    ) -> Result<ChoiceEntry, crate::VmmError> {
        let entry = self.choose_next(request, live);
        self.abort_if_strict_diverged()?;
        Ok(entry)
    }

    /// Like [`Chooser::note_divergence`], but aborts when strict.
    pub fn note_divergence_checked(
        &mut self,
        divergence: Divergence,
    ) -> Result<(), crate::VmmError> {
        self.note_divergence(divergence);
        self.abort_if_strict_diverged()
    }

    fn abort_if_strict_diverged(&self) -> Result<(), crate::VmmError> {
        if let Some(d) = self.fatal_divergence() {
            return Err(crate::VmmError::ReplayDivergence(d.to_string()));
        }
        Ok(())
    }

    /// Records a divergence found *outside* the choice mechanism itself.
    ///
    /// `choose_next` above can only ever check a choice's own **tag** — by
    /// design, since a clock read's replayed value is *supposed* to differ
    /// from whatever the host clock would say now. Exactly two choices have
    /// a value the machine can independently recompute, and each compares
    /// the two and reports a disagreement here rather than silently taking
    /// one of them:
    ///
    /// - a **device completion** (plans/M7.md decision 7: the model derives
    ///   it deterministically from the guest's ring and the VMM's own
    ///   disk), from `crate::service_blk`; and
    /// - a cross-core **admission** (plans/M8.md item C3, decision 42: the
    ///   guest performed it in guest memory and the recorder witnessed it),
    ///   from `crate::witness_admissions`.
    ///
    /// Nothing else calls this: a divergence nobody can recompute has no
    /// business being invented.
    pub fn note_divergence(&mut self, divergence: Divergence) {
        self.divergences.push(divergence);
    }

    /// How many choices this chooser has resolved so far — the index the
    /// next one will occupy, used by `note_divergence`'s own callers to
    /// name *which* choice disagreed.
    pub fn resolved_count(&self) -> usize {
        self.log.len()
    }

    /// Finalizes this chooser: in replay mode, any recorded choices never
    /// consumed are an overrun (the guest took a different, shorter path
    /// the second time). Returns the choice sequence actually produced or
    /// consumed (`BootOutcome::choices`'s own source) plus every
    /// divergence found along the way.
    fn finish(self) -> (Vec<ChoiceEntry>, Vec<Divergence>) {
        let mut divergences = self.divergences;
        if let ChooserMode::Replay {
            log,
            mut idx,
            admission_bag,
        } = self.mode
        {
            // Admissions live in the bag; Progress park-count under overlap
            // is host-schedule-dependent — leftover Progress alone is not
            // a divergence (decision 7/8). Any other leftover tag is.
            while idx < log.len()
                && matches!(
                    log[idx],
                    ChoiceEntry::Admission { .. } | ChoiceEntry::Progress { .. }
                )
            {
                idx += 1;
            }
            let bag_left: usize = admission_bag.values().sum();
            if bag_left > 0 {
                divergences.push(Divergence::AdmissionCountMismatch {
                    index: idx,
                    detail: format!(
                        "recording has {bag_left} unconsumed Admission choice(s) \
                         this boot never performed (consumed positional cursor at {idx} of {} recorded)",
                        log.len()
                    ),
                });
            } else if idx < log.len() {
                divergences.push(Divergence::ChoiceLogOverrun {
                    consumed: idx,
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
pub(crate) fn finish_chooser(
    chooser: Chooser,
) -> Result<(Vec<ChoiceEntry>, Vec<Divergence>), crate::VmmError> {
    let strict = chooser.strict;
    let (log, divergences) = chooser.finish();
    if strict {
        if let Some(d) = divergences.first() {
            return Err(crate::VmmError::ReplayDivergence(d.to_string()));
        }
    }
    Ok((log, divergences))
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
    let (outcome, divergences) = boot_image_core(report_path, img_path, None)?;
    debug_assert!(
        divergences.is_empty(),
        "a live recording boot cannot diverge from itself: {divergences:?}"
    );
    Ok(RecordFile::from_outcome(&outcome))
}

/// Boots `img_path`/`report_path` again, feeding `recorded.choices` back
/// through the identical `Chooser::choose_checked` a live boot uses (06 §8's
/// replay half: "replay feeds the log from virtual device models") and
/// compares every recorded fact against this fresh boot's own. Choice-log
/// divergences abort the boot (`Err`) under the live strict chooser;
/// transcript/exit mismatches found after a clean choice replay are still
/// returned in the `Ok` list. A genuine VMM/boot failure (a bad report, an
/// HVF error, a timeout) is also `Err`, never folded into the divergence
/// list — those are boot failures, not determinism findings.
pub fn replay(
    report_path: &Path,
    img_path: &Path,
    recorded: &RecordFile,
) -> Result<Vec<Divergence>, VmmError> {
    let (outcome, mut divergences) =
        boot_image_core(report_path, img_path, Some(recorded.choices.clone()))?;
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
            transcript_digest: digest_hex(b"sample-transcript"),
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
    fn choice_entry_rejects_a_repeated_field() {
        let err = ChoiceEntry::parse_fields("ClockRead value=1 value=2")
            .expect_err("repeated keys must refuse");
        assert!(
            err.contains("repeats field `value`"),
            "must name the duplicate key: {err}"
        );
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
    /// supports — `Admission` included, whose field set item C3 kept
    /// byte-identical when it made the tag real, so no recording written
    /// before this milestone needs a format migration (module doc above)
    /// — compared against an
    /// exact, hand-written expected string. This is the golden `to_text`
    /// itself never needs a `tests/golden/` entry for: the format is
    /// small and stable enough to pin as a plain Rust string literal,
    /// exactly like M5's own record.rs already did for the clock-only
    /// shape.
    ///
    /// Grown at plans/M7.md item F with `DeviceCompletion` — the one tag
    /// M7 adds (decision 7), and the first one this VMM genuinely emits
    /// from a device model rather than from the exit loop's own clock/park
    /// handling.
    #[test]
    fn choice_log_format_is_pinned_including_every_tag() {
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
                ChoiceEntry::Progress { core: 1 },
                ChoiceEntry::DeviceCompletion {
                    device: "blk".to_string(),
                    queue: 0,
                    head: 3,
                    status: 0,
                    len: 513,
                    digest: "fedcba9876543210".to_string(),
                },
            ],
            transcript_digest: "0123456789abcdef".to_string(),
            exit_code: 0,
            exits: 3,
        };
        let expected = "ChoiceLog v1\n\
             choice_count=6\n\
             choice[0]=ClockRead value=12345\n\
             choice[1]=DeadlineWake deadline_ns=500000\n\
             choice[2]=VectorRaise vector=0\n\
             choice[3]=Admission mailbox=Store sender=root\n\
             choice[4]=Progress core=1\n\
             choice[5]=DeviceCompletion device=blk queue=0 head=3 status=0 len=513 digest=fedcba9876543210\n\
             transcript_digest=0123456789abcdef\n\
             exit_code=0\n\
             exits=3\n";
        assert_eq!(rec.to_text(), expected);
        assert_eq!(RecordFile::parse(expected).expect("parses"), rec);
    }

    /// plans/M15.md item I: `Progress` tag parse / round-trip.
    #[test]
    fn progress_choice_parses_and_round_trips() {
        let entry = ChoiceEntry::Progress { core: 2 };
        assert_eq!(entry.to_text_fields(), "Progress core=2");
        assert_eq!(
            ChoiceEntry::parse_fields("Progress core=2").expect("parses"),
            entry
        );
        let rec = RecordFile {
            choices: vec![entry.clone()],
            transcript_digest: digest_hex(b"p"),
            exit_code: 0,
            exits: 1,
        };
        assert_eq!(RecordFile::parse(&rec.to_text()).expect("round-trip"), rec);
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
        let (log, divergences) = finish_chooser(c).expect("non-strict finish");
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
        let (_, divergences) = finish_chooser(c).expect("non-strict finish");
        assert!(divergences.is_empty());
    }

    #[test]
    fn replayer_diverges_loudly_on_underrun_but_still_completes() {
        // Non-strict (unit-test) mode collects every divergence; live boots
        // use `.strict()` and abort via `choose_checked` instead.
        let mut c = Chooser::replayer(vec![]);
        let got = c.choose_next(ChoiceRequest::ClockRead, || {
            panic!("replay must never call `live`")
        });
        assert_eq!(got, ChoiceEntry::ClockRead { value: 0 }); // safe fallback
        let (_, divergences) = finish_chooser(c).expect("non-strict finish");
        assert_eq!(
            divergences,
            vec![Divergence::ChoiceLogUnderrun {
                index: 0,
                recorded: 0
            }]
        );
    }

    #[test]
    fn strict_replayer_aborts_on_first_divergence() {
        let mut c = Chooser::replayer(vec![]).strict();
        let err = c
            .choose_checked(ChoiceRequest::ClockRead, || {
                panic!("replay must never call `live`")
            })
            .expect_err("strict mode must abort");
        assert!(err.to_string().contains("replay divergence"), "got {err}");
    }

    #[test]
    fn replayer_diverges_loudly_on_a_tag_mismatch() {
        let mut c = Chooser::replayer(vec![ChoiceEntry::ClockRead { value: 1 }]);
        let got = c.choose_next(ChoiceRequest::DeadlineWake { deadline_ns: 5 }, || {
            panic!("replay must never call `live`")
        });
        assert_eq!(got, ChoiceEntry::DeadlineWake { deadline_ns: 5 }); // safe fallback
        let (_, divergences) = finish_chooser(c).expect("non-strict finish");
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
        let (_, divergences) = finish_chooser(c).expect("non-strict finish");
        assert_eq!(
            divergences,
            vec![Divergence::ChoiceLogOverrun {
                consumed: 1,
                recorded: 2
            }]
        );
    }

    /// plans/M8.md item H Target B: an Admission underrun/overrun is named
    /// as a count mismatch, not a generic choice-log exhaustion.
    #[test]
    fn admission_count_tampers_are_named_not_generic() {
        let mut c = Chooser::replayer(vec![]);
        let _ = c.choose_next(
            ChoiceRequest::Admission {
                mailbox: "Sink".into(),
                sender: "core0".into(),
            },
            || panic!("replay must never call live"),
        );
        let (_, divs) = finish_chooser(c).expect("non-strict finish");
        assert_eq!(divs.len(), 1);
        let msg = divs[0].to_string();
        assert!(
            msg.contains("admission count mismatch") && msg.contains("does not have"),
            "{msg}"
        );

        let mut c = Chooser::replayer(vec![
            ChoiceEntry::Admission {
                mailbox: "Sink".into(),
                sender: "core0".into(),
            },
            ChoiceEntry::Admission {
                mailbox: "Sink".into(),
                sender: "core2".into(),
            },
        ]);
        let _ = c.choose_next(
            ChoiceRequest::Admission {
                mailbox: "Sink".into(),
                sender: "core0".into(),
            },
            || panic!("live"),
        );
        let (_, divs) = finish_chooser(c).expect("non-strict finish");
        let msg = divs[0].to_string();
        assert!(
            msg.contains("admission count mismatch") && msg.contains("unconsumed Admission"),
            "{msg}"
        );
    }

    #[test]
    fn admission_mismatch_display_names_the_diverging_field() {
        let sender_only = Divergence::AdmissionMismatch {
            index: 1,
            recorded: "Admission mailbox=Sink sender=core0".into(),
            actual: "Admission mailbox=Sink sender=core2".into(),
        };
        let msg = sender_only.to_string();
        assert!(msg.contains("admission mismatch (sender)"), "{msg}");
        assert!(!msg.contains("(mailbox)"), "{msg}");

        let mailbox_only = Divergence::AdmissionMismatch {
            index: 0,
            recorded: "Admission mailbox=Far sender=core0".into(),
            actual: "Admission mailbox=Sink sender=core0".into(),
        };
        let msg = mailbox_only.to_string();
        assert!(msg.contains("admission mismatch (mailbox)"), "{msg}");

        let both = Divergence::AdmissionMismatch {
            index: 0,
            recorded: "Admission mailbox=Far sender=core0".into(),
            actual: "Admission mailbox=Sink sender=core2".into(),
        };
        let msg = both.to_string();
        assert!(msg.contains("admission mismatch (mailbox+sender)"), "{msg}");
    }
}
