//! Per-block declaration stubs for `corpus --sema` (plans/M9.md items J1b/J1c).
//!
//! **Keyed by a content hash of the block's own body** — the first 12 hex
//! chars of SHA-256 over the exact fence text (plans/M10.md item A3,
//! decision 710). It used to be keyed by `(doc, start_line)`, which meant
//! inserting *anything* above a fence shifted its line number and silently
//! detached its preamble; M10 item A hit exactly that and got three
//! phantom "disagreements". A content key is immune to edits *above* the
//! block, and an edit *to* the block loses the key loudly — see decision
//! 711 for why that is the correct direction.
//!
//! `doc` and `line` are audit aids, not part of the match: they let a human
//! find the block, and they may go stale after an insertion without
//! breaking anything (`corpus --sema` prints each block's live line and
//! key).
//!
//! Each stub may only declare names the snippet already references; it must
//! not restructure, weaken, or dodge the interesting part of the example.
//!
//! The stubs live here (not in the markdown) so rendered docs stay
//! unchanged. A human reading this file is the audit surface for "what
//! did we assume the prose had already introduced?".
//!
//! Method-shaped fence items (`fn …(self, …)`) cannot sit at module scope.
//! `nest_items_into` names a type declared in the preamble; the harness
//! splices the fence's own item text into that type. Fence content is never
//! discarded — J1c deleted `drop_fragment_items` for exactly that reason.

/// The content key of a corpus block: the first 12 hex chars of SHA-256
/// over the block's exact body text. This is the *only* thing that keys a
/// block here and in `corpus_sema_census` (plans/M10.md item A3, decision
/// 710); line numbers are audit aids and never participate in matching.
///
/// 12 hex chars = 48 bits. A collision is not a silent correctness hazard:
/// `verify_corpus_sema_keys` in `main.rs` fails closed if two live blocks
/// ever share a key.
pub fn block_key(body: &str) -> String {
    wrela_compiler::report::sha256_hex(body.as_bytes())[..12].to_string()
}

/// One block's injected declarations / wrapper shape.
pub struct CorpusSemaContext {
    /// Repo-relative doc holding the block when this row was written
    /// (audit aid; **not** matched on — see the module docs).
    #[allow(dead_code)]
    pub doc: &'static str,
    /// The block's content key (`block_key` of its body). The match key.
    pub key: &'static str,
    /// The block's first body line when this row was written (audit aid;
    /// **not** matched on, and allowed to go stale after an insertion).
    #[allow(dead_code)]
    pub line: usize,
    /// Human-facing section title (audit aid; not used by lookup).
    #[allow(dead_code)]
    pub section: &'static str,
    /// Declarations inserted into the wrapped module before the fragment's
    /// own items (types the snippet names but does not define). When
    /// `nest_items_into` is set, this must leave that type's body open
    /// (fields / earlier methods) so the harness can splice fence items
    /// as further members before `postamble`.
    pub preamble: &'static str,
    /// Helpers inserted after the fragment's items (so they may reference
    /// types the fragment itself declares). When nesting, indented members
    /// that continue the same type body after the spliced fence items.
    pub postamble: &'static str,
    /// Parameter list for `fn _corpus_snippet(...)` (no surrounding parens).
    pub params: &'static str,
    /// Optional return type (no leading `->`).
    pub ret: &'static str,
    /// When non-empty, appended after the fragment statements as
    /// `return <expr>` — mechanical wrapper epilogue so a `Result`-typed
    /// `_corpus_snippet` (needed for `?`) has a value on the fall-through
    /// path. Not part of the doc fence.
    pub ret_ok: &'static str,
    /// When true, the wrapper is `async fn` (needed for `await` / `send` /
    /// `with group`).
    pub async_wrapper: bool,
    /// When non-empty, fence `Item` entries are members of this type (must
    /// appear as `struct <name>` or `enum <name>` in the preamble). The
    /// harness indents and splices the fence's own item text into the type
    /// body between preamble and postamble. Empty = items stay at module
    /// scope (decision 501).
    pub nest_items_into: &'static str,
}

