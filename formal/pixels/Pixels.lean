import Pixels.SmoothObject
import Pixels.Deformation
import Pixels.TrustBoundary
import Pixels.Interval
import Pixels.SmoothMin
import Pixels.SupportTree
import Pixels.Csg
import Pixels.Capacity
import Pixels.Projective
import Pixels.Bernstein
import Pixels.Primitive
import Pixels.EventCover
import Pixels.Dyadic
import Pixels.RootIsolation
import Pixels.Krawczyk
import Pixels.RunCertificate
import Pixels.QOrder
import Pixels.FixedQ
import Pixels.Coverage
import Pixels.Normal
import Pixels.MaterialBound
import Pixels.Compositing
import Pixels.TransparencyTail
import Pixels.DisplayByte
import Pixels.Kinetic
import Pixels.KernelCorrespondence

#print axioms Pixels.run_certificate_first_visible
#print axioms Pixels.complete_event_cell_preserves_structure
#print axioms Pixels.fixed_q_winner_matches_real_winner
#print axioms Pixels.coverage_composite_within_budget
#print axioms Pixels.transparent_summary_within_budget
#print axioms Pixels.display_singleton_is_exact
#print axioms Pixels.kinetic_slack_preserves_run
#print axioms Pixels.renderer_trust_boundary
#print axioms Pixels.Iv32.endpoint_containment
#print axioms Pixels.Iv32.hull_containment
#print axioms Pixels.Iv32.f32_conversion_radius_contains
#print axioms Pixels.Iv32.outward_conversion_contains
#print axioms Pixels.Iv32.intersection_containment
#print axioms Pixels.affine_interval_vector_component
#print axioms Pixels.material_affine_scalar_contains
#print axioms Pixels.adjacent_q_order_three
#print axioms Pixels.q_component_sort_canonical
#print axioms Pixels.bernstein_root_count_zero_excludes
#print axioms Pixels.bernstein_identically_zero_is_degenerate
#print axioms Pixels.bernstein_average_contains
#print axioms Pixels.bernstein_lerp_ratio_contains
#print axioms Pixels.rational_subdivision_parameter_matches_cell_split
#print axioms Pixels.bernstein_certificate_unique_root
#print axioms Pixels.bernstein_certificate_subdivided_unique_root
#print axioms Pixels.centered_affine_mean
#print axioms Pixels.centered_quadratic_mean
#print axioms Pixels.fixed_q_step
#print axioms Pixels.fixed_q_setup_no_overflow
#print axioms Pixels.ordered_certificate_selection
#print axioms Pixels.half_plane_area_enclosure
#print axioms Pixels.krawczyk_strict_inclusion
#print axioms Pixels.line_coverage_interval
#print axioms Pixels.material_residual_bound
#print axioms Pixels.material_footprint_bounds
#print axioms Pixels.monotone_curve_strip_enclosure
#print axioms Pixels.monotone_piece_union_preserves_area_bounds
#print axioms Pixels.normal_lower_bound_nonzero
#print axioms Pixels.inverse_depth_normal_reconstruction
#print axioms Pixels.normal_cone_reconstruction
#print axioms Pixels.normal_dot_expansion
#print axioms Pixels.normalized_dot_unit_interval
#print axioms Pixels.root_bracket_step
#print axioms Pixels.bounded_subdivision_complete_or_unresolved
#print axioms Pixels.quadratic_stationary_split_exact
#print axioms Pixels.strict_interval_q_order
#print axioms Pixels.transfer_compose_associative
#print axioms Pixels.transfer_balanced_model
#print axioms Pixels.transfer_replace_model
#print axioms Pixels.rgb_singleton_is_exact
#print axioms Pixels.exposure_channel_contains
#print axioms Pixels.csg_first_transition_contract
#print axioms Pixels.csg_oriented_toggle_contract
#print axioms Pixels.csg_boundary_influence_contract
#print axioms Pixels.smoothInteriorRoot
#print axioms Pixels.smoothMinLeftSaturated
#print axioms Pixels.smoothMinBounds
#print axioms Pixels.rootCoveredBySupportBound
#print axioms Pixels.smoothInteriorCandidateCoverage
#print axioms Pixels.SmoothObject.scalarBounds
#print axioms Pixels.SmoothObject.composedRootHasSupportedLeaf
#print axioms Pixels.SmoothObject.composedRootHasPathSupportedLeaf
#print axioms Pixels.SmoothObjectRootProgram.composedRootHasCandidate
#print axioms Pixels.RoundedSmoothObject.rootHasSupportedLeaf
#print axioms Pixels.sinusoidalAmplitudeBound
#print axioms Pixels.sinusoidalGradientBound
#print axioms Pixels.sinusoidalHessianBound
#print axioms Pixels.sinusoidalThirdDerivativeBound
#print axioms Pixels.Interval.add_contains
#print axioms Pixels.Interval.contains_zero_model
#print axioms Pixels.Interval.strict_positive_model
#print axioms Pixels.Interval.strict_negative_model
#print axioms Pixels.Interval.restricted_add_contract
#print axioms Pixels.Interval.scale_pow2_contains
#print axioms Pixels.fixed_domain_contract
#print axioms Pixels.Interval.affine_contains
#print axioms Pixels.Interval.neg_contains
#print axioms Pixels.Interval.sub_contains
#print axioms Pixels.Interval.min_contains
#print axioms Pixels.Interval.max_contains
#print axioms Pixels.square_nonnegative
#print axioms Pixels.Interval.abs_contains
#print axioms Pixels.Interval.mulHull_contains
#print axioms Pixels.Interval.square_contains
#print axioms Pixels.Interval.clamp_contains
#print axioms Pixels.Interval.sqrt_contains
#print axioms Pixels.Interval.reciprocalPositive_contains
#print axioms Pixels.Interval.reciprocalNegative_contains
#print axioms Pixels.Interval.divPositive_contains
#print axioms Pixels.Interval.dot3_source_f32_contains
#print axioms Pixels.Interval.cross_source_f32_contains
#print axioms Pixels.Interval.length3_source_f32_contains
#print axioms Pixels.Interval.normalize_source_f32_contains_nonzero
#print axioms Pixels.Interval.normalize_zero_containing_finite
#print axioms Pixels.Interval.select_source_f32_contains
#print axioms Pixels.Interval.smoothMin_source_f32_contains
#print axioms Pixels.convexDerivativeBound
#print axioms Pixels.SupportTree.childBudget_le
#print axioms Pixels.subtract_eval
#print axioms Pixels.CsgExpr.force_eval
#print axioms Pixels.CsgExpr.compiled_program_correct
#print axioms Pixels.capacity_add
#print axioms Pixels.capacity_product
#print axioms Pixels.StructuralCounts.eventGenerators_fits
#print axioms Pixels.scratchBytes_monotone
#print axioms Pixels.StructuralCapacityInputs.runRecords_exact
#print axioms Pixels.StructuralCapacityInputs.rendererState_includes_both_snapshots
#print axioms Pixels.StructuralCapacityInputs.eventRecords_monotone
#print axioms Pixels.StructuralCapacityInputs.rendererState_fits
#print axioms Pixels.projectivePlaneCancellation
#print axioms Pixels.canonicalRayForwardComponent
#print axioms Pixels.normalizedRayCameraCancellation
#print axioms Pixels.bernsteinPositiveHull
#print axioms Pixels.bernsteinNegativeHull
#print axioms Pixels.bernsteinCoefficientEnclosure
#print axioms Pixels.polynomial_horner_model
#print axioms Pixels.polynomial_compose_model
#print axioms Pixels.polynomial_sparse_term_model
#print axioms Pixels.powerToBernsteinDegreeSixExact
#print axioms Pixels.powerToBernsteinDegreeEightExact
#print axioms Pixels.taylorWithRemainder
#print axioms Pixels.bernsteinSubdivisionPositive
#print axioms Pixels.quadraticCorrectionFaces
#print axioms Pixels.quadratic_candidate_schedule_exact
#print axioms Pixels.powerDerivative
#print axioms Pixels.positiveCoefficientSignVariationZero
#print axioms Pixels.color_matrix_channel_expansion
#print axioms Pixels.monotone_lut_endpoint_enclosure
#print axioms Pixels.quantize_ties_even_model
#print axioms Pixels.sphereProjectiveEquivalence
#print axioms Pixels.validityPredicatesEquivalent
#print axioms Pixels.planarFeatureZeroUnderValidity
#print axioms Pixels.sphereFeatureZeroUnderValidity
#print axioms Pixels.boxFaceProjectiveEquivalence
#print axioms Pixels.roundedBoxFaceProjectiveEquivalence
#print axioms Pixels.cylinderCapProjectiveEquivalence
#print axioms Pixels.coneCapProjectiveEquivalence
#print axioms Pixels.genericQuadricProjectiveEquivalence
#print axioms Pixels.segmentSideProjectiveEquivalence
#print axioms Pixels.segmentSideFeatureZeroUnderValidity
#print axioms Pixels.roundedBoxEdgeProjectiveEquivalence
#print axioms Pixels.roundedBoxCornerProjectiveEquivalence
#print axioms Pixels.capsuleSideProjectiveEquivalence
#print axioms Pixels.capsuleCapProjectiveEquivalence
#print axioms Pixels.cylinderSideProjectiveEquivalence
#print axioms Pixels.coneSideProjectiveEquivalence
#print axioms Pixels.torusProjectiveEquivalence
#print axioms Pixels.torusFeatureZeroUnderValidity
#print axioms Pixels.conditionalEventCover
