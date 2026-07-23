//! Dumb pretty-printer: `AST -> wrela source text` (plans/M1.md item E,
//! `wrela dump --stage=pretty`). This is the parser's `diff-eval`: a second,
//! independent implementation that a parsed tree must survive being pushed
//! back through — `xtask roundtrip` parses, prints, reparses, and diffs the
//! two (span-stripped) AST dumps.
//!
//! Two rules, applied uniformly instead of computing real operator
//! precedence: (1) every statement/declaration is printed at its own
//! logical indentation depth, 4 spaces per level, exactly mirroring what
//! the lexer's INDENT/DEDENT rule expects; (2) every expression that is a
//! child of another expression is wrapped in parens unless it is one of a
//! fixed set of self-delimiting ("atomic") forms — literals, names, and
//! anything already bounded by its own brackets/keywords (`.field`,
//! `[index]`, `(call)`, tuples, lists, `.Variant`). This over-parenthesizes
//! relative to what's strictly necessary (plans/M1.md item E explicitly
//! allows this: "canonical output need not match input text, only reparse
//! to the same AST"), but it is always correct, because it needs no
//! precedence table at all: a real AST from this parser can only nest a
//! *looser* node inside a *tighter* position via an explicit source-level
//! grouping (parens are transparent — dropped at parse time, ast.rs), and
//! wrapping every non-atomic child in its own parens exactly reconstructs
//! that grouping regardless of why it was there.
//!
//! `Expr::Closure` is deliberately excluded from the "atomic" set even
//! though it is self-delimiting on its own (`|params| ...`): its
//! short-form body is parsed via the same greedy `parse_or` used
//! everywhere else, so a closure written as a non-final operand of an
//! enclosing operator chain (only reachable via an explicit source
//! grouping, e.g. `(|x| x) + 1`) would otherwise swallow whatever follows
//! it on reparse. Always parenthesizing it when nested is the safe,
//! dumb answer.

use super::ast::*;
use super::parser::FragmentEntry;

const INDENT_UNIT: &str = "    ";

pub fn pretty(module: &Module) -> String {
    print_module(module)
}

/// Bare textual rendering of one expression, reusing the pretty-printer's
/// own (dumb, always-safe-to-over-parenthesize) expression logic. sema's
/// resolved `Type` (types.rs) embeds unevaluated `Expr`s for array
/// lengths, `Bytes[N]`, and generic const arguments (plans/M2.md item B
/// decision 4: const arguments stay unevaluated until item H); this is
/// its one rendering primitive, so the check dump's spelling of those
/// stays byte-identical to the pretty-printer's rather than duplicating
/// the expression-printing rules.
pub(crate) fn print_expr_bare(e: &Expr) -> String {
    print_expr(e, 0)
}

/// Pretty-prints a bare fragment (`parse_fragment`'s result): each
/// top-level item or statement in source order, blank-line separated.
pub fn pretty_fragment(entries: &[FragmentEntry]) -> String {
    let mut out = String::new();
    for (i, entry) in entries.iter().enumerate() {
        if i > 0 {
            out.push('\n');
        }
        match entry {
            FragmentEntry::Item(item) => print_item(item, 0, &mut out),
            FragmentEntry::Stmt(stmt) => print_stmt(stmt, 0, &mut out),
        }
    }
    out
}

fn indent_str(n: usize) -> String {
    INDENT_UNIT.repeat(n)
}

/// Appends one logical line: `indent` levels of 4-space indent, `line`,
/// then a newline. The workhorse of every statement/declaration printer
/// below.
fn push_line(out: &mut String, indent: usize, line: &str) {
    out.push_str(&indent_str(indent));
    out.push_str(line);
    out.push('\n');
}

/// `(elems...)`, with the one-element case forced to a trailing comma
/// (`(x,)`) — without it, `parse_type`/`parse_primary`/`parse_pattern`'s
/// shared "`(` ... `)`" grouping logic returns the single inner node
/// unwrapped rather than building a real `Tuple`/`TupleType`/tuple
/// `Pattern`, per their own "pure grouping" fallback (ast.rs, `is_place_expr`
/// doc comment; parser.rs `parse_type`/`parse_primary`/`parse_pattern_primary`).
fn tuple_like<T>(elems: &[T], f: impl Fn(&T) -> String) -> String {
    match elems.len() {
        0 => "()".to_string(),
        1 => format!("({},)", f(&elems[0])),
        _ => format!("({})", elems.iter().map(f).collect::<Vec<_>>().join(", ")),
    }
}

