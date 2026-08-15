//! Versioned local event vocabulary.

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum EventKind {
    ProjectedBoundEnter,
    ProjectedBoundExit,
    Silhouette,
    FeatureBoundary,
    RepeatBoundary,
    SmoothBandEnter,
    SmoothCenterTie,
    MaterialBoundary,
    NearClip,
    FarClip,
    FixedPointResetOnly,
    DepthSwap,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EventSide {
    Inactive,
    Active,
    OutsideValidity,
    InsideValidity,
    RepeatLeft,
    RepeatRight,
    SmoothLeft,
    SmoothRight,
    IdentityLeft,
    IdentityRight,
    MaterialLeft,
    MaterialRight,
    OutsideClip,
    InsideClip,
    ResetOnly,
    DepthAFront,
    DepthBFront,
    RecomputeRootSet,
    Ambiguous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EventSideMeaning {
    pub negative: EventSide,
    pub zero: EventSide,
    pub positive: EventSide,
}

impl EventSideMeaning {
    pub const fn crossing(negative: EventSide, positive: EventSide) -> Self {
        Self {
            negative,
            zero: EventSide::Ambiguous,
            positive,
        }
    }
}

/// Wire tag for the sealed event record's kind operand.
///
/// The frame program, the generated guest classifier, and every host decoder
/// read this one mapping; `program::event_kind` is a thin forward to it.
pub fn kind_wire_tag(kind: EventKind) -> u64 {
    match kind {
        EventKind::ProjectedBoundEnter => 1,
        EventKind::ProjectedBoundExit => 2,
        EventKind::Silhouette => 3,
        EventKind::FeatureBoundary => 4,
        EventKind::RepeatBoundary => 5,
        EventKind::SmoothBandEnter => 6,
        EventKind::SmoothCenterTie => 7,
        EventKind::MaterialBoundary => 8,
        EventKind::NearClip => 9,
        EventKind::FarClip => 10,
        EventKind::FixedPointResetOnly => 11,
        EventKind::DepthSwap => 12,
    }
}

/// Every event kind, so generators can enumerate the vocabulary instead of
/// restating it.
pub const ALL_EVENT_KINDS: &[EventKind] = &[
    EventKind::ProjectedBoundEnter,
    EventKind::ProjectedBoundExit,
    EventKind::Silhouette,
    EventKind::FeatureBoundary,
    EventKind::RepeatBoundary,
    EventKind::SmoothBandEnter,
    EventKind::SmoothCenterTie,
    EventKind::MaterialBoundary,
    EventKind::NearClip,
    EventKind::FarClip,
    EventKind::FixedPointResetOnly,
    EventKind::DepthSwap,
];

/// The sealed representation vocabulary, as plain tags without the payloads
/// carried by `events::EventRepresentation`. Splitting the tag out is what
/// lets the classification below live beside the kinds it pairs with.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum RepresentationTag {
    LinearLeadingCoefficient,
    QuadraticDiscriminant,
    SparsePredicate,
    DeformationTaylorPredicate,
    TorusLocalOracle,
    SmoothBandTaylorPredicate,
    SmoothTieTaylorPredicate,
    MaterialDifferenceTaylorPredicate,
    RepeatAffineBoundary,
    ClipQ,
    ProjectedBoundary,
    FixedPointReset,
    DirectDepthCrossProduct,
    TaylorDepthDifference,
}

pub const ALL_REPRESENTATION_TAGS: &[RepresentationTag] = &[
    RepresentationTag::LinearLeadingCoefficient,
    RepresentationTag::QuadraticDiscriminant,
    RepresentationTag::SparsePredicate,
    RepresentationTag::DeformationTaylorPredicate,
    RepresentationTag::TorusLocalOracle,
    RepresentationTag::SmoothBandTaylorPredicate,
    RepresentationTag::SmoothTieTaylorPredicate,
    RepresentationTag::MaterialDifferenceTaylorPredicate,
    RepresentationTag::RepeatAffineBoundary,
    RepresentationTag::ClipQ,
    RepresentationTag::ProjectedBoundary,
    RepresentationTag::FixedPointReset,
    RepresentationTag::DirectDepthCrossProduct,
    RepresentationTag::TaylorDepthDifference,
];

pub fn representation_wire_tag(tag: RepresentationTag) -> u16 {
    match tag {
        RepresentationTag::LinearLeadingCoefficient => 1,
        RepresentationTag::QuadraticDiscriminant => 2,
        RepresentationTag::SparsePredicate => 3,
        RepresentationTag::DeformationTaylorPredicate => 4,
        RepresentationTag::TorusLocalOracle => 5,
        RepresentationTag::SmoothBandTaylorPredicate => 6,
        RepresentationTag::SmoothTieTaylorPredicate => 7,
        RepresentationTag::MaterialDifferenceTaylorPredicate => 8,
        RepresentationTag::RepeatAffineBoundary => 9,
        RepresentationTag::ClipQ => 10,
        RepresentationTag::ProjectedBoundary => 11,
        RepresentationTag::FixedPointReset => 12,
        RepresentationTag::DirectDepthCrossProduct => 13,
        RepresentationTag::TaylorDepthDifference => 14,
    }
}

/// Bits of the guest-facing event class word.
///
/// The analytic coverage tiers in `stdlib/core/render.wr` used to restate the
/// kind and representation numbers inline — `record[2] == 1 and kind[1] == 3`
/// and a hand-written `kind in {1,2,3,4,5,9,10}` occupancy set. A new
/// occupancy-bearing kind added in Rust without editing those literals would
/// have silently unsoundened the "every occupancy-bearing event provably
/// misses, so the centre ray names the whole pixel" rule: the guest would not
/// have known the new boundary existed. The classification now lives here,
/// the generated `__wrela_pixels_p7_event_class` is emitted from it, and the
/// matches below are exhaustive, so adding a variant fails to compile.
pub mod event_class {
    /// This kind can bound where a surface is visible, so a pixel it covers
    /// has no uniform-occupancy conclusion until it is reduced to a curve.
    pub const OCCUPANCY: u64 = 1;
    /// Reduces to a pure `uv` curve whose zero set is the boundary.
    pub const CURVE: u64 = 2;
    /// A curve with no sign convention for "occupied": the side has to be
    /// resolved from an occupancy sample rather than assumed.
    pub const ORIENTED: u64 = 4;
    /// The curve is the owning feature's ray polynomial levelled at a sealed
    /// clip `q` rather than a curve of its own.
    pub const CLIP: u64 = 8;
    /// Sealed integer pixel edges: they can move a conservative candidate
    /// span but cross no pixel interior and carry zero coverage measure.
    pub const PROJECTED_BOUNDARY: u64 = 16;
    /// A feature-validity predicate, integrable through its eliminant.
    pub const PREDICATE: u64 = 32;
    /// A bounded-displacement silhouette: no closed form, miss-testable only.
    pub const DEFORMATION: u64 = 64;
    /// A polynomial silhouette the projected-union tier can integrate.
    pub const UNION_POLYNOMIAL: u64 = 128;
    /// A linear leading coefficient, which can be identically zero over a
    /// cell and therefore bound a zero-measure region.
    pub const LINEAR_LEADING: u64 = 256;
    /// The event marks a domain re-parameterization rather than a boundary of
    /// its own: a finite repeat fold splits the domain between instances, but
    /// occupancy on either side is still bounded by those instances' own
    /// silhouettes. A tier that already tracks every silhouette covering a
    /// pixel may therefore ignore it — but only such a tier, which is why
    /// this is a separate bit from `OCCUPANCY` rather than a removal of it.
    pub const REPARAMETERIZATION: u64 = 512;
    /// The event can make a pixel need local arrangement — that is, it is a
    /// sealed boundary of *something* the renderer displays, whether that is
    /// occupancy, material, blend branch or depth order. Only a fixed-point
    /// reset marker is excluded, because it partitions the numeric domain
    /// without any geometry attached. `structural_corridor_pixel` used to
    /// restate this as `(kind >= 1 and kind <= 10) or kind == 12`, a third
    /// independent copy of the vocabulary.
    pub const LOCAL_ARRANGEMENT: u64 = 1024;
    /// A smooth-combiner band boundary. It bounds occupancy (the composite
    /// surface deviates from the members inside the band — a smooth union
    /// bulges beyond the members' union), but it is not a curve any analytic
    /// tier can track as a member. A coverage tier that reasons from tracked
    /// member curves must not decline its whole conservative span; it must
    /// instead stop trusting member-structural occupancy conclusions and
    /// corroborate them against composite occupancy itself.
    pub const SMOOTH_BAND: u64 = 2048;
}

/// Can this kind bound where a surface is visible?
///
/// `MaterialBoundary` and `SmoothCenterTie` change which material or blend
/// branch is displayed, not whether the ray hits anything, and are charged
/// by their own tiers. `DepthSwap` likewise changes which of two surfaces is
/// in front without changing occupancy.
///
/// `SmoothBandEnter` DOES bound occupancy. Inside the band a smooth
/// combiner's composite surface deviates from the member surfaces — a smooth
/// union bulges beyond the members' union (the bridge at a blend neck) — so
/// the composite silhouette inside the band is no member's silhouette, and
/// any tier that reasons "the tracked member curves are the complete set of
/// visibility boundaries in this pixel" must treat the band boundary as one
/// of those boundaries. Classifying it as non-occupancy let the analytic
/// coverage tiers integrate member silhouettes alone across the band:
/// measured on `check-pixels-smooth-csg`, the four neck-saddle pixels
/// displayed the member-union coverage 218 where the blended surface truly
/// covers 237.
pub fn kind_bounds_occupancy(kind: EventKind) -> bool {
    match kind {
        EventKind::ProjectedBoundEnter
        | EventKind::ProjectedBoundExit
        | EventKind::Silhouette
        | EventKind::SmoothBandEnter
        | EventKind::FeatureBoundary
        | EventKind::RepeatBoundary
        | EventKind::NearClip
        | EventKind::FarClip => true,
        EventKind::SmoothCenterTie
        | EventKind::MaterialBoundary
        | EventKind::FixedPointResetOnly
        | EventKind::DepthSwap => false,
    }
}

/// The complete class word for one sealed `(representation, kind)` pairing.
pub fn event_class(representation: RepresentationTag, kind: EventKind) -> u64 {
    use EventKind as K;
    use RepresentationTag as R;
    let mut class = 0;
    if kind_bounds_occupancy(kind) {
        class |= event_class::OCCUPANCY;
    }
    if kind != EventKind::FixedPointResetOnly {
        class |= event_class::LOCAL_ARRANGEMENT;
    }
    class |= match (representation, kind) {
        (R::LinearLeadingCoefficient, K::Silhouette) => {
            event_class::CURVE | event_class::ORIENTED | event_class::LINEAR_LEADING
        }
        (R::QuadraticDiscriminant, K::Silhouette) => {
            event_class::CURVE | event_class::UNION_POLYNOMIAL
        }
        (R::TorusLocalOracle, K::Silhouette) => event_class::UNION_POLYNOMIAL,
        (R::SparsePredicate, K::FeatureBoundary) => event_class::PREDICATE,
        (R::DeformationTaylorPredicate, K::Silhouette) => event_class::DEFORMATION,
        (R::ClipQ, K::NearClip | K::FarClip) => {
            event_class::CURVE | event_class::ORIENTED | event_class::CLIP
        }
        (R::ProjectedBoundary, K::ProjectedBoundEnter | K::ProjectedBoundExit) => {
            event_class::PROJECTED_BOUNDARY
        }
        (R::RepeatAffineBoundary, K::RepeatBoundary) => event_class::REPARAMETERIZATION,
        (R::SmoothBandTaylorPredicate, K::SmoothBandEnter) => event_class::SMOOTH_BAND,
        _ => 0,
    };
    class
}

#[cfg(test)]
mod taxonomy_tests {
    use super::*;

    #[test]
    fn wire_tags_are_unique_and_dense() {
        let kinds: Vec<u64> = ALL_EVENT_KINDS.iter().copied().map(kind_wire_tag).collect();
        let mut sorted = kinds.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), kinds.len(), "event kind wire tags collide");
        assert_eq!(sorted, (1..=kinds.len() as u64).collect::<Vec<_>>());

        let tags: Vec<u16> = ALL_REPRESENTATION_TAGS
            .iter()
            .copied()
            .map(representation_wire_tag)
            .collect();
        let mut sorted = tags.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(sorted.len(), tags.len(), "representation wire tags collide");
        assert_eq!(sorted, (1..=tags.len() as u16).collect::<Vec<_>>());
    }

    #[test]
    fn a_class_carrying_curve_bits_always_bounds_occupancy() {
        // The coverage tiers reason "this pixel is covered by an
        // occupancy-bearing event, but it reduces to a curve that provably
        // misses". A representation that reduces to a curve while its kind is
        // not occupancy-bearing would let that reasoning skip a boundary.
        for representation in ALL_REPRESENTATION_TAGS.iter().copied() {
            for kind in ALL_EVENT_KINDS.iter().copied() {
                let class = event_class(representation, kind);
                let geometric = class
                    & (event_class::CURVE
                        | event_class::PREDICATE
                        | event_class::DEFORMATION
                        | event_class::UNION_POLYNOMIAL);
                if geometric != 0 {
                    assert!(
                        class & event_class::OCCUPANCY != 0,
                        "{representation:?}/{kind:?} reduces to a boundary curve but is \
                         not classified as occupancy-bearing"
                    );
                }
            }
        }
    }

    #[test]
    fn oriented_curves_are_exactly_the_ones_without_a_sign_convention() {
        // A discriminant's positive side is the occupied one by construction;
        // a leading coefficient and a clip level set carry no such rule, so
        // they must be flagged for occupancy-sampled side resolution.
        for representation in ALL_REPRESENTATION_TAGS.iter().copied() {
            for kind in ALL_EVENT_KINDS.iter().copied() {
                let class = event_class(representation, kind);
                if class & event_class::ORIENTED != 0 {
                    assert!(
                        class & event_class::CURVE != 0,
                        "{representation:?}/{kind:?} is oriented but not a curve"
                    );
                    assert!(
                        matches!(
                            representation,
                            RepresentationTag::LinearLeadingCoefficient | RepresentationTag::ClipQ
                        ),
                        "{representation:?} gained an unconvention-ed side"
                    );
                }
            }
        }
    }
}
