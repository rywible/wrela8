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

#print axioms Pixels.trustBoundaryScaffold
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
#print axioms Pixels.powerToBernsteinDegreeSixExact
#print axioms Pixels.powerToBernsteinDegreeEightExact
#print axioms Pixels.taylorWithRemainder
#print axioms Pixels.bernsteinSubdivisionPositive
#print axioms Pixels.quadraticCorrectionFaces
#print axioms Pixels.powerDerivative
#print axioms Pixels.positiveCoefficientSignVariationZero
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