// --- module / doc / attrs / imports ----------------------------------------

fn print_module(m: &Module) -> String {
    let mut out = String::new();
    print_doc(&m.doc, 0, &mut out);
    push_line(&mut out, 0, &format!("module {}", m.path.join(".")));
    if !m.imports.is_empty() {
        out.push('\n');
        for import in &m.imports {
            print_import(import, &mut out);
        }
    }
    for item in &m.items {
        out.push('\n');
        print_item(item, 0, &mut out);
    }
    out
}

/// `## text` per line — `Doc.text` joins consecutive `##` lines with `\n`
/// (ast.rs); each line here always gets exactly one space after `##` so
/// the parser's `strip_doc_leading_space` (which removes exactly one
/// leading space, if present) reconstructs the original line byte-for-byte
/// even when it was empty.
fn print_doc(doc: &Option<Doc>, indent: usize, out: &mut String) {
    if let Some(doc) = doc {
        for line in doc.text.split('\n') {
            push_line(out, indent, &format!("## {line}"));
        }
    }
}

fn print_attrs(attrs: &[Attr], indent: usize, out: &mut String) {
    for attr in attrs {
        let mut line = format!("@{}", attr.name);
        if !attr.args.is_empty() {
            line.push('(');
            line.push_str(&print_args(&attr.args, indent));
            line.push(')');
        }
        push_line(out, indent, &line);
    }
}

fn print_import(import: &Import, out: &mut String) {
    let mut line = String::new();
    if import.is_pub {
        line.push_str("pub ");
    }
    line.push_str("from ");
    line.push_str(&import.path.join("."));
    line.push_str(" import ");
    let names = import
        .names
        .iter()
        .map(|n| match &n.alias {
            Some(a) => format!("{} as {a}", n.name),
            None => n.name.clone(),
        })
        .collect::<Vec<_>>()
        .join(", ");
    line.push_str(&names);
    push_line(out, 0, &line);
}

fn mode_prefix(mode: AccessMode) -> &'static str {
    match mode {
        AccessMode::Read => "",
        AccessMode::Mut => "mut ",
        AccessMode::Take => "take ",
    }
}

fn print_generics_suffix(generics: &[GenericParam], indent: usize, line: &mut String) {
    if generics.is_empty() {
        return;
    }
    let parts: Vec<String> = generics
        .iter()
        .map(|g| match g {
            GenericParam::Type { name, .. } => name.clone(),
            GenericParam::Const { name, ty, .. } => {
                format!("const {name}: {}", print_type(ty, indent))
            }
        })
        .collect();
    line.push('[');
    line.push_str(&parts.join(", "));
    line.push(']');
}

// --- items / members --------------------------------------------------

fn print_item(item: &Item, indent: usize, out: &mut String) {
    match item {
        Item::Const(c) => {
            print_doc(&c.doc, indent, out);
            print_attrs(&c.attrs, indent, out);
            let mut line = String::new();
            if c.is_pub {
                line.push_str("pub ");
            }
            line.push_str("const ");
            line.push_str(&c.name);
            if let Some(ty) = &c.ty {
                line.push_str(": ");
                line.push_str(&print_type(ty, indent));
            }
            line.push_str(" = ");
            line.push_str(&print_expr(&c.value, indent));
            push_line(out, indent, &line);
        }
        Item::Fn(f) => {
            print_doc(&f.doc, indent, out);
            print_attrs(&f.attrs, indent, out);
            print_fn_header_and_body(
                &format!("fn {}", f.name),
                f.is_pub,
                f.is_async,
                &f.generics,
                &f.receiver,
                &f.params,
                &f.ret,
                &f.body,
                indent,
                out,
            );
        }
        Item::Struct(s) => {
            print_doc(&s.doc, indent, out);
            print_attrs(&s.attrs, indent, out);
            let mut line = String::new();
            if s.is_pub {
                line.push_str("pub ");
            }
            if s.is_resource {
                line.push_str("resource ");
            }
            line.push_str("struct ");
            line.push_str(&s.name);
            print_generics_suffix(&s.generics, indent, &mut line);
            print_deriving_suffix(&s.deriving, &mut line);
            line.push(':');
            push_line(out, indent, &line);
            for m in &s.members {
                print_member(m, indent + 1, out);
            }
        }
        Item::Enum(e) => {
            print_doc(&e.doc, indent, out);
            print_attrs(&e.attrs, indent, out);
            let mut line = String::new();
            if e.is_pub {
                line.push_str("pub ");
            }
            line.push_str("enum ");
            line.push_str(&e.name);
            print_generics_suffix(&e.generics, indent, &mut line);
            print_deriving_suffix(&e.deriving, &mut line);
            line.push(':');
            push_line(out, indent, &line);
            for v in &e.variants {
                print_variant(v, indent + 1, out);
            }
        }
        Item::Pool(p) => {
            print_doc(&p.doc, indent, out);
            print_attrs(&p.attrs, indent, out);
            push_line(out, indent, &format!("pool {}", p.name));
        }
        Item::ComptimeIf(c) => {
            print_doc(&c.doc, indent, out);
            print_attrs(&c.attrs, indent, out);
            push_line(
                out,
                indent,
                &format!("comptime if {}:", print_expr(&c.cond, indent)),
            );
            for it in &c.then_branch {
                print_item(it, indent + 1, out);
            }
            if let Some(else_branch) = &c.else_branch {
                push_line(out, indent, "comptime else:");
                for it in else_branch {
                    print_item(it, indent + 1, out);
                }
            }
        }
    }
}

