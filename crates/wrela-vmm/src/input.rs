//! Host input normalization and record/replay routing.

use std::io::{Read, Seek};
use std::os::unix::fs::MetadataExt;
use std::path::PathBuf;
use wrela_machine::input::{Control, InputEventV1, InputQueue};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostInputKind {
    MacKey,
    LinuxKey,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HostInputEvent {
    pub kind: HostInputKind,
    pub host_code: u16,
    pub pressed: bool,
    pub repeat: bool,
    pub timestamp_ns: u64,
    pub host_controller_id: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InputMode {
    Record,
    Replay,
}

#[derive(Debug)]
pub struct InputRouter {
    mode: InputMode,
    queue: InputQueue,
}

impl InputRouter {
    pub fn new(mode: InputMode) -> Self {
        Self {
            mode,
            queue: InputQueue::default(),
        }
    }

    /// Translate a live host event. Timestamps and host controller identities
    /// are intentionally ignored; player assignment is a sealed caller input.
    /// Repeat events are suppressed so host repeat policy cannot enter replay.
    pub fn route_live(&mut self, player: u8, event: HostInputEvent) -> Result<bool, String> {
        if self.mode == InputMode::Replay || event.repeat {
            return Ok(false);
        }
        let Some(control) = normalize_control(event.kind, event.host_code) else {
            return Ok(false);
        };
        self.queue.push(player, control, i16::from(event.pressed))?;
        Ok(true)
    }

    pub fn route_recorded(&mut self, event: InputEventV1) -> Result<(), String> {
        if self.mode != InputMode::Replay {
            return Err("input router: recorded input is accepted only in replay mode".into());
        }
        self.queue.push_recorded(event)
    }

    pub fn pop(&mut self) -> Option<InputEventV1> {
        self.queue.pop()
    }

    pub fn is_empty(&self) -> bool {
        self.queue.is_empty()
    }
}

#[derive(Debug)]
pub(crate) struct InputDevice {
    router: InputRouter,
    source: Option<InputSource>,
    offset: u64,
    tail: String,
}

#[derive(Debug)]
struct InputSource {
    path: PathBuf,
    file: std::fs::File,
    device: u64,
    inode: u64,
}

impl InputDevice {
    pub(crate) fn from_process(replay: bool) -> Result<Self, String> {
        let source_path = if replay {
            None
        } else {
            std::env::var_os("WRELA_INPUT_EVENTS").map(PathBuf::from)
        };
        let source = source_path
            .map(|path| {
                let file = std::fs::File::open(&path).map_err(|error| {
                    format!("open input event transport `{}`: {error}", path.display())
                })?;
                let metadata = file.metadata().map_err(|error| {
                    format!("stat input event transport `{}`: {error}", path.display())
                })?;
                if !metadata.is_file() {
                    return Err(format!(
                        "input event transport `{}` is not a regular file",
                        path.display()
                    ));
                }
                Ok(InputSource {
                    path,
                    file,
                    device: metadata.dev(),
                    inode: metadata.ino(),
                })
            })
            .transpose()?;
        Ok(Self {
            router: InputRouter::new(if replay {
                InputMode::Replay
            } else {
                InputMode::Record
            }),
            source,
            offset: 0,
            tail: String::new(),
        })
    }

    fn poll_live(&mut self) -> Result<(), String> {
        self.poll_live_after_snapshot(|| {})
    }

    fn poll_live_after_snapshot(&mut self, after_snapshot: impl FnOnce()) -> Result<(), String> {
        let Some(source) = &mut self.source else {
            return Ok(());
        };
        let path_metadata = std::fs::metadata(&source.path).map_err(|error| {
            format!(
                "stat input transport `{}` while polling: {error}",
                source.path.display()
            )
        })?;
        if path_metadata.dev() != source.device || path_metadata.ino() != source.inode {
            return Err("input transport path was replaced while the VMM was running".into());
        }
        let len = source
            .file
            .metadata()
            .map_err(|error| format!("stat input transport `{}`: {error}", source.path.display()))?
            .len();
        if len < self.offset {
            return Err("input transport was truncated while the VMM was running".into());
        }
        after_snapshot();
        source
            .file
            .seek(std::io::SeekFrom::Start(self.offset))
            .map_err(|error| format!("seek input transport: {error}"))?;
        let extent = len - self.offset;
        let mut bytes = vec![
            0;
            usize::try_from(extent).map_err(|_| {
                "input transport snapshot is too large for this host".to_string()
            })?
        ];
        source
            .file
            .read_exact(&mut bytes)
            .map_err(|error| format!("read input transport snapshot: {error}"))?;
        self.offset = self
            .offset
            .checked_add(extent)
            .ok_or_else(|| "input transport offset overflowed".to_string())?;
        self.tail.push_str(
            std::str::from_utf8(&bytes)
                .map_err(|_| "input transport contains non-UTF-8 bytes".to_string())?,
        );
        while let Some(newline) = self.tail.find('\n') {
            let line = self.tail[..newline].trim_end_matches('\r').to_string();
            self.tail.drain(..=newline);
            if line.is_empty() {
                continue;
            }
            let (player, event) = parse_transport_line(&line)?;
            self.router.route_live(player, event)?;
        }
        Ok(())
    }

    pub(crate) fn finalize(&mut self) -> Result<(), String> {
        self.poll_live()?;
        if !self.tail.is_empty() {
            return Err("input transport ended with an unterminated event line".into());
        }
        Ok(())
    }

    pub(crate) fn pending(&mut self, chooser: &crate::record::Chooser) -> Result<bool, String> {
        if chooser.is_replaying() {
            return Ok(chooser.replay_input_pending());
        }
        self.poll_live()?;
        Ok(!self.router.is_empty())
    }

    pub(crate) fn pop(
        &mut self,
        chooser: &mut crate::record::Chooser,
    ) -> Result<Option<InputEventV1>, crate::VmmError> {
        if !chooser.is_replaying() {
            self.poll_live().map_err(crate::VmmError::GuestFault)?;
        }
        chooser.choose_input(self.router.pop())
    }
}

fn parse_transport_line(line: &str) -> Result<(u8, HostInputEvent), String> {
    let mut fields = std::collections::BTreeMap::new();
    for part in line.split_whitespace() {
        let (key, value) = part
            .split_once('=')
            .ok_or_else(|| format!("input event field `{part}` has no `=`"))?;
        if fields.insert(key, value).is_some() {
            return Err(format!("input event repeats `{key}`"));
        }
    }
    if fields.len() != 5 {
        return Err(
            "input event must contain exactly kind, code, pressed, repeat, and player".into(),
        );
    }
    let field = |name: &str| {
        fields
            .get(name)
            .copied()
            .ok_or_else(|| format!("input event lacks `{name}`"))
    };
    let kind = match field("kind")? {
        "mac" => HostInputKind::MacKey,
        "linux" => HostInputKind::LinuxKey,
        other => return Err(format!("input event kind `{other}` is not mac or linux")),
    };
    let boolean = |name: &str| match field(name)? {
        "0" => Ok(false),
        "1" => Ok(true),
        other => Err(format!(
            "input event `{name}` is `{other}`, expected 0 or 1"
        )),
    };
    Ok((
        field("player")?
            .parse()
            .map_err(|error| format!("bad input player: {error}"))?,
        HostInputEvent {
            kind,
            host_code: field("code")?
                .parse()
                .map_err(|error| format!("bad input code: {error}"))?,
            pressed: boolean("pressed")?,
            repeat: boolean("repeat")?,
            timestamp_ns: 0,
            host_controller_id: 0,
        },
    ))
}

fn normalize_control(kind: HostInputKind, code: u16) -> Option<Control> {
    match (kind, code) {
        // macOS virtual key codes.
        (HostInputKind::MacKey, 126) => Some(Control::Up),
        (HostInputKind::MacKey, 125) => Some(Control::Down),
        (HostInputKind::MacKey, 123) => Some(Control::Left),
        (HostInputKind::MacKey, 124) => Some(Control::Right),
        (HostInputKind::MacKey, 49) => Some(Control::Primary),
        (HostInputKind::MacKey, 0) => Some(Control::Secondary),
        (HostInputKind::MacKey, 36) => Some(Control::Start),
        // Linux evdev key codes.
        (HostInputKind::LinuxKey, 103) => Some(Control::Up),
        (HostInputKind::LinuxKey, 108) => Some(Control::Down),
        (HostInputKind::LinuxKey, 105) => Some(Control::Left),
        (HostInputKind::LinuxKey, 106) => Some(Control::Right),
        (HostInputKind::LinuxKey, 57) => Some(Control::Primary),
        (HostInputKind::LinuxKey, 30) => Some(Control::Secondary),
        (HostInputKind::LinuxKey, 28) => Some(Control::Start),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn input_device(path: PathBuf) -> InputDevice {
        let file = std::fs::File::open(&path).unwrap();
        let metadata = file.metadata().unwrap();
        InputDevice {
            router: InputRouter::new(InputMode::Record),
            source: Some(InputSource {
                path,
                file,
                device: metadata.dev(),
                inode: metadata.ino(),
            }),
            offset: 0,
            tail: String::new(),
        }
    }

    fn host(kind: HostInputKind, code: u16, timestamp_ns: u64, id: u64) -> HostInputEvent {
        HostInputEvent {
            kind,
            host_code: code,
            pressed: true,
            repeat: false,
            timestamp_ns,
            host_controller_id: id,
        }
    }

    #[test]
    fn mac_and_linux_details_normalize_to_one_ordered_queue() {
        let mut mac = InputRouter::new(InputMode::Record);
        let mut linux = InputRouter::new(InputMode::Record);
        assert!(
            mac.route_live(0, host(HostInputKind::MacKey, 126, 1, 7))
                .unwrap()
        );
        assert!(
            linux
                .route_live(0, host(HostInputKind::LinuxKey, 103, u64::MAX, 999))
                .unwrap()
        );
        assert_eq!(mac.pop(), linux.pop());
    }

    #[test]
    fn every_machine_control_has_matching_mac_and_linux_codes() {
        let mappings = [
            (Control::Up, 126, 103),
            (Control::Down, 125, 108),
            (Control::Left, 123, 105),
            (Control::Right, 124, 106),
            (Control::Primary, 49, 57),
            (Control::Secondary, 0, 30),
            (Control::Start, 36, 28),
        ];
        for (control, mac, linux) in mappings {
            assert_eq!(normalize_control(HostInputKind::MacKey, mac), Some(control));
            assert_eq!(
                normalize_control(HostInputKind::LinuxKey, linux),
                Some(control)
            );
        }
        assert_eq!(normalize_control(HostInputKind::MacKey, u16::MAX), None);
        assert_eq!(normalize_control(HostInputKind::LinuxKey, u16::MAX), None);
    }

    #[test]
    fn repeat_is_normalized_and_replay_suppresses_live_input() {
        let mut event = host(HostInputKind::MacKey, 49, 1, 1);
        event.repeat = true;
        let mut record = InputRouter::new(InputMode::Record);
        assert!(!record.route_live(0, event).unwrap());
        assert_eq!(record.pop(), None);

        let mut replay = InputRouter::new(InputMode::Replay);
        assert!(
            !replay
                .route_live(0, host(HostInputKind::LinuxKey, 57, 2, 2))
                .unwrap()
        );
        replay
            .route_recorded(InputEventV1 {
                sequence: 0,
                player: 0,
                control: Control::Primary,
                value: 1,
            })
            .unwrap();
        assert_eq!(replay.pop().unwrap().control, Control::Primary);
    }

    #[test]
    fn transport_parser_normalizes_metal_drm_and_forge_lines() {
        let (player, mac) =
            parse_transport_line("kind=mac code=126 pressed=1 repeat=0 player=2").unwrap();
        assert_eq!(player, 2);
        let (_, linux) =
            parse_transport_line("kind=linux code=103 pressed=1 repeat=0 player=2").unwrap();
        let mut a = InputRouter::new(InputMode::Record);
        let mut b = InputRouter::new(InputMode::Record);
        a.route_live(player, mac).unwrap();
        b.route_live(player, linux).unwrap();
        assert_eq!(a.pop(), b.pop());
        assert!(parse_transport_line("kind=mac code=1 pressed=2 repeat=0 player=0").is_err());
    }

    #[test]
    fn append_only_transport_records_normalized_events_and_refuses_truncation() {
        use std::io::Write as _;

        let path =
            std::env::temp_dir().join(format!("wrela-input-events-{:016x}", std::process::id()));
        std::fs::File::create(&path).unwrap();
        let mut device = input_device(path.clone());
        let mut chooser = crate::record::Chooser::recorder();
        let mut append = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        writeln!(append, "kind=mac code=126 pressed=1 repeat=0 player=2").unwrap();
        append.flush().unwrap();
        assert!(device.pending(&chooser).unwrap());
        let first = device.pop(&mut chooser).unwrap().unwrap();
        assert_eq!(first.sequence, 0);
        assert_eq!(first.control, Control::Up);
        assert_eq!(first.player, 2);

        writeln!(append, "kind=linux code=103 pressed=0 repeat=0 player=2").unwrap();
        append.flush().unwrap();
        let second = device.pop(&mut chooser).unwrap().unwrap();
        assert_eq!(second.sequence, 1);
        assert_eq!(second.control, Control::Up);
        assert_eq!(second.value, 0);
        let (choices, divergences) = crate::record::finish_chooser(chooser).unwrap();
        assert!(divergences.is_empty());
        assert!(
            choices
                .iter()
                .all(|choice| matches!(choice, crate::record::ChoiceEntry::InputEvent(_)))
        );

        drop(append);
        std::fs::File::create(&path).unwrap();
        assert!(device.pending(&crate::record::Chooser::recorder()).is_err());
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn snapshot_extent_prevents_a_concurrent_append_from_being_read_twice() {
        use std::io::Write as _;

        let path = std::env::temp_dir().join(format!(
            "wrela-input-events-concurrent-{:016x}",
            std::process::id()
        ));
        std::fs::write(&path, "kind=mac code=126 pressed=1 repeat=0 player=0\n").unwrap();
        let mut append = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        let mut device = input_device(path.clone());
        device
            .poll_live_after_snapshot(|| {
                writeln!(append, "kind=linux code=57 pressed=1 repeat=0 player=0").unwrap();
                append.flush().unwrap();
            })
            .unwrap();
        assert_eq!(device.router.pop().unwrap().control, Control::Up);
        assert!(device.router.is_empty());
        device.poll_live().unwrap();
        assert_eq!(device.router.pop().unwrap().control, Control::Primary);
        assert!(device.router.is_empty());
        drop(append);
        std::fs::remove_file(path).unwrap();
    }

    #[test]
    fn path_replacement_and_unterminated_final_event_fail_closed() {
        let path = std::env::temp_dir().join(format!(
            "wrela-input-events-replace-{:016x}",
            std::process::id()
        ));
        let replacement = path.with_extension("replacement");
        std::fs::write(&path, "kind=mac code=126 pressed=1 repeat=0 player=0\n").unwrap();
        let mut replaced = input_device(path.clone());
        std::fs::write(
            &replacement,
            "kind=linux code=103 pressed=1 repeat=0 player=0\nextra\n",
        )
        .unwrap();
        std::fs::rename(&replacement, &path).unwrap();
        assert!(
            replaced
                .poll_live()
                .unwrap_err()
                .contains("path was replaced")
        );
        std::fs::remove_file(&path).unwrap();

        std::fs::write(&path, "kind=mac code=126 pressed=1 repeat=0 player=0").unwrap();
        let mut unterminated = input_device(path.clone());
        assert!(
            unterminated
                .finalize()
                .unwrap_err()
                .contains("unterminated")
        );
        std::fs::remove_file(path).unwrap();
    }
}
