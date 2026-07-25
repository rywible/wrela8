//! Split-ring geometry and virtio-blk feature bits (plans/M7.md item E1).
//!
//! **One derivation, many readers.** 03-hardware.md §3 / plans/M7.md
//! decision 5: every byte a device can reach is pool backing. The ring —
//! descriptor table, available ring, used ring, and the doorbell word —
//! lives inside a declared DMA pool, so its addresses are build outputs.
//! `sema` (does the pool hold the ring?), `layout` (places and reports
//! them), the report verifier (`verify_ring_windows`), and the VMM's own
//! `devices::BlkQueueConfig` size helpers all read **these** constants and
//! functions. A second, local derivation that could disagree about which
//! bytes the device reaches is deliberately forbidden.
//!
//! The numeric contract itself lives in `wrela_machine::virtio` (plans/M8.md
//! item H attack 7) — this module re-exports it so every existing
//! `crate::virtqueue::DESC_SIZE` site keeps working, and adds the
//! compiler-only surface on top (`place_ring`, feature-name mapping,
//! per-slot packaging).

pub use wrela_machine::virtio::{
    DESC_F_NEXT, DESC_F_WRITE, DESC_SIZE, DEVICE_FEATURES, DOORBELL_BYTES, F_BLK_FLUSH,
    F_VERSION_1, REQ_HEADER_SIZE, REQ_STATUS_SIZE, SLOT_BOOK_BYTES, SLOT_BOOK_EPOCH,
    SLOT_BOOK_LAST_USED, SLOT_BOOK_QUARANTINE_STAMP, SLOT_BOOK_QUIESCED, avail_bytes, desc_bytes,
    used_bytes,
};

/// One queue's ring regions, contiguous inside a DMA pool starting at
/// `pool_base`. Layout (decision: pack tightly, 8-byte-align each region
/// so every address the VMM validates is naturally aligned):
///
/// ```text
///   [desc table | avail ring | used ring | doorbell]
/// ```
///
/// Depth must be a nonzero power of two (VIRTIO 1.2 §2.6) — callers that
/// have already rejected a bad depth never reach `place_ring`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RingPlacement {
    pub depth: u16,
    pub desc: u64,
    pub avail: u64,
    pub used: u64,
    pub doorbell: u64,
    /// Total bytes consumed from the pool (desc..doorbell+8), including
    /// alignment padding between regions.
    pub bytes: u64,
}

/// Round `addr` up to a multiple of `align` (power of two).
fn align_up(addr: u64, align: u64) -> u64 {
    (addr + align - 1) & !(align - 1)
}

/// Place one split ring at the start of a DMA pool. Returns `None` if
/// `depth` is not a nonzero power of two (the caller turns that into a
/// named diagnostic — this fn stays a pure geometry fact).
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

/// Map a source-level feature *variant name* (e.g. `Flush` from
/// `Feature.Flush` / `VirtioFeature.Flush`) onto the bit this machine
/// knows. Unknown names are the caller's rejection — never silently
/// dropped. `VERSION_1` / `Version1` are accepted so an image can name
/// the mandatory bit explicitly; it is always OR'd in by
/// `accepted_features` either way.
pub fn feature_bit(variant: &str) -> Option<u64> {
    match variant {
        "Flush" | "BLK_FLUSH" | "VIRTIO_BLK_F_FLUSH" => Some(F_BLK_FLUSH),
        "Version1" | "VERSION_1" | "VIRTIO_F_VERSION_1" => Some(F_VERSION_1),
        // Named so a required `RingReset` fails the build with a clear
        // "not offered" rather than "unknown feature" — per-queue reset
        // is plans/M7.md item H2b, and `DEVICE_FEATURES` does not include it.
        "RingReset" | "RING_RESET" => None,
        _ => None,
    }
}

/// Build-time negotiation (plans/M7.md decision 14): the image's declared
/// required feature *variant names* against `DEVICE_FEATURES`. Always
/// includes `F_VERSION_1`. Returns the accepted mask, or names the bits
/// that were refused.
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

/// Descriptors a single virtio-blk operation needs (header + data +
/// status). Decision 2c: the report should carry the numbers that would
/// decide a bespoke ring later.
pub const DESCRIPTORS_PER_BLK_OP: u16 = 3;

/// `VirtQueue.publish`'s sealed write order (03-hardware.md §3:
/// "payload writes before publication, publication before doorbell";
/// the sealed ops named there are `write_descriptors`,
/// `publish_available`, `notify_queue`). plans/M7.md item E3 / decision
/// 16: this slice is what `Inst::QueuePublish` carries into the mwir
/// dump — the oracle that the order lives in `publish`, not in
/// `prepare_block` (decision 15).
pub const PUBLISH_WRITE_ORDER: &[&str] =
    &["write_descriptors", "publish_available", "notify_queue"];

