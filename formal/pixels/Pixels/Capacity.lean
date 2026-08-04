import Mathlib

namespace Pixels

def CapacityFits (needed ceiling : Nat) : Prop :=
  needed ≤ ceiling

theorem capacity_add
    (a b ceiling : Nat)
    (ha : a ≤ ceiling) (hb : b ≤ ceiling - a) :
    CapacityFits (a + b) ceiling := by
  simp only [CapacityFits]
  omega

theorem capacity_product
    (a b aCeiling bCeiling : Nat)
    (ha : CapacityFits a aCeiling)
    (hb : CapacityFits b bCeiling) :
    CapacityFits (a * b) (aCeiling * bCeiling) := by
  exact Nat.mul_le_mul ha hb

structure StructuralCounts where
  objects : Nat
  features : Nat
  repeatInstances : Nat
  materialCrossings : Nat

def StructuralCounts.eventGenerators (counts : StructuralCounts) : Nat :=
  counts.features + counts.repeatInstances + counts.materialCrossings

theorem StructuralCounts.eventGenerators_fits
    (counts ceilings : StructuralCounts)
    (hFeatures : counts.features ≤ ceilings.features)
    (hRepeats : counts.repeatInstances ≤ ceilings.repeatInstances)
    (hMaterials : counts.materialCrossings ≤ ceilings.materialCrossings) :
    counts.eventGenerators ≤ ceilings.eventGenerators := by
  simp only [eventGenerators]
  omega

def scratchBytes
    (runs runBytes corridors corridorBytes roots rootBytes shading shadingBytes : Nat) : Nat :=
  runs * runBytes + corridors * corridorBytes + roots * rootBytes + shading * shadingBytes

theorem scratchBytes_monotone
    (runs runCeiling runBytes
      corridors corridorCeiling corridorBytes
      roots rootCeiling rootBytes
      shading shadingCeiling shadingBytes : Nat)
    (hr : runs ≤ runCeiling)
    (hc : corridors ≤ corridorCeiling)
    (hroot : roots ≤ rootCeiling)
    (hs : shading ≤ shadingCeiling) :
    scratchBytes runs runBytes corridors corridorBytes roots rootBytes shading shadingBytes ≤
      scratchBytes runCeiling runBytes corridorCeiling corridorBytes
        rootCeiling rootBytes shadingCeiling shadingBytes := by
  simp only [scratchBytes]
  exact Nat.add_le_add
    (Nat.add_le_add
      (Nat.add_le_add (Nat.mul_le_mul_right runBytes hr)
        (Nat.mul_le_mul_right corridorBytes hc))
      (Nat.mul_le_mul_right rootBytes hroot))
    (Nat.mul_le_mul_right shadingBytes hs)

/-- The exact P3 Rust generator census, before checked machine-width casts. -/
structure StructuralCapacityInputs where
  primitiveGenerators : Nat
  validityGenerators : Nat
  materialGenerators : Nat
  repeatWrapGenerators : Nat
  maximumRootsPerGenerator : Nat
  dyadicIsolationDepth : Nat
  candidateRecords : Nat
  rootRecords : Nat
  sheetRecords : Nat
  transparentLayers : Nat
  workers : Nat
  pixels : Nat
  packedParameterBytes : Nat
  frameDependencyBytes : Nat
  probeBytes : Nat
  kineticBytes : Nat

def StructuralCapacityInputs.eventGeneratorCount
    (input : StructuralCapacityInputs) : Nat :=
  input.primitiveGenerators + input.validityGenerators +
    input.materialGenerators + input.repeatWrapGenerators

def StructuralCapacityInputs.eventRecords
    (input : StructuralCapacityInputs) : Nat :=
  input.eventGeneratorCount *
    (input.maximumRootsPerGenerator * 2 ^ input.dyadicIsolationDepth)

def StructuralCapacityInputs.runRecords
    (input : StructuralCapacityInputs) : Nat :=
  input.eventRecords + 1

def StructuralCapacityInputs.perWorkerScratch
    (input : StructuralCapacityInputs) : Nat :=
  input.candidateRecords * 64 +
  input.rootRecords * 32 +
  input.sheetRecords * 64 +
  input.eventRecords * 32 +
  input.runRecords * 32 +
  input.runRecords * 24 +
  input.runRecords * 16 +
  input.runRecords * 64 +
  input.runRecords * input.transparentLayers * 32

def StructuralCapacityInputs.rendererState
    (input : StructuralCapacityInputs) : Nat :=
  256 +
  2 * input.packedParameterBytes +
  2 * input.frameDependencyBytes +
  2 * input.pixels * 32 +
  input.workers * input.perWorkerScratch +
  2 * input.pixels * 4 +
  input.probeBytes +
  input.kineticBytes +
  input.pixels * 32 +
  input.pixels * 4 +
  64

theorem StructuralCapacityInputs.runRecords_exact
    (input : StructuralCapacityInputs) :
    input.runRecords =
      (input.primitiveGenerators + input.validityGenerators +
        input.materialGenerators + input.repeatWrapGenerators) *
        (input.maximumRootsPerGenerator * 2 ^ input.dyadicIsolationDepth) + 1 := by
  rfl

theorem StructuralCapacityInputs.rendererState_includes_both_snapshots
    (input : StructuralCapacityInputs) :
    2 * input.packedParameterBytes + 2 * input.frameDependencyBytes ≤
      input.rendererState := by
  simp only [rendererState]
  omega

theorem StructuralCapacityInputs.eventRecords_monotone
    (a b : StructuralCapacityInputs)
    (hprimitive : a.primitiveGenerators ≤ b.primitiveGenerators)
    (hvalidity : a.validityGenerators ≤ b.validityGenerators)
    (hmaterial : a.materialGenerators ≤ b.materialGenerators)
    (hrepeat : a.repeatWrapGenerators ≤ b.repeatWrapGenerators)
    (hroots : a.maximumRootsPerGenerator ≤ b.maximumRootsPerGenerator)
    (hdepth : a.dyadicIsolationDepth ≤ b.dyadicIsolationDepth) :
    a.eventRecords ≤ b.eventRecords := by
  simp only [eventRecords, eventGeneratorCount]
  exact Nat.mul_le_mul (by omega)
    (Nat.mul_le_mul hroots (pow_le_pow_right₀ (by omega : 1 ≤ (2 : Nat)) hdepth))

theorem StructuralCapacityInputs.rendererState_fits
    (input : StructuralCapacityInputs) (ceiling : Nat)
    (hfit : input.rendererState ≤ ceiling) :
    CapacityFits input.rendererState ceiling :=
  hfit

end Pixels
