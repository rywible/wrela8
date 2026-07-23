# Pre-M3 findings: cross-feature sema sweep

Adversarial sweep of the M2 semantic checker at feature *intersections*
(M2's own `err-*` goldens, and the M2-J sweep before this one, each test
one feature at a time; wrong-accepts live where passes hand off). Probed
systematically across the matrix in the session brief: generics×flow,
generics×access, closures×moves/exclusivity, defer×flow, match/patterns×
moves, init×flow, `?`×moves, for/while×exclusivity, receiver-inference×
generics, struct-literal/enum-payload×access.

Four real bugs were found and fixed narrowly in this session (each pinned
by a new `tests/golden/*-x-*` case — see the ledger entry for the full
list): a `flow.rs` `place_type` gap that silently skipped the resource
overwrite-live/implicit-copy checks for any field reached through an
*instantiated* generic struct (`Box[DmaBlock].item`, `targs` non-empty);
`check_takeable` only forbidding a whole/field `take` when the underlying
root was `mut`-mode, never `read`-mode, so a `take` written inside a
`match`/`is` **pattern** payload (which never passes through `access.rs`'s
own `Expr::Unary(Take, ...)` check — that check only sees ordinary `take
place` expressions, not `Pattern::Take`) could steal a resource out of a
scrutinee this function only borrows; the loop fixed-point re-walk
treating a resource local first bound *inside* a loop body as if it
carried a live value across iterations (a real, load-bearing false
positive: `q = make(i)` inside any `while`/`for` body spuriously failed
`check_overwrite_live` starting on the second fixed-point pass); and a
`mut` marker accepted on a struct-literal field / enum payload / builtin
pseudo-constructor argument, which — because plan item D deliberately
never mirrors payload positions against a declared parameter — bypassed
both the implicit-copy check (`Read`-only) and `take`'s own
deinitialization, letting a resource end up with two simultaneously live
owners.

Two further findings below were judged **structural** (would need new
bookkeeping across a function boundary, not a missed condition) and are
recorded here instead of patched, per the session's own instructions.

## 1. `defer`'s body is checked once, at registration state — not against every real exit

**Where:** `crates/wrela-compiler/src/sema/flow.rs`, `walk_defer`. The
function's own doc comment already flags this as a deliberate
simplification:

> Simplification (flagged in the session report): a `defer` body's named
> places are validated once, at registration, using the state at that
> point — not re-validated at every later exit the docs (§10) describe.

**Why it's wrong:** 02-language.md §10/§3.1 describes `defer` as running
at every exit the registration statement is still in scope for (block
exit, `?`, cancellation, abandonment). If a place the `defer` body reads
is `take`n *after* registration on some path, but the function still
returns normally on that path, the deferred body would (at runtime) read
a moved-out value — that should be a compile error naming the path, the
same way a field-take-without-restore is caught for an ordinary function
exit. The checker currently never re-derives state at each exit for a
`defer`'s own body, so it misses this entirely.

**Minimized repro (WRONG-ACCEPT — currently accepted, should be rejected):**

```wrela
module xprobe

resource struct Packet:
    size: u64

    init(mut self, size: u64):
        self.size = size

pub fn use_it(take p: Packet, cond: bool) -> u64:
    defer:
        v = p.size
    if cond:
        x = take p
        return x.size
    return 0
```

`p` is taken on the `cond` branch and the function returns from inside
that branch; the `defer` registered before the `if` still (nominally)
runs on exit and would read a moved-out `p`. Currently accepted with no
diagnostic. The legal twin (no path takes `p` before the function
returns) is correctly accepted the same way, confirming the checker isn't
simply failing to model `defer` params at all — it's specifically not
threading per-exit state through the deferred body.

