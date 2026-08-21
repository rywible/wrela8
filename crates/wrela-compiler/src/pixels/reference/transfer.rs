//! P10 ordered surface layers, premultiplied transfer trees, and certified tails.

use super::{
    csg::{self, CsgInstruction, Orientation},
    iv32::{FixedDomain, Iv32, NumericError},
    order::{self, OrderRelation},
};

pub const MAX_TRANSFER_LAYERS_V1: usize = 64;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceBoundary {
    pub sheet: u32,
    pub identity: u32,
    pub q_model: u32,
    pub q_error: Iv32,
    pub orientation: Orientation,
    pub material_summary: u32,
    pub object: u8,
    pub feature: u32,
    pub opaque: bool,
    pub emits_layer: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SurfaceLayer {
    pub sheet: u32,
    pub identity: u32,
    pub q_model: u32,
    pub q_error: Iv32,
    pub orientation: Orientation,
    pub material_summary: u32,
}

impl From<SurfaceBoundary> for SurfaceLayer {
    fn from(value: SurfaceBoundary) -> Self {
        Self {
            sheet: value.sheet,
            identity: value.identity,
            q_model: value.q_model,
            q_error: value.q_error,
            orientation: value.orientation,
            material_summary: value.material_summary,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum OrderedLayerGroup {
    Regular(SurfaceLayer),
    /// Connected q-overlap component. Runtime evaluates both analytic sides;
    /// member order has no visibility authority.
    EventCorridor(Vec<SurfaceLayer>),
}

/// Converts a complete, near-to-far all-root sweep to composite surface work.
/// Capacity is checked before any group is committed. Exact adjacent duplicate
/// primitive boundaries are removed, but geometry occupancy is always toggled.
pub fn ordered_surface_layers(
    csg_program: &[CsgInstruction],
    mut inside_bits: u64,
    boundaries: &[SurfaceBoundary],
    capacity: usize,
) -> Result<Vec<OrderedLayerGroup>, NumericError> {
    if capacity > MAX_TRANSFER_LAYERS_V1 {
        return Err(NumericError::CapacityExceeded);
    }
    let mut groups = Vec::<OrderedLayerGroup>::new();
    let mut occupied = csg::evaluate(csg_program, inside_bits)?;
    let mut previous: Option<SurfaceBoundary> = None;
    let mut previous_emitted: Option<SurfaceBoundary> = None;
    let mut emitted = 0_usize;

    for boundary in boundaries.iter().copied() {
        if let Some(prior) = previous {
            if order::compare(prior.q_error, boundary.q_error) == OrderRelation::Farther {
                return Err(NumericError::UnsupportedShape);
            }
        }
        let influential = csg::boundary_influences(csg_program, inside_bits, boundary.object)?;
        inside_bits = csg::oriented_toggle(inside_bits, boundary.object, boundary.orientation)?;
        let next_occupied = csg::evaluate(csg_program, inside_bits)?;
        let transition = influential && next_occupied != occupied;
        occupied = next_occupied;

        let duplicate = previous.is_some_and(|prior| {
            prior.object == boundary.object
                && prior.feature == boundary.feature
                && prior.orientation == boundary.orientation
                && prior.q_error == boundary.q_error
        });
        if transition && boundary.emits_layer && !duplicate {
            if emitted == capacity {
                return Err(NumericError::CapacityExceeded);
            }
            let layer = SurfaceLayer::from(boundary);
            if previous_emitted.is_some_and(|prior| {
                order::compare(prior.q_error, boundary.q_error) == OrderRelation::Corridor
            }) {
                match groups.last_mut() {
                    Some(OrderedLayerGroup::EventCorridor(members)) => members.push(layer),
                    Some(OrderedLayerGroup::Regular(prior)) => {
                        let prior = *prior;
                        *groups.last_mut().expect("group exists") =
                            OrderedLayerGroup::EventCorridor(vec![prior, layer]);
                    }
                    None => groups.push(OrderedLayerGroup::EventCorridor(vec![layer])),
                }
            } else {
                groups.push(OrderedLayerGroup::Regular(layer));
            }
            emitted += 1;
            previous_emitted = Some(boundary);
            if boundary.opaque {
                break;
            }
        }
        previous = Some(boundary);
    }
    Ok(groups)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Transfer {
    pub rgb: [Iv32; 3],
    pub transmittance: Iv32,
}

impl Transfer {
    pub fn identity(one: Iv32) -> Self {
        Self {
            rgb: [Iv32::point(0); 3],
            transmittance: one,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CandidateTransfer {
    pub rgb: [f64; 3],
    pub transmittance: f64,
}

impl CandidateTransfer {
    pub const IDENTITY: Self = Self {
        rgb: [0.0; 3],
        transmittance: 1.0,
    };

    pub fn compose(self, back: Self) -> Self {
        Self {
            rgb: std::array::from_fn(|channel| {
                self.rgb[channel] + self.transmittance * back.rgb[channel]
            }),
            transmittance: self.transmittance * back.transmittance,
        }
    }
}

pub fn compose(
    front: Transfer,
    back: Transfer,
    domain: FixedDomain,
    rounding_radius: i32,
) -> Result<Transfer, NumericError> {
    if rounding_radius < 0 {
        return Err(NumericError::InvalidDomain);
    }
    let rounding = Iv32::new(-rounding_radius, rounding_radius)?;
    let mut rgb = [Iv32::point(0); 3];
    for (channel, output) in rgb.iter_mut().enumerate() {
        *output = front.rgb[channel]
            .add(
                front.transmittance.multiply(back.rgb[channel], domain)?,
                domain,
            )?
            .add(rounding, domain)?;
    }
    Ok(Transfer {
        rgb,
        transmittance: front
            .transmittance
            .multiply(back.transmittance, domain)?
            .add(rounding, domain)?,
    })
}

pub fn balanced_summary(
    layers: &[Transfer],
    domain: FixedDomain,
    one: Iv32,
    rounding_radius: i32,
) -> Result<Transfer, NumericError> {
    Ok(TransferTree::build(layers, domain, one, rounding_radius)?.summary())
}

/// Fixed-layout heap tree. Leaves begin at `leaf_count`; unused leaves are
/// identity and leaf placement is stable for the sealed capacity.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferTree {
    capacity: usize,
    leaf_count: usize,
    nodes: Vec<Transfer>,
    domain: FixedDomain,
    one: Iv32,
    rounding_radius: i32,
}

impl TransferTree {
    pub fn build(
        layers: &[Transfer],
        domain: FixedDomain,
        one: Iv32,
        rounding_radius: i32,
    ) -> Result<Self, NumericError> {
        Self::with_capacity(layers.len(), layers, domain, one, rounding_radius)
    }

    pub fn with_capacity(
        capacity: usize,
        layers: &[Transfer],
        domain: FixedDomain,
        one: Iv32,
        rounding_radius: i32,
    ) -> Result<Self, NumericError> {
        if layers.len() > capacity || capacity > MAX_TRANSFER_LAYERS_V1 || rounding_radius < 0 {
            return Err(NumericError::CapacityExceeded);
        }
        let leaf_count = capacity.max(1).next_power_of_two();
        let identity = Transfer::identity(one);
        let mut nodes = vec![identity; leaf_count * 2];
        nodes[leaf_count..leaf_count + layers.len()].copy_from_slice(layers);
        for node in (1..leaf_count).rev() {
            nodes[node] = compose(
                nodes[node * 2],
                nodes[node * 2 + 1],
                domain,
                rounding_radius,
            )?;
        }
        Ok(Self {
            capacity,
            leaf_count,
            nodes,
            domain,
            one,
            rounding_radius,
        })
    }

    pub fn capacity(&self) -> usize {
        self.capacity
    }
    pub fn leaf_count(&self) -> usize {
        self.leaf_count
    }
    pub fn storage_nodes(&self) -> usize {
        self.nodes.len() - 1
    }
    pub fn summary(&self) -> Transfer {
        self.nodes[1]
    }
    pub fn leaf(&self, index: usize) -> Option<Transfer> {
        (index < self.capacity).then(|| self.nodes[self.leaf_count + index])
    }

    /// Returns exactly the number of repaired ancestors.
    pub fn replace(&mut self, index: usize, replacement: Transfer) -> Result<usize, NumericError> {
        if index >= self.capacity {
            return Err(NumericError::CapacityExceeded);
        }
        let mut node = self.leaf_count + index;
        self.nodes[node] = replacement;
        let mut repaired = 0;
        while node > 1 {
            node /= 2;
            self.nodes[node] = compose(
                self.nodes[node * 2],
                self.nodes[node * 2 + 1],
                self.domain,
                self.rounding_radius,
            )?;
            repaired += 1;
        }
        Ok(repaired)
    }

    pub fn identity(&self) -> Transfer {
        Transfer::identity(self.one)
    }
}

pub fn replace_and_summarize(
    layers: &mut [Transfer],
    index: usize,
    replacement: Transfer,
    domain: FixedDomain,
    one: Iv32,
    rounding_radius: i32,
) -> Result<Transfer, NumericError> {
    let target = layers
        .get_mut(index)
        .ok_or(NumericError::CapacityExceeded)?;
    *target = replacement;
    balanced_summary(layers, domain, one, rounding_radius)
}

/// Early sufficient product check. Equality is rejected so later arithmetic
/// cannot silently spend the entire assigned budget.
pub fn tail_product_with_post_is_below_budget(
    prefix_transmittance: Iv32,
    maximum_remaining_radiance: Iv32,
    post_sensitivity: Iv32,
    assigned_error_raw: i32,
    domain: FixedDomain,
) -> Result<bool, NumericError> {
    if assigned_error_raw < 0
        || prefix_transmittance.lo < 0
        || maximum_remaining_radiance.lo < 0
        || post_sensitivity.lo < 0
    {
        return Err(NumericError::InvalidDomain);
    }
    let possible = prefix_transmittance
        .multiply(maximum_remaining_radiance, domain)?
        .multiply(post_sensitivity, domain)?;
    Ok(possible.hi < assigned_error_raw)
}

pub fn final_byte_singleton(candidate: [u8; 3], endpoint_bytes: [[u8; 2]; 3]) -> bool {
    endpoint_bytes
        .iter()
        .zip(candidate)
        .all(|(bounds, byte)| bounds[0] == byte && bounds[1] == byte)
}

pub fn tail_can_stop_after_byte_proof(
    prefix_transmittance: Iv32,
    maximum_remaining_radiance: Iv32,
    post_sensitivity: Iv32,
    assigned_error_raw: i32,
    domain: FixedDomain,
    candidate: [u8; 3],
    endpoint_bytes: [[u8; 2]; 3],
) -> Result<bool, NumericError> {
    Ok(tail_product_with_post_is_below_budget(
        prefix_transmittance,
        maximum_remaining_radiance,
        post_sensitivity,
        assigned_error_raw,
        domain,
    )? && final_byte_singleton(candidate, endpoint_bytes))
}

/// Sealed scalar-correspondence adapter. The complete frame path additionally
/// calls `tail_can_stop_after_byte_proof` before it may replace a suffix.
pub fn tail_can_stop(
    prefix_transmittance: Iv32,
    maximum_remaining_radiance: Iv32,
    assigned_error_raw: i32,
    domain: FixedDomain,
) -> Result<bool, NumericError> {
    tail_product_with_post_is_below_budget(
        prefix_transmittance,
        maximum_remaining_radiance,
        Iv32::point(256),
        assigned_error_raw,
        domain,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn layer(q: Iv32, object: u8, feature: u32, opaque: bool) -> SurfaceBoundary {
        SurfaceBoundary {
            sheet: feature,
            identity: object.into(),
            q_model: feature,
            q_error: q,
            orientation: Orientation::Enter,
            material_summary: object.into(),
            object,
            feature,
            opaque,
            emits_layer: true,
        }
    }

    #[test]
    fn csg_layers_stop_at_opaque_and_check_capacity_before_commit() {
        let program = [
            CsgInstruction::Object(0),
            CsgInstruction::Object(1),
            CsgInstruction::Union,
        ];
        let roots = [
            layer(Iv32::point(30), 0, 4, true),
            layer(Iv32::point(20), 1, 5, false),
        ];
        let groups = ordered_surface_layers(&program, 0, &roots, 1).unwrap();
        assert_eq!(groups.len(), 1);
        assert!(matches!(groups[0], OrderedLayerGroup::Regular(_)));
        assert_eq!(
            ordered_surface_layers(&program, 0, &roots, 0),
            Err(NumericError::CapacityExceeded)
        );
    }

    #[test]
    fn coincident_layers_form_a_corridor() {
        let program = [CsgInstruction::Object(0)];
        let mut exit = layer(Iv32::new(21, 23).unwrap(), 0, 1, false);
        exit.orientation = Orientation::Exit;
        let roots = [layer(Iv32::new(20, 22).unwrap(), 0, 0, false), exit];
        let groups = ordered_surface_layers(&program, 0, &roots, 2).unwrap();
        assert!(matches!(&groups[0], OrderedLayerGroup::EventCorridor(v) if v.len() == 2));
    }

    #[test]
    fn omitted_boundary_cannot_pull_a_disjoint_layer_into_a_corridor() {
        let program = [
            CsgInstruction::Object(0),
            CsgInstruction::Object(1),
            CsgInstruction::Union,
        ];
        let mut omitted = layer(Iv32::new(20, 22).unwrap(), 1, 1, false);
        omitted.emits_layer = false;
        let mut final_boundary = layer(Iv32::new(21, 23).unwrap(), 1, 2, false);
        final_boundary.orientation = Orientation::Exit;
        let roots = [
            layer(Iv32::new(30, 31).unwrap(), 0, 0, false),
            omitted,
            final_boundary,
        ];
        let groups = ordered_surface_layers(&program, 0, &roots, 2).unwrap();
        assert!(
            groups
                .iter()
                .all(|group| matches!(group, OrderedLayerGroup::Regular(_)))
        );
    }

    #[test]
    fn tree_layout_is_fixed_and_repair_is_ancestor_only() {
        let domain = FixedDomain::full(-8);
        let one = Iv32::point(256);
        let clear = Transfer::identity(one);
        let mut tree = TransferTree::with_capacity(8, &[clear; 2], domain, one, 0).unwrap();
        assert_eq!((tree.leaf_count(), tree.storage_nodes()), (8, 15));
        assert_eq!(tree.replace(1, clear), Ok(3));
        assert_eq!(tree.summary(), clear);
        assert_eq!(tree.leaf(7), Some(tree.identity()));
    }

    #[test]
    fn candidate_association_and_front_back_order_are_exact() {
        let a = CandidateTransfer {
            rgb: [0.25, 0.0, 0.0],
            transmittance: 0.5,
        };
        let b = CandidateTransfer {
            rgb: [0.0, 0.5, 0.0],
            transmittance: 0.25,
        };
        let c = CandidateTransfer {
            rgb: [0.0, 0.0, 1.0],
            transmittance: 0.0,
        };
        assert_eq!(a.compose(b).compose(c), a.compose(b.compose(c)));
        assert_ne!(a.compose(b), b.compose(a));
    }

    #[test]
    fn tail_requires_strict_product_and_final_singleton() {
        let domain = FixedDomain::full(-8);
        assert_eq!(
            tail_product_with_post_is_below_budget(
                Iv32::point(8),
                Iv32::point(4096),
                Iv32::point(256),
                1,
                domain
            ),
            Ok(false)
        );
        assert_eq!(
            tail_can_stop_after_byte_proof(
                Iv32::point(1),
                Iv32::point(1),
                Iv32::point(1),
                2,
                domain,
                [7; 3],
                [[7, 7], [7, 8], [7, 7]]
            ),
            Ok(false)
        );
    }
}
