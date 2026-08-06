import Pixels.TrustBoundary
import Pixels.Bernstein
import Pixels.Dyadic
import Pixels.Interval
import Pixels.RootIsolation
import Pixels.Csg
import Pixels.Krawczyk

namespace Pixels

/-
Kernel-specific proof aliases are the semantic join between KERNELS.txt and
Lean. The manifest gate requires exactly `<kernel>_correspondence` and checks
that its proof term is the theorem named by that kernel's row. Lean then
type-checks every alias, so a row cannot pass by naming an unrelated symbol
that merely occurs somewhere in the formal source.
-/
def bernstein_compose_correspondence := @Pixels.bernsteinCoefficientEnclosure
def bernstein_sign_correspondence := @Pixels.bernsteinPositiveHull
def bernstein_subdivide_correspondence := @Pixels.bernsteinSubdivisionPositive
def byte_singleton_correspondence := @Pixels.display_singleton_is_exact
def coverage_line_correspondence := @Pixels.half_plane_area_enclosure
def coverage_quadratic_correspondence := @Pixels.monotone_curve_strip_enclosure
def csg_first_transition_correspondence := @Pixels.csg_first_transition_contract
def fixed_q_step_correspondence := @Pixels.fixed_q_step
def interval_add_correspondence := @Pixels.Interval.add_contains
def interval_intersect_correspondence := @Pixels.Iv32.intersection_containment
def interval_mul_correspondence := @Pixels.Interval.mulHull_contains
def interval_sub_correspondence := @Pixels.Interval.sub_contains
def kinetic_slack_correspondence := @Pixels.kinetic_slack_preserves_run
def normal_bound_correspondence := @Pixels.inverse_depth_normal_reconstruction
def q_order_correspondence := @Pixels.strict_interval_q_order
def quadratic_range_correspondence := @Pixels.quadratic_candidate_schedule_exact
def root_bracket_correspondence := @Pixels.root_bracket_step
def root_count_correspondence := @Pixels.bernstein_root_count_zero_excludes
def run_certificate_correspondence := @Pixels.run_certificate_first_visible
def transfer_compose_correspondence := @Pixels.transfer_compose_associative
def transparency_tail_correspondence := @Pixels.transparent_summary_within_budget
def interval_negate_correspondence := @Pixels.Interval.neg_contains
def interval_square_correspondence := @Pixels.Interval.square_contains
def interval_scale_pow2_correspondence := @Pixels.Interval.scale_pow2_contains
def interval_hull_correspondence := @Pixels.Iv32.hull_containment
def interval_min_correspondence := @Pixels.Interval.min_contains
def interval_max_correspondence := @Pixels.Interval.max_contains
def interval_abs_correspondence := @Pixels.Interval.abs_contains
def interval_affine_correspondence := @Pixels.Interval.affine_contains
def interval_convert_correspondence := @Pixels.Iv32.outward_conversion_contains
def bernstein_average_correspondence := @Pixels.bernstein_average_contains
def coverage_quadratic_split_correspondence := @Pixels.quadratic_stationary_split_exact
def coverage_quadratic_graph_correspondence := @Pixels.monotone_piece_union_preserves_area_bounds
def bernstein_certificate_correspondence := @Pixels.bernstein_certificate_unique_root
def krawczyk_certificate_correspondence := @Pixels.krawczyk_strict_inclusion
def normal_safe_correspondence := @Pixels.normal_lower_bound_nonzero
def normal_dot_correspondence := @Pixels.normal_dot_expansion
def material_affine_correspondence := @Pixels.material_affine_scalar_contains
def material_residual_correspondence := @Pixels.material_residual_bound
def exposure_channel_correspondence := @Pixels.exposure_channel_contains
def color_matrix_channel_correspondence := @Pixels.color_matrix_channel_expansion
def monotone_lut_correspondence := @Pixels.monotone_lut_endpoint_enclosure
def quantize_ties_even_correspondence := @Pixels.quantize_ties_even_model
def root_zero_corridor_correspondence := @Pixels.bernstein_identically_zero_is_degenerate
def fixed_q_setup_correspondence := @Pixels.fixed_q_setup_no_overflow
def normal_cone_dot_correspondence := @Pixels.normalized_dot_unit_interval
def polynomial_sparse_correspondence := @Pixels.polynomial_sparse_term_model
def polynomial_taylor_correspondence := @Pixels.taylorWithRemainder
def transfer_balanced_correspondence := @Pixels.transfer_balanced_model
def transfer_replace_correspondence := @Pixels.transfer_replace_model
def material_affine_moments_correspondence := @Pixels.centered_affine_mean
def material_quadratic_moments_correspondence := @Pixels.centered_quadratic_mean
def q_insertion_sort_correspondence := @Pixels.q_component_sort_canonical
def q_adjacent_slack_correspondence := @Pixels.adjacent_q_order_three
def csg_stack_eval_correspondence := @Pixels.CsgExpr.compiled_program_correct
def root_isolate_correspondence := @Pixels.bounded_subdivision_complete_or_unresolved
def csg_oriented_toggle_correspondence := @Pixels.csg_oriented_toggle_contract
def csg_boundary_influence_correspondence := @Pixels.csg_boundary_influence_contract
def bernstein_certificate_subdivided_correspondence := @Pixels.bernstein_certificate_subdivided_unique_root
def fixed_q_nearer_correspondence := @Pixels.fixed_q_winner_matches_real_winner
def coverage_color_error_correspondence := @Pixels.coverage_composite_within_budget
def material_footprint_correspondence := @Pixels.material_footprint_bounds
def material_affine_vector_correspondence := @Pixels.affine_interval_vector_component
def normal_cone_correspondence := @Pixels.normal_cone_reconstruction
def polynomial_horner_correspondence := @Pixels.polynomial_horner_model
def polynomial_derivative_correspondence := @Pixels.powerDerivative
def polynomial_compose_correspondence := @Pixels.polynomial_compose_model
def interval_mul_exp_correspondence := @Pixels.Interval.mulHull_contains
def interval_from_f32_bits_correspondence := @Pixels.Iv32.f32_conversion_radius_contains
def interval_contains_zero_correspondence := @Pixels.Interval.contains_zero_model
def interval_strict_positive_correspondence := @Pixels.Interval.strict_positive_model
def interval_strict_negative_correspondence := @Pixels.Interval.strict_negative_model
def interval_add_restricted_correspondence := @Pixels.Interval.restricted_add_contract
def rgb_singleton_correspondence := @Pixels.rgb_singleton_is_exact
def bernstein_lerp_ratio_correspondence := @Pixels.bernstein_lerp_ratio_contains
def fixed_domain_correspondence := @Pixels.fixed_domain_contract
def certify_run_correspondence := @Pixels.ordered_certificate_selection
def root_config_correspondence := @Pixels.bounded_subdivision_complete_or_unresolved

end Pixels
