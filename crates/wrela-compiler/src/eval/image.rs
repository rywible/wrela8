use std::collections::BTreeMap;

use crate::eval::value::Value;
use crate::sema::types::{self, Type};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum ImageDeclRef {
    Device(usize),
    Driver(usize),
    Actor(usize),
    Renderer(usize),
    Pool(String),
    DmaPool(String),
}

impl ImageDeclRef {
    pub fn render(&self) -> String {
        match self {
            ImageDeclRef::Device(i) => format!("device#{i}"),
            ImageDeclRef::Driver(i) => format!("driver#{i}"),
            ImageDeclRef::Actor(i) => format!("actor#{i}"),
            ImageDeclRef::Renderer(i) => format!("renderer#{i}"),
            ImageDeclRef::Pool(name) => format!("pool:{name}"),
            ImageDeclRef::DmaPool(name) => format!("dma_pool:{name}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct TypedValue {
    pub ty: Type,
    pub value: Value,
}

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
pub struct RendererDecl {
    pub params_type: Type,
    pub actor_type: Type,
    pub args: Vec<DeclArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct PoolDecl {
    pub payload_type: Type,
    pub args: Vec<DeclArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct OnFailureDecl {
    pub args: Vec<DeclArg>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct LayoutAssertDecl {
    pub fn_key: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ImageGraph {
    pub name: Option<TypedValue>,
    pub target: Option<TypedValue>,
    pub cores: usize,
    pub devices: Vec<DeviceDecl>,
    pub drivers: Vec<DriverDecl>,
    pub actors: Vec<ActorDecl>,
    pub renderers: Vec<RendererDecl>,
    pub pools: BTreeMap<String, PoolDecl>,
    pub dma_pools: BTreeMap<String, PoolDecl>,
    pub on_failures: Vec<OnFailureDecl>,
    pub layout_asserts: Vec<LayoutAssertDecl>,
    pub sealed: bool,
}

impl Default for ImageGraph {
    fn default() -> ImageGraph {
        ImageGraph {
            name: None,
            target: None,
            cores: 1,
            devices: Vec::new(),
            drivers: Vec::new(),
            actors: Vec::new(),
            renderers: Vec::new(),
            pools: BTreeMap::new(),
            dma_pools: BTreeMap::new(),
            on_failures: Vec::new(),
            layout_asserts: Vec::new(),
            sealed: false,
        }
    }
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

    pub fn declare_renderer(&mut self, params_type: Type, args: Vec<DeclArg>) -> Value {
        let idx = self.renderers.len();
        let actor_type = Type::Named(
            "Renderer".to_string(),
            vec![types::TypeArg::Type(params_type.clone())],
        );
        self.renderers.push(RendererDecl {
            params_type,
            actor_type,
            args,
        });
        Value::ImageDecl(ImageDeclRef::Renderer(idx))
    }

    fn bind_pool_name(&self, pool_name: &str) -> Result<(), String> {
        if self.pools.contains_key(pool_name) || self.dma_pools.contains_key(pool_name) {
            return Err(format!("pool `{pool_name}` is already bound"));
        }
        Ok(())
    }

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

    pub fn declare_on_failure(&mut self, args: Vec<DeclArg>) {
        self.on_failures.push(OnFailureDecl { args });
    }

    pub fn declare_check_layout(&mut self, fn_key: String) {
        self.layout_asserts.push(LayoutAssertDecl { fn_key });
    }
}

pub(crate) fn push_line(out: &mut String, depth: usize, line: &str) {
    out.push_str(&"  ".repeat(depth));
    out.push_str(line);
    out.push('\n');
}

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

pub struct TypedProgramEnums<'p> {
    pub enums: &'p BTreeMap<String, Vec<String>>,
}

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
    push_line(&mut out, 1, &format!("Cores count={}", graph.cores));
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
    for (i, d) in graph.renderers.iter().enumerate() {
        push_line(
            &mut out,
            1,
            &format!(
                "Renderer index={i} params={} actor={}",
                types::render_type(&d.params_type),
                types::render_type(&d.actor_type)
            ),
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
    for (i, s) in graph.on_failures.iter().enumerate() {
        push_line(&mut out, 1, &format!("OnFailure index={i}"));
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
    fn on_failure_and_check_layout_are_recorded_in_order() {
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
        g.declare_on_failure(vec![DeclArg {
            label: "policy".to_string(),
            ty: Type::Named("Failure".to_string(), vec![]),
            value: Value::Enum(1, vec![]),
        }]);
        assert_eq!(g.on_failures.len(), 1);
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
            "Image.on_failure",
            "Image.check_layout",
            "Image.seal",
            "ImageDecl.handle",
        ] {
            assert!(is_restricted_intrinsic(key), "{key} must be restricted");
        }
        for key in ["seconds", "not_an_intrinsic"] {
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
        assert_eq!(
            text,
            "ImageGraph v0\n  Cores count=1\n  Sealed value=false\n"
        );
    }

    #[test]
    fn cores_default_is_one() {
        let g = ImageGraph::default();
        assert_eq!(g.cores, 1);
        let g2 = ImageGraph::new(
            tv(
                Type::Static(Box::new(Type::Str)),
                Value::Str(b"img".to_vec()),
            ),
            tv(
                Type::Named("Target".to_string(), vec![]),
                Value::Enum(0, vec![]),
            ),
        );
        assert_eq!(g2.cores, 1);
    }
}
