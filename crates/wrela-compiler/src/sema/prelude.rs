//! The M2 builtin prelude (plans/M2.md decision 5): scalar types, the
//! fixed `Option`/`Result` vocabulary, `panic`, and the handful of
//! standard names the literal rules require (`Static`, `Str`, `Bytes`).
//! This is **not** the stdlib — the real one replaces this hardcoded
//! surface at its own milestone, which is also why the doc corpus (which
//! names stdlib types) is not sema-checked in M2 (plans/M2.md decision 5).
//!
//! Three small fixed arrays, one per source clause, rather than one
//! flat list — each documents *why* its names are in scope with no
//! import (02-language.md §2: "A fixed prelude is always in scope").
//! `is_builtin` is a linear scan across all three, which is dumb and
//! plenty fast for this table's size (decision 4: no premature
//! cleverness).

/// Scalar type names (02-language.md §6.1).
const SCALARS: &[&str] = &[
    "bool", "u8", "u16", "u32", "u64", "usize", "i8", "i16", "i32", "i64", "isize", "f32", "f64",
    "char", "unit", "never",
];

/// The `Option`/`Result` vocabulary (02-language.md §2).
const OPTION_RESULT: &[&str] = &["Option", "Some", "None", "Result", "Ok", "Err", "panic"];

/// The minimum standard surface the literal rules require
/// (02-language.md §1.1, plans/M2.md decision 5).
const LITERAL_SURFACE: &[&str] = &["Static", "Str", "Bytes"];

/// The `@image` builder surface's own bare (call-by-name) prelude names
/// (plans/M4.md item B, decision 5, 05-library.md §9): `Image` itself,
/// its two supporting helpers (`RestartIntensity`/`seconds`), and the
/// two prelude enums `builtin_enum_variants` below recognizes as a
/// bare-name *base* (`Target.wrela_machine_v1`, `Restart.OneForOne`) —
/// every one of these must resolve here first (`symbols::resolve` runs
/// before `sema::bodies` ever sees a call/field expression), or
/// `sema::bodies`'s own intrinsic-recognition arms would never be
/// reached at all. `img`/`decl`'s own method names (`device`, `driver`,
/// `actor`, `pool`, `dma_pool`, `supervise`, `check_layout`, `seal`,
/// `handle`) are never bare identifiers — they only ever appear after a
/// `.` — so they need no entry here.
const IMAGE_BUILDER: &[&str] = &["Image", "RestartIntensity", "seconds", "Target", "Restart"];

/// The actor/async surface's own bare prelude names (plans/M6.md item A,
/// 02-language.md §9): `Actor` (a type name, `Actor[T]` — `sema::types::resolve_named`'s
/// own new arm), `group` (`with group(...)`'s constructor-shaped call
/// name), and the two clock intrinsics `now`/`ms` (decision 11,
/// `sema::bodies`'s own `now`/`ms`/`seconds` dispatch). Every other new
/// M6 name (`CallError`, `Rejected`, `Admission`, `Peer`, `Group`,
/// `Instant`, `Duration`) is never spelled by source directly — only ever
/// synthesized by the compiler (await/send composition, `with group`'s
/// own binder, `now`/`seconds`'s own return type) — so none of them need
/// an entry here, mirroring `Duration`/`ImageDecl`'s own precedent above.
const ACTOR_SURFACE: &[&str] = &["Actor", "group", "now", "ms"];

/// Is `name` one of the fixed prelude names above?
pub fn is_builtin(name: &str) -> bool {
    SCALARS.contains(&name)
        || OPTION_RESULT.contains(&name)
        || LITERAL_SURFACE.contains(&name)
        || IMAGE_BUILDER.contains(&name)
        || ACTOR_SURFACE.contains(&name)
}

/// The `@image` builder surface's own fixed prelude enums (plans/M4.md
/// item B, decision 5): 05-library.md §9 names `Target`/`Restart` as
/// prelude vocabulary the builder intrinsics use, but neither ships as
/// real stdlib source in M4 — recognized directly, exactly like this
/// file's own `Option`/`Result` (`sema::bodies::check_field_expr`'s
/// fallback for a bare `Enum.Variant` reference). Returns the enum's
/// variants in fixed declaration order (also the order
/// `sema::bodies::check` injects into `TypedProgram::enums` so
/// `eval::interp::variant_index` indexes them identically to a real
/// module-declared enum, needing no evaluator-side special case).
pub fn builtin_enum_variants(name: &str) -> Option<&'static [&'static str]> {
    match name {
        // 02-language.md §12.1's worked example: `Target.wrela_machine_v1`.
        "Target" => Some(&["wrela_machine_v1"]),
        // 02-language.md §11's fixed restart-policy vocabulary
        // (`OneForOne`, `OneForAll`, `RestForOne`); the
        // virtio-storage.wr worked example additionally exercises
        // `Restart.OneForAll` (`img.supervise(strategy=Restart.OneForAll, ...)`).
        "Restart" => Some(&["OneForOne", "OneForAll", "RestForOne"]),
        _ => None,
    }
}
