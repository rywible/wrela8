# The language

This chapter is the complete user-facing language. If a rule is not here, in
[Hardware](03-hardware.md), or in [Library contracts](05-library.md), it is a
compiler obligation ([04](04-compiler.md)) and source code never spells it.

## 1. Source files

Wrela source is UTF-8 with the `.wr` suffix. Identifiers are ASCII
(`[A-Za-z_][A-Za-z0-9_]*`), case-sensitive; keywords are reserved. String,
character, and comment contents may use any Unicode scalar. Non-ASCII
identifiers are a future revision.

`#` begins a comment to end of line. `##` begins a documentation comment
attached to the immediately following declaration.

Blocks use a trailing `:` and significant indentation, exactly four spaces per
level; tabs in leading whitespace are errors. A newline ends a simple
statement unless it is inside `()`, `[]`, or `{}`. Exception: inside a suite
introduced by a `:` at the end of a line (a closure body in an argument
list), indentation layout resumes until the suite closes. A declaration
header may continue onto an indented line beginning with `->`. Comma lists
may have a trailing comma.

### 1.1 Literals

Integer literals: decimal, `0x`, `0o`, `0b`; underscores between digits. Type
comes from context; an unconstrained literal defaults to `i64` (or `u64` when
only that fits). Float literals require a fractional part or exponent and
default to `f64`. `true`/`false` are `bool`. A character literal holds one
Unicode scalar. Escapes: `\\ \" \' \n \r \t \0`, `\xNN` (byte strings only),
`\u{H...}` (text/char only).

A text literal has type `Static[Str]`; a `b"..."` literal of length `N` has
type `Static[Bytes[N]]`. Literals contain no raw newlines; `"""` is reserved.

An interpolated string `f"...{expr:spec}..."` type-checks every interpolation
and computes a compile-time maximum length, producing `String[..N]`.
Unbounded formatting, formatting a `Secret`, and interpolation in ISR-bound
code are rejected. `{{`/`}}` escape braces.

```wrela
module example.counter

pub struct Counter:
    value: u64

    pub fn get(read self) -> u64:
        return self.value

    pub fn increment(mut self, by: u64):
        self.value += by
```

## 2. Modules and packages

Every file begins with `module path.name`, which must match the file's path
under the package source root. There is one import form, absolute and
explicit: `from path import Name [as Alias]`, where `Name` may resolve to a
declaration or a submodule — `from core.bytes import Bytes` and
`from core import time` are the same construct. No wildcards.
`pub from ... import ...` re-exports. Imports are compile-time name bindings
and run no code, so import cycles between modules are legal; cycles through
constants, layouts, or image construction are errors.

Declarations are module-private unless `pub`.

A fixed prelude is always in scope: `Option`, `Some`, `None`, `Result`, `Ok`,
`Err`, `panic`. Scalar type names are builtin. Everything else is imported.

### 2.1 The build root

