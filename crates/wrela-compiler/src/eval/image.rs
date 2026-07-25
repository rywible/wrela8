//! The `@image` builder's own product (plans/M4.md item B, decision 5):
//! evaluating the one reachable `@image` fn on the M3 evaluator
//! (`eval::interp`) builds exactly one plain `ImageGraph` — BTreeMaps
//! keyed by name (pools/dma pools, the one declaration kind 05-library.md
//! §9 itself names by a bound `pool` identifier) plus construction-order
//! `Vec`s (everything else: devices, drivers, actors, `supervise`
//! groups, registered layout asserts) — declarations recorded in program
//! order, every argument a fully evaluated `eval::value::Value` alongside
//! its own static `sema::types::Type` (needed only so `dump` below can
//! render an enum construction or a list by name instead of a bare
//! variant index).
//!
//! This module owns the graph's shape and the one stable text dump
//! (house rule: every stage gets a dump before features) — `Kind
//! key=value` lines, two-space indent, the M1 dump style, deterministic
//! order throughout. It does **not** check the graph (item C: DAG,
//! one-parent supervision, init-argument matching, ...) — recording
//! faithfully is this item's whole job; item C reads this same struct.
//!
//! `img.dma_pool` used to be the one declaration kind that was recorded
//! and then immediately failed the whole build (plans/M4.md decision 10,
//! gap `image.graph.dma-pools`): `@layout(dma)` validation did not exist
//! until the hardware milestone, and this compiler never asserts against
//! nothing. plans/M7.md item B built that validation and item D consumes
//! it, so the recorder now returns an ordinary bound-pool handle and the
//! real checks (a `@layout(dma)` payload, a reachable device, an exact
//! count) live in `eval::image_checks::check_pool_decls`, over the whole
//! finished graph, like every other graph rule.
//! `img.check_layout` registers a `@layout_assert` fn and runs
//! nothing (decision 10's other half) — `LayoutAssertDecl` is the whole
//! of what M4 records for it.

use std::collections::BTreeMap;

use crate::eval::value::Value;
use crate::sema::types::{self, Type};

/// A builder declaration's own identity, as referenced by a *later*
/// intrinsic argument (`device=block_device`, `disk=disk.handle()`, a
/// `children=[...]` list, ...) — devices/drivers/actors are named by
/// their own construction-order index (nothing in 05 §9 gives them a
/// source name of their own — only the local they happen to be bound to,
/// which item B deliberately does not thread through, see `eval/mod.rs`'s
/// module doc); pools/dma pools are named by their own bound `pool` name
/// (02-language.md §4), which *is* source data.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImageDeclRef {
    Device(usize),
    Driver(usize),
    Actor(usize),
    Pool(String),
    DmaPool(String),
}

impl ImageDeclRef {
    /// The dump's own rendering of a declaration reference wherever it
    /// appears as another declaration's argument value.
    pub fn render(&self) -> String {
        match self {
            ImageDeclRef::Device(i) => format!("device#{i}"),
            ImageDeclRef::Driver(i) => format!("driver#{i}"),
            ImageDeclRef::Actor(i) => format!("actor#{i}"),
            ImageDeclRef::Pool(name) => format!("pool:{name}"),
            ImageDeclRef::DmaPool(name) => format!("dma_pool:{name}"),
        }
    }
}

/// A value together with its static type — the type is only ever needed
/// for rendering (an enum construction's own variant name, a list's own
/// element-by-element rendering), never for checking (item C's job).
#[derive(Debug, Clone, PartialEq)]
pub struct TypedValue {
    pub ty: Type,
    pub value: Value,
}

