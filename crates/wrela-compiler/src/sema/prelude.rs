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
/// `String` joined at plans/M9.md item C1 (`String[..N]`, 02 §6.2).
const LITERAL_SURFACE: &[&str] = &["Static", "Str", "Bytes", "String"];

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
const IMAGE_BUILDER: &[&str] = &[
    "Image",
    "RestartIntensity",
    // plans/M9.md item E: `seconds` stays prelude-visible (RestartIntensity
    // goldens) while its implementation lives in `stdlib/core/time.wr`.
    "seconds",
    "Target",
    "Restart",
    // plans/M7.md item G, decision 18: 03 §7's driver-mode const generic.
    "DriverMode",
];

/// The actor/async surface's own bare prelude names (plans/M6.md item A,
/// 02-language.md §9): `Actor` (a type name, `Actor[T]` — `sema::types::resolve_named`'s
/// own new arm), `group` (`with group(...)`'s constructor-shaped call
/// name), and `now` (still a sealed intrinsic) plus the Duration
/// constructors that item E moved to `stdlib/core/time.wr` while keeping
/// the names prelude-visible. `Instant`/`Duration` type names join so an
/// annotation resolves with no import (the constructors' return types).
///
/// `pool` joins `group` for exactly the same reason, and for no other
/// (plans/M8.md item R, decision 16): 02-language.md §10 names **two**
/// intrinsic `with` scopes, and `symbols::resolve` runs before
/// `sema::bodies::check_with` ever sees the constructor — so without an
/// entry here the scoped-pool half of the language's own two-construct
/// region model reported `error[name]: unknown name \`pool\`` (a typo
/// diagnostic for an intrinsic the parser has a dedicated arm for,
/// `syntax::parser`'s own `pool(...)` keyword case) instead of the
/// fail-closed `error[unimplemented]` `check_with` already had written
/// and could never reach. Being in scope is not being constructible: the
/// name resolves so the honest rejection can name it.
const ACTOR_SURFACE: &[&str] = &[
    "Actor", "group", "pool", "now",
    // plans/M9.md item E: Duration constructors + types. Implementations
    // are ordinary wrela in `stdlib/core/time.wr`; these entries are
    // resolution only (item I owns whether auto-visibility survives).
    "ns", "us", "ms", "minutes", "hours", "Duration", "Instant",
];

/// The typed-MMIO register wrappers (plans/M7.md item B, 03-hardware.md
/// §2's own worked example: `@offset(0x060) interrupt_status:
/// ReadOnly[u32]`). Only ever legal as an `@layout(mmio)` field's type —
/// `sema::types::check_layouts` rejects them anywhere else *inside* a
/// layout, and their *access* rules (volatile read/write, claim
/// partitioning) are plans/M7.md item C, which is why nothing but the
/// name and the wrapped type exists yet. Listed here so an ordinary
/// annotation naming one resolves at all (`symbols::resolve` runs before
/// `types::declare`, and an unresolvable name never reaches the type
/// resolver's own arm for these).
const MMIO_WRAPPERS: &[&str] = &["ReadOnly", "WriteOnly"];

/// The one marked-value mechanism's three policy names (plans/M7.md item
/// H2a, 03-hardware.md §8 / 05-library.md §6): `Untrusted` (live),
/// `Validated` / `Secret` (recognized so the mechanism can refuse them by
/// name as out of M7's honest-scope line, rather than as unknown types).
/// Resolution and sealing live in `sema::types::resolve_named` /
/// `sema::bodies`; being in scope is not being constructible.
const MARKED_VALUES: &[&str] = &["Untrusted", "Validated", "Secret"];

/// Fail-closed type names that resolve only to refuse by plan item
/// (plans/M7.md item H2a → E4): recognized so `symbols::resolve` does
/// not report `unknown name` before `types::resolve_named` can name the
/// owning item.
const FAIL_CLOSED_TYPES: &[&str] = &[];

