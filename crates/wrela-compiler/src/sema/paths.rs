use crate::syntax::ast::{Expr, UnaryOp};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub enum PathStep {
    Field(String),
    Index(i128),
    RuntimeIndex,
}

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

    pub fn prefix(&self, len: usize) -> StoragePath {
        StoragePath {
            root: self.root.clone(),
            steps: self.steps[..len].to_vec(),
        }
    }

    pub fn starts_with(&self, other: &StoragePath) -> bool {
        self.root == other.root
            && self.steps.len() >= other.steps.len()
            && self.steps[..other.steps.len()] == other.steps[..]
    }

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
        (PathStep::RuntimeIndex, PathStep::Index(_))
        | (PathStep::Index(_), PathStep::RuntimeIndex)
        | (PathStep::RuntimeIndex, PathStep::RuntimeIndex) => true,
        (PathStep::Field(_), PathStep::Index(_) | PathStep::RuntimeIndex)
        | (PathStep::Index(_) | PathStep::RuntimeIndex, PathStep::Field(_)) => false,
    }
}

pub fn const_index_value(e: &Expr) -> Option<i128> {
    match e {
        Expr::Int(_, text) => crate::sema::bodies::parse_int_literal(text),
        Expr::Unary(_, UnaryOp::Neg, inner) => const_index_value(inner).map(|v| -v),
        _ => None,
    }
}

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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::syntax::ast::Span;

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