/// Bytes of per-queue bookkeeping that sit in the control pool immediately
/// after the ring (plans/M7.md item E4 / decisions 20–22; H2b / decision 23
/// adds the live reset epoch; M8 item F grew the quiesce/quarantine words).
/// Single-flight for revision 0.1: one `last_used` cursor, one live
/// `current_epoch`, one meta record, one header slot, one status byte.
///
/// ```text
///   [ring … | last_used 8 | current_epoch 8 | quiesced 8 | quarantine_stamp 8
///          | meta 64 | header 16 | status 8-pad]
/// ```
///
/// Decision 22: `QueueOp` / `Receipt` is the absolute address of the meta
/// record (not the descriptor head). Drain owns `last_used`; await parks
/// the waiter turn-area and reply-stage addresses in the meta; drain
/// writes `IoCompletion` into that stage and sets `resume_ready`.
/// Decision 23: `current_epoch` starts at 0; `RunningDevice.reset` bumps
/// it; `prepare_block` stamps it into `SLOT_META_EPOCH`; drain rejects a
/// mismatch as `CompletionFault::StaleId`.
///
/// The `SLOT_BOOK_*` offsets themselves are `wrela_machine::virtio` (item H
/// attack 7); the meta / flag / packaging surface below is compiler-only.
pub const SLOT_META_BYTES: u64 = 64;
pub const SLOT_META_PAYLOAD: u64 = 0;
pub const SLOT_META_HEADER: u64 = 8;
pub const SLOT_META_STATUS: u64 = 16;
pub const SLOT_META_PAYLOAD_LEN: u64 = 24;
pub const SLOT_META_FLAGS: u64 = 32;
/// Stamped `current_epoch` at prepare time (03 §4's reset epoch half of
/// "ID, slot generation, and reset epoch"). Slot-reuse generation for
/// multi-flight is deferred with the free-list (decision 20); single-flight
/// uses `SLOT_FLAG_INFLIGHT` for the duplicate half.
pub const SLOT_META_EPOCH: u64 = 40;
pub const SLOT_META_WAITER: u64 = 48;
pub const SLOT_META_REPLY_STAGE: u64 = 56;
/// `flags` bit 0: device writes the payload (`T_IN` / `device_writes_payload=true`).
pub const SLOT_FLAG_DEVICE_WRITES: u64 = 1;
/// `flags` bit 1: publish claimed this slot; drain clears it on resolve.
pub const SLOT_FLAG_INFLIGHT: u64 = 2;
/// `flags` bit 2: drain (or reject) wrote an `IoCompletion` into the
/// completion stash; `await receipt` may take it without parking.
pub const SLOT_FLAG_RESOLVED: u64 = 4;
/// `flags` bit 3: `recover` retired this slot and **quarantined** its
/// payload (03-hardware.md §9: "affected regions and DMA slots are
/// quarantined"). The payload word in the meta is still the abandoned
/// buffer's address; nothing may hand it back until a quiescence event
/// separates the quarantine from the reclaim. `reclaim` clears it.
pub const SLOT_FLAG_QUARANTINED: u64 = 8;

/// Stash for a resolved `IoCompletion[P]` (payload + status + written_len
/// = 32 bytes under the frame ABI). Sits after the status pad so drain can
/// leave a completion for an `await` that has not yet registered a waiter.
pub const SLOT_COMPLETION_BYTES: u64 = 32;

/// `avail.flags` bit: `VIRTQ_AVAIL_F_NO_INTERRUPT` (VIRTIO 1.2 §2.6).
pub const AVAIL_F_NO_INTERRUPT: u16 = 1;

/// Total bytes after the ring this queue's packaging consumes.
pub const EXPECTED_HEAD: u16 = 0; // single-flight descriptor head

pub fn packaging_bytes() -> u64 {
    SLOT_BOOK_BYTES
        + SLOT_META_BYTES
        + REQ_HEADER_SIZE
        + 8 // status padded to 8
        + SLOT_COMPLETION_BYTES
}

/// Byte offset of the meta record from the pool base, given `place_ring(..).bytes`.
pub fn meta_offset(ring_bytes: u64) -> u64 {
    ring_bytes + SLOT_BOOK_BYTES
}

/// Byte offset of the IoCompletion stash from the pool base.
pub fn completion_offset(ring_bytes: u64) -> u64 {
    meta_offset(ring_bytes) + SLOT_META_BYTES + REQ_HEADER_SIZE + 8
}

/// 03-hardware.md §4: stale / duplicate / unknown IDs are driver faults.
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
    /// Guest abort message — each fault is its own diagnosable name.
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

/// Validate a device-reported used-ring id against the single-flight slot.
/// `current_epoch` is the queue's live reset epoch (plans/M7.md item H2b /
/// decision 23); a stamped `slot_epoch` from a prior epoch is `StaleId`.
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

