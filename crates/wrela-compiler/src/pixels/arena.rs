//! Append-only arenas with checked access and parallel source origins.

use std::marker::PhantomData;

use crate::syntax::ast::Span;

use super::ids::DenseId;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OriginSite {
    pub module: String,
    pub span: Span,
}

impl PartialOrd for OriginSite {
    fn partial_cmp(&self, other: &Self) -> Option<std::cmp::Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for OriginSite {
    fn cmp(&self, other: &Self) -> std::cmp::Ordering {
        (
            &self.module,
            self.span.byte_start,
            self.span.byte_end,
            self.span.line,
            self.span.col,
        )
            .cmp(&(
                &other.module,
                other.span.byte_start,
                other.span.byte_end,
                other.span.line,
                other.span.col,
            ))
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NodeOrigin {
    pub primary: OriginSite,
    pub expansion_chain: Vec<OriginSite>,
    pub merged: Vec<OriginSite>,
}

impl NodeOrigin {
    pub fn new(module: impl Into<String>, span: Span, expansion_chain: Vec<OriginSite>) -> Self {
        Self {
            primary: OriginSite {
                module: module.into(),
                span,
            },
            expansion_chain,
            merged: Vec::new(),
        }
    }

    pub fn synthetic(label: impl Into<String>) -> Self {
        Self::new(label, Span::default(), Vec::new())
    }

    pub fn merge(&mut self, other: &Self) {
        let mut sites = Vec::new();
        sites.extend(self.merged.iter().cloned());
        sites.push(other.primary.clone());
        sites.extend(other.expansion_chain.iter().cloned());
        sites.extend(other.merged.iter().cloned());
        sites.retain(|site| site != &self.primary && !self.expansion_chain.contains(site));
        sites.sort();
        sites.dedup();
        self.merged = sites;
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DebugArenaId<I> {
    id: I,
    arena_serial: u32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Arena<I, T> {
    nodes: Vec<T>,
    origins: Vec<NodeOrigin>,
    arena_serial: u32,
    marker: PhantomData<fn(I) -> I>,
}

impl<I: DenseId, T> Arena<I, T> {
    pub fn new(arena_serial: u32) -> Self {
        Self {
            nodes: Vec::new(),
            origins: Vec::new(),
            arena_serial,
            marker: PhantomData,
        }
    }

    pub fn serial(&self) -> u32 {
        self.arena_serial
    }

    pub fn push(&mut self, node: T, origin: NodeOrigin) -> Result<I, String> {
        let id = I::from_index(self.nodes.len())?;
        self.nodes.push(node);
        self.origins.push(origin);
        Ok(id)
    }

    pub fn get(&self, id: I) -> Result<&T, String> {
        self.nodes.get(id.index()).ok_or_else(|| {
            format!(
                "pixels::arena: stale ID {} for arena of length {}",
                id.index(),
                self.nodes.len()
            )
        })
    }

    pub fn get_mut(&mut self, id: I) -> Result<&mut T, String> {
        let len = self.nodes.len();
        self.nodes.get_mut(id.index()).ok_or_else(|| {
            format!(
                "pixels::arena: stale ID {} for arena of length {len}",
                id.index()
            )
        })
    }

    pub fn origin(&self, id: I) -> Result<&NodeOrigin, String> {
        self.origins.get(id.index()).ok_or_else(|| {
            format!(
                "pixels::arena: missing origin for ID {} in arena of length {}",
                id.index(),
                self.origins.len()
            )
        })
    }

    pub fn origin_mut(&mut self, id: I) -> Result<&mut NodeOrigin, String> {
        let len = self.origins.len();
        self.origins.get_mut(id.index()).ok_or_else(|| {
            format!(
                "pixels::arena: missing origin for ID {} in arena of length {len}",
                id.index()
            )
        })
    }

    pub fn iter(&self) -> impl ExactSizeIterator<Item = (I, &T)> {
        self.nodes.iter().enumerate().map(|(index, node)| {
            (
                I::from_index(index).expect("existing arena index fits"),
                node,
            )
        })
    }

    pub fn len(&self) -> usize {
        self.nodes.len()
    }

    pub fn is_empty(&self) -> bool {
        self.nodes.is_empty()
    }

    pub fn debug_id(&self, id: I) -> Result<DebugArenaId<I>, String> {
        self.get(id)?;
        Ok(DebugArenaId {
            id,
            arena_serial: self.arena_serial,
        })
    }

    pub fn debug_get(&self, id: DebugArenaId<I>) -> Result<&T, String> {
        if id.arena_serial != self.arena_serial {
            return Err("pixels::arena: ID belongs to a different arena".to_string());
        }
        self.get(id.id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pixels::ids::ScalarId;

    #[test]
    fn insertion_order_and_origin_coverage_are_exact() {
        let mut arena = Arena::<ScalarId, _>::new(1);
        let a = arena.push("a", NodeOrigin::synthetic("a")).unwrap();
        let b = arena.push("b", NodeOrigin::synthetic("b")).unwrap();
        assert_eq!(
            arena.iter().map(|(_, node)| *node).collect::<Vec<_>>(),
            ["a", "b"]
        );
        assert_eq!(arena.origin(a).unwrap().primary.module, "a");
        assert_eq!(arena.origin(b).unwrap().primary.module, "b");
    }

    #[test]
    fn merged_origins_do_not_repeat_primary_or_expansion_sites() {
        let primary = OriginSite {
            module: "primary".to_string(),
            span: Span::default(),
        };
        let expansion = OriginSite {
            module: "expansion".to_string(),
            span: Span::default(),
        };
        let additional = OriginSite {
            module: "additional".to_string(),
            span: Span::default(),
        };
        let mut origin = NodeOrigin {
            primary: primary.clone(),
            expansion_chain: vec![expansion.clone()],
            merged: vec![primary.clone()],
        };
        origin.merge(&NodeOrigin {
            primary: additional.clone(),
            expansion_chain: vec![expansion],
            merged: vec![primary],
        });
        assert_eq!(origin.merged, vec![additional]);
    }

    #[test]
    fn stale_and_wrong_arena_ids_are_checked() {
        let mut a = Arena::<ScalarId, _>::new(10);
        let mut b = Arena::<ScalarId, _>::new(11);
        let id = a.push(1, NodeOrigin::synthetic("a")).unwrap();
        b.push(2, NodeOrigin::synthetic("b")).unwrap();
        assert!(a.get(ScalarId(9)).unwrap_err().contains("stale ID"));
        let debug = a.debug_id(id).unwrap();
        assert!(b.debug_get(debug).unwrap_err().contains("different arena"));
    }
}
