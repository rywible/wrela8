# wrela standard library

Wrela source for the `core` package: prelude types, collections, formatting,
time, SIMD ops ([05-library.md](../docs/language/05-library.md)), and the
machine's complete driver set ([06-machine.md](../docs/language/06-machine.md)
§6). Ships with the toolchain; bound in every manifest as the `core`
dependency.

Empty until the compiler can check it — stdlib code lands behind the same
golden/ledger discipline as everything else, never ahead of it.
