# wrela standard library

Wrela source for the `core` package: prelude types, collections, formatting,
time, SIMD ops ([05-library.md](../docs/language/05-library.md)), and the
machine's complete driver set ([06-machine.md](../docs/language/06-machine.md)
§6). Ships with the toolchain; bound under the reserved import alias `core`
([02-language.md](../docs/language/02-language.md) §2 / §2.1).

## Layout

The `core` package root is `stdlib/core/`. A file at
`stdlib/core/io_error.wr` declares plain `module io_error` and is imported
as `from core.io_error import IoError`. The loader strips the `core` alias
and maps the remaining segments onto this directory (sibling
`stdlib/core/` next to a package root wins over the toolchain tree; see
`crates/wrela-compiler/src/loader.rs`).

Names that [02 §2](../docs/language/02-language.md) lists as the fixed
prelude (`Option` / `Result` / `panic`) stay always in scope with no
import — they are language builtins, not modules in this tree
(plans/M9.md item I). The five builder/hardware enums (`Target`,
`Restart`, `BootError`, `DriverMode`, `CompletionOutcome`) live here as
ordinary wrela and are auto-visible without an import for golden
stability; explicit `from core.<mod> import …` still compiles them
through the ordinary pipeline. Time constructors
(`ns`/`us`/`ms`/`seconds`/`minutes`/`hours`, plus `Duration`/`Instant`)
keep the same auto-visibility (item E decision 300 / item I decision
470). Everything else in this tree is imported.