/// Look up the stub for a doc block by its content key (`block_key`).
/// Nothing else is consulted — moving a block within a doc, or between
/// docs, keeps its stub attached; editing its body detaches it, which
/// `verify_corpus_sema_contexts` in `main.rs` then reports by name.
pub fn lookup(key: &str) -> Option<&'static CorpusSemaContext> {
    CORPUS_SEMA_CONTEXTS.iter().find(|c| c.key == key)
}

/// Every currently-contextualized block, plus any later additions. Absence
/// from this table means the block is checked as-written (the J1 `ok`
/// blocks without stubs, and `.wr` examples).
pub const CORPUS_SEMA_CONTEXTS: &[CorpusSemaContext] = &[
    // 02 §3.1 — `take current` then reassign. Names: Packet, current.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "91f2c87cf267",
        line: 150,
        section: "3.1 How a resource ends",
        preamble: r#"
resource struct Packet:
    init(mut self):
        return unit

    fn empty() -> Packet:
        return Packet()
"#,
        postamble: "",
        params: "take current: Packet",
        ret: "",
        ret_ok: "",
        async_wrapper: false,
        nest_items_into: "",
    },
    // 02 §4 — image pool + net_pool.get + nic.transmit.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "be05640fe581",
        line: 185,
        section: "4. Pools and `own`",
        preamble: r#"
resource struct Packet:
    init(mut self):
        return unit

@actor
struct Nic:
    pub async fn transmit(self, take payload: own[Packets] Packet):
        return unit

struct NetPool:
    fn get(self, capacity: usize) -> Result[own[Packets] Packet, unit]:
        panic("corpus context: net_pool.get")
"#,
        postamble: "",
        params: "net_pool: NetPool, nic: Actor[Nic]",
        ret: "Result[unit, unit]",
        ret_ok: "Ok(unit)",
        async_wrapper: true,
        nest_items_into: "",
    },
    // 02 §4 — scoped pool. Names: Node, compose. `with pool` itself is the
    // load-bearing surface (known unimplemented).
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "ae1b6fca2a64",
        line: 206,
        section: "4. Pools and `own`",
        preamble: r#"
struct Node:
    x: u32

fn compose(mut scene: Node):
    return unit
"#,
        postamble: "",
        params: "",
        ret: "",
        ret_ok: "",
        async_wrapper: false,
        nest_items_into: "",
    },
    // 02 §5.1 — access modes on call sites.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "dd7b0c3dadfe",
        line: 250,
        section: "5.1 Parameters and call sites",
        preamble: r#"
resource struct Packet:
    init(mut self):
        return unit

resource struct Buffer:
    init(mut self):
        return unit

fn inspect(packet: Packet):
    return unit

fn fill(mut buffer: Buffer):
    return unit

fn submit(queue: u32, take payload: Packet):
    return unit
"#,
        postamble: "",
        params: "take packet: Packet, mut buffer: Buffer",
        ret: "",
        ret_ok: "",
        async_wrapper: false,
        nest_items_into: "",
    },
    // 02 §7.1 — BlockCache.init + map_take. Names: CacheLine, N, Payloads,
    // DmaBlock. CacheLine.invalid must accept the taken element.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "b49b24ba36e0",
        line: 348,
        section: "7.1 Structs",
        preamble: r#"
pool Payloads

@layout(dma, endian=little)
struct DmaBlock:
    @offset(0) word: u64
    @offset(8) pad: u64

struct CacheLine:
    fn invalid(take block: own[Payloads] DmaBlock) -> CacheLine:
        return CacheLine()

const N: usize = 1
"#,
        postamble: "",
        params: "",
        ret: "",
        ret_ok: "",
        async_wrapper: false,
        nest_items_into: "",
    },
    // 02 §7.2 — Lookup enum with IoError + match. Prefer the real stdlib
    // `IoError` via import (J1c); the fragment wrap routes import-bearing
    // wraps through `loader::load_closure` so `from core…` resolves.
    // J2c: fence is Result-typed (Failed/Err is load-bearing); Absent was
    // `return None` (internal Option/Result mix — decision below).
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "7595bd03723a",
        line: 370,
        section: "7.2 Enums and matching",
        preamble: r#"
from core.io_error import IoError
"#,
        postamble: r#"
fn lookup(key: u32) -> Lookup[u32]:
    return Lookup.Absent

