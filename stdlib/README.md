# wrela standard library

Wrela source that ships with the toolchain under two reserved import
aliases ([02-language.md](../docs/language/02-language.md) §2 / §2.1):

- **`core`** — prelude types, collections, formatting, time, SIMD ops
  ([05-library.md](../docs/language/05-library.md)).
- **`drivers`** — `@driver` modules only, for queue device contracts
  ([06-machine.md](../docs/language/06-machine.md) §6). Thin devices
  (console, clock, entropy) are not `@driver`s and do not live here.

This tree is not a complete driver set for every machine-v1 row; only
queue devices earn modules under `drivers/` (today: virtio-blk as
`drivers.blk` once relocated; input/display when the pixels rung
schedules them).

## Layout

```text
stdlib/
  core/       # reserved alias `core`
  drivers/    # reserved alias `drivers` — `@driver` modules only
  tests/      # comptime suite root (M16; not an import alias)
```

### `core/`

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

### `drivers/`

The `drivers` package root is `stdlib/drivers/`. Resolution mirrors
`core`: sibling `stdlib/drivers/` next to a package root wins if
present; else the toolchain tree. A file at `stdlib/drivers/blk.wr`
declares `module blk` and is imported as
`from drivers.blk import BlkDriver`. This directory holds **only**
`@driver` modules — never console/clock/entropy helpers.
