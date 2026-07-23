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

## 1. `defer`'s body is checked once, at registration state — not against every real exit — **FIXED**

**Fixed** (a later session, pre-M3 shoring continuation): `sema/flow.rs`
now threads a per-body `DStack` — a true call stack of registered
`&DeferStmt`, in registration order — through the whole CFG walk.
`Stmt::Defer` only pushes onto it (no check at registration anymore);
`walk_block` checks and pops the defers it registered directly against
its own normal-completion state right before returning, and every
`return`/`?`/`break`/`continue` reached after a defer's registration
re-validates the currently active defers in place, via a new
`check_active_defers`, which reuses the same `walk_expr`/`walk_block`
machinery an ordinary statement is checked with (no separate
place-collection pass). Defers active at one exit are walked in
*reverse* registration order (02-language.md §10), threading one running
state clone through each one's own body-walk in turn, so a
later-registered/earlier-run defer's own effects (a `take`) are visible
to whichever earlier-registered/later-run defer is checked next.
`return`/`?` check the whole active stack (they exit the entire
function); `break`/`continue` check only the slice registered since the
nearest enclosing loop was entered — a defer outside the loop stays
pending for a later real exit. A defer inside a `for`/`while` body
registers/checks/pops fresh on every fixed-point re-walk of that body,
matching how the fixed point already re-walks every other statement.
This session's own minimized repro below is now rejected (pinned as
`golden/err-defer-moved-at-exit`); the reverse-order-sequencing case
(`golden/err-defer-taken-by-defer`) and the legal twins
(`golden/check-defer-exits`) are pinned alongside it. See
`ledger/ledger.toml`'s `values.teardown.defer-valid-at-exit` clause for
the full accounting, including the one wrong-reject question checked
against 02-language.md §10's own "a deferred action that needs a
moved-and-returning resource simply waits for it" sentence (none found —
that sentence is runtime cleanup-graph scheduling, 04-compiler.md §4,
not a static legality question, and doesn't collide with the dumb static
rule implemented here for anything the M2 corpus expresses).

**Where (historical, pre-fix):** `crates/wrela-compiler/src/sema/flow.rs`, `walk_defer`. The
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

## 2. `for`/`while`'s `take_binding` (`for take x in ...`) is parsed but never checked — loop bindings can `take` from an array the function doesn't own — **FIXED**

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

**Fixed** (a later session, pre-M3 shoring continuation): `access::check_for`
now requires `take_binding` and the iterable expression's own `take` to
agree — 02-language.md §3.2 names exactly one sanctioned combination
(`for take x in take array`); a `take` binding over a non-`take` iterable
is `error[access]` (this section's own minimized repro is now rejected;
pinned as `golden/err-x-for-take-element-borrowed`), as is the syntactic
mismatch of a `take`-marked for-loop head whose iterable itself isn't
`take`n (`golden/err-x-for-take-borrowed`) and the mirror-image mismatch,
a `take`n iterable with a plain binding (`golden/err-x-for-take-mismatch`).
A *plain* (non-`take`) binding over a resource-typed element is now bound
as a `read`-mode loan rather than an unrestricted owned local, closing
exactly the gap this repro exercised. `flow.rs`'s `walk_for` already
deinitialized a `take`n iterable correctly via the ordinary take-expression
path (item (c) below turned out to already be handled, verified rather
than built) — the sanctioned consuming form and its post-loop `Moved`
state are pinned by `golden/check-x-for-take-consume` and
`golden/err-x-for-take-reuse`. See `ledger/ledger.toml`'s
`values.resource.move-spells-take` clause for the full accounting.

The closely-related, lower-severity exclusivity gap noted just above
(mutating the iterable from inside the loop body, e.g. `for x in arr:
fill(mut arr)`) is **not** addressed by this fix and remains exactly the
documented M2 scope boundary — `flow.rs`'s `walk_storing_expr` still scopes
exclusivity to call-expression argument lists/receivers only, not "the
iterable is exclusively held for the loop's duration." M3's evaluator work
should still be aware the checker provides no guarantee there.