fn use(value: u32):
    return unit
"#,
        params: "key: u32",
        ret: "Result[Option[u32], IoError]",
        ret_ok: "Ok(None)",
        async_wrapper: false,
        nest_items_into: "",
    },
    // 02 §7.2 — `is .Some` sugar.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "8e5ee656b323",
        line: 396,
        section: "7.2 Enums and matching",
        preamble: r#"
fn lookup(key: u32) -> Option[usize]:
    return None

fn use(index: usize):
    return unit
"#,
        postamble: "",
        params: "key: u32",
        ret: "",
        ret_ok: "",
        async_wrapper: false,
        nest_items_into: "",
    },
    // 02 §8.1 — match Found/Absent. Lookup[T] instantiation.
    // J2c: Found assigns into a mut `value` param (the fence's `value =
    // item`); Absent returns; fall-through returns `Some(value)` so the
    // wrapper matches §8.1's definite-initialization merge shape.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "b05b056b05b1",
        line: 473,
        section: "8.1 Statements",
        preamble: r#"
enum Lookup[T]:
    Found(T)
    Absent

fn lookup(key: u32) -> Lookup[u32]:
    return Lookup.Absent
"#,
        postamble: "",
        params: "key: u32, mut value: u32",
        ret: "Option[u32]",
        ret_ok: "Some(value)",
        async_wrapper: false,
        nest_items_into: "",
    },
    // 02 §8.3 — closures as scoped access. Fence `fn entry` is a method on
    // Table; nest it into the preamble's Table (fields + resolve), keep the
    // trailing call statement for `_corpus_snippet`.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "276e58180606",
        line: 530,
        section: "8.3 Closures",
        preamble: r#"
struct Item:
    count: u32

struct Key:
    id: u32

enum MissingKey:
    Missing

struct Table:
    items: [Item; 4]

    fn resolve(self, key: Key) -> Result[usize, MissingKey]:
        return Ok(0)
"#,
        postamble: "",
        // `item.count += 1` is a suite closure → inferred `R = unit`
        // (plans/M13.md item Q); the binding `count` is that unit payload.
        params: "mut table: Table, key: Key",
        ret: "Result[unit, MissingKey]",
        ret_ok: "Ok(count)",
        async_wrapper: false,
        nest_items_into: "Table",
    },
    // 02 §9.3 — await codec.compress with take. plans/M13.md item I:
    // CallError is nameable; `?` converts through Result[..., CallError[E]].
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "f56a6f9b7f60",
        line: 628,
        section: "9.3 Messages",
        preamble: r#"
pool Bufs

resource struct Data:
    init(mut self):
        return unit

@actor
struct Codec:
    pub async fn compress(self, take input: own[Bufs] Data) -> own[Bufs] Data:
        return take input
"#,
        postamble: "",
        // `take`: the fence moves `data` into the call (`input=take data`)
        // and rebinds the reply (`data = await ...`).
        params: "codec: Actor[Codec], take data: own[Bufs] Data",
        ret: "Result[own[Bufs] Data, CallError[never]]",
        ret_ok: "Ok(take data)",
        async_wrapper: true,
        nest_items_into: "",
    },
    // 02 §9.4 — send / match send. take of a non-own resource in a message.
    // The fence uses `event` twice as parallel illustrations; one binding.
    // plans/M13.md item J: Err arm is CallError.NotAdmitted (Rejected gone).
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "ca57644b1058",
        line: 698,
        section: "9.4 Calls, errors, and admission",
        preamble: r#"
resource struct Event:
    init(mut self):
        return unit

@actor
struct Audit:
    pub fn record(self, take event: Event):
        return unit

@actor
struct Logger:
    pub fn record(self, take event: Event):
        return unit

fn stash(take event: Event):
    return unit
"#,
        postamble: "",
        params: "audit: Actor[Audit], logger: Actor[Logger], take event: Event",
        ret: "",
        ret_ok: "",
        async_wrapper: true,
        nest_items_into: "",
    },
    // 02 §9.5 — with group. Method is `read_file` (not `read`: `read` is a
    // keyword and cannot be a declared method name — decision 517).
    // plans/M13.md item I: CallError is nameable; `?` converts.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "84c332141ae2",
        line: 699,
        section: "9.5 Groups",
        preamble: r#"
