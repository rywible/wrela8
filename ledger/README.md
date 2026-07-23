# Spec ledger

`ledger.toml` maps every normative clause of `docs/language/` to the tests
that enforce it. It measures **coverage of the spec, not of the code** —
the implementation is disposable; this file and the docs are not.

Format:

```toml
[[clause]]
id = "area.topic.rule"          # stable, kebab/dot, never renamed
doc = "02-language.md#1"        # file (checked) + section (informative)
status = "test"                 # or "gap"
tests = ["golden/lex-basic"]    # paths under tests/, required when "test"
note = "optional context"
```

Rules (enforced by `cargo xtask ledger`):

- every clause id is unique; `doc` names a real file;
- `status = "test"` requires at least one existing test path;
- `status = "gap"` is legal and *visible* — it is the debt register, and
  shrinking it is the project's progress metric;
- a golden expectation file may change only alongside the clause that
  justifies the change (cite the id in the commit message).

When a doc rule lands or changes, add or update its clause in the same
commit. A rule with no clause does not exist.
