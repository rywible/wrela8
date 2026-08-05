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
