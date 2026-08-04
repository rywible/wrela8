//! Single classification table for the closed field operation surface.

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FieldIntrinsic {
    Plane,
    Sphere,
    Box,
    RoundBox,
    Capsule,
    FiniteCylinder,
    FiniteCone,
    Torus,
    Translate,
    Rotate,
    RigidTransform,
    UniformScale,
    FiniteRepeatX,
    FiniteRepeatY,
    FiniteRepeatZ,
    Union,
    Intersection,
    Subtract,
    SmoothUnion,
    SmoothIntersection,
    SmoothSubtract,
    Mark,
    SinusoidalDisplace,
}

pub const ALL: [(&str, FieldIntrinsic); 23] = [
    ("plane", FieldIntrinsic::Plane),
    ("sphere", FieldIntrinsic::Sphere),
    ("box", FieldIntrinsic::Box),
    ("round_box", FieldIntrinsic::RoundBox),
    ("capsule", FieldIntrinsic::Capsule),
    ("finite_cylinder", FieldIntrinsic::FiniteCylinder),
    ("finite_cone", FieldIntrinsic::FiniteCone),
    ("torus", FieldIntrinsic::Torus),
    ("translate", FieldIntrinsic::Translate),
    ("rotate", FieldIntrinsic::Rotate),
    ("rigid_transform", FieldIntrinsic::RigidTransform),
    ("uniform_scale", FieldIntrinsic::UniformScale),
    ("finite_repeat_x", FieldIntrinsic::FiniteRepeatX),
    ("finite_repeat_y", FieldIntrinsic::FiniteRepeatY),
    ("finite_repeat_z", FieldIntrinsic::FiniteRepeatZ),
    ("union", FieldIntrinsic::Union),
    ("intersection", FieldIntrinsic::Intersection),
    ("subtract", FieldIntrinsic::Subtract),
    ("smooth_union", FieldIntrinsic::SmoothUnion),
    ("smooth_intersection", FieldIntrinsic::SmoothIntersection),
    ("smooth_subtract", FieldIntrinsic::SmoothSubtract),
    ("mark", FieldIntrinsic::Mark),
    ("sinusoidal_displace", FieldIntrinsic::SinusoidalDisplace),
];

pub fn classify(name: &str) -> Option<FieldIntrinsic> {
    ALL.iter()
        .find_map(|(candidate, intrinsic)| (*candidate == name).then_some(*intrinsic))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_closed_field_operation_has_exactly_one_classification() {
        assert_eq!(
            ALL.map(|(name, _)| name),
            crate::sema::intrinsics::PIXELS_FIELD_SURFACE,
            "the typed surface and exhaustive enum lowering table must stay identical"
        );
        for (name, expected) in ALL {
            assert_eq!(classify(name), Some(expected));
        }
        assert_eq!(classify("unreviewed_field_op"), None);
    }

    macro_rules! lowering_test {
        ($test:ident, $name:literal, $intrinsic:ident, $needle:literal) => {
            #[test]
            fn $test() {
                assert_eq!(classify($name), Some(FieldIntrinsic::$intrinsic));
                let graph = include_str!(
                    "../../../../tests/golden/check-pixels-field-ops/expected/field-graph.txt"
                );
                assert!(
                    graph.contains($needle),
                    "{} lacks pinned end-to-end lowering coverage",
                    $name
                );
            }
        };
    }

    lowering_test!(lowers_plane, "plane", Plane, "kind=Plane");
    lowering_test!(lowers_sphere, "sphere", Sphere, "kind=Sphere");
    lowering_test!(lowers_box, "box", Box, "kind=Box");
    lowering_test!(lowers_round_box, "round_box", RoundBox, "kind=RoundBox");
    lowering_test!(lowers_capsule, "capsule", Capsule, "kind=Capsule");
    lowering_test!(
        lowers_finite_cylinder,
        "finite_cylinder",
        FiniteCylinder,
        "kind=FiniteCylinder"
    );
    lowering_test!(
        lowers_finite_cone,
        "finite_cone",
        FiniteCone,
        "kind=FiniteCone"
    );
    lowering_test!(lowers_torus, "torus", Torus, "kind=Torus");
    lowering_test!(
        lowers_translate,
        "translate",
        Translate,
        "transform=Translate"
    );
    lowering_test!(lowers_rotate, "rotate", Rotate, "transform=Rotate");
    lowering_test!(
        lowers_rigid_transform,
        "rigid_transform",
        RigidTransform,
        "transform=Rigid"
    );
    lowering_test!(
        lowers_uniform_scale,
        "uniform_scale",
        UniformScale,
        "transform=UniformScale"
    );
    lowering_test!(
        lowers_finite_repeat_x,
        "finite_repeat_x",
        FiniteRepeatX,
        "axis=X"
    );
    lowering_test!(
        lowers_finite_repeat_y,
        "finite_repeat_y",
        FiniteRepeatY,
        "axis=Y"
    );
    lowering_test!(
        lowers_finite_repeat_z,
        "finite_repeat_z",
        FiniteRepeatZ,
        "axis=Z"
    );
    lowering_test!(lowers_union, "union", Union, "kind=HardUnion");
    lowering_test!(
        lowers_intersection,
        "intersection",
        Intersection,
        "kind=HardIntersection"
    );
    lowering_test!(lowers_subtract, "subtract", Subtract, "kind=HardSubtract");
    lowering_test!(
        lowers_smooth_union,
        "smooth_union",
        SmoothUnion,
        "kind=SmoothUnion"
    );
    lowering_test!(
        lowers_smooth_intersection,
        "smooth_intersection",
        SmoothIntersection,
        "kind=SmoothIntersection"
    );
    lowering_test!(
        lowers_smooth_subtract,
        "smooth_subtract",
        SmoothSubtract,
        "kind=SmoothSubtract"
    );
    lowering_test!(lowers_mark, "mark", Mark, "kind=Mark");
    lowering_test!(
        lowers_sinusoidal_displace,
        "sinusoidal_displace",
        SinusoidalDisplace,
        "kind=BoundedDisplace"
    );
}