fn print_deriving_suffix(deriving: &[String], line: &mut String) {
    if deriving.is_empty() {
        return;
    }
    line.push_str(" deriving(");
    line.push_str(&deriving.join(", "));
    line.push(')');
}

fn print_member(member: &Member, indent: usize, out: &mut String) {
    match member {
        Member::Field(f) => {
            print_doc(&f.doc, indent, out);
            print_attrs(&f.attrs, indent, out);
            let mut line = String::new();
            if f.is_pub {
                line.push_str("pub ");
            }
            line.push_str(&f.name);
            line.push_str(": ");
            line.push_str(&print_type(&f.ty, indent));
            if let Some(default) = &f.default {
                line.push_str(" = ");
                line.push_str(&print_expr(default, indent));
            }
            push_line(out, indent, &line);
        }
        Member::Fn(f) => {
            print_doc(&f.doc, indent, out);
            print_attrs(&f.attrs, indent, out);
            print_fn_header_and_body(
                &format!("fn {}", f.name),
                f.is_pub,
                f.is_async,
                &f.generics,
                &f.receiver,
                &f.params,
                &f.ret,
                &f.body,
                indent,
                out,
            );
        }
        Member::Init(i) => {
            print_doc(&i.doc, indent, out);
            print_attrs(&i.attrs, indent, out);
            print_fn_header_and_body(
                "init",
                false,
                false,
                &[],
                &Some(i.receiver.clone()),
                &i.params,
                &i.ret,
                &Some(i.body.clone()),
                indent,
                out,
            );
        }
        Member::Pool(p) => {
            print_doc(&p.doc, indent, out);
            print_attrs(&p.attrs, indent, out);
            push_line(out, indent, &format!("pool {}", p.name));
        }
        Member::ComptimeIf(c) => {
            print_doc(&c.doc, indent, out);
            print_attrs(&c.attrs, indent, out);
            push_line(
                out,
                indent,
                &format!("comptime if {}:", print_expr(&c.cond, indent)),
            );
            for m in &c.then_branch {
                print_member(m, indent + 1, out);
            }
            if let Some(else_branch) = &c.else_branch {
                push_line(out, indent, "comptime else:");
                for m in else_branch {
                    print_member(m, indent + 1, out);
                }
            }
        }
    }
}

