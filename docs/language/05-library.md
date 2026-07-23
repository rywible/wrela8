# Library contracts

The minimum semantic contracts of standard types the language invariants
depend on. Implementations may be ordinary wrela, generated code, or sealed
target code; an alternative library may rename wrappers but must implement
the same typed effects, ownership transitions, bounds, and failure points —
and may not add a hidden failure, copy, allocation, authority, or suspension.

## 1. `Option`, `Result`, conversion

`Option[T] = Some(T) | None`; `Result[T, E] = Ok(T) | Err(E)`; ordinary enum
ownership rules. `?` on `Result` yields `T` or returns `Err` from the
enclosing `Result` function, converting through the target error type's
`from(take source)` when the types differ ([02 §7.4](02-language.md)) — no
conversion chains. `?` on `Option` propagates `None` only in an
`Option`-returning function; `ok_or(error)` converts explicitly.

For `mut Option[T]`, `take()` returns the owned `Option[T]` and leaves
`None` — the standard way to move a payload out of a runtime-selected slot.

## 2. Actor handles and calls

`Actor[T]` identifies one image actor instance and exposes only `T`'s public
actor methods. It is minted and installed by image construction; it cannot
appear in a message, reply, collection, or mutable input, and no numeric
actor ID is source-visible.

`Static[T]` is a copyable read-only handle to immutable image data, minted by
image construction or literals. It may cross actor boundaries, lends
`read T` for a call, and exposes no address or mutation. A public actor
signature that accepts runtime-sized immutable bytes says `Static[Bytes]`.

Every actor call is a sealed single-resolution awaitable; awaiting
consumes it. Results compose the declared type with `CallError`
([02 §9.4](02-language.md)):

```text
declared R            -> Result[R, CallError[never]]
declared Result[T, E] -> Result[T, CallError[E]]
```

`NotAdmitted` carries the reason (`Full | Restarting | StaleRequest |
DeadlineUnmeetable`) and hands back the call's `take` arguments in
declaration order; every other outcome consumed them. `PeerFailed` carries a
static actor identity, non-wrapping supervision epoch, and a bounded failure
category — never frame references, secrets, or unbounded text. Admission and
reply resolution are deterministic record/replay events.

`send actor.method(...)` has type `Result[unit, Rejected[...]]`, an ordinary
owned value whose error carries the moved payloads back; where admission is
build-proven the error type is `never` and the send stands as a statement.

## 3. `Completion` and `Receipt`

`Completion[T]` is a sealed single-resolution awaitable resource: resolved
exactly once with ownership of `T`; duplicate resolution is abandonment; wake
is level-triggered and idempotent.

`Receipt[P]` and the signature-inferred handoff convention are specified in
[03 §5](03-hardware.md). Awaiting a committed receipt yields
`IoCompletion[P]` — owned `payload: P` returned before
`status: Result[unit, IoError]` is inspected.

## 4. Groups, race, slots

`group(deadline=..., budget=..., capacity=...)` is the one suspend-safe
structured-concurrency scope ([02 §9.5](02-language.md)): it mints a
lineage every call in its dynamic extent inherits ambiently, owns up to
`capacity` child slots, and joins or cancels every child on exit. `start`
launches a named function as a child under the ambient lineage; `join_all`
returns results in start order.

`race(a, b, ...)` is a sealed contract, not grammar: it build-proves and
reserves all child slots before evaluating any alternative, returns a
generated closed sum naming the winner, and cancels and fully tears down
every loser (including in-flight device work, via receipts) before
returning. Simultaneous readiness resolves by argument order.

A service in-flight slot from `SlotMap.reserve()` exposes
`slot.resolve(take receipt) -> Result[Outcome, E]`: it parks the enclosing
actor-call reply on the slot, **ends the turn**, and wires the receipt's
exactly-once resolution to a generated later turn that fills the slot and
resolves the reply. The slot is reclaimed on every path. This is the sealed
face of the service slot idiom; a second synchronization vocabulary for the
same purpose is non-conforming.

## 5. Time

`Duration` is a checked nonnegative span of nanoseconds; constructors `ns`,
`us`, `ms`, `seconds`, `minutes`, `hours` are ordinary phase-neutral
functions. Arithmetic and comparison follow the standard method conventions
(§8) with ordinary checked overflow.

`Instant` is an opaque monotonic point: totally ordered, `Instant +
Duration` checked, differences yield `Duration`, never serialized as wall
time. `now() -> Instant` is a sealed effect available only to runtime code —
forbidden in comptime and ISR context — and is recorded/replayed. Targets
guarantee a monotonic horizon beyond every declared deadline. Wall time is a
separate capability, never used for scheduling.

## 6. Bytes, formatting, validation

Types implementing the static `Format` contract declare a compile-time
`max_formatted_len(spec)` and a writer that cannot exceed it; exceeding a
proven bound is abandonment. `f"..."` sums those bounds into `String[..N]`.
`Secret` has no `Format`. `panic` writes through a target-reserved
allocation-free formatter with a fixed maximum — a panic message that cannot
fit is a build error.