/// One builder argument, still carrying its own source label — no
/// declared parameter list exists for a builder intrinsic to align
/// positionally against (`sema::typed::TypedExprKind::Intrinsic`'s own
/// doc comment), so every argument's label is kept.
#[derive(Debug, Clone, PartialEq)]
pub struct DeclArg {
    pub label: String,
    pub ty: Type,
    pub value: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DeviceDecl {
    pub device_type: Type,
    pub args: Vec<DeclArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct DriverDecl {
    pub actor_type: Type,
    pub args: Vec<DeclArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ActorDecl {
    pub actor_type: Type,
    pub args: Vec<DeclArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PoolDecl {
    pub payload_type: Type,
    pub args: Vec<DeclArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct SuperviseDecl {
    pub args: Vec<DeclArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutAssertDecl {
    pub fn_key: String,
}

/// One `@image` fn's whole evaluated product (plans/M4.md item B,
/// decision 5). `name`/`target` come only from `Image(...)` itself
/// (decision 5's own "the image's name and target come only from
/// `Image(...)`, comptime-checked source, nowhere else"). `dma_pools` was
/// always empty until plans/M7.md item D: an `img.dma_pool` call recorded
/// its own entry and then failed the whole evaluation before `img.seal()`
/// could ever run (decision 10). It is an ordinary declaration map now,
/// keyed exactly like `pools` and disjoint from it — 05-library.md §9's
/// "bind the previously unbound pool name `P` exactly once" is one rule
/// over one name space, not two (`bind_pool_name`, below).
#[derive(Debug, Clone, PartialEq, Default)]
pub struct ImageGraph {
    pub name: Option<TypedValue>,
    pub target: Option<TypedValue>,
    pub devices: Vec<DeviceDecl>,
    pub drivers: Vec<DriverDecl>,
    pub actors: Vec<ActorDecl>,
    pub pools: BTreeMap<String, PoolDecl>,
    pub dma_pools: BTreeMap<String, PoolDecl>,
    pub supervisions: Vec<SuperviseDecl>,
    pub layout_asserts: Vec<LayoutAssertDecl>,
    pub sealed: bool,
}

impl ImageGraph {
    pub fn new(name: TypedValue, target: TypedValue) -> ImageGraph {
        ImageGraph {
            name: Some(name),
            target: Some(target),
            ..ImageGraph::default()
        }
    }

    pub fn declare_device(&mut self, device_type: Type, args: Vec<DeclArg>) -> Value {
        let idx = self.devices.len();
        self.devices.push(DeviceDecl { device_type, args });
        Value::ImageDecl(ImageDeclRef::Device(idx))
    }

    pub fn declare_driver(&mut self, actor_type: Type, args: Vec<DeclArg>) -> Value {
        let idx = self.drivers.len();
        self.drivers.push(DriverDecl { actor_type, args });
        Value::ImageDecl(ImageDeclRef::Driver(idx))
    }

    pub fn declare_actor(&mut self, actor_type: Type, args: Vec<DeclArg>) -> Value {
        let idx = self.actors.len();
        self.actors.push(ActorDecl { actor_type, args });
        Value::ImageDecl(ImageDeclRef::Actor(idx))
    }

    /// 05-library.md §9's "bind the previously unbound pool name `P`
    /// exactly once", as one predicate over **one** name space: a name
    /// already bound by either form — `img.pool` or `img.dma_pool` — is
    /// already bound, full stop. Before plans/M7.md item D each recorder
    /// only ever consulted its own map, which was harmless exactly
    /// because `declare_dma_pool` failed the whole build closed and
    /// `dma_pools` could therefore never hold anything by the time a
    /// second call ran. Making DMA pools real makes
    /// `img.pool(name=P, ...)` followed by `img.dma_pool(name=P, ...)`
    /// reachable, and it must be the same rejection as binding either
    /// form twice — a pool name names one pool node
    /// (02-language.md §4), not one per form.
    fn bind_pool_name(&self, pool_name: &str) -> Result<(), String> {
        if self.pools.contains_key(pool_name) || self.dma_pools.contains_key(pool_name) {
            return Err(format!("pool `{pool_name}` is already bound"));
        }
        Ok(())
    }

    /// Binds `pool_name` (05-library.md §9: "bind the previously unbound
    /// pool name `P` exactly once ... binding a name twice ... is a build
    /// error" — item C's own full check over the whole graph, once
    /// nominal-vs-value-construction is checkable; this recorder rejects
    /// only the one case it can name honestly on its own: this exact call
    /// binding the same name a second time, which would otherwise
    /// silently clobber the first declaration rather than recording
    /// both).
    pub fn declare_pool(
        &mut self,
        pool_name: String,
        payload_type: Type,
        args: Vec<DeclArg>,
    ) -> Result<Value, String> {
        self.bind_pool_name(&pool_name)?;
        self.pools
            .insert(pool_name.clone(), PoolDecl { payload_type, args });
        Ok(Value::ImageDecl(ImageDeclRef::Pool(pool_name)))
    }

    /// The DMA form of the same binding (plans/M7.md item D). Until this
    /// item it recorded the declaration and then failed the whole build
    /// closed (plans/M4.md decision 10, gap `image.graph.dma-pools`),
    /// because `@layout(dma)` validation did not exist and this compiler
    /// never asserts against nothing. It exists now (item B), so the
    /// recorder records and the real rules — a `@layout(dma)` payload
    /// type, a device this image declares, an exact positive count —
    /// are checked over the whole finished graph by
    /// `eval::image_checks::check_pool_decls`, which is where every other
    /// graph rule lives and where the payload type's own layout table is
    /// reachable.
    pub fn declare_dma_pool(
        &mut self,
        pool_name: String,
        payload_type: Type,
        args: Vec<DeclArg>,
    ) -> Result<Value, String> {
        self.bind_pool_name(&pool_name)?;
        self.dma_pools
            .insert(pool_name.clone(), PoolDecl { payload_type, args });
        Ok(Value::ImageDecl(ImageDeclRef::DmaPool(pool_name)))
    }

    pub fn declare_supervise(&mut self, args: Vec<DeclArg>) {
        self.supervisions.push(SuperviseDecl { args });
    }

    pub fn declare_check_layout(&mut self, fn_key: String) {
        self.layout_asserts.push(LayoutAssertDecl { fn_key });
    }
}

// --- the `--stage=image` dump (deliverable 3): `Kind key=value`, two-
// space indent, the M1 dump style, deterministic order throughout
// (program order for devices/drivers/actors/supervisions/layout
// asserts, `BTreeMap` order for pools/dma pools). -------------------------

/// `pub(crate)`, not private: `report.rs` (plans/M4.md item D) reuses this
/// exact line-writer so the report's own `Kind key=value` lines are
/// byte-identical in style to this stage's — one indentation mechanism,
/// not two.
pub(crate) fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

/// Renders one already-evaluated `Value` typed as `ty` — the one place
/// this module needs the enum/list's own static type at all: a bare
/// `Value::Enum` carries only its variant *index* (`sema::typed`'s own
/// module doc: "the typed tree names variants, not indices"), and a
/// `Value::Array`'s own element type is needed to render nested enums by
/// name too (`required_features=[...]`, `children=[...]`).
///
/// `pub(crate)`: `report.rs` (plans/M4.md item D) reuses this verbatim so
/// every `Arg`/`Name`/`Target` fact in the report renders identically to
/// the raw `--stage=image` dump — the report is a superset of facts, not
/// a second rendering mechanism.
pub(crate) fn render_value(program: &TypedProgramEnums, ty: &Type, v: &Value) -> String {
    match (ty, v) {
        (Type::Named(name, _), Value::Enum(idx, payload)) => {
            let variant = match name.as_str() {
                "Option" => {
                    if *idx == crate::eval::value::OPTION_SOME {
                        "Some"
                    } else {
                        "None"
                    }
                }
                "Result" => {
                    if *idx == crate::eval::value::RESULT_OK {
                        "Ok"
                    } else {
                        "Err"
                    }
                }
                _ => program
                    .enums
                    .get(name)
                    .and_then(|vs| vs.get(*idx))
                    .map(String::as_str)
                    .unwrap_or("?"),
            };
            if payload.is_empty() {
                format!("{name}.{variant}")
            } else {
                format!(
                    "{name}.{variant}({})",
                    payload
                        .iter()
                        .map(|p| render_bare_value(p))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        (Type::Array(elem, _), Value::Array(items)) => format!(
            "[{}]",
            items
                .iter()
                .map(|i| render_value(program, elem, i))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        _ => render_bare_value(v),
    }
}

/// A minimal read-only view of `TypedProgram` this module needs for
/// rendering (just the enum-name table) — avoids `eval::image` depending
/// on `sema::typed::TypedProgram`'s full shape for a dump that only ever
/// reads one field of it.
pub struct TypedProgramEnums<'p> {
    pub enums: &'p BTreeMap<String, Vec<String>>,
}

/// Renders a `Value` with no type context (a scalar, a string, a decl
/// reference, a nested tuple/struct with no further enum/list
/// information to render by name) — the fallback every typed case above
/// still recurses into for payloads/leaves.
///
/// `pub(crate)`: `report.rs` reuses this for the same reason
/// `render_value`/`push_line` above are `pub(crate)` now.
pub(crate) fn render_bare_value(v: &Value) -> String {
    match v {
        Value::U8(n) => n.to_string(),
        Value::U16(n) => n.to_string(),
        Value::U32(n) => n.to_string(),
        Value::U64(n) => n.to_string(),
        Value::Usize(n) => n.to_string(),
        Value::I8(n) => n.to_string(),
        Value::I16(n) => n.to_string(),
        Value::I32(n) => n.to_string(),
        Value::I64(n) => n.to_string(),
        Value::Isize(n) => n.to_string(),
        Value::F32(n) => n.to_string(),
        Value::F64(n) => n.to_string(),
        Value::Bool(b) => b.to_string(),
        Value::Char(c) => c.to_string(),
        Value::Unit => "unit".to_string(),
        Value::Str(b) => String::from_utf8_lossy(b).into_owned(),
        Value::Bytes(b) => format!("bytes[{}]", b.len()),
        Value::Tuple(items) | Value::Array(items) | Value::Struct(items) => format!(
            "({})",
            items
                .iter()
                .map(render_bare_value)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Value::Enum(idx, payload) => {
            if payload.is_empty() {
                format!("variant#{idx}")
            } else {
                format!(
                    "variant#{idx}({})",
                    payload
                        .iter()
                        .map(render_bare_value)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        Value::Fn(key) => key.spelling(),
        Value::Closure { .. } => "<closure>".to_string(),
        Value::ImageDecl(r) => r.render(),
    }
}

fn dump_args(program: &TypedProgramEnums, args: &[DeclArg], depth: usize, out: &mut String) {
    for a in args {
        push_line(
            out,
            depth,
            &format!(
                "Arg label={} value={}",
                a.label,
                render_value(program, &a.ty, &a.value)
            ),
        );
    }
}

/// The `--stage=image` dump (deliverable 3): a versioned header line, one
/// section per declaration kind, absent entirely when empty (no
/// placeholders for facts that don't exist, mirroring 04-compiler.md
/// §7's own report discipline one milestone early).
pub fn dump(enums: &BTreeMap<String, Vec<String>>, graph: &ImageGraph) -> String {
    let program = TypedProgramEnums { enums };
    let mut out = String::new();
    out.push_str("ImageGraph v0\n");
    if let Some(name) = &graph.name {
        push_line(
            &mut out,
            1,
            &format!(
                "Name value={}",
                render_value(&program, &name.ty, &name.value)
            ),
        );
    }
    if let Some(target) = &graph.target {
        push_line(
            &mut out,
            1,
            &format!(
                "Target value={}",
                render_value(&program, &target.ty, &target.value)
            ),
        );
    }
    for (i, d) in graph.devices.iter().enumerate() {
        push_line(
            &mut out,
            1,
            &format!(
                "Device index={i} type={}",
                types::render_type(&d.device_type)
            ),
        );
        dump_args(&program, &d.args, 2, &mut out);
    }
    for (i, d) in graph.drivers.iter().enumerate() {
        push_line(
            &mut out,
            1,
            &format!(
                "Driver index={i} type={}",
                types::render_type(&d.actor_type)
            ),
        );
        dump_args(&program, &d.args, 2, &mut out);
    }
    for (i, d) in graph.actors.iter().enumerate() {
        push_line(
            &mut out,
            1,
            &format!("Actor index={i} type={}", types::render_type(&d.actor_type)),
        );
        dump_args(&program, &d.args, 2, &mut out);
    }
    for (name, d) in &graph.pools {
        push_line(
            &mut out,
            1,
            &format!(
                "Pool name={name} type={}",
                types::render_type(&d.payload_type)
            ),
        );
        dump_args(&program, &d.args, 2, &mut out);
    }
    for (name, d) in &graph.dma_pools {
        push_line(
            &mut out,
            1,
            &format!(
                "DmaPool name={name} type={}",
                types::render_type(&d.payload_type)
            ),
        );
        dump_args(&program, &d.args, 2, &mut out);
    }
    for (i, s) in graph.supervisions.iter().enumerate() {
        push_line(&mut out, 1, &format!("Supervise index={i}"));
        dump_args(&program, &s.args, 2, &mut out);
    }
    for a in &graph.layout_asserts {
        push_line(&mut out, 1, &format!("LayoutAssert fn={}", a.fn_key));
    }
    push_line(&mut out, 1, &format!("Sealed value={}", graph.sealed));
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tv(ty: Type, value: Value) -> TypedValue {
        TypedValue { ty, value }
    }

    #[test]
    fn devices_drivers_actors_record_in_construction_order() {
        let mut g = ImageGraph::new(
            tv(
                Type::Static(Box::new(Type::Str)),
                Value::Str(b"img".to_vec()),
            ),
            tv(
                Type::Named("Target".to_string(), vec![]),
                Value::Enum(0, vec![]),
            ),
        );
        let first = g.declare_driver(Type::Named("A".to_string(), vec![]), vec![]);
        let second = g.declare_driver(Type::Named("B".to_string(), vec![]), vec![]);
        assert_eq!(first, Value::ImageDecl(ImageDeclRef::Driver(0)));
        assert_eq!(second, Value::ImageDecl(ImageDeclRef::Driver(1)));
        assert_eq!(g.drivers.len(), 2);
        assert_eq!(
            g.drivers[0].actor_type,
            Type::Named("A".to_string(), vec![])
        );
        assert_eq!(
            g.drivers[1].actor_type,
            Type::Named("B".to_string(), vec![])
        );
    }

    #[test]
    fn pools_are_keyed_by_bound_name_not_construction_order() {
        let mut g = ImageGraph::default();
        let v = g
            .declare_pool("Buffers".to_string(), Type::U32, vec![])
            .expect("first bind succeeds");
        assert_eq!(
            v,
            Value::ImageDecl(ImageDeclRef::Pool("Buffers".to_string()))
        );
        assert!(g.pools.contains_key("Buffers"));
    }

    #[test]
    fn binding_the_same_pool_name_twice_is_rejected() {
        let mut g = ImageGraph::default();
        g.declare_pool("Buffers".to_string(), Type::U32, vec![])
            .expect("first bind succeeds");
        let err = g
            .declare_pool("Buffers".to_string(), Type::U32, vec![])
            .expect_err("a second bind of the same name must fail");
        assert!(err.contains("Buffers"));
    }

    #[test]
    fn a_dma_pool_binds_its_name_like_any_other_pool() {
        // plans/M7.md item D: this used to record the declaration and
        // then fail the whole build closed (plans/M4.md decision 10).
        let mut g = ImageGraph::default();
        let v = g
            .declare_dma_pool("Payloads".to_string(), Type::U8, vec![])
            .expect("a DMA pool is an ordinary bound pool now");
        assert_eq!(
            v,
            Value::ImageDecl(ImageDeclRef::DmaPool("Payloads".to_string()))
        );
        assert!(g.dma_pools.contains_key("Payloads"));
    }

    #[test]
    fn a_pool_name_is_one_name_space_across_both_forms() {
        // 05-library.md §9: "bind the previously unbound pool name `P`
        // exactly once". Unreachable before item D (the DMA form failed
        // the build before a second call could run), reachable now, and
        // the same rejection either way round.
        let mut g = ImageGraph::default();
        g.declare_pool("Shared".to_string(), Type::U32, vec![])
            .expect("first bind succeeds");
        let err = g
            .declare_dma_pool("Shared".to_string(), Type::U8, vec![])
            .expect_err("the DMA form binds the same one name space");
        assert!(err.contains("Shared"), "{err}");

        let mut g2 = ImageGraph::default();
        g2.declare_dma_pool("Shared".to_string(), Type::U8, vec![])
            .expect("first bind succeeds");
        let err2 = g2
            .declare_pool("Shared".to_string(), Type::U32, vec![])
            .expect_err("and the plain form binds it too");
        assert!(err2.contains("Shared"), "{err2}");
    }

    #[test]
    fn supervise_and_check_layout_are_recorded_in_order() {
        let mut g = ImageGraph::default();
        g.declare_check_layout("fn_a".to_string());
        g.declare_check_layout("fn_b".to_string());
        assert_eq!(
            g.layout_asserts
                .iter()
                .map(|a| a.fn_key.clone())
                .collect::<Vec<_>>(),
            vec!["fn_a".to_string(), "fn_b".to_string()]
        );
        g.declare_supervise(vec![DeclArg {
            label: "strategy".to_string(),
            ty: Type::Named("Restart".to_string(), vec![]),
            value: Value::Enum(0, vec![]),
        }]);
        assert_eq!(g.supervisions.len(), 1);
    }

    #[test]
    fn is_restricted_intrinsic_recognizes_exactly_the_graph_building_set() {
        use crate::sema::typed::is_restricted_intrinsic;
        for key in [
            "Image",
            "Image.device",
            "Image.driver",
            "Image.actor",
            "Image.pool",
            "Image.dma_pool",
            "Image.supervise",
            "Image.check_layout",
            "Image.seal",
            "ImageDecl.handle",
        ] {
            assert!(is_restricted_intrinsic(key), "{key} must be restricted");
        }
        for key in ["RestartIntensity", "seconds", "not_an_intrinsic"] {
            assert!(
                !is_restricted_intrinsic(key),
                "{key} must not be restricted"
            );
        }
    }

    #[test]
    fn dump_is_absent_for_empty_sections() {
        let g = ImageGraph::default();
        let enums = BTreeMap::new();
        let text = dump(&enums, &g);
        assert_eq!(text, "ImageGraph v0\n  Sealed value=false\n");
    }
}
