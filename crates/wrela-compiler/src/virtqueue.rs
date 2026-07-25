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
//! Numbers match `wrela_vmm::devices` byte-for-byte (`DESC_SIZE`,
//! `avail_bytes` / `used_bytes` formulas, `DEVICE_FEATURES`). The VMM is
//! the other half of the same contract; inventing a different layout here
//! would make `BlkDevice::new` refuse the report at boot.

/// Descriptor table entry size (`struct virtq_desc`: addr/len/flags/next).
pub const DESC_SIZE: u64 = 16;

/// Doorbell word (06-machine.md §5): one guest-writable `u64`.
pub const DOORBELL_BYTES: u64 = 8;

/// `VIRTIO_BLK_F_FLUSH` (feature bit 9).
pub const F_BLK_FLUSH: u64 = 1 << 9;
/// `VIRTIO_F_VERSION_1` (feature bit 32) — mandatory on this machine.
pub const F_VERSION_1: u64 = 1 << 32;

/// Everything the VMM's virtio-blk model offers (`devices::DEVICE_FEATURES`).
pub const DEVICE_FEATURES: u64 = F_VERSION_1 | F_BLK_FLUSH;

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

/// Descriptor-table byte count for a `depth`-deep queue.
pub fn desc_bytes(depth: u16) -> u64 {
    depth as u64 * DESC_SIZE
}

/// Available-ring byte count: `flags: u16, idx: u16, ring: [u16; depth]`.
/// (No event-idx: this machine never offers `VIRTIO_RING_F_EVENT_IDX`.)
pub fn avail_bytes(depth: u16) -> u64 {
    4 + 2 * depth as u64
}

/// Used-ring byte count: `flags: u16, idx: u16, ring: [(id,len); depth]`.
pub fn used_bytes(depth: u16) -> u64 {
    4 + 8 * depth as u64
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
        // is plans/M7.md item H2, and `DEVICE_FEATURES` does not include it.
        "RingReset" | "RING_RESET" => None,
        _ => None,
    }
}

/// Build-time negotiation (plans/M7.md decision 12): the image's declared
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
                     VIRTIO_BLK_F_FLUSH; `{name}` is not in that set — plans/M7.md decision 12: \
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
/// decide a bespoke ring later. E1 can honestly derive this constant;
/// occupancy bound / exits-per-op need E2/E4's proof and workload.
pub const DESCRIPTORS_PER_BLK_OP: u16 = 3;

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
}