Superseded design notes (kept for the record): correctly modeling this
needed (a) an explicit legality rule for the four `take_binding` ×
iterable-`take` combinations (only `for take x in take array` should
license moving `x` out; the other three combinations needed their own —
previously entirely unspecified in code — treatment: implemented as the
agreement check above), (b) tying a loop binding's storage back to "does
this function own the array" the same way a pattern payload's `take`
correctly checks the scrutinee's root mode (implemented as the
resource-element read-loan rule above), and (c) deciding what "the array
is now (partially) consumed" means for definite-init purposes after the
loop, since per-element moves through a runtime index are otherwise
forbidden (turned out to already be handled by the existing take-expression
path, per the paragraph above).

## 3. A place-postfix ambiguity inside an embedded (bracket-suppressed) suite lets a `take`/`mut` place silently swallow the *next*, unrelated statement's leading token — **FIXED**

**Fixed** (a later session, pre-M3 shoring continuation): the root cause was
the lexer, not the parser — `()[]{}` fully suppressing NEWLINE/INDENT/DEDENT
gave a multi-statement embedded suite no separator token at all, so no local
patch to `parse_unary`/`parse_call_args`'s grammar could disambiguate it (see
"why this wasn't fixed here" below, which already said as much). Fixed by
teaching the lexer **layout islands**: a `:` immediately followed by a
newline while bracket depth > 0 resumes real NEWLINE/INDENT/DEDENT tracking
for exactly that suite, in a fresh indentation sub-stack, until the suite's
own indentation closes it or an enclosing bracket closes without it ever
dedenting on its own line first (`crates/wrela-compiler/src/syntax/lexer.rs`,
its module doc comment, and ledger clause `syntax.lexer.layout-islands`,
which has the full accounting). The parser's `parse_stmt_suite` `Newline`
branch now handles every multi-line embedded suite the same way it always
handled a top-level one; the no-`Newline` inline branch
(`parse_inline_stmt_seq`) is narrowed to exactly one statement — reachable
only for a `:` followed by real content on the same physical line, since
every `:`-newline now opens an island or an ordinary block instead — with a
second statement rejected outright (`error[parse]: an embedded suite on one
line holds one statement; use an indented block`,
golden/err-inline-suite-two-stmts) rather than guessed at. The minimized
repro below now parses cleanly (no `error[parse]` at all; the two `x`/`y`
locals it never defines produce ordinary `error[name]: unknown name`
instead), and the sema-roundtrip oracle (`cargo xtask fuzz sema`, seed=1,
2_000_000 iterations — the exact campaign that previously failed at
iteration 76076) is clean. See the ledger clause for the full verification
record (golden/lex-layout-island, golden/check-embedded-suite, and every
existing golden — including ast-virtio's own two multi-statement embedded
closures — unchanged byte-for-byte).

**Where:** `crates/wrela-compiler/src/syntax/parser.rs`, `parse_unary`'s
`take` case and `parse_call_args`'s mode-marked argument parsing, both of
which parse a `mut`/`take` operand through the general expression grammar
(`parse_unary` -> `parse_postfix`, which accepts a trailing `(args)` as a
call) and only check `ast::is_place_expr` — a *shallow*, non-recursive
match on `Name`/`Field`/`Index` — after the fact. This is deliberate and
correct on its own: it is what lets `mut make_pair().a`/`take foo().field`
parse successfully as a syntactically call-shaped "place" and be rejected
later, with a better diagnostic, by `access.rs`'s own *recursive*
`is_full_place` (golden/err-access-nonplace pins exactly this).

**Not a sema bug — found by this session's own required deep-fuzz gate,
not by the two bugs this session was scoped to fix.** Recorded here
(structural, not patched) rather than in the ledger, per this file's own
convention.