/// Buffer-facing length from `used.len` (which always includes the status
/// byte). Device-writable payloads report `used.len - 1`; host-writable
/// (OUT) completions report `0` when the device wrote only status.
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
        // OUT: device should have written only the status byte.
        return Err(CompletionFault::BadLength {
            reported: used_len,
            capacity: payload_capacity,
        });
    }
    Ok(u64::from(buffer_facing))
}

/// 03-hardware.md §9's `CompletionOutcome`, as the tag a recovered receipt
/// yields. The numbers are **positions in
/// `sema::prelude::builtin_enum_variants("CompletionOutcome")`** — the same
/// order `lower::variant_index` compares a `case .Unknown:` arm against —
/// and `outcome_tags_match_the_prelude_enum_order` below is the oracle that
/// keeps the two from drifting apart. There is no second enum here on
/// purpose: this is the *encoding* of the source enum, not a copy of it.
pub const OUTCOME_COMPLETED: u64 = 0;
pub const OUTCOME_NOT_COMPLETED: u64 = 1;
pub const OUTCOME_UNKNOWN: u64 = 2;

/// What `VirtQueue.recover` reports for one slot, or that the slot is in no
/// recoverable state at all (a driver fault, like every other §4/§5
/// bookkeeping violation).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverOutcome {
    Completed,
    NotCompleted,
    Unknown,
    /// Neither published nor resolved: nothing to recover. Aborts by name.
    NotRecoverable,
}

impl RecoverOutcome {
    /// The tag `recover` stores, or `None` for the fault case.
    pub fn tag(self) -> Option<u64> {
        match self {
            RecoverOutcome::Completed => Some(OUTCOME_COMPLETED),
            RecoverOutcome::NotCompleted => Some(OUTCOME_NOT_COMPLETED),
            RecoverOutcome::Unknown => Some(OUTCOME_UNKNOWN),
            RecoverOutcome::NotRecoverable => None,
        }
    }

    /// Guest abort message for the fault case (03-hardware.md §5: a receipt
    /// resolves exactly once, and dropping one is illegal in every state —
    /// so a receipt that is neither in flight nor resolved cannot exist).
    pub fn not_recoverable_abort_message() -> &'static str {
        "driver fault: recover of a receipt that is neither in flight nor resolved \
         (03-hardware.md §5)"
    }
}

/// The whole of `VirtQueue.recover`'s decision, as one pure function
/// (plans/M8.md item G / decision 17). `codegen::emit_queue_recover` emits
/// exactly this ladder; the unit tests below are its oracle.
///
/// - **`Unknown`** — the stamped epoch is not the queue's live epoch
///   (03-hardware.md §9: "After a reset, a write may have happened"), or the
///   slot is still `INFLIGHT` (the device has not returned the descriptor
///   and never will now that the driver is abandoning it). Both are the
///   same fact: the operation was published and its effect is not
///   attributable.
/// - **`Completed` / `NotCompleted`** — the device *did* return the
///   descriptor under the live epoch (`RESOLVED`), so the outcome is known
///   and the virtio-blk status byte says which: `0` (`VIRTIO_BLK_S_OK`) is
///   `Completed`, anything else (`S_IOERR` / `S_UNSUPP`) is `NotCompleted`.
///
/// The epoch test comes **first**, so a receipt that a reset invalidated
/// reports `Unknown` even when the pre-reset drain had already resolved it —
/// the same ordering `validate_completion_id` uses to make a stale
/// completion `StaleId` rather than `DuplicateId`.
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

/// What `VirtQueue.reclaim` is allowed to do with the quarantined slot
/// (plans/M8.md item F / decision 37). `codegen::emit_queue_reclaim`
/// emits this ladder, compare for compare.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReclaimGate {
    /// Quiescence has been established since the quarantine: hand the
    /// payload handle back.
    Reclaim,
    /// Nothing is quarantined on this queue — `recover` never ran, or a
    /// previous `reclaim` already took the payload.
    NotQuarantined,
    /// 03-hardware.md §9's rule, refused: the device has not been
    /// quiesced since this slot was quarantined, so it may still write
    /// the buffer.
    NotQuiesced,
}

