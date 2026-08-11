//! Deterministic row-event isolation and half-open domain partitioning.

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventInterval {
    pub lo: u16,
    pub hi: u16,
    pub generator_id: u32,
    pub subdivision_depth: u8,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EventCorridor {
    pub x0: u16,
    pub x1: u16,
    pub first_event: u16,
    pub event_count: u16,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct RegularDomain {
    pub x0: u16,
    pub x1: u16,
    pub left_corridor: Option<u16>,
    pub right_corridor: Option<u16>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventError {
    InvalidDomain,
    InvalidEvent,
    CapacityExceeded,
}

#[derive(Debug)]
pub struct EventPartition<'a> {
    pub sorted_events: &'a [EventInterval],
    pub corridors: &'a [EventCorridor],
    pub regular: &'a [RegularDomain],
}

pub fn partition_row_events<'a>(
    tile_x0: u16,
    tile_x1: u16,
    events: &[EventInterval],
    event_scratch: &'a mut [EventInterval],
    corridor_output: &'a mut [EventCorridor],
    regular_output: &'a mut [RegularDomain],
) -> Result<EventPartition<'a>, EventError> {
    if tile_x0 >= tile_x1 || events.len() > event_scratch.len() {
        return Err(if tile_x0 >= tile_x1 {
            EventError::InvalidDomain
        } else {
            EventError::CapacityExceeded
        });
    }
    event_scratch[..events.len()].copy_from_slice(events);
    let sorted = &mut event_scratch[..events.len()];
    for event in sorted.iter() {
        if event.lo >= event.hi || event.lo < tile_x0 || event.hi > tile_x1 {
            return Err(EventError::InvalidEvent);
        }
    }
    for index in 1..sorted.len() {
        let value = sorted[index];
        let mut destination = index;
        while destination != 0
            && (value.lo, value.hi, value.generator_id)
                < (
                    sorted[destination - 1].lo,
                    sorted[destination - 1].hi,
                    sorted[destination - 1].generator_id,
                )
        {
            sorted[destination] = sorted[destination - 1];
            destination -= 1;
        }
        sorted[destination] = value;
    }

    let mut corridor_count = 0_usize;
    let mut event_start = 0_usize;
    while event_start < sorted.len() {
        if corridor_count == corridor_output.len() {
            return Err(EventError::CapacityExceeded);
        }
        let mut event_end = event_start + 1;
        let x0 = sorted[event_start].lo;
        let mut x1 = sorted[event_start].hi;
        while event_end < sorted.len() && sorted[event_end].lo < x1 {
            x1 = x1.max(sorted[event_end].hi);
            event_end += 1;
        }
        corridor_output[corridor_count] = EventCorridor {
            x0,
            x1,
            first_event: u16::try_from(event_start).map_err(|_| EventError::CapacityExceeded)?,
            event_count: u16::try_from(event_end - event_start)
                .map_err(|_| EventError::CapacityExceeded)?,
        };
        corridor_count += 1;
        event_start = event_end;
    }

    let mut regular_count = 0_usize;
    let mut cursor = tile_x0;
    for (corridor_id, corridor) in corridor_output[..corridor_count]
        .iter()
        .copied()
        .enumerate()
    {
        if cursor < corridor.x0 {
            if regular_count == regular_output.len() {
                return Err(EventError::CapacityExceeded);
            }
            regular_output[regular_count] = RegularDomain {
                x0: cursor,
                x1: corridor.x0,
                left_corridor: corridor_id
                    .checked_sub(1)
                    .and_then(|id| u16::try_from(id).ok()),
                right_corridor: u16::try_from(corridor_id).ok(),
            };
            regular_count += 1;
        }
        cursor = cursor.max(corridor.x1);
    }
    if cursor < tile_x1 {
        if regular_count == regular_output.len() {
            return Err(EventError::CapacityExceeded);
        }
        regular_output[regular_count] = RegularDomain {
            x0: cursor,
            x1: tile_x1,
            left_corridor: corridor_count
                .checked_sub(1)
                .and_then(|id| u16::try_from(id).ok()),
            right_corridor: None,
        };
        regular_count += 1;
    }
    if corridor_count == 0 && regular_count == 0 {
        if regular_output.is_empty() {
            return Err(EventError::CapacityExceeded);
        }
        regular_output[0] = RegularDomain {
            x0: tile_x0,
            x1: tile_x1,
            left_corridor: None,
            right_corridor: None,
        };
        regular_count = 1;
    }
    Ok(EventPartition {
        sorted_events: sorted,
        corridors: &corridor_output[..corridor_count],
        regular: &regular_output[..regular_count],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn overlapping_and_simultaneous_events_share_one_closed_corridor() {
        let events = [
            EventInterval {
                lo: 8,
                hi: 10,
                generator_id: 7,
                subdivision_depth: 2,
            },
            EventInterval {
                lo: 9,
                hi: 12,
                generator_id: 3,
                subdivision_depth: 1,
            },
            EventInterval {
                lo: 20,
                hi: 21,
                generator_id: 4,
                subdivision_depth: 0,
            },
        ];
        let mut sorted = [EventInterval::default(); 3];
        let mut corridors = [EventCorridor::default(); 3];
        let mut regular = [RegularDomain::default(); 4];
        let partition =
            partition_row_events(0, 32, &events, &mut sorted, &mut corridors, &mut regular)
                .unwrap();
        assert_eq!(
            partition.corridors,
            &[
                EventCorridor {
                    x0: 8,
                    x1: 12,
                    first_event: 0,
                    event_count: 2,
                },
                EventCorridor {
                    x0: 20,
                    x1: 21,
                    first_event: 2,
                    event_count: 1,
                },
            ]
        );
        assert_eq!(
            partition
                .sorted_events
                .iter()
                .map(|event| event.generator_id)
                .collect::<Vec<_>>(),
            [7, 3, 4]
        );
        assert_eq!(
            partition
                .regular
                .iter()
                .map(|domain| (domain.x0, domain.x1))
                .collect::<Vec<_>>(),
            [(0, 8), (12, 20), (21, 32)]
        );
    }

    #[test]
    fn capacity_failure_occurs_before_output_overrun() {
        let events = [EventInterval {
            lo: 1,
            hi: 2,
            generator_id: 0,
            subdivision_depth: 0,
        }];
        let mut sorted = [EventInterval::default(); 1];
        let mut corridors = [];
        let mut regular = [RegularDomain::default(); 2];
        assert_eq!(
            partition_row_events(0, 4, &events, &mut sorted, &mut corridors, &mut regular)
                .unwrap_err(),
            EventError::CapacityExceeded
        );
    }
}