fn print_variant(v: &Variant, indent: usize, out: &mut String) {
    let mut line = v.name.clone();
    match &v.payload {
        VariantPayload::None => {}
        VariantPayload::Tuple(types) => {
            line.push('(');
            line.push_str(
                &types
                    .iter()
                    .map(|t| print_type(t, indent))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            line.push(')');
        }
        VariantPayload::Named(fields) => {
            line.push('(');
            line.push_str(
                &fields
                    .iter()
                    .map(|f| format!("{}: {}", f.name, print_type(&f.ty, indent)))
                    .collect::<Vec<_>>()
                    .join(", "),
            );
            line.push(')');
        }
    }
    push_line(out, indent, &line);
}

/// Shared by `fn`/`init` items and members: `[pub] [async] KEYWORD_NAME
/// [generics](params) [-> Ret]:` header, then either a printed suite body
/// or (the rare bodyless signature shorthand, ast.rs `FnItem` doc comment)
/// nothing but the header line.
#[allow(clippy::too_many_arguments)]
fn print_fn_header_and_body(
    keyword_name: &str,
    is_pub: bool,
    is_async: bool,
    generics: &[GenericParam],
    receiver: &Option<Receiver>,
    params: &[Param],
    ret: &Option<Type>,
    body: &Option<Vec<Stmt>>,
    indent: usize,
    out: &mut String,
) {
    let mut line = String::new();
    if is_pub {
        line.push_str("pub ");
    }
    if is_async {
        line.push_str("async ");
    }
    line.push_str(keyword_name);
    print_generics_suffix(generics, indent, &mut line);
    line.push('(');
    let mut parts = Vec::new();
    if let Some(r) = receiver {
        parts.push(format!("{}self", mode_prefix(r.mode)));
    }
    for p in params {
        let mut s = format!(
            "{}{}: {}",
            mode_prefix(p.mode),
            p.name,
            print_type(&p.ty, indent)
        );
        if let Some(default) = &p.default {
            s.push_str(" = ");
            s.push_str(&print_expr(default, indent));
        }
        parts.push(s);
    }
    line.push_str(&parts.join(", "));
    line.push(')');
    if let Some(ret) = ret {
        line.push_str(" -> ");
        line.push_str(&print_type(ret, indent));
    }
    match body {
        Some(stmts) => {
            line.push(':');
            push_line(out, indent, &line);
            print_stmts(stmts, indent + 1, out);
        }
        None => push_line(out, indent, &line),
    }
}

// --- types (02-language.md §6) ------------------------------------------

fn print_type(ty: &Type, indent: usize) -> String {
    match ty {
        Type::Named(t) => {
            if t.args.is_empty() {
                t.name.clone()
            } else {
                format!(
                    "{}[{}]",
                    t.name,
                    t.args
                        .iter()
                        .map(|a| print_generic_arg(a, indent))
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        Type::Array(t) => format!(
            "[{}; {}]",
            print_type(&t.elem, indent),
            print_expr(&t.len, indent)
        ),
        Type::Tuple(t) => tuple_like(&t.elems, |elem| print_type(elem, indent)),
        Type::Own(t) => format!("own[{}] {}", t.pool.join("."), print_type(&t.inner, indent)),
        Type::Fn(t) => {
            let params = t
                .params
                .iter()
                .map(|p| format!("{}{}", mode_prefix(p.mode), print_type(&p.ty, indent)))
                .collect::<Vec<_>>()
                .join(", ");
            match &t.ret {
                Some(ret) => format!("fn({params}) -> {}", print_type(ret, indent)),
                None => format!("fn({params})"),
            }
        }
    }
}

fn print_generic_arg(arg: &GenericArg, indent: usize) -> String {
    match arg {
        GenericArg::Type(t) => print_type(t, indent),
        GenericArg::Expr(e) => print_expr(e, indent),
        GenericArg::Bound(e) => format!("..{}", print_expr(e, indent)),
    }
}

// --- patterns (02-language.md §7.2) -------------------------------------
//
// No `is_atomic`/operand distinction needed here: patterns have no
// operator-precedence chain (their only "loose" construct, `Or`, is only
// ever built by the outer `parse_pattern`, never nested inside a tighter
// pattern position), so every sub-pattern prints bare.

fn print_pattern(p: &Pattern) -> String {
    match p {
        Pattern::Wildcard(_) => "_".to_string(),
        // Always a plain literal or a `Unary(Neg, literal)` (parser.rs
        // `parse_pattern_primary`) — never a closure, so no indent needed.
        Pattern::Literal(_, e) => print_expr(e, 0),
        Pattern::Binding(_, name) => name.clone(),
        Pattern::Take(_, inner) => format!("take {}", print_pattern(inner)),
        Pattern::Variant {
            enum_name,
            variant,
            payload,
            ..
        } => {
            let base = match enum_name {
                Some(en) => format!("{en}.{variant}"),
                None => format!(".{variant}"),
            };
            if payload.is_empty() {
                base
            } else {
                format!(
                    "{base}({})",
                    payload
                        .iter()
                        .map(print_pattern)
                        .collect::<Vec<_>>()
                        .join(", ")
                )
            }
        }
        Pattern::Tuple(_, elems) => tuple_like(elems, |p| print_pattern(p)),
        Pattern::Array(_, elems) => format!(
            "[{}]",
            elems
                .iter()
                .map(print_pattern)
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Pattern::Or(_, alts) => alts
            .iter()
            .map(print_pattern)
            .collect::<Vec<_>>()
            .join(" | "),
    }
}

// --- expressions (02-language.md §8.2) -----------------------------------

/// True for expression forms that never need surrounding parens no matter
/// what encloses them — either true leaves, or forms already bounded by
/// their own brackets/keywords. `Closure` is deliberately excluded (module
/// doc comment above).
fn is_atomic(e: &Expr) -> bool {
    matches!(
        e,
        Expr::Int(..)
            | Expr::Float(..)
            | Expr::Str(..)
            | Expr::BStr(..)
            | Expr::Char(..)
            | Expr::FStr(..)
            | Expr::Bool(..)
            | Expr::Unit(..)
            | Expr::Name(..)
            | Expr::Field(..)
            | Expr::Index(..)
            | Expr::Call(..)
            | Expr::Tuple(..)
            | Expr::List(..)
            | Expr::DotVariant(..)
    )
}

/// Prints `e` as a sub-expression (an operand of some enclosing operator,
/// or the base of a postfix chain): wraps in parens unless `e` is atomic.
fn print_operand(e: &Expr, indent: usize) -> String {
    if is_atomic(e) {
        print_expr(e, indent)
    } else {
        format!("({})", print_expr(e, indent))
    }
}

/// Prints `e` bare — for genuine top-level positions (a statement's whole
/// expression, a call/index/list/tuple element, a default value, ...)
/// where the surrounding grammar already bounds the expression with a
/// hard delimiter (newline, comma, closing bracket) that no operator-chain
/// function recognizes as a continuation, so no ambiguity is possible.
fn print_expr(e: &Expr, indent: usize) -> String {
    match e {
        Expr::Int(_, text)
        | Expr::Float(_, text)
        | Expr::Str(_, text)
        | Expr::BStr(_, text)
        | Expr::Char(_, text) => text.clone(),
        Expr::FStr(f) => print_fstring(f),
        Expr::Bool(_, v) => v.to_string(),
        Expr::Unit(_) => "unit".to_string(),
        Expr::Name(_, name) => name.clone(),
        Expr::Field(base, _, name) => format!("{}.{}", print_operand(base, indent), name),
        Expr::Index(base, _, args) => format!(
            "{}[{}]",
            print_operand(base, indent),
            args.iter()
                .map(|a| print_expr(a, indent))
                .collect::<Vec<_>>()
                .join(", ")
        ),
        Expr::Call(callee, _, args) => format!(
            "{}({})",
            print_operand(callee, indent),
            print_args(args, indent)
        ),
        Expr::Unary(_, op, inner) => {
            let opd = print_operand(inner, indent);
            match op {
                UnaryOp::Neg => format!("-{opd}"),
                UnaryOp::BitNot => format!("~{opd}"),
                UnaryOp::Await => format!("await {opd}"),
                UnaryOp::Take => format!("take {opd}"),
            }
        }
        Expr::Try(_, inner) => format!("{}?", print_operand(inner, indent)),
        Expr::Binary(_, op, l, r) => format!(
            "{} {} {}",
            print_operand(l, indent),
            op.as_str(),
            print_operand(r, indent)
        ),
        Expr::Range(_, l, r, inclusive) => format!(
            "{}{}{}",
            print_operand(l, indent),
            if *inclusive { "..=" } else { ".." },
            print_operand(r, indent)
        ),
        Expr::Is(_, l, pat) => format!("{} is {}", print_operand(l, indent), print_pattern(pat)),
        Expr::Not(_, inner) => format!("not {}", print_operand(inner, indent)),
        Expr::And(_, l, r) => format!(
            "{} and {}",
            print_operand(l, indent),
            print_operand(r, indent)
        ),
        Expr::Or(_, l, r) => format!(
            "{} or {}",
            print_operand(l, indent),
            print_operand(r, indent)
        ),
        Expr::DotVariant(_, name, args) => {
            if args.is_empty() {
                format!(".{name}")
            } else {
                format!(".{name}({})", print_args(args, indent))
            }
        }
        Expr::Closure(c) => print_closure(c, indent),
        // `send`'s operand is always a `Call` (atomic) — parser.rs
        // `parse_unary` rejects anything else — so bare `print_expr` and
        // `print_operand` agree here; bare reads more naturally.
        Expr::Send(_, inner) => format!("send {}", print_expr(inner, indent)),
        Expr::Tuple(_, elems) => tuple_like(elems, |elem| print_expr(elem, indent)),
        Expr::List(_, elems) => format!(
            "[{}]",
            elems
                .iter()
                .map(|elem| print_expr(elem, indent))
                .collect::<Vec<_>>()
                .join(", ")
        ),
    }
}

/// `f"..."` reassembly (02-language.md §1.1): a `Literal` part is raw
/// source text with `{`/`}` already un-escaped by the parser's
/// `split_fstring` (ast.rs `FStringPart` doc comment), so re-escaping them
/// back to `{{`/`}}` here exactly inverts that step; an `Interp` part's
/// text is raw and unparsed (M1 does not recurse into it), so it is
/// wrapped in braces and echoed verbatim.
fn print_fstring(f: &FStringLit) -> String {
    let mut body = String::new();
    for part in &f.parts {
        match part {
            FStringPart::Literal(_, text) => {
                body.push_str(&text.replace('{', "{{").replace('}', "}}"));
            }
            FStringPart::Interp(_, text) => {
                body.push('{');
                body.push_str(text);
                body.push('}');
            }
        }
    }
    format!("f\"{body}\"")
}

fn print_args(args: &[Arg], indent: usize) -> String {
    args.iter()
        .map(|a| print_arg(a, indent))
        .collect::<Vec<_>>()
        .join(", ")
}

fn print_arg(arg: &Arg, indent: usize) -> String {
    let mut s = String::new();
    if let Some(label) = &arg.label {
        s.push_str(label);
        s.push('=');
    }
    s.push_str(mode_prefix(arg.mode));
    s.push_str(&print_expr(&arg.value, indent));
    s
}

/// `|params| expr` or `|params|: suite` (02-language.md §8.3). A suite
/// body is printed as a normal indented block one level deeper than the
/// closure's own enclosing line, exactly like any other suite — see the
/// module doc comment on why this reparses correctly even when the
/// closure sits inside an enclosing `()[]{}` that suppresses layout
/// tokens (lexer.rs's layout-island doc comment; parser.rs's own module
/// doc comment on embedded suites).
fn print_closure(c: &ClosureExpr, indent: usize) -> String {
    let params = c
        .params
        .iter()
        .map(|p| {
            let mut s = format!("{}{}", mode_prefix(p.mode), p.name);
            if let Some(ty) = &p.ty {
                s.push_str(": ");
                s.push_str(&print_type(ty, indent));
            }
            s
        })
        .collect::<Vec<_>>()
        .join(", ");
    match &c.body {
        ClosureBody::Expr(e) => format!("|{params}| {}", print_expr(e, indent)),
        ClosureBody::Suite(stmts) => {
            // Keep `print_stmts`'s own trailing newline (ledger clause
            // syntax.lexer.layout-islands, sema-roundtrip fuzz finding): a
            // suite-form closure can be a non-final call argument
            // (`f(x, |p|:\n    a\n    b, y)`), and whatever the caller
            // glues on after this string — a `, next_arg`, a bare `)` —
            // must start on its own fresh line so the last statement gets
            // a real terminator instead of that following text landing on
            // its line with nothing but a comma between them (exactly the
            // single-statement-per-line ambiguity `parse_inline_stmt_seq`
            // now rejects, reintroduced from the printer's side). A closure
            // that is itself a whole statement's RHS costs one blank line
            // from `push_line`'s own trailing newline landing after this
            // one; harmless (blank lines are layout-transparent) and no
            // existing golden pins exact `pretty`-stage text.
            let mut out = format!("|{params}|:\n");
            print_stmts(stmts, indent + 1, &mut out);
            out
        }
    }
}

// --- statements (02-language.md §8.1, §9.4, §10) -------------------------

fn print_stmts(stmts: &[Stmt], indent: usize, out: &mut String) {
    for s in stmts {
        print_stmt(s, indent, out);
    }
}

fn print_stmt(stmt: &Stmt, indent: usize, out: &mut String) {
    match stmt {
        Stmt::Assign(a) => {
            let mut line = print_expr(&a.target, indent);
            if let Some(ty) = &a.ty {
                line.push_str(": ");
                line.push_str(&print_type(ty, indent));
            }
            line.push(' ');
            line.push_str(a.op.as_str());
            line.push(' ');
            line.push_str(&print_expr(&a.value, indent));
            push_line(out, indent, &line);
        }
        Stmt::If(i) => {
            push_line(out, indent, &format!("if {}:", print_expr(&i.cond, indent)));
            print_stmts(&i.then_branch, indent + 1, out);
            for elif in &i.elifs {
                push_line(
                    out,
                    indent,
                    &format!("elif {}:", print_expr(&elif.cond, indent)),
                );
                print_stmts(&elif.body, indent + 1, out);
            }
            if let Some(else_branch) = &i.else_branch {
                push_line(out, indent, "else:");
                print_stmts(else_branch, indent + 1, out);
            }
        }
        Stmt::Match(m) => {
            push_line(
                out,
                indent,
                &format!("match {}:", print_expr(&m.scrutinee, indent)),
            );
            for arm in &m.arms {
                let mut header = format!("case {}", print_pattern(&arm.pattern));
                if let Some(guard) = &arm.guard {
                    header.push_str(&format!(" if {}", print_expr(guard, indent)));
                }
                header.push(':');
                push_line(out, indent + 1, &header);
                print_stmts(&arm.body, indent + 2, out);
            }
        }
        Stmt::For(f) => {
            let mut header = "for ".to_string();
            if f.take_binding {
                header.push_str("take ");
            }
            header.push_str(&f.name);
            header.push_str(" in ");
            header.push_str(&print_expr(&f.iterable, indent));
            header.push(':');
            push_line(out, indent, &header);
            print_stmts(&f.body, indent + 1, out);
        }
        Stmt::While(w) => {
            push_line(
                out,
                indent,
                &format!("while {}:", print_expr(&w.cond, indent)),
            );
            print_stmts(&w.body, indent + 1, out);
        }
        Stmt::Break(_) => push_line(out, indent, "break"),
        Stmt::Continue(_) => push_line(out, indent, "continue"),
        Stmt::Pass(_) => push_line(out, indent, "pass"),
        Stmt::Return(_, value) => match value {
            Some(v) => push_line(out, indent, &format!("return {}", print_expr(v, indent))),
            None => push_line(out, indent, "return"),
        },
        Stmt::Assert(a) => {
            let mut line = format!("assert {}", print_expr(&a.cond, indent));
            if let Some(msg) = &a.message {
                line.push_str(&format!(", {}", print_expr(msg, indent)));
            }
            push_line(out, indent, &line);
        }
        Stmt::Defer(d) => match &d.body {
            DeferBody::Expr(e) => {
                push_line(out, indent, &format!("defer {}", print_expr(e, indent)))
            }
            DeferBody::Suite(stmts) => {
                push_line(out, indent, "defer:");
                print_stmts(stmts, indent + 1, out);
            }
        },
        Stmt::With(w) => {
            let mut header = format!("with {}", print_expr(&w.expr, indent));
            if let Some(name) = &w.as_name {
                header.push_str(&format!(" as {name}"));
            }
            header.push(':');
            push_line(out, indent, &header);
            print_stmts(&w.body, indent + 1, out);
        }
        Stmt::Send(_, e) => push_line(out, indent, &format!("send {}", print_expr(e, indent))),
        Stmt::Expr(_, e) => push_line(out, indent, &print_expr(e, indent)),
        Stmt::ComptimeIf(c) => {
            push_line(
                out,
                indent,
                &format!("comptime if {}:", print_expr(&c.cond, indent)),
            );
            print_stmts(&c.then_branch, indent + 1, out);
            if let Some(else_branch) = &c.else_branch {
                push_line(out, indent, "comptime else:");
                print_stmts(else_branch, indent + 1, out);
            }
        }
        Stmt::ComptimeAssert(_, cond, message) => {
            let mut line = format!("comptime assert {}", print_expr(cond, indent));
            if let Some(msg) = message {
                line.push_str(&format!(", {}", print_expr(msg, indent)));
            }
            push_line(out, indent, &line);
        }
    }
}