**Why it's wrong:** `crates/wrela-compiler/src/syntax/parser.rs`'s own
module doc comment documents that `()[]{}` fully suppress
`NEWLINE`/`INDENT`/`DEDENT` in the lexer, so a suite embedded inside an
enclosing call's argument list has *no separator token at all* between
its statements — `parse_inline_stmt_seq` relies entirely on "the previous
statement's expression grammar naturally stops here" to find each
boundary. That assumption breaks when one statement ends in a bare place
(a `Name`) immediately followed — with nothing between them but
whitespace the lexer has no reason to treat specially inside a suppressed
region — by a wholly separate *next* statement that happens to start with
`(`: the general postfix parser (used for `take`'s/`mut`'s operand,
exactly the same as any other expression) cannot tell "the next
statement's own leading paren" apart from "a call on this place," and
greedily consumes it as one, silently absorbing tokens that belong to an
entirely different statement.

**Minimized repro** (found via `cargo xtask fuzz sema` deep, seed=1,
iteration=76076, mutating golden/err-x-closure-take-twice; confirmed
reproducible identically on the pre-session base commit, i.e. **not**
introduced by either sema fix in this session):

```wrela
module examples.err_x_closure_take_twice

resource struct Packet:
    size: u64

    init(mut self, size: u64):
        self.size = size

pub fn run(body: fn() -> u64) -> u64:
    return body()

pub fn use_it(take p: Packet) -> u64:
    a = run(||:
        x = take p
        (((((((((eg + inv) + call_result) + f) + g) + closure(1)) + block_closure(1)) + pair.len()) + one.len()) + items.len()) + gu
        x.size)
    b = run(||:
        y = take p
        y.size)
    return a + b
```

This parses fine as written (three back-to-back closure-body statements:
`x = take p`, the huge `+`-chain ending in a bare `gu`, then `x.size`).
Pretty-printing it and reparsing (the sema-roundtrip oracle,
sema.check.roundtrip-stable) fails: `error[parse]: operand of \`take\`
must be a place expression (name, field, index) at 14:129`. Root cause
verified by inspecting the reparsed token stream: the pretty-printer
re-renders the long `+`-chain with explicit (redundant, but harmless on
their own) grouping parens around it, purely cosmetic; on reparse, the
first closure's `take p` — with no separator token between `p` and what
follows, since the whole closure sits inside `run(`'s open paren, which
suppresses layout entirely per the module doc comment — sees `p`
immediately followed by that next statement's own leading `(`.
`parse_postfix` (used for `take`'s operand, same as any other expression)
reads `p(` as a call and consumes the *entire* parenthesized chain up to
its own matching `)` as that call's single argument; only then does
`take`'s `is_place_expr` check see the result (`Call(p, [...])`) and
correctly reject it — but at the wrong place, having already eaten tokens
that belonged to a separate statement, which is why the diagnostic cites
a position deep inside what was meant to be the *next* statement's own
expression.

**Why this wasn't fixed here:** a first attempt — restricting `take`'s
(and the mode-marked call-argument's) operand to a dedicated
call-free postfix parser (place = name + any depth of `.field`/`[index]`,
no `(args)`) — regressed golden/err-access-nonplace: `fill(mut
make_pair().a)` relies on parsing *through* the call syntactically (so
`access.rs`'s richer, recursive `is_full_place` check can reject it with
a clearer diagnostic later) rather than failing at the parser level; a
call-free operand grammar breaks that entirely, since it stops parsing at
`make_pair` and leaves `().a` as unconsumed trailing tokens, turning a
clean `error[access]` into a garbled `error[parse]: expected \`)\`, found
\`(\`` for a program the checker is *supposed* to accept as far as
parsing goes. Reverted rather than shipped once this regression surfaced
in `cargo xtask golden`. The underlying tension — a `mut`/`take` operand
must still parse *through* a non-place call so semantic passes can give
the better diagnostic, yet a bare place immediately before an unrelated
next statement's leading `(` must *not* swallow it — cannot be resolved by
locally restricting one call site's grammar; it needs either a real
statement separator survivable inside a bracket-suppressed suite (the
grammar has none today — no semicolon or equivalent exists anywhere in
02-language.md) or a redesign of how `parse_inline_stmt_seq` finds
boundaries in that context. That is new grammar design, not a local
patch — recorded for a dedicated session rather than rushed, exactly the
reasoning finding #1 above already used for `defer`.

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
