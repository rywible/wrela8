//! Per-block declaration stubs for `corpus --sema` (plans/M9.md item J1b).
//!
//! Keyed by repo-relative doc path and the block's first body line — the
//! same coordinates the `--sema` report prints. Each stub may only declare
//! names the snippet already references; it must not restructure, weaken,
//! or dodge the interesting part of the example.
//!
//! The stubs live here (not in the markdown) so rendered docs stay
//! unchanged. A human reading this file is the audit surface for "what
//! did we assume the prose had already introduced?".

/// One block's injected declarations / wrapper shape.
pub struct CorpusSemaContext {
    pub doc: &'static str,
    pub start_line: usize,
    /// Human-facing section title (audit aid; not used by lookup).
    #[allow(dead_code)]
    pub section: &'static str,
    /// Declarations inserted into the wrapped module before the fragment's
    /// own items (types the snippet names but does not define).
    pub preamble: &'static str,
    /// Helpers inserted after the fragment's items (so they may reference
    /// types the fragment itself declares).
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
    /// When true, fragment `Item` entries are dropped because the preamble
    /// already carries them (method-shaped fragments that must live on a
    /// type the preamble declares — fragment wrap cannot nest them).
    pub drop_fragment_items: bool,
}

/// Look up the stub for a doc block. `doc` is repo-relative.
pub fn lookup(doc: &str, start_line: usize) -> Option<&'static CorpusSemaContext> {
    CORPUS_SEMA_CONTEXTS
        .iter()
        .find(|c| c.doc == doc && c.start_line == start_line)
}

/// Every currently-noisy block from J1, plus any later additions. Absence
/// from this table means the block is checked as-written (the 8 J1 `ok`
/// blocks, and `.wr` examples).
pub const CORPUS_SEMA_CONTEXTS: &[CorpusSemaContext] = &[
    // 02 §3.1 — `take current` then reassign. Names: Packet, current.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 150,
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
        drop_fragment_items: false,
    },
    // 02 §4 — image pool + net_pool.get + nic.transmit.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 185,
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
        drop_fragment_items: false,
    },
    // 02 §4 — scoped pool. Names: Node, compose. `with pool` itself is the
    // load-bearing surface (known unimplemented).
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 206,
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
        drop_fragment_items: false,
    },
    // 02 §5.1 — access modes on call sites.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 250,
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
        drop_fragment_items: false,
    },
    // 02 §7.1 — BlockCache.init + map_take. Names: CacheLine, N, Payloads,
    // DmaBlock. CacheLine.invalid must accept the taken element.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 348,
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
        drop_fragment_items: false,
    },
    // 02 §7.2 — Lookup enum with IoError + match. IoError is the name the
    // snippet uses; lookup/use come after the fragment's enum (postamble).
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 370,
        section: "7.2 Enums and matching",
        preamble: r#"
enum IoError:
    OutOfRange
"#,
        postamble: r#"
fn lookup(key: u32) -> Lookup[u32]:
    return Lookup.Absent

fn use(value: u32):
    return unit
"#,
        params: "key: u32",
        ret: "Option[u32]",
        ret_ok: "",
        async_wrapper: false,
        drop_fragment_items: false,
    },
    // 02 §7.2 — `is .Some` sugar.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 396,
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
        drop_fragment_items: false,
    },
    // 02 §8.1 — match Found/Absent. Lookup[T] instantiation.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 473,
        section: "8.1 Statements",
        preamble: r#"
enum Lookup[T]:
    Found(T)
    Absent

fn lookup(key: u32) -> Lookup[u32]:
    return Lookup.Absent
"#,
        postamble: "",
        params: "key: u32",
        ret: "Option[u32]",
        ret_ok: "",
        async_wrapper: false,
        drop_fragment_items: false,
    },
    // 02 §8.3 — closures as scoped access. The doc's `fn entry` is a
    // method (has `self`, called as `table.entry`); fragment wrap would
    // lift it to a free fn, so the preamble carries Table with the same
    // entry body the fence shows, and only the call statement is kept
    // from the fragment (drop_fragment_items).
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 530,
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

    fn entry[R](mut self, key: Key, body: fn(mut Item) -> R) -> Result[R, MissingKey]:
        index = self.resolve(key)?
        return Ok(body(mut self.items[index]))
"#,
        postamble: "",
        params: "mut table: Table, key: Key",
        ret: "Result[u32, MissingKey]",
        ret_ok: "Ok(count)",
        async_wrapper: false,
        drop_fragment_items: true,
    },
    // 02 §9.3 — await codec.compress with take. CallError is not
    // source-nameable, so `?` on the await cannot convert.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 604,
        section: "9.3 Messages",
        preamble: r#"