`Bytes.read_wire[W](offset)` decodes only `@layout(wire)` types, checking the full
encoded extent, and returns `Result[W, WireError]`. `Untrusted[T]` (taint
in), `Validated[F, T]` (proof out), and `Secret[T]` (never out) are the
three instances of the one marked-value mechanism ([03 §8](03-hardware.md)):
`Untrusted` gates device/external control values until checked-narrowed;
`Validated[F, T]` is minted only by the declared
`FormatValidator[F, T].validate(data) -> Result[Validated[F, T], F.Error]`
and unwrapped with `into_value(take self)`; `Secret` admits only
secret-preserving transforms. The compiler enforces a wrapper where an API
requires it; it does not claim the validator matches a prose format.

## 7. Collections and iteration

`[T; N]` supports constant-index moves; runtime-indexed `take` is illegal.
Whole-array consumption:

```text
[T; N].map_take(fn(take T) -> U) -> [U; N]
[T; N].try_map_take(fn(take T) -> Result[U, E]) -> Result[[U; N], E]
```

`map_take` consumes each element exactly once; `try_map_take` unwinds
constructed outputs and reclaims remaining inputs on `Err` (both element
types must be auto-reclaimable; protocol resources need an explicit loop).

`List[T, ..N]` and `SlotMap[T, ..N]` never exceed `N`. `SlotMap` mints a
fresh non-wrapping instance ID; `SlotMap.Key` is a data struct over
`(map_id, index, generation)`; `remove` retires a slot's generation before
reuse and never wraps (`GenerationExhausted` on exhaustion); lookups
validate all three fields, so foreign and stale keys miss instead of
aliasing. Graph-shaped data stores keys, not references.

Iteration follows the two access forms of the language:

- **Owned iteration** with `for`: `for key in map.keys():` yields copyable
  keys and `for take x in take array:` consumes — both may `await` in the
  body.
- **Lent iteration** with non-escaping closures, which cannot suspend:

```text
list.each(body: fn(read T))          map.get(key, fn(read T) -> R)  -> Option[R]
list.each_mut(body: fn(mut T))       map.get_mut(key, fn(mut T) -> R) -> Option[R]
map.each_pair(body: fn(Key, read T))
```

## 8. Method conventions and deriving

Operators desugar structurally to the named methods of
[02 §7.4](02-language.md); this chapter fixes the standard shapes a type
declares to participate:

```wrela
fn add(read self, right: Self) -> Self
fn subtract(read self, right: Self) -> Self
fn less_than(read self, right: Self) -> bool   # a strict total order
fn from(take source: Source) -> Self            # consumed by `?`
```

Operands are `read`; operator expressions never move or mutate; `a += b` is
`a = a.add(b)` with the destination evaluated once. Structural `==`/`!=` on
every data type are compiler-generated; core scalar operators never desugar.
`deriving(Format)` generates the bounded formatting contract of §6;
`deriving(From)` requires exactly one variant/field and generates the `from`
that `?` consumes. `Duration` declares `add`/`subtract`/`less_than` with
ordinary checked overflow.

## 9. Image builder intrinsics

`Image`, `group`, actor admission, and pool construction are
compiler-recognized intrinsics even when a package supplies their surface:

- `Image(name, target)` — comptime-only, produces one resource builder;
  `seal(take builder)` succeeds only when every declaration is fully bound.
- `img.device[D](transport=..., required_features=...)` — a build contract;
  boot still verifies the real device.
- `img.driver(A[...], device=d, ...)` / `img.actor(A, mailbox=n, ...)` —
  actor declarations whose arguments must match `A.init` (or its literal
  constructor) after generated capabilities and handles are substituted.
  `decl.handle()` installs an `Actor[A]` identity as another actor's
  `init` dependency.
- `img.pool[T](name=P, slots=N, max_payload=B)` and
  `img.dma_pool[T](name=P, device=d, count=N)` — bind the previously
  unbound pool name `P` exactly once, reserve exact backing, and create the
  initial handles; the DMA form requires a `@layout(dma)` `T` and device
  reachability. Binding a name twice, or constructing it as a value, is a
  build error.
- `img.supervise(children=..., strategy=..., intensity=...)` — exactly one
  parent per actor/task. Restart provisions are derived from the wiring and
  shown by tooling; a resource `init` argument without one recovery source
  is a build error.
- `img.check_layout(f)` registers a `@layout_assert`.

## 10. Naming

Enum variants are CamelCase; functions are snake_case. A statically-proved
variant of a possibly-failing operation is named `*_proven`
(`reserve_proven`, `get_proven`) — never an unrelated name, never a silent
change of failure mode. Bounded-occupancy parameters are spelled `..N` in
public signatures; exact extents stay `N`.
