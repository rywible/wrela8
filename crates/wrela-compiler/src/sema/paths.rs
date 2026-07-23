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

// --- tests --------------------------------------------------------------
//
// 02-language.md §3: "exclusivity is checked on storage paths (fields,
// indexes, and potential overlaps), not variable names"; §3.2: moving out
// of an array through a runtime index is forbidden because "the analysis
// would depend on runtime history" — this module's own doc comment reads
// that as applying just as much to *proving* two paths distinct. These
// tests pin `overlaps`/`starts_with` (the predicates flow.rs's
// exclusivity/move analysis is keyed on, plans/M2.md item E/F) directly,
// since neither goldens nor the fuzzer confirm algebraic properties like
// symmetry.
#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::Span;

    /// Every path of depth 0..=`max_depth` reachable from `root` using the
    /// fixed step vocabulary below (two fields, two const indices, one
    /// runtime index) — small enough to enumerate exhaustively rather than
    /// sample.
    fn all_paths(root: &str, max_depth: usize) -> Vec<StoragePath> {
        let vocab: Vec<PathStep> = vec![
            PathStep::Field("f".to_string()),
            PathStep::Field("g".to_string()),
            PathStep::Index(0),
            PathStep::Index(1),
            PathStep::RuntimeIndex,
        ];
        let mut all = vec![StoragePath::root(root)];
        let mut frontier = vec![StoragePath::root(root)];
        for _ in 0..max_depth {
            let mut next = Vec::new();
            for p in &frontier {
                for step in &vocab {
                    let mut steps = p.steps.clone();
                    steps.push(step.clone());
                    next.push(StoragePath {
                        root: p.root.clone(),
                        steps,
                    });
                }
            }
            all.extend(next.iter().cloned());
            frontier = next;
        }
        all
    }

    /// `overlaps` must be reflexive (a path always overlaps itself — it is
    /// always at least as exclusive as itself) and symmetric (overlap is
    /// not directional) over every path up to depth 3 rooted at `a`/`b`.
    #[test]
    fn overlaps_is_reflexive_and_symmetric() {
        let mut paths = all_paths("a", 3);
        paths.extend(all_paths("b", 3));
        for p in &paths {
            assert!(
                p.overlaps(p),
                "{} should overlap itself (reflexive)",
                render_path(p)
            );
        }
        for p in &paths {
            for q in &paths {
                assert_eq!(
                    p.overlaps(q),
                    q.overlaps(p),
                    "overlaps must be symmetric for `{}` / `{}`",
                    render_path(p),
                    render_path(q)
                );
            }
        }
    }

    /// The named prefix/disjointness cases 02-language.md §3/§3.2 and this
    /// module's own doc comments spell out explicitly.
    #[test]
    fn overlaps_prefix_and_disjointness_cases() {
        let a = StoragePath::root("a");
        let b = StoragePath::root("b");
        let a_f = a.field("f");
        let a_g = a.field("g");
        let a_0 = a.index(PathStep::Index(0));
        let a_1 = a.index(PathStep::Index(1));
        let a_rt = a.index(PathStep::RuntimeIndex);
        let b_f = b.field("f");

        let cases: Vec<(&str, &StoragePath, &StoragePath, bool)> = vec![
            ("a overlaps a.f (a is a prefix of a.f)", &a, &a_f, true),
            (
                "a.f vs a.g disjoint (distinct fields never overlap)",
                &a_f,
                &a_g,
                false,
            ),
            (
                "a[0] vs a[1] disjoint (distinct const indices)",
                &a_0,
                &a_1,
                false,
            ),
            (
                "a[_] (runtime) overlaps a[0] (cannot prove distinct)",
                &a_rt,
                &a_0,
                true,
            ),
            (
                "a[_] (runtime) overlaps a[1] (cannot prove distinct)",
                &a_rt,
                &a_1,
                true,
            ),
            ("different roots never overlap", &a, &b, false),
            (
                "different roots, same field name, never overlap",
                &a_f,
                &b_f,
                false,
            ),
        ];
        for (msg, x, y, expected) in cases {
            assert_eq!(x.overlaps(y), expected, "{msg}");
        }
    }

    /// `starts_with` is the prefix relation `overlaps` is built on
    /// ("self is at or under other").
    #[test]
    fn starts_with_prefix_relation() {
        let a = StoragePath::root("a");
        let a_f = a.field("f");
        let a_f_g = a_f.field("g");
        assert!(a_f.starts_with(&a), "a.f is at-or-under a");
        assert!(a_f_g.starts_with(&a_f), "a.f.g is at-or-under a.f");
        assert!(a_f_g.starts_with(&a), "a.f.g is at-or-under a (transitive)");
        assert!(!a.starts_with(&a_f), "a is not under a.f (wrong direction)");
        assert!(a.starts_with(&a), "starts_with is reflexive (equal counts)");
    }

    /// `const_index_value` (decision 4's "unconstrained integer literal"
    /// reading, this module's own doc comment on `PathStep::Index`): a
    /// bare integer literal or a unary-negated one is constant; a `const`
    /// name or any other expression is deliberately *not* — even though it
    /// may in fact be a compile-time constant, this stage does not fold
    /// it.
    #[test]
    fn const_index_value_literal_only() {
        let span = Span::default();
        let cases: Vec<(&str, Expr, Option<i128>)> = vec![
            ("bare literal 0", Expr::Int(span, "0".to_string()), Some(0)),
            (
                "bare literal 42",
                Expr::Int(span, "42".to_string()),
                Some(42),
            ),
            (
                "unary-negated literal",
                Expr::Unary(
                    span,
                    UnaryOp::Neg,
                    Box::new(Expr::Int(span, "5".to_string())),
                ),
                Some(-5),
            ),
            (
                "a const name is not a literal here",
                Expr::Name(span, "N".to_string()),
                None,
            ),
            (
                "a non-Neg unary op on a literal is not a constant index",
                Expr::Unary(
                    span,
                    UnaryOp::BitNot,
                    Box::new(Expr::Int(span, "1".to_string())),
                ),
                None,
            ),
        ];
        for (msg, expr, expected) in cases {
            assert_eq!(const_index_value(&expr), expected, "{msg}");
        }
    }
}
