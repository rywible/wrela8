use std::collections::BTreeMap;
use std::path::Path;

use crate::{BootOutcome, VmmError, boot_image_core};

pub fn digest_hex(bytes: &[u8]) -> String {
    wrela_machine::sha256::sha256_hex(bytes)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceEntry {
    ClockRead {
        value: u64,
    },
    DeadlineWake {
        deadline_ns: u64,
    },
    VectorRaise {
        vector: u64,
    },
    Admission {
        mailbox: String,
        sender: String,
    },
    DeviceCompletion {
        device: String,
        queue: u32,
        head: u32,
        status: u32,
        len: u32,
        digest: String,
    },
    Progress {
        core: u32,
    },
    EntropyRead {
        bytes: Vec<u8>,
    },
    FramePresent {
        sequence: u64,
        digest: String,
    },
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
            ChoiceEntry::EntropyRead { .. } => "EntropyRead",
            ChoiceEntry::FramePresent { .. } => "FramePresent",
        }
    }

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
            ChoiceEntry::EntropyRead { bytes } => {
                format!(
                    "EntropyRead len={} hex={}",
                    bytes.len(),
                    bytes_to_lowercase_hex(bytes)
                )
            }
            ChoiceEntry::FramePresent { sequence, digest } => {
                format!("FramePresent sequence={sequence} digest={digest}")
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
            "EntropyRead" => {
                let len: u64 = field("len")?
                    .parse()
                    .map_err(|e| format!("bad EntropyRead len: {e}"))?;
                let hex = field("hex")?;
                let expected_hex_len = len
                    .checked_mul(2)
                    .ok_or_else(|| format!("bad EntropyRead len {len}: hex length overflow"))?;
                if hex.len() as u64 != expected_hex_len {
                    return Err(format!(
                        "bad EntropyRead hex length: got {} chars, want {} (2*len={len})",
                        hex.len(),
                        expected_hex_len
                    ));
                }
                let bytes =
                    lowercase_hex_to_bytes(hex).map_err(|e| format!("bad EntropyRead hex: {e}"))?;
                Ok(ChoiceEntry::EntropyRead { bytes })
            }
            "FramePresent" => {
                let sequence = field("sequence")?
                    .parse()
                    .map_err(|e| format!("bad FramePresent sequence: {e}"))?;
                let digest = field("digest")?;
                if !wrela_machine::sha256::is_sha256_hex(digest) {
                    return Err(format!("bad FramePresent digest: {digest:?}"));
                }
                Ok(ChoiceEntry::FramePresent {
                    sequence,
                    digest: digest.to_string(),
                })
            }
            other => Err(format!("unknown choice tag `{other}`")),
        }
    }
}

fn bytes_to_lowercase_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

fn lowercase_hex_to_bytes(hex: &str) -> Result<Vec<u8>, String> {
    if hex.len() % 2 != 0 {
        return Err(format!("odd hex length {}", hex.len()));
    }
    let mut out = Vec::with_capacity(hex.len() / 2);
    let mut chars = hex.chars();
    while let Some(hi) = chars.next() {
        let lo = chars.next().expect("even length checked above");
        let hi_n = lowercase_hex_nibble(hi)?;
        let lo_n = lowercase_hex_nibble(lo)?;
        out.push((hi_n << 4) | lo_n);
    }
    Ok(out)
}

