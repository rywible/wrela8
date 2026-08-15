//! Versioned, decision-inert certificate telemetry.

pub const CERTIFICATE_TELEMETRY_VERSION: u16 = 2;
pub const RUN_LENGTH_BINS: usize = 8;
pub const ROOT_METHOD_COUNT: usize = 3;
pub const COMPOSITION_SHAPE_COUNT: usize = 4;
pub const EXPIRY_CAUSE_COUNT: usize = 8;
pub const MARGIN_OWNER_COUNT: usize = 8;
pub const DENSITY_BINS: usize = 8;
pub const SUBDIVISION_BINS: usize = 16;
pub const REBUILD_REASON_COUNT: usize = 9;
pub const FAILURE_CAUSE_COUNT: usize = 8;
pub const RASTER_CONFORMANCE_COUNTERS: usize = 8;
pub const CERTIFICATE_TELEMETRY_COUNTERS_V2: u64 = (RUN_LENGTH_BINS
    + ROOT_METHOD_COUNT
    + COMPOSITION_SHAPE_COUNT
    + EXPIRY_CAUSE_COUNT
    + MARGIN_OWNER_COUNT
    + 6 * DENSITY_BINS
    + 2 * SUBDIVISION_BINS
    + 2 * REBUILD_REASON_COUNT
    + FAILURE_CAUSE_COUNT
    + 5
    + RASTER_CONFORMANCE_COUNTERS) as u64;

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RootMethod {
    BernsteinFaces = 0,
    MonotoneTube = 1,
    Krawczyk = 2,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CompositionShape {
    General = 0,
    Plane = 1,
    Sphere = 2,
    Torus = 3,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExpiryCause {
    DomainEnd = 0,
    Residual = 1,
    Validity = 2,
    Order = 3,
    Branch = 4,
    Numeric = 5,
    FixedQ = 6,
    Event = 7,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MarginOwner {
    Root = 0,
    Feature = 1,
    Order = 2,
    Csg = 3,
    Branch = 4,
    Numeric = 5,
    FixedQ = 6,
    Event = 7,
}

#[repr(u8)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RebuildReason {
    None = 0,
    XSplit = 1,
    QSplit = 2,
    FeatureSplit = 3,
    BranchSplit = 4,
    EventArrangement = 5,
    PixelCell = 6,
    SubpixelIntegration = 7,
    Exhausted = 8,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CertificateTelemetry {
    pub version: u16,
    pub run_lengths: [u64; RUN_LENGTH_BINS],
    pub root_methods: [u64; ROOT_METHOD_COUNT],
    pub composition_shapes: [u64; COMPOSITION_SHAPE_COUNT],
    pub expiry_causes: [u64; EXPIRY_CAUSE_COUNT],
    pub margin_owners: [u64; MARGIN_OWNER_COUNT],
    pub active_feature_density: [u64; DENSITY_BINS],
    pub active_sheet_density: [u64; DENSITY_BINS],
    pub active_event_density: [u64; DENSITY_BINS],
    pub active_predicate_density: [u64; DENSITY_BINS],
    pub leaf_sublevel_density: [u64; DENSITY_BINS],
    pub smooth_cluster_density: [u64; DENSITY_BINS],
    pub root_subdivision_depth: [u64; SUBDIVISION_BINS],
    pub event_subdivision_depth: [u64; SUBDIVISION_BINS],
    pub rebuild_entries: [u64; REBUILD_REASON_COUNT],
    pub rebuild_terminals: [u64; REBUILD_REASON_COUNT],
    pub numeric_failures: [u64; FAILURE_CAUSE_COUNT],
    pub regular_pixels: u64,
    pub corridor_pixels: u64,
    pub proposed_records: u64,
    pub revalidated_records: u64,
    pub new_records: u64,
    pub raster_packets: u64,
    pub raster_scalar_edges: u64,
    pub raster_q_checked: u64,
    pub raster_normal_checked: u64,
    pub raster_world_positions: u64,
    pub raster_background_pixels: u64,
    pub raster_event_pixels: u64,
    pub raster_validation_failures: u64,
}

impl Default for CertificateTelemetry {
    fn default() -> Self {
        Self {
            version: CERTIFICATE_TELEMETRY_VERSION,
            run_lengths: [0; RUN_LENGTH_BINS],
            root_methods: [0; ROOT_METHOD_COUNT],
            composition_shapes: [0; COMPOSITION_SHAPE_COUNT],
            expiry_causes: [0; EXPIRY_CAUSE_COUNT],
            margin_owners: [0; MARGIN_OWNER_COUNT],
            active_feature_density: [0; DENSITY_BINS],
            active_sheet_density: [0; DENSITY_BINS],
            active_event_density: [0; DENSITY_BINS],
            active_predicate_density: [0; DENSITY_BINS],
            leaf_sublevel_density: [0; DENSITY_BINS],
            smooth_cluster_density: [0; DENSITY_BINS],
            root_subdivision_depth: [0; SUBDIVISION_BINS],
            event_subdivision_depth: [0; SUBDIVISION_BINS],
            rebuild_entries: [0; REBUILD_REASON_COUNT],
            rebuild_terminals: [0; REBUILD_REASON_COUNT],
            numeric_failures: [0; FAILURE_CAUSE_COUNT],
            regular_pixels: 0,
            corridor_pixels: 0,
            proposed_records: 0,
            revalidated_records: 0,
            new_records: 0,
            raster_packets: 0,
            raster_scalar_edges: 0,
            raster_q_checked: 0,
            raster_normal_checked: 0,
            raster_world_positions: 0,
            raster_background_pixels: 0,
            raster_event_pixels: 0,
            raster_validation_failures: 0,
        }
    }
}

impl CertificateTelemetry {
    pub fn charge_run(
        &mut self,
        length: u16,
        method: RootMethod,
        shape: CompositionShape,
        expiry: ExpiryCause,
        owner: MarginOwner,
    ) {
        self.run_lengths[run_length_bin(length)] += 1;
        self.root_methods[method as usize] += 1;
        self.composition_shapes[shape as usize] += 1;
        self.expiry_causes[expiry as usize] += 1;
        self.margin_owners[owner as usize] += 1;
        self.regular_pixels += u64::from(length);
    }

    pub fn charge_density(
        &mut self,
        features: usize,
        sheets: usize,
        events: usize,
        predicates: usize,
    ) {
        self.active_feature_density[density_bin(features)] += 1;
        self.active_sheet_density[density_bin(sheets)] += 1;
        self.active_event_density[density_bin(events)] += 1;
        self.active_predicate_density[density_bin(predicates)] += 1;
    }

    pub fn charge_rebuild_entry(&mut self, reason: RebuildReason) {
        self.rebuild_entries[reason as usize] += 1;
    }

    pub fn charge_rebuild_terminal(&mut self, reason: RebuildReason) {
        self.rebuild_terminals[reason as usize] += 1;
    }

    pub fn merge_in_tile_order(&mut self, tile: &Self) -> Result<(), &'static str> {
        if self.version != CERTIFICATE_TELEMETRY_VERSION
            || tile.version != CERTIFICATE_TELEMETRY_VERSION
        {
            return Err("certificate telemetry version mismatch");
        }
        macro_rules! merge {
            ($field:ident) => {
                for (target, source) in self.$field.iter_mut().zip(tile.$field) {
                    *target = target
                        .checked_add(source)
                        .ok_or("certificate telemetry overflow")?;
                }
            };
        }
        merge!(run_lengths);
        merge!(root_methods);
        merge!(composition_shapes);
        merge!(expiry_causes);
        merge!(margin_owners);
        merge!(active_feature_density);
        merge!(active_sheet_density);
        merge!(active_event_density);
        merge!(active_predicate_density);
        merge!(leaf_sublevel_density);
        merge!(smooth_cluster_density);
        merge!(root_subdivision_depth);
        merge!(event_subdivision_depth);
        merge!(rebuild_entries);
        merge!(rebuild_terminals);
        merge!(numeric_failures);
        for (target, source) in [
            (&mut self.regular_pixels, tile.regular_pixels),
            (&mut self.corridor_pixels, tile.corridor_pixels),
            (&mut self.proposed_records, tile.proposed_records),
            (&mut self.revalidated_records, tile.revalidated_records),
            (&mut self.new_records, tile.new_records),
            (&mut self.raster_packets, tile.raster_packets),
            (&mut self.raster_scalar_edges, tile.raster_scalar_edges),
            (&mut self.raster_q_checked, tile.raster_q_checked),
            (&mut self.raster_normal_checked, tile.raster_normal_checked),
            (
                &mut self.raster_world_positions,
                tile.raster_world_positions,
            ),
            (
                &mut self.raster_background_pixels,
                tile.raster_background_pixels,
            ),
            (&mut self.raster_event_pixels, tile.raster_event_pixels),
            (
                &mut self.raster_validation_failures,
                tile.raster_validation_failures,
            ),
        ] {
            *target = target
                .checked_add(source)
                .ok_or("certificate telemetry overflow")?;
        }
        Ok(())
    }
}

const fn run_length_bin(length: u16) -> usize {
    match length {
        0 | 1 => 0,
        2 => 1,
        3..=4 => 2,
        5..=8 => 3,
        9..=16 => 4,
        17..=32 => 5,
        33..=64 => 6,
        _ => 7,
    }
}

const fn density_bin(count: usize) -> usize {
    match count {
        0 => 0,
        1 => 1,
        2 => 2,
        3..=4 => 3,
        5..=8 => 4,
        9..=16 => 5,
        17..=32 => 6,
        _ => 7,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn telemetry_merge_is_stable_and_exactly_charges_pixels() {
        let mut first = CertificateTelemetry::default();
        first.charge_run(
            1,
            RootMethod::BernsteinFaces,
            CompositionShape::Plane,
            ExpiryCause::DomainEnd,
            MarginOwner::Root,
        );
        let mut second = CertificateTelemetry::default();
        second.charge_run(
            64,
            RootMethod::Krawczyk,
            CompositionShape::Torus,
            ExpiryCause::Event,
            MarginOwner::Event,
        );
        first.merge_in_tile_order(&second).unwrap();
        assert_eq!(first.regular_pixels, 65);
        assert_eq!(first.run_lengths[0], 1);
        assert_eq!(first.run_lengths[6], 1);
        assert_eq!(first.root_methods, [1, 0, 1]);
    }

    #[test]
    fn versioned_counter_count_matches_every_fixed_field() {
        let counted = RUN_LENGTH_BINS
            + ROOT_METHOD_COUNT
            + COMPOSITION_SHAPE_COUNT
            + EXPIRY_CAUSE_COUNT
            + MARGIN_OWNER_COUNT
            + 6 * DENSITY_BINS
            + 2 * SUBDIVISION_BINS
            + 2 * REBUILD_REASON_COUNT
            + FAILURE_CAUSE_COUNT
            + 5
            + RASTER_CONFORMANCE_COUNTERS;
        assert_eq!(CERTIFICATE_TELEMETRY_COUNTERS_V2, counted as u64);
        assert_eq!(CERTIFICATE_TELEMETRY_COUNTERS_V2, 150);
    }
}
