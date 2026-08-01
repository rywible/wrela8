pub use wrela_machine::virtio::{
    DESC_F_NEXT, DESC_F_WRITE, DESC_SIZE, DEVICE_FEATURES, DOORBELL_BYTES, F_BLK_FLUSH,
    F_VERSION_1, REQ_HEADER_SIZE, REQ_STATUS_SIZE, SLOT_BOOK_BYTES, SLOT_BOOK_EPOCH,
    SLOT_BOOK_LAST_USED, SLOT_BOOK_QUARANTINE_STAMP, SLOT_BOOK_QUIESCED, avail_bytes, desc_bytes,
    used_bytes,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingPlacement {
    pub depth: u16,
    pub desc: u64,
    pub avail: u64,
    pub used: u64,
    pub doorbell: u64,
    pub bytes: u64,
}

fn align_up(addr: u64, align: u64) -> u64 {
    (addr + align - 1) & !(align - 1)
}

pub fn place_ring(pool_base: u64, depth: u16) -> Option<RingPlacement> {
    if depth == 0 || !depth.is_power_of_two() {
        return None;
    }
    let desc = pool_base;
    let avail = align_up(desc + desc_bytes(depth), 8);
    let used = align_up(avail + avail_bytes(depth), 8);
    let doorbell = align_up(used + used_bytes(depth), 8);
    let end = doorbell + DOORBELL_BYTES;
    Some(RingPlacement {
        depth,
        desc,
        avail,
        used,
        doorbell,
        bytes: end - pool_base,
    })
}

pub fn feature_bit(variant: &str) -> Option<u64> {
    match variant {
        "Flush" | "BLK_FLUSH" | "VIRTIO_BLK_F_FLUSH" => Some(F_BLK_FLUSH),
        "Version1" | "VERSION_1" | "VIRTIO_F_VERSION_1" => Some(F_VERSION_1),
        "RingReset" | "RING_RESET" => None,
        _ => None,
    }
}

pub fn accepted_features(required_variant_names: &[&str]) -> Result<u64, String> {
    let mut requested = F_VERSION_1;
    for name in required_variant_names {
        match feature_bit(name) {
            Some(bit) => requested |= bit,
            None => {
                return Err(format!(
                    "the image requires virtio-blk feature `{name}`, which this device model \
                     does not offer (offered: {DEVICE_FEATURES:#x} = VIRTIO_F_VERSION_1 | \
                     VIRTIO_BLK_F_FLUSH; `{name}` is not in that set — plans/M7.md decision 14: \
                     an unofferable required feature is a *build* error)"
                ));
            }
        }
    }
    let unknown = requested & !DEVICE_FEATURES;
    if unknown != 0 {
        return Err(format!(
            "the image requires virtio-blk feature bits {unknown:#x}, which this device model \
             does not offer (offered: {DEVICE_FEATURES:#x}; requested: {requested:#x})"
        ));
    }
    Ok(requested)
}

pub const DESCRIPTORS_PER_BLK_OP: u16 = 3;

pub const PUBLISH_WRITE_ORDER: &[&str] =
    &["write_descriptors", "publish_available", "notify_queue"];

pub const SLOT_META_BYTES: u64 = 64;
pub const SLOT_META_PAYLOAD: u64 = 0;
pub const SLOT_META_HEADER: u64 = 8;
pub const SLOT_META_STATUS: u64 = 16;
pub const SLOT_META_PAYLOAD_LEN: u64 = 24;
pub const SLOT_META_FLAGS: u64 = 32;
pub const SLOT_META_EPOCH: u64 = 40;
pub const SLOT_META_WAITER: u64 = 48;
pub const SLOT_META_REPLY_STAGE: u64 = 56;
pub const SLOT_FLAG_DEVICE_WRITES: u64 = 1;
pub const SLOT_FLAG_INFLIGHT: u64 = 2;
pub const SLOT_FLAG_RESOLVED: u64 = 4;
pub const SLOT_FLAG_QUARANTINED: u64 = 8;

pub const SLOT_COMPLETION_BYTES: u64 = 32;

pub const AVAIL_F_NO_INTERRUPT: u16 = 1;

pub const EXPECTED_HEAD: u16 = 0;

pub fn packaging_bytes() -> u64 {
    SLOT_BOOK_BYTES + SLOT_META_BYTES + REQ_HEADER_SIZE + 8 + SLOT_COMPLETION_BYTES
}

pub fn meta_offset(ring_bytes: u64) -> u64 {
    ring_bytes + SLOT_BOOK_BYTES
}

pub fn completion_offset(ring_bytes: u64) -> u64 {
    meta_offset(ring_bytes) + SLOT_META_BYTES + REQ_HEADER_SIZE + 8
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompletionFault {
    UnknownId {
        id: u16,
    },
    DuplicateId {
        id: u16,
    },
    StaleId {
        id: u16,
        slot_epoch: u64,
        current_epoch: u64,
    },
    BadLength {
        reported: u32,
        capacity: u32,
    },
}

impl CompletionFault {
    pub fn abort_message(self) -> &'static str {
        match self {
            CompletionFault::UnknownId { .. } => {
                "driver fault: unknown used-ring id (03-hardware.md §4)"
            }
            CompletionFault::DuplicateId { .. } => {
                "driver fault: duplicate used-ring id (03-hardware.md §4)"
            }
            CompletionFault::StaleId { .. } => {
                "driver fault: stale used-ring id (03-hardware.md §4)"
            }
            CompletionFault::BadLength { .. } => {
                "driver fault: bad used-ring length (03-hardware.md §4)"
            }
        }
    }
}

pub fn validate_completion_id(
    id: u16,
    expected_head: u16,
    inflight: bool,
    slot_epoch: u64,
    current_epoch: u64,
) -> Result<(), CompletionFault> {
    if id != expected_head {
        return Err(CompletionFault::UnknownId { id });
    }
    if slot_epoch != current_epoch {
        return Err(CompletionFault::StaleId {
            id,
            slot_epoch,
            current_epoch,
        });
    }
    if !inflight {
        return Err(CompletionFault::DuplicateId { id });
    }
    Ok(())
}

pub fn validate_completion_length(
    used_len: u32,
    payload_capacity: u32,
    device_writes: bool,
) -> Result<u64, CompletionFault> {
    if used_len < 1 {
        return Err(CompletionFault::BadLength {
            reported: used_len,
            capacity: payload_capacity,
        });
    }
    let buffer_facing = used_len - 1;
    if device_writes && buffer_facing > payload_capacity {
        return Err(CompletionFault::BadLength {
            reported: used_len,
            capacity: payload_capacity,
        });
    }
    if !device_writes && buffer_facing != 0 {
        return Err(CompletionFault::BadLength {
            reported: used_len,
            capacity: payload_capacity,
        });
    }
    Ok(u64::from(buffer_facing))
}

pub const OUTCOME_COMPLETED: u64 = 0;
pub const OUTCOME_NOT_COMPLETED: u64 = 1;
pub const OUTCOME_UNKNOWN: u64 = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverOutcome {
    Completed,
    NotCompleted,
    Unknown,
    NotRecoverable,
}

impl RecoverOutcome {
    pub fn tag(self) -> Option<u64> {
        match self {
            RecoverOutcome::Completed => Some(OUTCOME_COMPLETED),
            RecoverOutcome::NotCompleted => Some(OUTCOME_NOT_COMPLETED),
            RecoverOutcome::Unknown => Some(OUTCOME_UNKNOWN),
            RecoverOutcome::NotRecoverable => None,
        }
    }

    pub fn not_recoverable_abort_message() -> &'static str {
        "driver fault: recover of a receipt that is neither in flight nor resolved \
         (03-hardware.md §5)"
    }
}