fn lowercase_hex_nibble(c: char) -> Result<u8, String> {
    match c {
        '0'..='9' => Ok((c as u8) - b'0'),
        'a'..='f' => Ok((c as u8) - b'a' + 10),
        _ => Err(format!("non-lowercase-hex digit {c:?}")),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChoiceRequest {
    ClockRead,
    DeadlineWake {
        deadline_ns: u64,
    },
    VectorRaise {
        vector: u64,
    },
    DeviceCompletion {
        device: String,
        queue: u32,
        head: u32,
        status: u32,
        len: u32,
        digest: String,
    },
    Admission {
        mailbox: String,
        sender: String,
    },
    Progress {
        core: u32,
    },
    EntropyRead {
        len: u64,
    },
    FramePresent {
        sequence: u64,
        digest: String,
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
            ChoiceRequest::EntropyRead { .. } => "EntropyRead",
            ChoiceRequest::FramePresent { .. } => "FramePresent",
        }
    }

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
            ChoiceRequest::EntropyRead { len } => {
                assert!(
                    *len <= wrela_machine::machine_info::ENTROPY_LEN_MAX,
                    "EntropyRead fallback len {len} exceeds ENTROPY_LEN_MAX"
                );
                ChoiceEntry::EntropyRead {
                    bytes: vec![0; *len as usize],
                }
            }
            ChoiceRequest::FramePresent { sequence, digest } => ChoiceEntry::FramePresent {
                sequence: *sequence,
                digest: digest.clone(),
            },
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Divergence {
    ChoiceLogUnderrun {
        index: usize,
        recorded: usize,
    },
    ChoiceLogOverrun {
        consumed: usize,
        recorded: usize,
    },
    ChoiceTagMismatch {
        index: usize,
        expected: String,
        actual: String,
    },
    TranscriptDigestMismatch {
        expected: String,
        actual: String,
    },
    ExitCodeMismatch {
        expected: u64,
        actual: u64,
    },
    DeviceCompletionMismatch {
        index: usize,
        recorded: String,
        actual: String,
    },
    AdmissionMismatch {
        index: usize,
        recorded: String,
        actual: String,
    },
    AdmissionCountMismatch {
        index: usize,
        detail: String,
    },
    ProgressMismatch {
        index: usize,
        recorded: u32,
        actual: u32,
    },
    FramePresentMismatch {
        index: usize,
        recorded: String,
        actual: String,
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
            Divergence::FramePresentMismatch {
                index,
                recorded,
                actual,
            } => write!(
                f,
                "choice #{index} frame present mismatch: recorded `{recorded}`, this boot \
                 presented `{actual}`"
            ),
        }
    }
}

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
    Replay {
        log: Vec<ChoiceEntry>,
        idx: usize,
        admission_bag: BTreeMap<(String, String), usize>,
    },
}

pub struct Chooser {
    mode: ChooserMode,
    log: Vec<ChoiceEntry>,
    divergences: Vec<Divergence>,
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
            strict: false,
        }
    }

    pub fn is_recording(&self) -> bool {
        matches!(self.mode, ChooserMode::Record)
    }

    pub fn is_replaying(&self) -> bool {
        matches!(self.mode, ChooserMode::Replay { .. })
    }

    pub fn strict(mut self) -> Chooser {
        self.strict = true;
        self
    }

    pub fn fatal_divergence(&self) -> Option<&Divergence> {
        if self.strict {
            self.divergences.first()
        } else {
            None
        }
    }

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
                            let alt = admission_bag
                                .keys()
                                .find(|(m, _)| m == mailbox)
                                .or_else(|| admission_bag.keys().find(|(_, s)| s == sender))
                                .cloned();
                            if let Some((m, s)) = alt {
                                if let Some(n) = admission_bag.get_mut(&(m.clone(), s.clone())) {
                                    *n -= 1;
                                    if *n == 0 {
                                        admission_bag.remove(&(m.clone(), s.clone()));
                                    }
                                }
                                self.divergences.push(Divergence::AdmissionMismatch {
                                    index,
                                    recorded: format!("Admission mailbox={m} sender={s}"),
                                    actual: format!("Admission mailbox={mailbox} sender={sender}"),
                                });
                                let fallback = request.fallback();
                                self.log.push(fallback.clone());
                                return fallback;
                            }
                            let entry = ChoiceEntry::Admission {
                                mailbox: mailbox.clone(),
                                sender: sender.clone(),
                            };
                            self.log.push(entry.clone());
                            return entry;
                        }
                    }
                }

                while *idx < log.len() && matches!(log[*idx], ChoiceEntry::Admission { .. }) {
                    *idx += 1;
                }
                let Some(entry) = log.get(*idx).cloned() else {
                    let index = *idx;
                    let recorded = log.len();
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

    pub fn choose_checked(
        &mut self,
        request: ChoiceRequest,
        live: impl FnOnce() -> ChoiceEntry,
    ) -> Result<ChoiceEntry, crate::VmmError> {
        let entry = self.choose_next(request, live);
        self.abort_if_strict_diverged()?;
        Ok(entry)
    }

    pub fn check_frame_present(
        &mut self,
        sequence: u64,
        digest: String,
    ) -> Result<(), crate::VmmError> {
        let request = ChoiceRequest::FramePresent { sequence, digest };
        let actual = request.fallback();
        let index = self.resolved_count();
        let chosen = self.choose_checked(request, || actual.clone())?;
        if chosen != actual {
            self.note_divergence_checked(Divergence::FramePresentMismatch {
                index,
                recorded: chosen.to_text_fields(),
                actual: actual.to_text_fields(),
            })?;
        }
        Ok(())
    }

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

    pub fn note_divergence(&mut self, divergence: Divergence) {
        self.divergences.push(divergence);
    }

    pub fn resolved_count(&self) -> usize {
        self.log.len()
    }

    fn finish(self) -> (Vec<ChoiceEntry>, Vec<Divergence>) {
        let mut divergences = self.divergences;
        if let ChooserMode::Replay {
            log,
            mut idx,
            admission_bag: _,
        } = self.mode
        {
            while idx < log.len()
                && matches!(
                    log[idx],
                    ChoiceEntry::Admission { .. } | ChoiceEntry::Progress { .. }
                )
            {
                idx += 1;
            }
            if idx < log.len() {
                divergences.push(Divergence::ChoiceLogOverrun {
                    consumed: idx,
                    recorded: log.len(),
                });
            }
        }
        (self.log, divergences)
    }
}

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

