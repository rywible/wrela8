pub fn block_key(body: &str) -> String {
    wrela_compiler::report::sha256_hex(body.as_bytes())[..12].to_string()
}

pub struct CorpusSemaContext {
    #[allow(dead_code)]
    pub doc: &'static str,
    pub key: &'static str,
    #[allow(dead_code)]
    pub line: usize,
    #[allow(dead_code)]
    pub section: &'static str,
    pub preamble: &'static str,
    pub postamble: &'static str,
    pub params: &'static str,
    pub ret: &'static str,
    pub ret_ok: &'static str,
    pub async_wrapper: bool,
    pub nest_items_into: &'static str,
}

pub fn lookup(key: &str) -> Option<&'static CorpusSemaContext> {
    CORPUS_SEMA_CONTEXTS.iter().find(|c| c.key == key)
}

pub const CORPUS_SEMA_CONTEXTS: &[CorpusSemaContext] = &[
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
        params: "mut table: Table, key: Key",
        ret: "Result[unit, MissingKey]",
        ret_ok: "Ok(count)",
        async_wrapper: false,
        nest_items_into: "Table",
    },
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
        params: "codec: Actor[Codec], take data: own[Bufs] Data",
        ret: "Result[own[Bufs] Data, CallError[never]]",
        ret_ok: "Ok(take data)",
        async_wrapper: true,
        nest_items_into: "",
    },
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
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "7c68d809d413",
        line: 721,
        section: "9.4 Calls, errors, and admission",
        preamble: r#"
@actor
struct Logger:
    pub fn record(self, code: u64):
        return unit
"#,
        postamble: "",
        params: "logger: Actor[Logger]",
        ret: "",
        ret_ok: "",
        async_wrapper: true,
        nest_items_into: "",
    },
    CorpusSemaContext {
        doc: "docs/language/02-language.md",
        key: "84c332141ae2",
        line: 734,
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
        ret_ok: "Ok(path)",
        async_wrapper: true,
        nest_items_into: "",
    },
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