pub fn recover_outcome(
    slot_epoch: u64,
    current_epoch: u64,
    flags: u64,
    device_status: u8,
) -> RecoverOutcome {
    if slot_epoch != current_epoch {
        return RecoverOutcome::Unknown;
    }
    if flags & SLOT_FLAG_INFLIGHT != 0 {
        return RecoverOutcome::Unknown;
    }
    if flags & SLOT_FLAG_RESOLVED != 0 {
        if device_status == 0 {
            return RecoverOutcome::Completed;
        }
        return RecoverOutcome::NotCompleted;
    }
    RecoverOutcome::NotRecoverable
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReclaimGate {
    Reclaim,
    NotQuarantined,
    NotQuiesced,
}

impl ReclaimGate {
    pub fn abort_message(self) -> &'static str {
        match self {
            ReclaimGate::Reclaim => "",
            ReclaimGate::NotQuarantined => {
                "driver fault: reclaim of a slot that is not quarantined (03-hardware.md §9)"
            }
            ReclaimGate::NotQuiesced => {
                "driver fault: reclaim before quiescence (03-hardware.md §9: no reclaim \
                 precedes quiescence)"
            }
        }
    }
}

pub fn reclaim_gate(flags: u64, quiesced: u64, stamp: u64) -> ReclaimGate {
    if flags & SLOT_FLAG_QUARANTINED == 0 {
        return ReclaimGate::NotQuarantined;
    }
    if quiesced == stamp {
        return ReclaimGate::NotQuiesced;
    }
    ReclaimGate::Reclaim
}