@actor
struct Storage:
    id: u32

    pub async fn read_file(self, path: u32) -> u32:
        return path

async fn fetch_part(index: u32) -> u32:
    return index
"#,
        postamble: "",
        params: "storage: Actor[Storage], path: u32",
        ret: "Result[u32, CallError[never]]",
        // `result` is scoped inside the fence's `with group` block; the
        // mechanical fall-through uses `path` (still in the wrapper).
        ret_ok: "Ok(path)",
        async_wrapper: true,
        nest_items_into: "",
    },
    // 03 §3 — publish / await receipt / completion.status.
    // `queue.publish` is the sealed VirtQueue intrinsic (not a DeclStruct
    // method returning Receipt — that shape is @driver-handoff-only per
    // 03 §5 / handoff.rs). A @driver root reaches `_corpus_snippet` so
    // provenance (03 §1) accepts the DMA touch; the free wrapper holds
    // the fence statements because they are statement-shaped.
    CorpusSemaContext {
        doc: "docs/language/03-hardware.md",
        key: "f4ee6fe571a7",
        line: 86,
        section: "3. DMA",
        preamble: r#"
from core.io_error import IoError

pool Payloads

@layout(dma, endian=little)
struct Buf:
    @offset(0) word: u64
    @offset(8) pad: u64

@driver
struct Blk:
    q: VirtQueue[..8]

    async fn kick(mut self, take prepared: QueueOp[own[Payloads] Buf, true]) -> Result[unit, IoError]:
        return _corpus_snippet(queue=mut self.q, prepared=take prepared)
"#,
        postamble: "",
        params: "mut queue: VirtQueue[..8], take prepared: QueueOp[own[Payloads] Buf, true]",
        ret: "Result[unit, IoError]",
        ret_ok: "Ok(unit)",
        async_wrapper: true,
        nest_items_into: "",
    },
    // 03 §6 — ISR body. Nest the fence's `fn on_queue_irq` into BlkDriver;
    // preamble `init` carries `irq.bind` (the wiring the prose names one
    // paragraph above — `init` is a struct member, not a fragment-top
    // item, so the fence cannot open with it). drain_used in postamble.
    CorpusSemaContext {
        doc: "docs/language/03-hardware.md",
        key: "5bbef735fe9c",
        line: 222,
        section: "6. Interrupts",
        preamble: r#"
@layout(mmio, endian=little)
struct VirtioIrqMmio:
    @offset(0x060) interrupt_status: ReadOnly[u32]
    @offset(0x064) interrupt_ack: WriteOnly[u32]

const INT_VRING: u32 = 1
const INT_CONFIG: u32 = 2

struct VirtioBlock:
    id: u32

@driver
struct BlkDriver:
    irq_regs: Mmio[VirtioIrqMmio]
    pending: InterruptCell[u32]
    irq: IrqCap[u32]

    init(mut self, take cap: DeviceCap[VirtioBlock]):
        claimed = VirtioBlock.claim(cap=take cap)
        self.irq_regs = claimed.map_partition(VirtioIrqMmio)
        self.pending = InterruptCell(0)
        irq = claimed.take_irq()
        irq.bind(self.on_queue_irq)
        self.irq = take irq
"#,
        postamble: r#"
    @task
    fn drain_used(mut self):
        return unit
"#,
        params: "",
        ret: "",
        ret_ok: "",
        async_wrapper: false,
        nest_items_into: "BlkDriver",
    },
    // 03 §8 — Untrusted narrowing. Checks clean with Completion + buffer.
    CorpusSemaContext {
        doc: "docs/language/03-hardware.md",
        key: "f0f2c62e8a44",
        line: 266,
        section: "8. Untrusted device data",
        preamble: r#"
struct Completion:
    written_len: Untrusted[usize]

struct Buf:
    fn capacity(self) -> usize:
        return 64
"#,
        postamble: "",
        params: "completion: Completion, buffer: Buf",
        ret: "Result[usize, unit]",
        ret_ok: "Ok(written)",
        async_wrapper: false,
        nest_items_into: "",
    },
];