There is no manifest. A build is pointed at one source file — the module
declaring the `@image` function ([§12.1](#121-the-image-constructor)) —
and everything else is derived. A module's declared path must agree with
its file path; walking that agreement upward from the root file anchors
the package root. The build's module graph is the transitive import
closure of the root — exactly the graph [04 §1](04-compiler.md)'s Closure
obligations are checked over; an unimported module is not part of the
build. The toolchain's `core` package resolves under the reserved alias
`core` with no declaration; revision 0.1 ships no other acquirable
package, no lockfile, and no package acquisition. The image itself
declares its name and target, comptime-checked, so no build fact lives
outside the program.

Revision 0.1 has no build configuration file of any kind: comptime
quotas and reporting thresholds are language-defined constants, and
every build-affecting input — compiler revision, target, quotas, input
digests — is recorded in the report's build identity
([04 §7–8](04-compiler.md)). Non-image tools point at files the same
way (`wrela test file.wr`, stage dumps); their imports resolve by the
same closure rule.

## 3. Values and access

wrela has value semantics: source names values, never addresses. The entire
ownership model is two sentences:

- **Data copies.** Scalars, and any enum, array, tuple, or struct built only
  from data, behave like integers: assignment, construction, and messages
  duplicate them. The compiler tracks every copy's cost and reports the
  expensive ones ([04 §7](04-compiler.md)); a `@budget` turns a hot-path
  copy into an error. Cost lives in the report, not the syntax.
- **Resources move.** A `resource struct`, a pool handle `own[P] T`, a
  capability, a receipt — and any composite containing one — has exactly one
  owner and cannot be copied. A named resource leaves its place only through
  the explicit `take place` — as an argument, an assignment source, a
  literal operand, or a pattern payload — after which the place is
  uninitialized. A fresh value (a call result, a literal) binds without
  `take`. There is no implicit move anywhere in the language.

Every use of a value is one of three access modes:

| Access | Meaning | Aliasing |
|---|---|---|
| `read` | Observe for the duration of the call. The default. | May coexist with other reads. |
| `mut` | Mutate the caller's value in place. | Exclusive for its duration. |
| `take` | Transfer ownership of a resource. | Source becomes uninitialized. |

`read` is a loan, never a copy. While `mut x` is active, no other path may
touch the same storage; exclusivity is checked on storage paths (fields,
indexes, and potential overlaps), not variable names.

```wrela
fn hash(data: Bytes) -> u64: ...        # read
fn fill(mut data: Bytes): ...           # exclusive mutation
fn enqueue(take data: own[Packets] Packet): ...  # move

h = hash(data)
fill(mut data)
enqueue(take data)                       # data is uninitialized after this
```

### 3.1 How a resource ends

How a resource may end is derived from its type, not declared by a taxonomy:

- If the type has a compiler-known, non-failing reclaim action (pool handles;
  plain resource structs), the compiler runs it automatically on block exit,
  `?`, cancellation, and abandonment, in reverse initialization order. The
  action and its destination appear in the build report.
- If its only consumers are protocol operations (capabilities, permits,
  receipts), every control-flow path must explicitly consume, return, or
  transfer it, or cover it with a `defer` that does (§10). Forgetting one is
  a compile error naming the path.

Overwriting a live resource is always an error: move it or finish it first.

```wrela
next = take current      # current is uninitialized here
current = Packet.empty()
```

### 3.2 Definite initialization

The first assignment to a name introduces it; an annotation is optional when
the type is inferable. Reassignment keeps the type. Shadowing an outer local
is an error; pick another name. The compiler tracks initialization on every
control-flow edge: reading an unassigned local or a taken-from place is an
error, and a `take` from a field must be restored on every normal path
before the value is used whole or the turn returns. Only resources can
become uninitialized after introduction, so the analysis is exactly the
resource-move graph.

Moving out of an array through a runtime index is forbidden (the analysis
would depend on runtime history). Consume an array whole with
`for take x in take array`, or use the sealed `map_take` builder
([05](05-library.md)); `Option.take()` moves out of a runtime-selected slot
by leaving `None` behind.

## 4. Pools and `own`

There is no ambient heap. Runtime-variable allocation happens only through a
bounded **pool** whose capacity is a build-time fact. A pool has one of two
lifetimes: **image** (declared by the image constructor, lives forever) or
**scoped** (opened by `with pool(...)`, reset at its deterministic close).

An image pool is bound to a **pool name** — a module- or actor-scoped
`pool Name` declaration that the image binds to exactly one pool node.
`own[Name] T` is a movable, uniquely owned handle to a `T` allocated from
that pool. No alias to the payload exists outside the handle, so the handle
may move through a mailbox without copying or locking.

```wrela
pool Packets                                  # bound once, by the image

packet: own[Packets] Packet = net_pool.get(capacity=2048)?
await nic.transmit(payload=take packet)
```

- In private code the pool name is inferred and displayed by tooling; in
  `pub` signatures and actor methods it is mandatory. An actor-scoped name is
  spelled `Owner.Name` from outside.
- Calling a method through an `own` handle derives a call-local access to the
  payload per the method's receiver (`buffer.len()` reads,
  `buffer.set_len(n)` mutates); a `take self` method consumes the handle.
- Reclaiming a handle returns its slot to the pool that minted it, wherever
  the handle traveled.
- If the compiler proves an allocation cannot exceed pool capacity, the
  operation is infallible (the `*_proven` API family); otherwise it returns
  `CapacityError`.

A scoped pool is the bulk-region form of the same concept:

```wrela
with pool(capacity=256.KiB) as frame:
    scene = frame.list[Node](max=4096)
    compose(mut scene)
# one reset releases everything
```

Locals, task frames, and request-scoped values need no pool: the compiler
places them, and reports (or, under `@no_promote` or a hard `@budget`,
rejects) any allocation it must promote to image lifetime.
[04 §3](04-compiler.md) specifies placement and promotion reporting.

## 5. Functions

```wrela
fn checksum(data: Bytes) -> u64: ...
async fn fetch(client: ClientHandle) -> Result[own[NetBufs] Bytes, NetError]: ...
```

`fn` is synchronous and never suspends. `async fn` may suspend with prefix
`await`. These are the only function colors. A function bound to an interrupt
vector by the image is an ordinary `fn` whose transitive effects the compiler
restricts to the ISR set ([03 §6](03-hardware.md)); ISR-ness comes from the
binding, not a keyword.

A `fn` is phase-neutral: it may also be evaluated at build time when its
transitive call closure is deterministic, I/O-free, and free of async/actor
operations (§12).

An `async fn` cannot be a detached future value: it is awaited, sent one-way
through an actor, or installed into a bounded task slot by the image or a
group. A body whose result is `unit` may fall off the end.

### 5.1 Parameters and call sites

Every parameter has an access mode; `read` is the unwritten default. The
receiver, when present, is the first parameter, spelled with its effect:
`read self`, `mut self`, `take self`. Every `pub` method spells its receiver
effect; a private method may write plain `self` and the
compiler infers and displays the least effect.

Non-receiver `mut` and `take` are **mirrored at the call site**, and the
operand must be an explicit place:

```wrela
inspect(packet)                    # read: unmarked
fill(mut buffer)
submit(queue=0, payload=take packet)
```

A missing, extra, or wrong marker is a compile error. The receiver is the one
exception: `cache.clear()` needs no second `mut`.

Argument labels are always available and never required: `submit(0, take
packet)` and `submit(queue=0, payload=take packet)` are both legal, each
argument bound at most once, evaluated in source order. Changing an exported
receiver or parameter effect is an API-breaking change.

## 6. Types

### 6.1 Scalars

`bool`; `u8 u16 u32 u64 usize`; `i8 i16 i32 i64 isize`; `f32 f64` where the
target enables them; `char`; `unit`; and the uninhabited `never`.

Ordinary `+ - *`, negation, and `MIN / -1` **abandon on overflow in every
profile**; there is no arithmetic undefined behavior. Wrapping arithmetic is
its own operator family — `+% -% *%` reduce modulo `2^width` — because the
code that wants it (rings, counters, hashes) reads better dense than through
method names. Division truncates toward zero; division by zero abandons.
Shifts abandon on out-of-range counts or (for `<<`) lost bits. Checked
library forms (`checked_add`, ...) return `Result`.

There are no implicit numeric conversions and no cast operator. Conversion
is the same method shape as everything else: `x.to[T]()` is checked (build
error at comptime, abandonment at runtime, when out of range);
`x.checked_to[T]()` returns a `Result`; `x.truncate_to[T]()` is explicit bit
truncation for driver code. Floats are strict IEEE 754 with canonical NaN;
no fast-math in revision 0.1.

### 6.2 Compound and standard types

- `[T; N]` — fixed array; `(A, B)` — tuple (one-element: `(T,)`).
- `Option[T]`, `Result[T, E]`.
- `Bytes[N]` exact bytes; `Bytes[..N]` bytes up to `N`; `String[..N]` owned
  UTF-8 up to `N`; `List[T, ..N]`; `SlotMap[T, ..N]`. The `..N` prefix always
  means bounded runtime occupancy; plain `N` means exact extent.
- `own[P] T` — pool handle (§4).
- `Static[T]` — a copyable read-only handle to immutable image data. It may
  cross actor boundaries; it exposes no address and no mutation.
- SIMD vectors `u8x16`, `i16x8`, `u32x4`, `f32x4`, and peers — ordinary
  data, lowered to NEON ([05 §8.1](05-library.md)).

A `read` or `mut` **parameter** of a bounded type may omit the bound:
`fn hash(data: Bytes)` accepts any capacity, and `fn fill(mut s: String)`
likewise. This shorthand exists only in parameter position; fields and locals
always state their bound. There is no `null`; use `Option`.

Bracket forms are structurally distinct: `[T; N]` is an array type,
`Name[T, N]` supplies generic arguments, `value[index]` indexes.

### 6.3 Generics

Type and `const` parameters are compile-time; every instantiation is
concrete and monomorphized. A const argument is `bool`, `char`, an integer,
or a fieldless enum, evaluated by the comptime engine. There are no runtime
type variables, no variance, and no erased containers.

```wrela
struct Ring[T, const N: usize]:
    items: [Option[T]; N]
```

## 7. Structs, enums, generics

### 7.1 Structs

`struct` is a product value — data if every field is data; `resource struct`
makes it a resource by fiat. Fields are private to the defining module
unless `pub`.

A struct without an `init` is constructed by its named-field literal: every
field supplied exactly once unless defaulted, positional only for one-field
structs.

```wrela
pub struct Point:
    x: i32
    y: i32

p = Point(x=10, y=20)
```

A struct may instead declare `init`, colocating construction with the type;
`Type(named_arguments...)` then invokes it. `init` begins with `mut self`,
is never `pub` or generic, and — deliberately — introduces **no new
analysis**: inside `init`, each field of `self` is checked exactly like an
uninitialized local, assigned once on every path before `self` is used
whole or `init` returns. `init` may return `Result[unit, E]`; an `Err` exit
tears down the fields initialized so far by the ordinary local-cleanup rule
(protocol resources are consumed or `defer`-covered like anywhere else).

```wrela
pub struct BlockCache:
    lines: [CacheLine; N]

    init(mut self, take blocks: [own[Payloads] DmaBlock; N]):
        self.lines = blocks.map_take(CacheLine.invalid)
```

Construction is in place either way — elision of a literal or an `init`
into its destination is guaranteed, never best-effort, so no aggregate is
ever moved by being built.

A struct marked `@actor` or `@driver` is an actor root and implicitly a
resource (§9); the image wiring invokes its `init` (or literal), and
supervised restart re-runs the same `init` from declared restart
provisions. A method without `self` is associated: `Type.method(...)`.

### 7.2 Enums and matching

An `enum` is a closed sum; variants are CamelCase constructors and may carry
payloads.

```wrela
enum Lookup[T]:
    Found(T)
    Absent
    Failed(IoError)

match lookup(key):
    case .Found(value):
        use(value)
    case .Absent:
        return Ok(None)
    case .Failed(error):
        return Err(error)
```

`match` is exhaustive, checked after comptime specialization; a wildcard arm
must cover something. In patterns a variant is `.Name(...)` or
`Enum.Name(...)`; a bare identifier is always a binding. Patterns include
tuple and fixed-array destructuring, literals, `_`, `|` alternatives (same
bindings, same types), and `if` guards (a guarded arm never contributes to
exhaustiveness). Matching never moves a resource payload implicitly: moving
one out writes `take` in the pattern; otherwise the arm gets the least read
access its body needs.

`is` is a refutable test; its bindings flow into the success branch:

```wrela
if lookup(key) is .Some(index):
    use(index)
```

In expression position, `.Variant(...)` is legal wherever the expected type
is a known enum.

### 7.3 Structural generics

There are no interface declarations. A generic simply uses its parameters,
and — because the closed world knows every instantiation — each one is
checked concretely:

```wrela
pub fn hash_pair[T](a: T, b: T) -> u64:
    return a.hash() ^ b.hash()
```

`hash_pair[Sector]` compiles exactly when `Sector` has a matching
`hash(read self) -> u64`. The **contract is compiler output, not user
input**: the compiler infers the requirement set of every generic —
methods, effects, copyability — and tooling and the build report display it.
A missing requirement is an instantiation error naming the chain:

```text
error[generic]: `hash_pair[Sector]` requires `Sector.hash(read self) -> u64`
  required by `a.hash()` at util/hash.wr:2
  instantiated at storage/extent.wr:41
```

With no interface declarations there are no `impl` blocks, no orphan rules,
and no coherence checking — there is nothing to conflict. Semantic intent
lives in doc comments and the displayed contract. Runtime heterogeneity is
still an explicit closed enum; there is still no `dyn`.

### 7.4 Method conventions

Operators and `?` resolve through named methods, structurally, always as
direct specialized calls:

| Form | Desugars to |
|---|---|
| `a + b`, `a - b`, `a * b`, `a / b`, `a % b` | `a.add(b)`, `a.subtract(b)`, `a.multiply(b)`, `a.divide(b)`, `a.remainder(b)` |
| `a < b` | `a.less_than(b)` (a strict total order; `> <= >=` derive from it) |
| `a += b` | `a = a.add(b)`, destination evaluated once |
| `Err(e)?` needing conversion | `TargetError.from(take e)` |

Operands are `read`; an operator expression never moves or mutates. Core
scalar operators are built in and never desugar. `==`/`!=` are generated
structurally for every data type. Error conversion for `?` is explicit: the
propagated error must be the enclosing error type or that type must declare
a matching `from(take source)` — no chains, no implicit widening.

### 7.5 Deriving

`deriving(...)` on a struct or enum generates methods from the closed list:
`Format` (the bounded formatting contract of [05 §6](05-library.md)) and,
for one-variant/one-field shapes, `From` (the conversion `?` consumes).
Structural `==`/`!=` needs no deriving — every data type has it. It is not
a macro system; other names are errors.

```wrela
enum ConfigError deriving(From):
    Invalid(String[..64])
```

## 8. Control flow, closures, and expressions

### 8.1 Statements

Revision 0.1 provides `if`/`elif`/`else`, `match`, `for`, `while`, `break`,
`continue`, `return`, `pass`, `assert`, `defer`, `send`, and `with`.
`match` and `if` are **statements, not expressions**; a conditional value is
written by assigning in each arm, and definite-initialization analysis merges
the arms:

```wrela
match lookup(key):
    case .Found(item):
        value = item
    case .Absent:
        return None
```

`for` iterates a closed set of forms: ranges (`0 .. n` half-open, `0 ..= n`
inclusive), fixed arrays (including consuming `for take x in take xs`), and
the container operations of [05 §7](05-library.md). There is deliberately no
user-defined iteration protocol in 0.1. The iterable is evaluated once; the
binding is fresh each iteration; `break`/`continue`/`return`/`?` run all
exited cleanup before transferring control.

A `for` or `while` in an `async fn` checkpoints at its back edge unless
annotated with a proven `@budget(bound=...)`. Every loop in a synchronous
`fn` needs a finite bound ([04 §2](04-compiler.md)). Revision 0.1 discharges
the synchronous half by requiring a statement attribute
`@budget(bound=N)` immediately preceding the `for` or `while`, where `N` is
a comptime-known integer ≥ 1 (an integer literal). Acceptance emits a
hidden trip counter that aborts if the loop body runs more than `N` times —
a fail-closed runtime bound, not a cost model. A synchronous `for`/`while`
without that attribute is `error[sema]`. **Exception (force-rooted runtime
event-loop entries):** the designated per-core park/run loop bodies in
`stdlib/core/runtime.wr` — today `__wrela_rt_secondary_entry`, and later the
primary entry driver when item K migrates it — may omit sync `@budget` on
their loops. Those functions *are* the cooperative scheduler of
[04 §2](04-compiler.md); a trip-counter abort is the wrong discharge for an
intentional unbounded park→wake loop. The exemption is by **exact function
name** (force-rooted / designated keys only), not by module membership:
ordinary sync loops in `runtime.wr` still require `@budget`. Async
checkpoint behaviour and ISR-bound loop rejection are unchanged by this
discharge; proving a budget that replaces an async checkpoint, and
cycle/latency proofs, remain later work ([04 §2](04-compiler.md)).

`pass` is an explicit no-op. `defer` and `with` are §10. `send` is §9.

### 8.2 Expressions

Precedence, tightest first: member/call/index; unary `-`, `~`, `await`,
`take`; postfix `?`; `* / % *%`; `+ - +% -%`; `<< >>`; `& ^ |`; ranges;
comparisons and `is`; `not`; `and`; `or`. So `await op()?` means
`(await op())?`. `&` is only bitwise AND; there is no reference operator, no
cast operator, and no membership operator — conversion and containment are
methods. Comparisons do not chain.

Evaluation is left-to-right exactly once, except `and`/`or` short-circuit.
A call evaluates receiver, then arguments in source order; `mut` activates
and `take` moves when its argument finishes evaluating, so later arguments
cannot touch overlapping storage — overlaps are compile errors, not
reorderings. Assignment evaluates its right side first, then the destination
place once. Temporaries tear down in reverse completion order at the end of
their full expression.

`?` applies to `Result` (and, in `Option`-returning functions, to `Option`):
`Err(e)?` runs lexical teardown, applies the target's `from` conversion if
needed, and returns from the enclosing function.

### 8.3 Closures

There is exactly one kind of closure: **synchronous and non-escaping**,
written `|params| expression` or `|params|: suite`. Parameter access modes
use function syntax; the structural type is `fn(read T, mut U, take V) -> R`.
A closure borrows its captures with the access its body needs and cannot
outlive them — checked lexically, since it cannot be stored, sent,
returned, or awaited. There are no async or escaping closures in revision
0.1: work that outlives a call is a named function installed as a task or
group child. A named function may be passed wherever a matching function
type is expected (`array.map_take(Line.reset)`); it is a compile-time item,
not a storable pointer.

Closures are also the language's **scoped-access mechanism** —
the job references and lifetimes do elsewhere:

```wrela
# An accessor lends a field for exactly one call:
fn entry[R](mut self, key: Key, body: fn(mut Item) -> R)
    -> Result[R, MissingKey]:
    index = self.resolve(key)?
    return Ok(body(mut self.items[index]))

count = table.entry(key, |mut item: Item| item.count += 1)?
```

The lent value obeys ordinary exclusivity for the duration of the call, and
because a synchronous closure cannot contain `await`, cannot be stored, and
cannot escape, the loan provably ends when the call returns. Nothing further
needs to be specified: there are no reference types, no lifetime parameters,
and no rules about storing or suspending borrows, because no expressible
program can try.

## 9. Actors and async

### 9.1 Actor roots

A struct marked `@actor` (application and service logic) or `@driver`
(hardware authority; [03](03-hardware.md)) is an actor: the sole owner of its
mutable fields. Other actors hold generated `Actor[T]` handles minted by the
image — every possible call edge is a build-time fact. Handles cannot appear
in messages, replies, or runtime collections.

Every actor also has exactly one build-time **core** on the machine's three
cores — inferred deterministically ([04 §3](04-compiler.md)) or set with
`core=` in the image wiring. Nothing in this chapter changes across cores:
a cross-core call is the same typed call. Cross-core parallelism is
therefore actors plus moved ownership — the flagship's compositor fans
tile buffers out to worker actors on other cores and gathers them for
scanout ([06 §7](06-machine.md)), with no shared mutation anywhere.

```wrela
@actor
pub struct Storage:
    pool Payloads                     # actor-scoped pool name
    cache: BlockCache
    disk: Actor[BlkDriver]

    pub async fn read_file(mut self, ino: u32,
                           take out: own[Storage.Payloads] Bytes)
        -> Result[own[Storage.Payloads] Bytes, FsError]:
        ...
```

### 9.2 Turns

An external message starts a **turn**. The actor keeps it until the handler
returns, errors, or abandons; awaiting a dependency lets other actors run but
admits no new message into this actor. Actor state before an `await`
therefore cannot be changed by a second message. Calls on `self` are ordinary
calls; a call through any `Actor[T]` handle is a message, whatever the
lowering.

One rule spans suspension: a whole-value access rooted at the current actor
(`self.fs.cache`) may live across `await` — the frame records the field path
and re-derives it — but an access rooted in an external argument may not, and
a closure-lent access never does (a lending call is synchronous).

Every mailbox is bounded; the compiler derives the capacity from the closed
sender set and fails the build if no finite bound exists
([04 §2](04-compiler.md)).

### 9.3 Messages

A message may contain: data, copied in (the report shows every message's
size); `Static[T]` handles to immutable image data; and resources moved
with `take` (pool handles, receipts). It may not contain `mut` loans, lent
closures, or object identity. To let another actor transform a buffer
without copying, transfer it and get it back:

```wrela
data = await codec.compress(input=take data)?
```

### 9.4 Calls, errors, and admission

Calling a public method through `Actor[T]` yields an awaitable, whether the
handler body is `fn` or `async fn`. Its result composes the declared result
with **one** error type:

```text
declared R            -> Result[R, CallError[never]]
declared Result[T, E] -> Result[T, CallError[E]]

CallError[E] =
    Op(E)                     # the callee's declared error
  | Cancelled                 # the enclosing group was cancelled
  | DeadlineExceeded          # admitted, then the deadline passed
  | NotAdmitted(Admission)    # never ran: mailbox full, deadline
                              # unmeetable, callee restarting, lineage stale
  | PeerFailed(Peer)          # callee abandoned or restarted first
```

One signature is exempt from that composition, and its own chapter names it:
a call to a `@driver` method carrying the handoff calling convention
([03 §5](03-hardware.md)) has result `Receipt[P]` exactly as declared. The
receipt is the caller's endpoint on work no handler has done yet, so its
failure vocabulary is the receipt's own state machine, reached by awaiting
it — not `CallError`.

`CallError` is the whole failure vocabulary for suspending work: group joins
and installed tasks use the same variants minus the actor-specific ones.
`?` converts it through an ordinary explicit `from` (§7.4), and whole-image
analysis erases variants a given call cannot produce.

Ownership across admission is one rule with no special carrier types:
**arguments always evaluate and move; an outcome that did not consume them
hands them back inside the error.** `NotAdmitted` carries the `take`
arguments (in declaration order) back to the caller as an owned value; every
other outcome means the message was committed and a `take` argument is gone —
an API that promises to return an input says so in its reply type or returns
a `Receipt` ([03 §5](03-hardware.md)). A caller can never hang on a dead
epoch: abandonment resolves every outstanding reply with `PeerFailed`
exactly once.

One-way messages have a single form. `send actor.method(...)` enqueues a
unit-returning method; its type is `Result[unit, Rejected[...]]` with the
moved payloads handed back in the error — an ordinary storable value. When
mailbox analysis proves admission cannot fail (including during a restart
window), the error type is `never` and `send` stands as a bare statement;
otherwise the result must be consumed:

```wrela
send audit.record(event=take event)          # capacity proven at build time

match send logger.record(event=take event):
    case .Ok(_):
        pass                                  # event was moved
    case .Err(rejected):
        stash(take rejected.event)            # handed back in the error
```

This is the language's one proof-conditioned form: the same spelling is
infallible exactly where the compiler has proved it, mirroring the library's
`*_proven` convention.

### 9.5 Groups

One construct is the unit of deadline, cancellation, and child concurrency:

```wrela
with group(deadline=now() + ms(50)):          # a bounded operation
    result = await storage.read_file(path)?

with group(capacity=4) as g:                  # bounded child work
    g.start(fetch_part, index=0)
    g.start(fetch_part, index=1)
    results = await g.join_all()
```

A group owns an optional deadline and budget, up to `capacity` child
activations (default zero), a cancellation domain, and a cleanup graph.
There is no detached spawn: a child is started into a group and cannot
outlive it. What earlier drafts called a *request* is a group without
children; a *nursery* is a group without a deadline. `race(a, b, ...)` is a
sealed contract over the same machinery: it reserves every child slot before
evaluating an alternative, returns a closed sum naming the winner, and fully
tears down every loser — including in-flight device work — before returning.

Every `async fn` implicitly carries the **ambient lineage** of its enclosing
group (or its task root); admission, `start`, and deadline/cancellation
propagation read it, signatures never thread a context parameter, and
tooling displays the inference. Rare escapes: an explicit `group=` argument
on a call, and `@detached` for work independent of any enclosing group.

Deadlines are inherited and can only narrow. Cancellation becomes observable
at `await` and checkpoints, never between arbitrary instructions. When a
group is cancelled, generated teardown closes admission, cancels children,
hands in-flight device receipts to driver recovery, runs ready cleanup in
deterministic reverse order, and resolves `Cancelled` to the parent only
when the cleanup graph is empty — a caller never observes a winner while a
loser still owns memory or a device request. The mechanics are the
compiler's ([04 §4](04-compiler.md)); source sees only `CallError` and its
own `defer`s running.

Long-running roots are installed by the image as `@task(...)` entries with a
priority, budget, and declared failure policy; a task returning
`Result[unit, E]` delivers `Err` to its supervisor action, never silently.

### 9.7 Non-blocking services

A non-driver actor that performs I/O for many clients should not hold its
turn across the I/O. The standard shape is the **service slot idiom**:
reserve a slot in a bounded in-flight table, submit downstream for a
`Receipt`, and end the turn with `slot.resolve(take receipt)` — the reply is
parked on the slot and resolved by a later generated turn
([05 §4](05-library.md)). The compiler's actor-chatter diagnostic names this
repair when a turn blocks queued senders.

## 10. Deterministic teardown

`defer` is the one user-facing cleanup construct. It registers a statement or
suite against the enclosing block, to run on **every** exit — fallthrough,
`return`/`break`/`continue`, `?`, cancellation, and supervisor cleanup after
abandonment — in reverse registration order:

```wrela
fn update_route(mut self):
    self.irqs.mask()
    defer self.irqs.unmask()
    ...

fn transfer(mut self, ...) -> Result[unit, LedgerError]:
    txn = self.begin_transaction()
    defer self.finish_or_rollback(mut txn)     # txn records commit state
    txn.debit(source, amount)?
    txn.credit(destination, amount)?
    txn.commit()
    return Ok(unit)
```

A deferred action cannot `await` and cannot fail recoverably — it handles
its own errors or abandons. Its accesses activate when it *runs*, not when
it is registered: the compiler verifies the places it names are valid at
every exit and sequences it in the same cleanup dependency graph the runtime
uses for cancellation ([04 §4](04-compiler.md)), so a deferred action that
needs a moved-and-returning resource simply waits for it. `defer` is also
how a protocol resource satisfies its consume-on-every-path obligation
(§3.1) when no single explicit path covers them all.

`with` opens the two intrinsic suspend-safe scopes — `group` (§9.5) and
scoped `pool` (§4) — whose bodies may hold `await` and whose teardown is
generated. There are no other `with` forms and no user-declared scope
protocols: acquisition-and-release APIs are ordinary functions used with
`defer`, or closure-taking functions (§8.3) when the API wants to guarantee
the release itself.

## 11. Faults

| Class | Representation |
|---|---|
| Build error | Compiler diagnostic; no artifact. |
| Recoverable fault | `Result[T, E]`, propagated with `?`. |
| Failed call | `CallError[E]` (§9.4). |
| Abandonment | Uncatchable; the actor stops and its supervisor decides. |
| Target-fatal | Failed recovery; the target halts or reboots by policy. |

`panic(message)`, failed `assert`, checked-arithmetic failure, out-of-bounds
access, and violated invariants **abandon** the actor: the turn stops,
generated cleanup runs (it can never skip resource teardown), and the
supervisor applies its declared policy. Abandonment is not catchable; a
boundary must not disguise hostile input as a bug (a bad superblock is an
`Err`, an impossible internal state is abandonment).

The image graph is also the supervision tree: every actor and task has a
parent policy (`OneForOne`, `OneForAll`, `RestForOne`) with a restart
intensity bound; exceeding it escalates. Restart re-runs the actor's `init`
from declared **restart provisions** (re-minted device capability, re-drawn
pool handles, a retained immutable dependency) — every resource `init`
argument must have exactly one recovery source in the image, checked at
build time. Restart mechanics, epochs, and recovery ordering are specified
in [04 §5](04-compiler.md).

## 12. Comptime

`comptime` evaluates ordinary typed wrela during the build — no second
language, no textual macros. Exactly two surface forms: `comptime if`
(statements or declarations; only the selected branch exists after
specialization) and `comptime assert` (build error, never a runtime panic).
Forcing evaluation to build time needs no operator: a `const` initializer,
a generic argument, and the whole `@image` body are already comptime
contexts.

A plain `fn` is comptime-callable when its transitive closure is
deterministic and free of I/O, async/actor operations, and hardware effects;
legality is inferred (no annotation) and violations are diagnosed with the
offending call path. Evaluation emulates the **target** (widths, endianness,
layout) under finite language-defined step/memory quotas (fixed constants
in revision 0.1, recorded in the build identity); exceeding a quota is a
build error with the hottest stack. External inputs enter only as
declared content-addressed build inputs. Comptime values become runtime data
only when they have a concrete target layout; they land in read-only image
data.

`Secret[T]` never serializes into the image, a diagnostic, or a comptime
control decision; secrets are provisioned at boot through a target channel.

Any configuration that changes the actor, task, ISR, or effect graph MUST be
a comptime type/const argument (for example `BlkDriver[DriverMode.Irq]`),
never a runtime value — so the graph that is validated is the graph that is
emitted.

### 12.1 The image constructor

Exactly one reachable `@image fn` returns the `Image`. It is evaluated only
by the compiler and declares the whole runtime graph: devices, drivers,
actors, pools, mailbox capacities, supervision, baked artifacts, and
post-layout checks. See the worked example
([virtio_storage.wr](examples/virtio_storage.wr)) and the builder contracts
in [05 §9](05-library.md).

```wrela
@image
pub fn build() -> Image:
    img = Image(name="appliance", target=Target.wrela_machine_v1)
    disk = img.driver(BlkDriver[DriverMode.Irq], device=blk_device, ...)
    storage = img.actor(Storage, disk=disk.handle(), mailbox=16, ...)
    img.supervise(children=[disk, storage], strategy=Restart.OneForOne,
                  intensity=RestartIntensity(max=3, within=seconds(10)))
    return img.seal()
```

Construction edges (moves, initialization order) must form a DAG; handle
edges may be cyclic. Boot allocates everything, initializes in dependency
order, then atomically opens mailboxes. `@layout_assert` functions run after
layout against the read-only `ImageReport` and can fail the build.

### 12.2 Tests

`@test` on a zero-argument `fn`/`async fn` declares a test. A test whose
closure is comptime-legal runs in the build evaluator; `@test(runtime)` or an
illegal closure makes it a generated image test booted on the digest-pinned
wrela machine runner — the VMM itself ([06](06-machine.md)) — with
statically bounded frames, events, output, and timeouts. Image-level
scenarios (console send/expect, input events, golden-frame digests,
shutdown, exit) are declared in scenario files supplied to the runner
invocation. There is no hosted or mocked test target.

A `@test(runtime)` fn may additionally declare parameters of type
`Actor[T]`: the runner supplies the handle of the image's unique declared
instance of `T` — the same image-minted handle any wired actor would hold.
If the closure's image declares zero or more than one instance of `T`,
the build fails naming the candidates; nothing is searched for or mocked.
This mirrors `@test(exhaustive)` below: a test's parameters declare what
the harness must supply, and are the only parameters a test may have.

`@test(exhaustive)` on a comptime-legal `fn` with parameters enumerates the
fn's entire input domain and runs the body once per case, each under its own
fresh quota. Every parameter must have a finite enumerable type — `bool`,
`u8`, `i8`, or a fieldless enum — and the product of the domain sizes is
bounded by a fixed build limit; a domain over the limit fails the test
rather than sampling. A passing exhaustive test is a verified statement
about every input, not a sample; a failing one reports the first
counterexample in the fixed enumeration order (rightmost parameter varying
fastest).

```wrela
@test(exhaustive)
fn wrap_then_unwrap_is_identity(x: u8):
    assert (x +% 1) -% 1 == x, "round trip"
```

## 13. Attributes

Attributes are resolved names with typed comptime arguments; they attach
metadata, never expand source. Unknown attributes are errors unless from a
declared non-semantic tool namespace. The revision 0.1 built-ins:

| Attribute | Meaning |
|---|---|
| `@image` | The unique build-time image constructor. |
| `@actor` / `@driver` | Actor root; drivers alone may hold hardware authority. |
| `@task(...)` | Statically bounded task entry: trigger, priority, budget, failure policy. |
| `@layout(kind, ...)` | Exact byte layout, `kind` one of `dma`, `mmio`, `wire`, `runtime` ([03 §3](03-hardware.md)). |
| `@offset(n)` | Field offset inside a `@layout` declaration. |
| `@placed(addr)` | Binds a module-level `static` of a `@layout(runtime)` type to a fixed address ([03 §3.1](03-hardware.md)). |
| `@layout_assert` | Post-layout build assertion over `ImageReport`. |
| `@test` / `@test(runtime)` / `@test(exhaustive)` | Test declaration (§12.2). |
| `@budget(...)` | Proven work/memory bound on a function or the loop it precedes. In statement position, `@budget(bound=N)` with comptime-known integer `N` ≥ 1 must immediately precede a `for` or `while` (§8.1). |
| `@no_promote` | Reject image-lifetime promotion in the annotated scope. |
| `@detached` | Work independent of any enclosing group (§9.5). |

Of the built-ins, only `@budget(...)` may be a statement attribute, and in
that position it must immediately precede `for` or `while` at the same
indentation. Other statement attributes, and a loop-position `@budget` on a
non-loop, are errors. Function-level `@budget` (call-graph / memory bound)
is named here but is not the §8.1 sync-loop discharge.

A module-level `static NAME: Type` is the construct
[03 §3.1](03-hardware.md) binds with `@placed(addr)`; `@placed` is legal
only on such a static of a `@layout(runtime)` type.