/// plans/M7.md item E1: `BootError` (03-hardware.md §1/§9's own `init`
/// failure vocabulary) and `VirtQueue` (03 §4's queue type). Both must
/// resolve with no import — an annotation naming one is how a driver is
/// written. `BootError`'s one variant is in `builtin_enum_variants`;
/// `VirtQueue[..N]`'s bound argument is `sema::types::resolve_named`.
// plans/M7.md item E3: `Receipt` (03 §5) and `IoError` (`reject`'s
// `error=` / `IoCompletion.status`'s Err arm). `Receipt[P]`'s argument
// is resolved in `sema::types::resolve_named`; `IoError`'s variants are
// in `builtin_enum_variants`.
// plans/M7.md item E4: `IoCompletion[P]` — 03 §3/§8's completion value
// (`payload`, `status`, `written_len: Untrusted[usize]`).
// plans/M8.md item G, decision 17: `CompletionOutcome` (03-hardware.md §9)
// — the outcome `VirtQueue.recover` reports for a receipt resolved through
// 03 §5's `Recovery` state. A prelude enum like `BootError`/`IoError`, and
// in scope as a name so an annotation (`-> CompletionOutcome`, a field, a
// `match` arm's explicit `CompletionOutcome.Unknown`) resolves with no
// import.
const HARDWARE_SURFACE: &[&str] = &[
    "BootError",
    "VirtQueue",
    "QueuePermit",
    "QueueOp",
    "Receipt",
    // plans/M9.md item A2: `IoError` moved to `stdlib/core/io_error.wr`
    // and is imported (`from core.io_error import IoError`). Kept out of
    // this array deliberately — a half-moved name would be two sources
    // of truth.
    "IoCompletion",
    "CompletionOutcome",
];

/// plans/M7.md item G: 03-hardware.md §6's ISR/ordinary channel. Prelude
/// so an annotation resolves with no import (the worked example's
/// `from runtime.interrupt import InterruptCell` is aspirational — this
/// machine has no stdlib module for it yet, and the type is a builtin
/// like `Actor[T]`, not a capability).
const INTERRUPT_CELL: &[&str] = &["InterruptCell"];

/// plans/M7.md item G: `wake(...)` — 03 §6's bottom-half wake. Bare
/// call name, always in scope (no import).
const WAKE: &[&str] = &["wake"];

/// Is `name` one of the fixed prelude names above?
///
/// The one entry with no array of its own here: 03-hardware.md §1's four
/// capability type names (`DeviceCap`/`Mmio`/`IrqCap`/`DmaPool`), which
/// live in `eval::image_checks`'s own `CAPABILITY_TYPES` because the
/// image checker and `layout::build_boot_init_calls` already shared that
/// list before this file needed it (plans/M7.md item A: one list, read by
/// every consumer, never copied). They are prelude names for the same
/// reason `Actor` is — an annotation naming one must resolve with no
/// import — and for no other: being in scope is not being constructible
/// (`sema::types::validate_capability_types` and `sema::bodies`' own
/// forgery arms).
pub fn is_builtin(name: &str) -> bool {
    SCALARS.contains(&name)
        || OPTION_RESULT.contains(&name)
        || LITERAL_SURFACE.contains(&name)
        || IMAGE_BUILDER.contains(&name)
        || ACTOR_SURFACE.contains(&name)
        || MMIO_WRAPPERS.contains(&name)
        || MARKED_VALUES.contains(&name)
        || FAIL_CLOSED_TYPES.contains(&name)
        || HARDWARE_SURFACE.contains(&name)
        || INTERRUPT_CELL.contains(&name)
        || WAKE.contains(&name)
        || crate::eval::image_checks::is_sealed_authority_type_name(name)
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
        // plans/M7.md item E1: 03-hardware.md §1/§9's own `init` failure
        // vocabulary. One unit variant — enough to spell `Err(BootError.Failed)`
        // (and enough for a vacuity control that returns `Err` from `init`);
        // richer taxonomy is not load-bearing at E1 because every live
        // transition on this machine is a build-time fact that cannot fail
        // at runtime (decision 14).
        "BootError" => Some(&["Failed"]),
        // plans/M7.md item E3: 03-hardware.md §5 / the reference driver's
        // `reject(error=IoError.OutOfRange)`. Moved to
        // `stdlib/core/io_error.wr` at plans/M9.md item A2 — no longer a
        // prelude enum. `IoCompletion.status`'s Err arm still names the
        // type as `Type::Named("IoError")` in the compiler; source that
        // constructs `IoError.OutOfRange` imports it.
        // =================================================================
        // plans/M7.md item G, decision 18: 03-hardware.md §7's driver mode
        // const-generic vocabulary (`BlkDriver[DriverMode.Irq]`).
        // =================================================================
        "DriverMode" => Some(&["Irq", "Poll"]),
        // =================================================================
        // plans/M8.md item G, decision 17: 03-hardware.md §9's own enum,
        // verbatim and in its declaration order —
        //
        //     enum CompletionOutcome:
        //         Completed
        //         NotCompleted
        //         Unknown
        //
        // Declaration order is load-bearing twice over: it is the tag
        // `lower::variant_index` compares (via `TypedProgram::enums`), and
        // it is the order `matches::shape_of` reports missing arms in.
        // =================================================================
        "CompletionOutcome" => Some(&["Completed", "NotCompleted", "Unknown"]),
        _ => None,
    }
}
