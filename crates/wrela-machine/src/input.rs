//! Portable, host-neutral input queue state.
//!
//! Host key codes, timestamps, controller identifiers, and repeat policy are
//! deliberately absent. Host adapters translate those details before an
//! event enters this queue.

use std::collections::VecDeque;

pub const MAX_PLAYERS: u8 = 4;
pub const QUEUE_CAPACITY: usize = 256;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Control {
    Up,
    Down,
    Left,
    Right,
    Primary,
    Secondary,
    Start,
}

pub const fn control_name(control: Control) -> &'static str {
    match control {
        Control::Up => "up",
        Control::Down => "down",
        Control::Left => "left",
        Control::Right => "right",
        Control::Primary => "primary",
        Control::Secondary => "secondary",
        Control::Start => "start",
    }
}

pub fn parse_control(name: &str) -> Option<Control> {
    match name {
        "up" => Some(Control::Up),
        "down" => Some(Control::Down),
        "left" => Some(Control::Left),
        "right" => Some(Control::Right),
        "primary" => Some(Control::Primary),
        "secondary" => Some(Control::Secondary),
        "start" => Some(Control::Start),
        _ => None,
    }
}

pub const fn control_code(control: Control) -> u8 {
    match control {
        Control::Up => 0,
        Control::Down => 1,
        Control::Left => 2,
        Control::Right => 3,
        Control::Primary => 4,
        Control::Secondary => 5,
        Control::Start => 6,
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InputEventV1 {
    pub sequence: u64,
    pub player: u8,
    pub control: Control,
    /// Digital controls use 0 or 1. The signed representation is reserved for
    /// a future normalized axis without admitting host-native ranges.
    pub value: i16,
}

pub fn validate_event(event: InputEventV1) -> Result<(), String> {
    if event.player >= MAX_PLAYERS {
        return Err(format!(
            "input queue: player {} is outside 0..{}",
            event.player, MAX_PLAYERS
        ));
    }
    if !matches!(event.value, 0 | 1) {
        return Err(format!(
            "input queue: digital control value {} is not 0 or 1",
            event.value
        ));
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct InputQueue {
    next_sequence: u64,
    events: VecDeque<InputEventV1>,
}

impl InputQueue {
    pub fn push(&mut self, player: u8, control: Control, value: i16) -> Result<(), String> {
        self.push_event(InputEventV1 {
            sequence: self.next_sequence,
            player,
            control,
            value,
        })
    }

    pub fn push_recorded(&mut self, event: InputEventV1) -> Result<(), String> {
        self.push_event(event)
    }

    fn push_event(&mut self, event: InputEventV1) -> Result<(), String> {
        validate_event(event)?;
        if event.sequence != self.next_sequence {
            return Err(format!(
                "input queue: expected sequence {}, got {}",
                self.next_sequence, event.sequence
            ));
        }
        if self.events.len() == QUEUE_CAPACITY {
            return Err("input queue: full; refuse to drop or reorder input".into());
        }
        self.next_sequence = self
            .next_sequence
            .checked_add(1)
            .ok_or_else(|| "input queue: sequence overflow".to_string())?;
        self.events.push_back(event);
        Ok(())
    }

    pub fn pop(&mut self) -> Option<InputEventV1> {
        self.events.pop_front()
    }

    pub fn len(&self) -> usize {
        self.events.len()
    }

    pub fn is_empty(&self) -> bool {
        self.events.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn queue_is_ordered_bounded_and_fail_closed() {
        let mut queue = InputQueue::default();
        queue.push(0, Control::Primary, 1).unwrap();
        queue.push(0, Control::Primary, 0).unwrap();
        assert_eq!(queue.pop().unwrap().sequence, 0);
        assert_eq!(queue.pop().unwrap().sequence, 1);
        assert!(queue.push(MAX_PLAYERS, Control::Start, 1).is_err());
        assert!(queue.push(0, Control::Start, -1).is_err());
        for _ in 0..QUEUE_CAPACITY {
            queue.push(0, Control::Start, 1).unwrap();
        }
        assert!(queue.push(0, Control::Start, 1).is_err());
        let mut replay = InputQueue::default();
        assert!(
            queue
                .push_recorded(InputEventV1 {
                    sequence: 9,
                    player: 0,
                    control: Control::Start,
                    value: 1,
                })
                .is_err()
        );
        assert!(
            replay
                .push_recorded(InputEventV1 {
                    sequence: 9,
                    player: 0,
                    control: Control::Start,
                    value: 1,
                })
                .is_err()
        );
    }
}