const FORMAT_HEADER: &str = "ChoiceLog v1";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RecordFile {
    pub choices: Vec<ChoiceEntry>,
    pub transcript_digest: String,
    pub exit_code: u64,
    pub exits: u64,
}

impl RecordFile {
    pub fn from_outcome(outcome: &BootOutcome) -> RecordFile {
        RecordFile {
            choices: outcome.choices.clone(),
            transcript_digest: digest_hex(&outcome.transcript),
            exit_code: outcome.exit_code,
            exits: outcome.exits,
        }
    }

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

pub fn record(report_path: &Path, img_path: &Path) -> Result<RecordFile, VmmError> {
    let (outcome, divergences) = boot_image_core(report_path, img_path, None)?;
    debug_assert!(
        divergences.is_empty(),
        "a live recording boot cannot diverge from itself: {divergences:?}"
    );
    Ok(RecordFile::from_outcome(&outcome))
}

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
    let recorded_frames: Vec<wrela_machine::pixels::PresentedFrame> = recorded
        .choices
        .iter()
        .filter_map(|choice| match choice {
            ChoiceEntry::FramePresent { sequence, digest } => {
                Some(wrela_machine::pixels::PresentedFrame {
                    sequence: *sequence,
                    digest: digest.clone(),
                    bgra: Vec::new(),
                })
            }
            _ => None,
        })
        .collect();
    if crate::replay::verify_frame_replay(&recorded_frames, &outcome.frames).is_err()
        && !divergences
            .iter()
            .any(|d| matches!(d, Divergence::FramePresentMismatch { .. }))
    {
        divergences.push(Divergence::FramePresentMismatch {
            index: 0,
            recorded: String::from_utf8_lossy(&crate::replay::frame_log_bytes(&recorded_frames))
                .trim()
                .to_string(),
            actual: String::from_utf8_lossy(&crate::replay::frame_log_bytes(&outcome.frames))
                .trim()
                .to_string(),
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
                ChoiceEntry::EntropyRead {
                    bytes: vec![0xde, 0xad, 0xbe, 0xef],
                },
                ChoiceEntry::FramePresent {
                    sequence: 0,
                    digest: "e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855"
                        .to_string(),
                },
            ],
            transcript_digest: "0123456789abcdef".to_string(),
            exit_code: 0,
            exits: 3,
        };
        let expected = "ChoiceLog v1\n\
             choice_count=8\n\
             choice[0]=ClockRead value=12345\n\
             choice[1]=DeadlineWake deadline_ns=500000\n\
             choice[2]=VectorRaise vector=0\n\
             choice[3]=Admission mailbox=Store sender=root\n\
             choice[4]=Progress core=1\n\
             choice[5]=DeviceCompletion device=blk queue=0 head=3 status=0 len=513 digest=fedcba9876543210\n\
             choice[6]=EntropyRead len=4 hex=deadbeef\n\
             choice[7]=FramePresent sequence=0 digest=e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\n\
             transcript_digest=0123456789abcdef\n\
             exit_code=0\n\
             exits=3\n";
        assert_eq!(rec.to_text(), expected);
        assert_eq!(RecordFile::parse(expected).expect("parses"), rec);
    }

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
    fn entropy_read_choice_parses_and_round_trips() {
        let entry = ChoiceEntry::EntropyRead {
            bytes: vec![0xab, 0xcd, 0xef, 0x01],
        };
        assert_eq!(entry.tag(), "EntropyRead");
        assert_eq!(ChoiceRequest::EntropyRead { len: 4 }.tag(), "EntropyRead");
        assert_eq!(entry.to_text_fields(), "EntropyRead len=4 hex=abcdef01");
        assert_eq!(
            ChoiceEntry::parse_fields("EntropyRead len=4 hex=abcdef01").expect("parses"),
            entry
        );
        let rec = RecordFile {
            choices: vec![entry.clone()],
            transcript_digest: digest_hex(b"e"),
            exit_code: 0,
            exits: 1,
        };
        assert_eq!(RecordFile::parse(&rec.to_text()).expect("round-trip"), rec);

        assert!(
            ChoiceEntry::parse_fields("EntropyRead len=4 hex=abcd").is_err(),
            "hex shorter than 2*len must refuse"
        );
        assert!(
            ChoiceEntry::parse_fields("EntropyRead len=2 hex=abcdef").is_err(),
            "hex longer than 2*len must refuse"
        );
        assert!(
            ChoiceEntry::parse_fields("EntropyRead len=2 hex=ABCD").is_err(),
            "uppercase hex must refuse"
        );

        let fb = ChoiceRequest::EntropyRead { len: 8 }.fallback();
        assert_eq!(fb, ChoiceEntry::EntropyRead { bytes: vec![0; 8] });
    }

