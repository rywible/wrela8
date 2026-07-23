//! Storage paths (plans/M2.md items E/F, shared): the representation the
//! flow pass's definite-initialization/move/exclusivity analysis is keyed
//! on — a place broken into its base root (a local, a parameter, or
//! `self`) plus a chain of field/index projections, so overlap is checked
//! on storage, not on variable names (02-language.md §3).
//!
//! Shape (deliverable 1): a root name plus `Vec<PathStep>`. Two paths
//! overlap exactly when one is a prefix of the other (including equal) —
//! distinct fields never overlap; a runtime index overlaps any index of
//! the same base (constant or runtime), since without knowing the runtime
//! value the compiler cannot prove they differ (02-language.md §3.2's
//! "moving out of an array through a runtime index is forbidden ... the
//! analysis would depend on runtime history" applies just as much to
//! overlap-proving).

use crate::syntax::ast::{Expr, UnaryOp};

/// One projection step from a path's parent onto a narrower place.
/// `Index` carries a compile-time-constant index (an integer literal,
/// optionally unary-negated — the only shape decision 4's "unconstrained
/// integer literal" treats as known at this stage; a bare `const` name or
/// any other expression is `RuntimeIndex` even if it happens to be
/// loop-invariant, per the dumbest-sound reading). Two `Index` steps with
/// different constants never overlap; every other index combination
/// (`RuntimeIndex` on either or both sides) does, since the compiler
/// cannot rule out aliasing.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathStep {
    Field(String),
    Index(i128),
    RuntimeIndex,
}

/// A storage path: a root name (local, parameter, or `self`) plus zero or
/// more projection steps, outermost-first.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct StoragePath {
    pub root: String,
    pub steps: Vec<PathStep>,
}

impl StoragePath {
    pub fn root(name: impl Into<String>) -> StoragePath {
        StoragePath {
            root: name.into(),
            steps: Vec::new(),
        }
    }

    pub fn field(&self, name: impl Into<String>) -> StoragePath {
        let mut p = self.clone();
        p.steps.push(PathStep::Field(name.into()));
        p
    }

    pub fn index(&self, step: PathStep) -> StoragePath {
        let mut p = self.clone();
        p.steps.push(step);
        p
    }

    pub fn is_root(&self) -> bool {
        self.steps.is_empty()
    }

    /// The path truncated to its first `len` steps (`len <= self.steps.len()`).
    pub fn prefix(&self, len: usize) -> StoragePath {
        StoragePath {
            root: self.root.clone(),
            steps: self.steps[..len].to_vec(),
        }
    }

    /// Whether `other` is a prefix of `self` (including `other == self`) —
    /// "self is at or under other" (used to find every tracked entry a
    /// whole-value use of `other` subsumes, and to clear stale descendant
    /// entries on a fresh assignment/take of `other`).
    pub fn starts_with(&self, other: &StoragePath) -> bool {
        self.root == other.root
            && self.steps.len() >= other.steps.len()
            && self.steps[..other.steps.len()] == other.steps[..]
    }

    /// Two storage paths overlap exactly when one is a prefix of the
    /// other (deliverable 1: "Overlap = one is a prefix of the other or
    /// they are equal; distinct fields never overlap; a runtime index
    /// overlaps any index of the same base").
    pub fn overlaps(&self, other: &StoragePath) -> bool {
        if self.root != other.root {
            return false;
        }
        let n = self.steps.len().min(other.steps.len());
        for i in 0..n {
            if !steps_overlap(&self.steps[i], &other.steps[i]) {
                return false;
            }
        }
        true
    }
}

fn steps_overlap(a: &PathStep, b: &PathStep) -> bool {
    match (a, b) {
        (PathStep::Field(x), PathStep::Field(y)) => x == y,
        (PathStep::Index(x), PathStep::Index(y)) => x == y,
        // A runtime index cannot be proven distinct from anything at the
        // same position — conservative (sound) overlap.
        (PathStep::RuntimeIndex, PathStep::Index(_))
        | (PathStep::Index(_), PathStep::RuntimeIndex)
        | (PathStep::RuntimeIndex, PathStep::RuntimeIndex) => true,
        // A field and an index never occur at the same position for a
        // well-typed program (a field projects a struct, an index a
        // fixed array/`Bytes`) — never overlap if it somehow arises.
        (PathStep::Field(_), PathStep::Index(_) | PathStep::RuntimeIndex)
        | (PathStep::Index(_) | PathStep::RuntimeIndex, PathStep::Field(_)) => false,
    }
}

/// A compile-time-constant integer index: a bare integer literal, or one
/// negated by unary `-` (decision 4's literal-only reading — a `const`
/// reference or any computed expression is deliberately *not* treated as
/// constant here, even though a real constant-folding pass could resolve
/// some of them; the dumbest sound reading, per the item's own
/// instructions, is to require a literal).
pub fn const_index_value(e: &Expr) -> Option<i128> {
    match e {
        Expr::Int(_, text) => crate::sema::bodies::parse_int_literal(text),
        Expr::Unary(_, UnaryOp::Neg, inner) => const_index_value(inner).map(|v| -v),
        _ => None,
    }
}

/// Renders a storage path back to source-like text for diagnostics
/// (`self.field[0]`), matching the language's own member/index syntax.
pub fn render_path(p: &StoragePath) -> String {
    let mut out = p.root.clone();
    for step in &p.steps {
        match step {
            PathStep::Field(name) => {
                out.push('.');
                out.push_str(name);
            }
            PathStep::Index(i) => {
                out.push('[');
                out.push_str(&i.to_string());
                out.push(']');
            }
            PathStep::RuntimeIndex => out.push_str("[_]"),
        }
    }
    out
}