**Why this wasn't fixed here:** a correct fix needs to associate each
`defer` with *every* real exit reachable after its registration point (an
early `return`, every `break`/`continue` out of an enclosing loop that
exits the defer's scope, and the implicit end-of-block exit) and re-walk
the deferred body against each exit's own state — effectively turning
`defer` into a second, exit-indexed CFG query layered on the same
structural walk. That's new bookkeeping across `walk_block`/`walk_stmt`'s
control-flow plumbing, not a one-line condition fix, and risks subtly
changing the (currently simple, single-state) `walk_defer` contract other
code may rely on. Flagged for M3 (or a dedicated `defer`-hardening item)
to design properly rather than patch under time pressure.

## 2. `for`/`while`'s `take_binding` (`for take x in ...`) is parsed but never checked — loop bindings can `take` from an array the function doesn't own

**Where:** `crates/wrela-compiler/src/syntax/ast.rs`'s `ForStmt::take_binding`
is set by the parser (`crates/wrela-compiler/src/syntax/parser.rs`) and
read only by the pretty-printer
(`crates/wrela-compiler/src/syntax/printer.rs`). No sema pass —
`bodies.rs`, `access.rs`, `flow.rs`, `matches.rs` — ever reads it.

**Why it's wrong:** 02-language.md §3.2 names exactly one sanctioned way
to move resource elements out of a fixed array: `for take x in take
array`, consuming the whole array (runtime-indexed moves are forbidden
because "the analysis would depend on runtime history"). A *plain* `for x
in arr` (no `take_binding`, iterable not itself `take`n) should therefore
never let `x` be `take`n in the loop body — `x` is a per-iteration
element, not an owned value the function can move out of storage it may
only be lending (a `read`-mode parameter). But `flow.rs`'s `walk_for`
seeds the loop binding unconditionally as an ordinary owned local
(`PathState::Init`, absent from `wctx.modes`, exactly like a fresh
`call()` result), with nothing connecting its takeability back to the
array's own storage/ownership — so `check_takeable` sees an ordinary,
freely-takeable local regardless of `take_binding`, the iterable's own
mode, or whether the array is even owned by this function.

**Minimized repro (WRONG-ACCEPT — currently accepted, should be rejected):**

```wrela
module xprobe

resource struct Packet:
    size: u64

    init(mut self, size: u64):
        self.size = size

pub fn consume(take p: Packet) -> u64:
    return p.size

pub fn use_it(arr: [Packet; 2]) -> u64:
    total: u64 = 0
    for p in arr:
        total += consume(take p)
    return total
```

`arr` is an unmarked (`read`) parameter — a loan, not owned by
`use_it` — yet each iteration's `p` is freely `take`n and moved into
`consume`, which deinitializes what amounts to the *caller's* array
element. The legal twin (`p.size` read without `take`) is correctly
accepted, confirming this isn't a blanket rejection of resource-array
iteration, only of the missing ownership tie-in for `take`.

A closely related, lower-severity gap in the same area: exclusivity
(02-language.md §8.2) is deliberately scoped by `flow.rs`'s own
`walk_storing_expr` doc comment to "call-expression argument
lists/receivers only" — it does not extend to "the iterable is
exclusively held for a `for`/`while` loop's duration." Mutating the
iterable from inside the loop body (e.g. `for x in arr: fill(mut arr)`,
where `fill` mutates the very array being iterated) is accepted; this
appears to be an intentional M2 scope boundary rather than a violation of
02-language.md's own (call-centric) wording, so it is noted here for
awareness rather than classified as a wrong-accept, but M3's evaluator
work should be aware the checker provides no guarantee here.

**Why this wasn't fixed here:** correctly modeling this needs (a) an
explicit legality rule for the four `take_binding` × iterable-`take`
combinations (only `for take x in take array` should license moving `x`
out; the other three combinations need their own — currently entirely
unspecified in code — treatment), (b) tying a loop binding's storage back
to "does this function own the array" the same way a pattern payload's
`take` now correctly checks the scrutinee's root mode (this session's fix
#2), and (c) deciding what "the array is now (partially) consumed" means
for definite-init purposes after the loop, since per-element moves through
a runtime index are otherwise forbidden. That's a small design decision
plus new state, not a local patch — recorded for M3 rather than rushed.

## Other intersections probed, judged correct (no finding)

For completeness (avoiding re-probing during M3): generics×flow generally
works once finding above (fixed) is accounted for — implicit copy of a
resource-typed generic parameter, `==` on a resource-typed generic
parameter, and generic-instantiation call-site mirroring/receiver-mode
inference (including a private plain-`self` method whose only mutation of
`self` is through an instantiated generic helper call) are all correctly
checked. `?`'s evaluation of an expression that takes a local is Moved on
both the `Ok` continuation and the (shared, unforked) early-return state
by construction. A `take` appearing in only one `|` alternative of a
match arm conservatively (and correctly, per `flow.rs`'s own reasoning)
marks the *whole* scrutinee `Moved` for that arm's body, since which
alternative fired can't be told apart statically. `init`'s own
whole-`self` uses (including indirectly, via a method call on `self`
before every field is assigned) are correctly rejected; an `Err` exit
after partial field assignment is correctly accepted per §7.1's ordinary
local-cleanup rule.