    #[test]
    fn frame_present_choice_parses_and_round_trips() {
        let entry = ChoiceEntry::FramePresent {
            sequence: 7,
            digest: wrela_machine::sha256::sha256_hex(b"frame"),
        };
        assert_eq!(entry.tag(), "FramePresent");
        assert_eq!(
            ChoiceEntry::parse_fields(&entry.to_text_fields()).expect("parses"),
            entry
        );
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
        let text = sample().to_text();
        assert!(!text.contains("SystemTime"));
        assert!(text.ends_with('\n'));
    }

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
        let mut c = Chooser::replayer(vec![]);
        let got = c.choose_next(ChoiceRequest::ClockRead, || {
            panic!("replay must never call `live`")
        });
        assert_eq!(got, ChoiceEntry::ClockRead { value: 0 });
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
        assert_eq!(got, ChoiceEntry::DeadlineWake { deadline_ns: 5 });
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
    fn frame_present_replay_compares_sequence_and_digest_payload() {
        let recorded = ChoiceEntry::FramePresent {
            sequence: 0,
            digest: "recorded".to_string(),
        };
        let mut chooser = Chooser::replayer(vec![recorded]);
        chooser
            .check_frame_present(0, "actual".to_string())
            .expect("non-strict replay records the mismatch");
        let (_, divergences) = finish_chooser(chooser).expect("finish replay");
        assert!(matches!(
            divergences.as_slice(),
            [Divergence::FramePresentMismatch { index: 0, .. }]
        ));

        let recorded = ChoiceEntry::FramePresent {
            sequence: 3,
            digest: "same".to_string(),
        };
        let error = Chooser::replayer(vec![recorded])
            .strict()
            .check_frame_present(4, "same".to_string())
            .expect_err("strict replay rejects the first payload mismatch");
        assert!(
            error.to_string().contains("frame present mismatch"),
            "{error}"
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
        assert!(
            divs.is_empty(),
            "cap-1 under-count extras must not diverge: {divs:?}"
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
        assert!(
            divs.is_empty(),
            "leftover Admissions under overlap must not diverge: {divs:?}"
        );

        let mut c = Chooser::replayer(vec![ChoiceEntry::Admission {
            mailbox: "Sink".into(),
            sender: "core2".into(),
        }]);
        let _ = c.choose_next(
            ChoiceRequest::Admission {
                mailbox: "Sink".into(),
                sender: "core0".into(),
            },
            || panic!("live"),
        );
        let (_, divs) = finish_chooser(c).expect("non-strict finish");
        assert_eq!(divs.len(), 1);
        let msg = divs[0].to_string();
        assert!(msg.contains("admission mismatch (sender)"), "{msg}");
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