pool Bufs

resource struct Data:
    init(mut self):
        return unit

@actor
struct Codec:
    pub async fn compress(self, take input: own[Bufs] Data) -> own[Bufs] Data:
        return input
"#,
        postamble: "",
        params: "codec: Actor[Codec], data: own[Bufs] Data",
        ret: "Result[unit, unit]",
        ret_ok: "Ok(unit)",
        async_wrapper: true,
        drop_fragment_items: false,
    },
    // 02 §9.4 — send / match send. take of a non-own resource in a message.
    // The fence uses `event` twice as parallel illustrations; one binding.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 656,
        section: "9.4 Calls, errors, and admission",
        preamble: r#"
resource struct Event:
    init(mut self):
        return unit

resource struct Rejected:
    event: Event

    init(mut self, take event: Event):
        self.event = event
        return unit

@actor
struct Audit:
    pub fn record(self, take event: Event):
        return unit

@actor
struct Logger:
    pub fn record(self, take event: Event) -> Result[unit, Rejected]:
        return Ok(unit)

fn stash(take event: Event):
    return unit
"#,
        postamble: "",
        params: "audit: Actor[Audit], logger: Actor[Logger], take event: Event",
        ret: "",
        ret_ok: "",
        async_wrapper: true,
        drop_fragment_items: false,
    },
    // 02 §9.5 — with group. Method name `read` is a keyword and cannot be
    // declared; Storage is provided without it so the call surfaces
    // honestly. fetch_part is the name the snippet starts.
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        start_line: 674,
        section: "9.5 Groups",
        preamble: r#"
@actor
struct Storage:
    id: u32

async fn fetch_part(index: u32) -> u32:
    return index
"#,
        postamble: "",
        params: "storage: Actor[Storage], path: u32",
        ret: "Result[unit, unit]",
        ret_ok: "Ok(unit)",
        async_wrapper: true,
        drop_fragment_items: false,
    },
    // 03 §3 — publish / await receipt / completion.status.
    CorpusSemaContext {
        doc: "docs/language/03-hardware.md",
        start_line: 85,
        section: "3. DMA",
        preamble: r#"
pool Payloads

@layout(dma, endian=little)
struct Buf:
    @offset(0) word: u64
    @offset(8) pad: u64

resource struct Prepared:
    init(mut self):
        return unit

struct Queue:
    fn publish(self, take operation: Prepared) -> Receipt[IoCompletion[own[Payloads] Buf]]:
        panic("corpus context: queue.publish")
"#,
        postamble: "",
        params: "queue: Queue, take prepared: Prepared",
        ret: "Result[unit, unit]",
        ret_ok: "Ok(unit)",
        async_wrapper: true,
        drop_fragment_items: false,
    },
    // 03 §6 — ISR body. Nested onto BlkDriver (drop_fragment_items) with
    // the field/const names the body uses. No irq.bind / @image — those
    // names are not in the snippet; wake-outside-ISR is the honest
    // residual without inventing wiring the fence never wrote.
    CorpusSemaContext {
        doc: "docs/language/03-hardware.md",
        start_line: 180,
        section: "6. Interrupts",
        preamble: r#"
@layout(mmio, endian=little)
struct VirtioIrqMmio:
    @offset(0x060) interrupt_status: ReadOnly[u32]
    @offset(0x064) interrupt_ack: WriteOnly[u32]

const INT_VRING: u32 = 1
const INT_CONFIG: u32 = 2

@driver
struct BlkDriver:
    irq_regs: Mmio[VirtioIrqMmio]
    pending: InterruptCell[u32]

    fn on_queue_irq(self):
        status = self.irq_regs.interrupt_status.read()
        handled = status & (INT_VRING | INT_CONFIG)
        if handled != 0:
            self.pending.fetch_or_release(handled)
            self.irq_regs.interrupt_ack.write(handled)
            wake(BlkDriver.drain_used)

    @task
    fn drain_used(mut self):
        return unit
"#,
        postamble: "",
        params: "",
        ret: "",
        ret_ok: "",
        async_wrapper: false,
        drop_fragment_items: true,
    },
    // 03 §8 — Untrusted narrowing. Checks clean with Completion + buffer.
    CorpusSemaContext {
        doc: "docs/language/03-hardware.md",
        start_line: 224,
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
        drop_fragment_items: false,
    },
];