pub fn control_bytes_needed(depth: u16) -> Option<u64> {
    let placed = place_ring(0, depth)?;
    Some(placed.bytes + packaging_bytes())
}

pub fn occupancy_bound(queue_depth: u16, descriptors_per_op: u16) -> u16 {
    if descriptors_per_op == 0 {
        return 0;
    }
    queue_depth / descriptors_per_op
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ring_geometry_matches_the_vmm_formulas() {
        let p = place_ring(0x4060_0000, 8).expect("8 is a power of two");
        assert_eq!(p.desc, 0x4060_0000);
        assert_eq!(p.desc + desc_bytes(8), 0x4060_0000 + 8 * 16);
        assert_eq!(avail_bytes(8), 4 + 2 * 8);
        assert_eq!(used_bytes(8), 4 + 8 * 8);
        assert_eq!(p.doorbell + DOORBELL_BYTES, 0x4060_0000 + p.bytes);
        assert!(p.avail >= p.desc + desc_bytes(8));
        assert!(p.used >= p.avail + avail_bytes(8));
        assert!(p.doorbell >= p.used + used_bytes(8));
    }

    #[test]
    fn a_non_power_of_two_depth_places_nothing() {
        assert!(place_ring(0, 7).is_none());
        assert!(place_ring(0, 0).is_none());
    }

    #[test]
    fn flush_is_offered_and_ring_reset_is_not() {
        assert_eq!(accepted_features(&["Flush"]).unwrap(), DEVICE_FEATURES);
        assert_eq!(accepted_features(&[]).unwrap(), F_VERSION_1);
        let err = accepted_features(&["RingReset"]).unwrap_err();
        assert!(err.contains("RingReset"), "{err}");
        assert!(err.contains("does not offer"), "{err}");
    }

    #[test]
    fn occupancy_bound_is_floor_depth_over_descriptors() {
        assert_eq!(occupancy_bound(128, 3), 42);
        assert_eq!(occupancy_bound(8, 3), 2);
        assert_eq!(occupancy_bound(7, 3), 2);
        assert_eq!(occupancy_bound(2, 3), 0);
        assert_eq!(occupancy_bound(3, 3), 1);
    }

    #[test]
    fn publish_write_order_is_descriptors_then_available_then_doorbell() {
        assert_eq!(
            PUBLISH_WRITE_ORDER,
            &["write_descriptors", "publish_available", "notify_queue"]
        );
    }

    #[test]
    fn completion_id_unknown_duplicate_and_stale_are_distinct_faults() {
        assert_eq!(
            validate_completion_id(7, EXPECTED_HEAD, true, 0, 0),
            Err(CompletionFault::UnknownId { id: 7 })
        );
        assert_eq!(
            validate_completion_id(EXPECTED_HEAD, EXPECTED_HEAD, false, 0, 0),
            Err(CompletionFault::DuplicateId { id: EXPECTED_HEAD })
        );
        assert_eq!(
            validate_completion_id(EXPECTED_HEAD, EXPECTED_HEAD, true, 1, 0),
            Err(CompletionFault::StaleId {
                id: EXPECTED_HEAD,
                slot_epoch: 1,
                current_epoch: 0,
            })
        );
        assert!(validate_completion_id(EXPECTED_HEAD, EXPECTED_HEAD, true, 0, 0).is_ok());
        assert_ne!(
            CompletionFault::UnknownId { id: 0 }.abort_message(),
            CompletionFault::DuplicateId { id: 0 }.abort_message()
        );
        assert_ne!(
            CompletionFault::DuplicateId { id: 0 }.abort_message(),
            CompletionFault::StaleId {
                id: 0,
                slot_epoch: 1,
                current_epoch: 0
            }
            .abort_message()
        );
    }

    #[test]
    fn outcome_tags_match_the_stdlib_enum_order() {
        let variants = crate::sema::stdlib_enums::variant_strs("CompletionOutcome")
            .expect("stdlib enums load")
            .expect("`CompletionOutcome` is a stdlib enum");
        assert_eq!(variants, &["Completed", "NotCompleted", "Unknown"]);
        assert_eq!(variants[OUTCOME_COMPLETED as usize], "Completed");
        assert_eq!(variants[OUTCOME_NOT_COMPLETED as usize], "NotCompleted");
        assert_eq!(variants[OUTCOME_UNKNOWN as usize], "Unknown");
    }

    #[test]
    fn recover_reports_unknown_across_a_reset_and_while_in_flight() {
        assert_eq!(
            recover_outcome(0, 1, SLOT_FLAG_RESOLVED, 0),
            RecoverOutcome::Unknown,
            "a stale epoch outranks a resolved slot"
        );
        assert_eq!(
            recover_outcome(0, 1, SLOT_FLAG_INFLIGHT, 0),
            RecoverOutcome::Unknown
        );
        assert_eq!(
            recover_outcome(3, 3, SLOT_FLAG_INFLIGHT, 0),
            RecoverOutcome::Unknown
        );
    }

    #[test]
    fn recover_reports_the_device_status_when_the_epoch_still_holds() {
        assert_eq!(
            recover_outcome(3, 3, SLOT_FLAG_RESOLVED, 0),
            RecoverOutcome::Completed
        );
        assert_eq!(
            recover_outcome(3, 3, SLOT_FLAG_RESOLVED, 1),
            RecoverOutcome::NotCompleted
        );
        assert_eq!(
            recover_outcome(3, 3, SLOT_FLAG_DEVICE_WRITES, 0),
            RecoverOutcome::NotRecoverable
        );
        assert_eq!(RecoverOutcome::NotRecoverable.tag(), None);
    }

    #[test]
    fn reclaim_is_refused_until_a_quiescence_separates_it_from_the_quarantine() {
        assert_eq!(
            reclaim_gate(SLOT_FLAG_QUARANTINED, 0, 0),
            ReclaimGate::NotQuiesced,
            "recover stamped the live count; nothing has quiesced since"
        );
        assert_eq!(
            reclaim_gate(SLOT_FLAG_QUARANTINED, 1, 0),
            ReclaimGate::Reclaim
        );
        assert_eq!(reclaim_gate(0, 9, 0), ReclaimGate::NotQuarantined);
        assert_eq!(
            reclaim_gate(SLOT_FLAG_RESOLVED, 9, 0),
            ReclaimGate::NotQuarantined
        );
        assert_ne!(
            ReclaimGate::NotQuarantined.abort_message(),
            ReclaimGate::NotQuiesced.abort_message()
        );
    }

    #[test]
    fn the_book_words_do_not_overlap_and_fit_the_book() {
        let mut offs = [
            SLOT_BOOK_LAST_USED,
            SLOT_BOOK_EPOCH,
            SLOT_BOOK_QUIESCED,
            SLOT_BOOK_QUARANTINE_STAMP,
        ];
        offs.sort_unstable();
        for w in offs.windows(2) {
            assert_eq!(w[1] - w[0], 8, "book words are one u64 apart: {offs:?}");
        }
        assert_eq!(offs[offs.len() - 1] + 8, SLOT_BOOK_BYTES);
    }

    #[test]
    fn completion_length_is_buffer_facing_and_rejects_oversize() {
        assert_eq!(validate_completion_length(513, 512, true), Ok(512));
        assert_eq!(
            validate_completion_length(1025, 512, true),
            Err(CompletionFault::BadLength {
                reported: 1025,
                capacity: 512
            })
        );
        assert_eq!(validate_completion_length(1, 512, false), Ok(0));
        assert_eq!(
            validate_completion_length(513, 512, false),
            Err(CompletionFault::BadLength {
                reported: 513,
                capacity: 512
            })
        );
    }
}