impl ReclaimGate {
    pub fn abort_message(self) -> &'static str {
        match self {
            // Never aborts; present so the ladder has one message table.
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

/// The whole of `VirtQueue.reclaim`'s gate, as one pure function.
///
/// `quiesced` is the **host-written** quiesce count for this queue
/// (`SLOT_BOOK_QUIESCED`; only `mmio::QUIESCE_MMIO_ADDR`'s handler in the
/// VMM ever increments it) and `stamp` is the value `recover` copied into
/// `SLOT_BOOK_QUARANTINE_STAMP` at quarantine time. They are equal exactly
/// when no device quiescence has happened in between — which is the case
/// 03-hardware.md §9 forbids reclaiming in.
pub fn reclaim_gate(flags: u64, quiesced: u64, stamp: u64) -> ReclaimGate {
    if flags & SLOT_FLAG_QUARANTINED == 0 {
        return ReclaimGate::NotQuarantined;
    }
    if quiesced == stamp {
        return ReclaimGate::NotQuiesced;
    }
    ReclaimGate::Reclaim
}

/// Control-pool bytes a `depth`-deep queue needs: ring + packaging.
pub fn control_bytes_needed(depth: u16) -> Option<u64> {
    let placed = place_ring(0, depth)?;
    Some(placed.bytes + packaging_bytes())
}

/// Maximum concurrent direct operations a queue can hold
/// (03-hardware.md §4: "three direct descriptors in a 128-deep queue
/// means at most 42 in flight" — `floor(128/3) = 42`). Plans/M7.md item
/// E2 emits this as `BlkAccounting occupancy_bound=`.
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
        // Regions do not overlap.
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
        // 03-hardware.md §4's own worked numbers, hand-computed.
        assert_eq!(occupancy_bound(128, 3), 42);
        assert_eq!(occupancy_bound(8, 3), 2);
        assert_eq!(occupancy_bound(7, 3), 2);
        assert_eq!(occupancy_bound(2, 3), 0);
        assert_eq!(occupancy_bound(3, 3), 1);
    }

    #[test]
    fn publish_write_order_is_descriptors_then_available_then_doorbell() {
        // 03-hardware.md §3's normative order, pinned for Inst::QueuePublish.
        assert_eq!(
            PUBLISH_WRITE_ORDER,
            &["write_descriptors", "publish_available", "notify_queue"]
        );
    }

    #[test]
    fn completion_id_unknown_duplicate_and_stale_are_distinct_faults() {
        // 03-hardware.md §4: each of the three is its own driver fault.
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
    fn outcome_tags_match_the_prelude_enum_order() {
        // The one place the encoding and the source enum meet: a tag is a
        // position in `builtin_enum_variants`, never an independent number.
        let variants = crate::sema::prelude::builtin_enum_variants("CompletionOutcome")
            .expect("`CompletionOutcome` is a prelude enum");
        assert_eq!(variants, &["Completed", "NotCompleted", "Unknown"]);
        assert_eq!(variants[OUTCOME_COMPLETED as usize], "Completed");
        assert_eq!(variants[OUTCOME_NOT_COMPLETED as usize], "NotCompleted");
        assert_eq!(variants[OUTCOME_UNKNOWN as usize], "Unknown");
    }

    #[test]
    fn recover_reports_unknown_across_a_reset_and_while_in_flight() {
        // 03-hardware.md §9: after a reset a write may have happened.
        assert_eq!(
            recover_outcome(0, 1, SLOT_FLAG_RESOLVED, 0),
            RecoverOutcome::Unknown,
            "a stale epoch outranks a resolved slot"
        );
        assert_eq!(
            recover_outcome(0, 1, SLOT_FLAG_INFLIGHT, 0),
            RecoverOutcome::Unknown
        );
        // Live epoch, still in flight: equally unattributable.
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
        // Neither in flight nor resolved: nothing to recover.
        assert_eq!(
            recover_outcome(3, 3, SLOT_FLAG_DEVICE_WRITES, 0),
            RecoverOutcome::NotRecoverable
        );
        assert_eq!(RecoverOutcome::NotRecoverable.tag(), None);
    }

    #[test]
    fn reclaim_is_refused_until_a_quiescence_separates_it_from_the_quarantine() {
        // 03-hardware.md §9: "only then is memory reclaimed".
        assert_eq!(
            reclaim_gate(SLOT_FLAG_QUARANTINED, 0, 0),
            ReclaimGate::NotQuiesced,
            "recover stamped the live count; nothing has quiesced since"
        );
        assert_eq!(
            reclaim_gate(SLOT_FLAG_QUARANTINED, 1, 0),
            ReclaimGate::Reclaim
        );
        // A slot nothing quarantined is never reclaimable, whatever the
        // counts say — a second `reclaim` clears the flag and lands here.
        assert_eq!(reclaim_gate(0, 9, 0), ReclaimGate::NotQuarantined);
        assert_eq!(
            reclaim_gate(SLOT_FLAG_RESOLVED, 9, 0),
            ReclaimGate::NotQuarantined
        );
        // Each refusal is its own diagnosable name.
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
        // Read of 512: used.len = 513 (payload + status).
        assert_eq!(validate_completion_length(513, 512, true), Ok(512));
        assert_eq!(
            validate_completion_length(1025, 512, true),
            Err(CompletionFault::BadLength {
                reported: 1025,
                capacity: 512
            })
        );
        // Write: device wrote only status.
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
