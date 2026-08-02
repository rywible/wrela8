//! Analysis and rewriting for the first, conservative scalar replacement pass.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use crate::mwir::{Inst, LayoutCtx, MwirFn, MwirProgram, Temp};
use crate::sema::types::Type;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Leaf {
    pub path: Vec<usize>,
    pub ty: Type,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Decision {
    Candidate { leaves: Vec<Leaf> },
    Rejected { reason: String, at: Option<usize> },
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct SroaReport {
    pub decisions: BTreeMap<Temp, Decision>,
}

fn strip_static(ty: &Type) -> &Type {
    match ty {
        Type::Static(inner) => strip_static(inner),
        other => other,
    }
}

fn struct_fields<'a>(ty: &'a Type, layout: &'a LayoutCtx) -> Option<&'a [Type]> {
    let Type::Named(name, args) = strip_static(ty) else {
        return None;
    };
    let key = if args.is_empty() {
        name.clone()
    } else {
        crate::sema::types::render_type(&Type::Named(name.clone(), args.clone()))
    };
    layout.structs.get(&key).map(Vec::as_slice)
}

fn is_supported_aggregate(ty: &Type, layout: &LayoutCtx) -> bool {
    match strip_static(ty) {
        Type::Tuple(_) => true,
        Type::Named(..) => struct_fields(ty, layout).is_some(),
        _ => false,
    }
}

fn leaves(
    ty: &Type,
    layout: &LayoutCtx,
    path: &mut Vec<usize>,
    out: &mut Vec<Leaf>,
) -> Result<(), String> {
    match strip_static(ty) {
        Type::Tuple(elems) => {
            for (i, elem) in elems.iter().enumerate() {
                path.push(i);
                leaves(elem, layout, path, out)?;
                path.pop();
            }
        }
        Type::Named(..) if struct_fields(ty, layout).is_some() => {
            let fields = struct_fields(ty, layout).expect("checked above");
            for (i, elem) in fields.iter().enumerate() {
                path.push(i);
                leaves(elem, layout, path, out)?;
                path.pop();
            }
        }
        Type::Array(..) | Type::Bytes(..) => {
            return Err("unsupported-nested-array".to_string());
        }
        Type::Option(..) | Type::Result(..) | Type::Named(..) => {
            return Err("unsupported-nested-enum".to_string());
        }
        _ => out.push(Leaf {
            path: path.clone(),
            ty: ty.clone(),
        }),
    }
    Ok(())
}

fn reject(report: &mut SroaReport, t: Temp, reason: impl Into<String>, at: usize) {
    report.decisions.insert(
        t,
        Decision::Rejected {
            reason: reason.into(),
            at: Some(at),
        },
    );
}

fn candidate_leaves(f: &MwirFn, t: Temp, layout: &LayoutCtx) -> Result<Vec<Leaf>, String> {
    let ty = f
        .temp_types
        .get(t.0)
        .ok_or_else(|| format!("temp {t} is outside the function type vector"))?;
    let mut out = Vec::new();
    leaves(ty, layout, &mut Vec::new(), &mut out)?;
    if out.is_empty() {
        return Err("aggregate-has-no-leaves".to_string());
    }
    Ok(out)
}

/// Discover all struct/tuple aggregate temps and conservatively reject any
/// whose complete use graph is not visible in MWIR.
pub fn analyze(f: &MwirFn, layout: &LayoutCtx) -> SroaReport {
    let mut report = SroaReport::default();
    for i in 0..f.temp_types.len() {
        let t = Temp(i);
        if !crate::codegen::is_aggregate(&f.temp_types[i]) {
            continue;
        }
        match candidate_leaves(f, t, layout) {
            Ok(leaves) if is_supported_aggregate(&f.temp_types[i], layout) => {
                report.decisions.insert(t, Decision::Candidate { leaves });
            }
            Ok(_) => reject(&mut report, t, "unsupported-aggregate-type", 0),
            Err(reason) => reject(&mut report, t, reason, 0),
        }
    }

    // Keep the initial candidate universe separate from rejection decisions.
    // Aggregate identity can flow through copies, nested construction,
    // projection, and field update.  If any member of such a connected use
    // graph escapes, every member must be rejected; scalarizing only one side
    // can leave an original aggregate instruction reading a temp whose
    // constructor was removed.
    let candidates: BTreeSet<Temp> = report
        .decisions
        .iter()
        .filter_map(|(t, d)| matches!(d, Decision::Candidate { .. }).then_some(*t))
        .collect();
    let mut edges: BTreeMap<Temp, BTreeSet<Temp>> = candidates
        .iter()
        .copied()
        .map(|t| (t, BTreeSet::new()))
        .collect();
    let mut rejected = BTreeMap::<Temp, (String, usize)>::new();
    let mut mark = |t: Temp, reason: &str, at: usize| {
        if candidates.contains(&t) {
            rejected
                .entry(t)
                .or_insert_with(|| (reason.to_string(), at));
        }
    };
    let connect = |a: Temp, b: Temp, edges: &mut BTreeMap<Temp, BTreeSet<Temp>>| {
        if candidates.contains(&a) && candidates.contains(&b) {
            edges.entry(a).or_default().insert(b);
            edges.entry(b).or_default().insert(a);
        }
    };

    for (t, _) in f.receiver.iter().chain(f.params.iter()) {
        mark(*t, "abi-pinned-parameter", 0);
    }
    for (at, inst) in f.body.iter().enumerate() {
        match inst {
            Inst::Copy { dst, src } => match (candidates.contains(dst), candidates.contains(src)) {
                (true, true) => {
                    connect(*dst, *src, &mut edges);
                    if f.temp_types.get(dst.0) != f.temp_types.get(src.0) {
                        mark(*dst, "aggregate-copy-type-mismatch", at);
                        mark(*src, "aggregate-copy-type-mismatch", at);
                    }
                }
                (true, false) => mark(*dst, "aggregate-copy-boundary", at),
                (false, true) => mark(*src, "aggregate-copy-boundary", at),
                (false, false) => {}
            },
            Inst::MakeAggregate { dst, elems } => {
                if candidates.contains(dst) {
                    for &elem in elems {
                        connect(*dst, elem, &mut edges);
                    }
                } else {
                    for &elem in elems {
                        mark(elem, "aggregate-constructor-boundary", at);
                    }
                }
            }
            Inst::Project { dst, base, .. } => {
                if candidates.contains(base) {
                    connect(*dst, *base, &mut edges);
                } else {
                    mark(*dst, "aggregate-project-boundary", at);
                }
            }
            Inst::SetField { base, value, .. } => {
                if candidates.contains(base) {
                    connect(*base, *value, &mut edges);
                } else {
                    mark(*value, "aggregate-field-boundary", at);
                }
            }
            Inst::Call {
                dst,
                write_backs,
                args,
                ..
            } => {
                mark(*dst, "defined-by-call", at);
                for &t in args {
                    mark(t, "passed-to-call", at);
                }
                for &(_, t) in write_backs {
                    mark(t, "passed-to-call", at);
                }
            }
            Inst::Return { value: Some(t) } => mark(*t, "returned-from-function", at),
            Inst::MmioRead { dst, base, .. } | Inst::MemLoad { dst, base, .. } => {
                mark(*dst, "raw-memory-or-address-use", at);
                mark(*base, "raw-memory-or-address-use", at);
            }
            Inst::MmioWrite { base, value, .. } | Inst::MemStore { base, value, .. } => {
                mark(*base, "raw-memory-or-address-use", at);
                mark(*value, "raw-memory-or-address-use", at);
            }
            Inst::PtrOffset { dst, base, .. } => {
                mark(*dst, "raw-memory-or-address-use", at);
                mark(*base, "raw-memory-or-address-use", at);
            }
            Inst::IndexGet {
                dst, base, index, ..
            }
            | Inst::IndexGetProven {
                dst, base, index, ..
            }
            | Inst::PlacedIndexGet {
                dst, base, index, ..
            }
            | Inst::PlacedIndexGetProven {
                dst, base, index, ..
            }
            | Inst::BytesIndexGet { dst, base, index } => {
                mark(*dst, "dynamic-index", at);
                mark(*base, "dynamic-index", at);
                mark(*index, "dynamic-index", at);
            }
            Inst::IndexSet {
                base, index, value, ..
            }
            | Inst::IndexSetProven {
                base, index, value, ..
            }
            | Inst::PlacedIndexSet {
                base, index, value, ..
            }
            | Inst::PlacedIndexSetProven {
                base, index, value, ..
            } => {
                mark(*base, "dynamic-index", at);
                mark(*index, "dynamic-index", at);
                mark(*value, "dynamic-index", at);
            }
            Inst::MakeEnum { dst, payload, .. } => {
                mark(*dst, "used-by-enum-operation", at);
                for &t in payload {
                    mark(t, "used-by-enum-operation", at);
                }
            }
            Inst::EnumTag { dst, src } | Inst::EnumPayload { dst, src, .. } => {
                mark(*dst, "used-by-enum-operation", at);
                mark(*src, "used-by-enum-operation", at);
            }
            other => {
                let facts = crate::mwir_facts::inst_facts(other);
                for t in facts.uses.into_iter().chain(facts.defs) {
                    mark(t, "unsupported-aggregate-use", at);
                }
            }
        }
    }

    // Propagate the first stable rejection through each identity component.
    // Component traversal and reason selection are numeric/source ordered.
    let mut seen = BTreeSet::new();
    for &root in &candidates {
        if !seen.insert(root) {
            continue;
        }
        let mut component = Vec::new();
        let mut stack = vec![root];
        while let Some(t) = stack.pop() {
            component.push(t);
            for &next in edges.get(&t).into_iter().flatten().rev() {
                if seen.insert(next) {
                    stack.push(next);
                }
            }
        }
        component.sort_unstable();
        let reason = component
            .iter()
            .filter_map(|t| {
                rejected
                    .get(t)
                    .map(|(reason, at)| (*at, *t, reason.clone()))
            })
            .min_by_key(|(at, t, _)| (*at, *t));
        if let Some((at, _, reason)) = reason {
            for t in component {
                reject(&mut report, t, reason.clone(), at);
            }
        }
    }
    report
}

fn decision_candidate<'a>(report: &'a SroaReport, t: Temp) -> Option<&'a [Leaf]> {
    match report.decisions.get(&t) {
        Some(Decision::Candidate { leaves }) => Some(leaves),
        _ => None,
    }
}

fn leaf_temp(
    map: &BTreeMap<(Temp, Vec<usize>), Temp>,
    aggregate: Temp,
    path: &[usize],
) -> Option<Temp> {
    map.get(&(aggregate, path.to_vec())).copied()
}

fn append_copy(out: &mut Vec<Inst>, dst: Temp, src: Temp) {
    if dst != src {
        out.push(Inst::Copy { dst, src });
    }
}

fn source_leaf(
    map: &BTreeMap<(Temp, Vec<usize>), Temp>,
    aggregate: Temp,
    path: &[usize],
) -> Result<Temp, String> {
    leaf_temp(map, aggregate, path).ok_or_else(|| format!("missing scalar leaf for {aggregate}"))
}

fn rewrite_inst(
    inst: &Inst,
    report: &SroaReport,
    leaves_by_temp: &BTreeMap<Temp, Vec<Leaf>>,
    leaf_map: &BTreeMap<(Temp, Vec<usize>), Temp>,
    types: &mut Vec<Type>,
    fresh: &mut Vec<Inst>,
) -> Result<Option<Inst>, String> {
    match inst {
        Inst::MakeAggregate { dst, elems } if decision_candidate(report, *dst).is_some() => {
            let dst_leaves = leaves_by_temp[dst].clone();
            for leaf in dst_leaves {
                let first = *leaf
                    .path
                    .first()
                    .ok_or_else(|| "aggregate leaf has empty path".to_string())?;
                let src_aggregate = *elems
                    .get(first)
                    .ok_or_else(|| format!("aggregate constructor field {first} is missing"))?;
                let src_path = leaf.path[1..].to_vec();
                let src = if decision_candidate(report, src_aggregate).is_some() {
                    source_leaf(leaf_map, src_aggregate, &src_path)?
                } else if src_path.is_empty() {
                    src_aggregate
                } else {
                    return Err("aggregate constructor has an unsupported nested value".to_string());
                };
                let dst_scalar = source_leaf(leaf_map, *dst, &leaf.path)?;
                append_copy(fresh, dst_scalar, src);
            }
            Ok(None)
        }
        Inst::Copy { dst, src }
            if decision_candidate(report, *dst).is_some()
                && decision_candidate(report, *src).is_some() =>
        {
            // Always use scratches.  This is the conservative parallel-copy
            // form and remains correct for overlapping/cyclic leaf maps.
            let src_leaves = leaves_by_temp[src].clone();
            let mut scratches = Vec::with_capacity(src_leaves.len());
            // Read every source before writing any destination.  Keeping the
            // two phases separate preserves aggregate parallel-copy semantics
            // even if a future leaf allocator coalesces source/destination
            // identities.
            for leaf in &src_leaves {
                let scratch = Temp(types.len());
                types.push(leaf.ty.clone());
                let source = source_leaf(leaf_map, *src, &leaf.path)?;
                fresh.push(Inst::Copy {
                    dst: scratch,
                    src: source,
                });
                scratches.push((leaf.path.clone(), scratch));
            }
            for (path, scratch) in scratches {
                let dst_scalar = source_leaf(leaf_map, *dst, &path)?;
                fresh.push(Inst::Copy {
                    dst: dst_scalar,
                    src: scratch,
                });
            }
            Ok(None)
        }
        Inst::Project { dst, base, index } if decision_candidate(report, *base).is_some() => {
            let base_leaves = leaves_by_temp[base].clone();
            let matches: Vec<Leaf> = base_leaves
                .into_iter()
                .filter(|l| l.path.first() == Some(index))
                .collect();
            if decision_candidate(report, *dst).is_some() {
                for leaf in matches {
                    let suffix = leaf.path[1..].to_vec();
                    let d = source_leaf(leaf_map, *dst, &suffix)?;
                    let s = source_leaf(leaf_map, *base, &leaf.path)?;
                    append_copy(fresh, d, s);
                }
                Ok(None)
            } else {
                let leaf = matches
                    .into_iter()
                    .find(|l| l.path.len() == 1)
                    .ok_or_else(|| "projection does not select a scalar leaf".to_string())?;
                let s = source_leaf(leaf_map, *base, &leaf.path)?;
                Ok(Some(Inst::Copy { dst: *dst, src: s }))
            }
        }
        Inst::SetField { base, index, value } if decision_candidate(report, *base).is_some() => {
            let base_leaves = leaves_by_temp[base].clone();
            for leaf in base_leaves {
                if leaf.path.first() != Some(index) {
                    continue;
                }
                let suffix = leaf.path[1..].to_vec();
                let src = if decision_candidate(report, *value).is_some() {
                    source_leaf(leaf_map, *value, &suffix)?
                } else if suffix.is_empty() {
                    *value
                } else {
                    return Err("field update has an unsupported nested value".to_string());
                };
                let dst = source_leaf(leaf_map, *base, &leaf.path)?;
                append_copy(fresh, dst, src);
            }
            Ok(None)
        }
        _ => Ok(Some(inst.clone())),
    }
}

fn rewrite_with_report(f: &MwirFn, report: &SroaReport) -> Result<MwirFn, String> {
    let mut leaves_by_temp = BTreeMap::new();
    for (&t, decision) in &report.decisions {
        if let Decision::Candidate { leaves } = decision {
            leaves_by_temp.insert(t, leaves.clone());
        }
    }
    if leaves_by_temp.is_empty() {
        return Ok(f.clone());
    }

    let mut out = f.clone();
    let mut leaf_map = BTreeMap::new();
    for (&aggregate, leaves) in &leaves_by_temp {
        for leaf in leaves {
            let t = Temp(out.temp_types.len());
            out.temp_types.push(leaf.ty.clone());
            leaf_map.insert((aggregate, leaf.path.clone()), t);
        }
    }

    let mut body = Vec::new();
    let mut starts = vec![0usize; f.body.len() + 1];
    for (i, inst) in f.body.iter().enumerate() {
        starts[i] = body.len();
        let mut generated = Vec::new();
        let rewritten = rewrite_inst(
            inst,
            &report,
            &leaves_by_temp,
            &leaf_map,
            &mut out.temp_types,
            &mut generated,
        )?;
        body.extend(generated);
        if let Some(op) = rewritten {
            body.push(op);
        }
    }
    starts[f.body.len()] = body.len();
    for inst in &mut body {
        match inst {
            Inst::Jump { target } | Inst::JumpIfFalse { target, .. } => {
                if *target > f.body.len() {
                    return Err(format!(
                        "rewrite encountered invalid jump target {}",
                        *target
                    ));
                }
                *target = starts[*target];
            }
            _ => {}
        }
    }
    out.body = body;
    Ok(out)
}

/// Rewrite accepted candidates.  A fresh scalar temp is allocated for every
/// leaf; old aggregate temps remain in the type vector but are unreachable,
/// which keeps all original temp identities stable for diagnostics.
pub fn rewrite(f: &MwirFn, layout: &LayoutCtx) -> Result<(MwirFn, SroaReport), String> {
    let report = analyze(f, layout);
    let out = rewrite_with_report(f, &report)?;
    Ok((out, report))
}

fn path(path: &[usize]) -> String {
    let mut out = String::new();
    for p in path {
        let _ = write!(out, ".{p}");
    }
    out
}

pub fn dump_report(report: &SroaReport) -> String {
    let mut out = String::new();
    for (t, decision) in &report.decisions {
        match decision {
            Decision::Candidate { leaves } => {
                let items = leaves
                    .iter()
                    .map(|l| {
                        format!(
                            "{}:{}",
                            path(&l.path),
                            crate::sema::types::render_type(&l.ty)
                        )
                    })
                    .collect::<Vec<_>>()
                    .join(",");
                let _ = writeln!(out, "sroa {t} candidate leaves=[{items}]");
            }
            Decision::Rejected { reason, at } => {
                let at = at.map_or_else(|| "-".to_string(), |i| i.to_string());
                let _ = writeln!(out, "sroa {t} rejected reason={reason} at={at}");
            }
        }
    }
    out
}

pub fn rewrite_program(
    program: &MwirProgram,
    layout: &LayoutCtx,
) -> Result<(MwirProgram, BTreeMap<String, SroaReport>), String> {
    let mut out = program.clone();
    let mut reports = BTreeMap::new();
    for (key, f) in &program.fns {
        let (rewritten, report) = rewrite(f, layout)?;
        out.fns.insert(key.clone(), rewritten);
        reports.insert(key.clone(), report);
    }
    Ok((out, reports))
}

fn flow_unsafe_temps(f: &crate::flowwir::FlowWirFn) -> Vec<Temp> {
    use crate::flowwir::{AwaitKind, FlowInst, Transition};
    let mut out = BTreeSet::new();
    let mut mwir_states = BTreeMap::<Temp, BTreeSet<usize>>::new();
    for (state_id, state) in f.states.iter().enumerate() {
        let mixed = state.ops.iter().any(|op| !matches!(op, FlowInst::Mwir(_)));
        for op in &state.ops {
            match op {
                FlowInst::Mwir(inst) => {
                    let facts = crate::mwir_facts::inst_facts(inst);
                    for temp in facts.uses.into_iter().chain(facts.defs) {
                        mwir_states.entry(temp).or_default().insert(state_id);
                        if mixed {
                            out.insert(temp);
                        }
                    }
                }
                FlowInst::SelfPath { dst, .. }
                | FlowInst::Now { dst }
                | FlowInst::Entropy { dst, .. } => {
                    out.insert(*dst);
                }
                FlowInst::Duration { dst, n } => {
                    out.extend([*dst, *n]);
                }
                FlowInst::Send {
                    dst,
                    target,
                    arg_temps,
                    take_arg_temps,
                    ..
                } => {
                    out.extend([*dst, *target]);
                    out.extend(arg_temps.iter().copied());
                    out.extend(take_arg_temps.iter().copied());
                }
                FlowInst::GroupCreate {
                    group_temp,
                    capacity,
                    deadline,
                } => {
                    out.insert(*group_temp);
                    out.extend(capacity.iter().chain(deadline).copied());
                }
                FlowInst::GroupStart {
                    group_temp,
                    arg_temps,
                    ..
                } => {
                    out.insert(*group_temp);
                    out.extend(arg_temps.iter().copied());
                }
                FlowInst::GroupClose { group_temp, .. } => {
                    out.insert(*group_temp);
                }
            }
        }
        match &state.transition {
            Transition::Return(value) => out.extend(value.iter().copied()),
            Transition::Await {
                what, result_temp, ..
            } => {
                out.insert(*result_temp);
                match what {
                    AwaitKind::ActorCall {
                        target_temp,
                        arg_temps,
                        take_arg_temps,
                        ..
                    } => {
                        out.insert(*target_temp);
                        out.extend(arg_temps.iter().copied());
                        out.extend(take_arg_temps.iter().copied());
                    }
                    AwaitKind::GroupJoin { group_temp, .. } => {
                        out.insert(*group_temp);
                    }
                    AwaitKind::Receipt { receipt_temp } => {
                        out.insert(*receipt_temp);
                    }
                }
            }
            Transition::Branch { cond_temp, .. } => {
                out.insert(*cond_temp);
            }
            Transition::Jump(_) | Transition::Abort { .. } => {}
        }
    }
    for (temp, states) in mwir_states {
        if states.len() > 1 {
            out.insert(temp);
        }
    }
    out.into_iter().collect()
}

/// Apply the same scalar leaf identities to every state of a Flow function.
/// Flow/suspension operands are modeled as escaping uses, so an aggregate that
/// crosses the runtime ABI is rejected before any state's constructor is
/// removed.
pub fn rewrite_flow_program(
    program: &crate::flowwir::FlowWirProgram,
    layout: &LayoutCtx,
) -> Result<(crate::flowwir::FlowWirProgram, BTreeMap<String, SroaReport>), String> {
    use crate::flowwir::FlowInst;
    let mut out = program.clone();
    let mut reports = BTreeMap::new();
    for (key, f) in &program.fns {
        let mut analysis_body = Vec::new();
        for state in &f.states {
            analysis_body.extend(state.ops.iter().filter_map(|op| match op {
                FlowInst::Mwir(inst) => Some(inst.clone()),
                _ => None,
            }));
        }
        // A synthetic return is an analysis-only aggregate escape.  It also
        // propagates through aggregate-copy components in `analyze`.
        analysis_body.extend(
            flow_unsafe_temps(f)
                .into_iter()
                .map(|temp| Inst::Return { value: Some(temp) }),
        );
        let analysis_fn = MwirFn {
            receiver: f.receiver,
            params: f.params.clone(),
            ret: f.ret.clone(),
            temp_types: f.frame.temp_types.clone(),
            body: analysis_body,
        };
        let report = analyze(&analysis_fn, layout);
        let leaf_count: usize = report
            .decisions
            .values()
            .filter_map(|decision| match decision {
                Decision::Candidate { leaves } => Some(leaves.len()),
                Decision::Rejected { .. } => None,
            })
            .sum();
        let leaf_end = f.frame.temp_types.len() + leaf_count;
        let mut rewritten_states = Vec::with_capacity(f.states.len());
        let mut shared_types: Option<Vec<Type>> = None;
        let mut scratch_types = Vec::new();
        for state in &f.states {
            if state.ops.iter().any(|op| !matches!(op, FlowInst::Mwir(_))) {
                rewritten_states.push(state.clone());
                continue;
            }
            let local = MwirFn {
                receiver: f.receiver,
                params: f.params.clone(),
                ret: f.ret.clone(),
                temp_types: f.frame.temp_types.clone(),
                body: state
                    .ops
                    .iter()
                    .map(|op| match op {
                        FlowInst::Mwir(inst) => inst.clone(),
                        _ => unreachable!(),
                    })
                    .collect(),
            };
            let mut rewritten = rewrite_with_report(&local, &report)?;
            if rewritten.temp_types.len() < leaf_end {
                return Err(format!(
                    "Flow SROA for `{key}` produced an inconsistent leaf vector"
                ));
            }
            match &shared_types {
                None => shared_types = Some(rewritten.temp_types[..leaf_end].to_vec()),
                Some(types) if types != &rewritten.temp_types[..leaf_end] => {
                    return Err(format!(
                        "Flow SROA for `{key}` assigned inconsistent leaf types across states"
                    ));
                }
                Some(_) => {}
            }
            let mut scratch_map = BTreeMap::new();
            for old in leaf_end..rewritten.temp_types.len() {
                let new = Temp(leaf_end + scratch_types.len());
                scratch_types.push(rewritten.temp_types[old].clone());
                scratch_map.insert(Temp(old), new);
            }
            for inst in &mut rewritten.body {
                crate::mwir_opt::visit_temps_mut(inst, &mut |temp| {
                    if let Some(new) = scratch_map.get(temp) {
                        *temp = *new;
                    }
                });
            }
            let mut state = state.clone();
            state.ops = rewritten.body.into_iter().map(FlowInst::Mwir).collect();
            rewritten_states.push(state);
        }
        let mut function = f.clone();
        if let Some(mut types) = shared_types {
            types.extend(scratch_types);
            function.frame.temp_types = types;
            function.states = rewritten_states;
        }
        out.fns.insert(key.clone(), function);
        reports.insert(key.clone(), report);
    }
    Ok((out, reports))
}

pub fn dump_program(program: &MwirProgram, layout: &LayoutCtx) -> String {
    let mut out = String::new();
    out.push_str("MWIR-OPT\n");
    for (key, f) in &program.fns {
        out.push_str(&format!("  function {key}\n"));
        out.push_str(&dump_report(&analyze(f, layout)));
    }
    out
}

pub fn dump_flow_program(
    program: &crate::flowwir::FlowWirProgram,
    layout: &LayoutCtx,
) -> Result<String, String> {
    let (_, reports) = rewrite_flow_program(program, layout)?;
    let mut out = String::new();
    for (key, report) in reports {
        out.push_str(&format!("  flow function {key}\n"));
        out.push_str(&dump_report(&report));
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mwir::Inst;

    fn struct_layout() -> LayoutCtx {
        LayoutCtx {
            structs: BTreeMap::from([("Pair".to_string(), vec![Type::U64, Type::Bool])]),
            ..LayoutCtx::default()
        }
    }

    fn pair_fn() -> MwirFn {
        MwirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            temp_types: vec![Type::Named("Pair".into(), vec![]), Type::U64, Type::Bool],
            body: vec![
                Inst::MakeAggregate {
                    dst: Temp(0),
                    elems: vec![Temp(1), Temp(2)],
                },
                Inst::Project {
                    dst: Temp(1),
                    base: Temp(0),
                    index: 0,
                },
                Inst::Return {
                    value: Some(Temp(1)),
                },
            ],
        }
    }

    #[test]
    fn construction_and_projection_are_candidates() {
        let f = pair_fn();
        let report = analyze(&f, &struct_layout());
        assert!(matches!(
            report.decisions[&Temp(0)],
            Decision::Candidate { .. }
        ));
        let (rewritten, _) = rewrite(&f, &struct_layout()).expect("rewrite");
        assert!(
            !rewritten
                .body
                .iter()
                .any(|i| matches!(i, Inst::MakeAggregate { .. }))
        );
        assert!(
            !rewritten
                .body
                .iter()
                .any(|i| matches!(i, Inst::Project { .. }))
        );
    }

    #[test]
    fn aggregate_copy_is_rewritten_as_a_two_phase_parallel_copy() {
        let pair = Type::Named("Pair".into(), vec![]);
        let f = MwirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            temp_types: vec![pair.clone(), Type::U64, Type::Bool, pair],
            body: vec![
                Inst::MakeAggregate {
                    dst: Temp(0),
                    elems: vec![Temp(1), Temp(2)],
                },
                Inst::Copy {
                    dst: Temp(3),
                    src: Temp(0),
                },
                Inst::Project {
                    dst: Temp(1),
                    base: Temp(3),
                    index: 0,
                },
                Inst::Return {
                    value: Some(Temp(1)),
                },
            ],
        };
        let (rewritten, _) = rewrite(&f, &struct_layout()).expect("rewrite");
        assert_eq!(
            &rewritten.body[2..6],
            &[
                Inst::Copy {
                    dst: Temp(8),
                    src: Temp(4),
                },
                Inst::Copy {
                    dst: Temp(9),
                    src: Temp(5),
                },
                Inst::Copy {
                    dst: Temp(6),
                    src: Temp(8),
                },
                Inst::Copy {
                    dst: Temp(7),
                    src: Temp(9),
                },
            ],
            "all source leaves must be captured before any destination leaf is written"
        );
    }

    #[test]
    fn calls_reject_aggregate_identity() {
        let mut f = pair_fn();
        f.body.insert(
            1,
            Inst::Call {
                dst: Temp(1),
                write_backs: Vec::new(),
                key: "f".into(),
                args: vec![Temp(0)],
            },
        );
        assert!(matches!(
            analyze(&f, &struct_layout()).decisions[&Temp(0)],
            Decision::Rejected { .. }
        ));
    }

    #[test]
    fn flow_local_aggregate_is_rewritten_but_suspend_operand_is_rejected() {
        use crate::flowwir::{FlowInst, FlowWirFn, FlowWirProgram, FrameLayout, State, Transition};
        let local = pair_fn();
        let function = FlowWirFn {
            receiver: None,
            params: Vec::new(),
            ret: Type::U64,
            frame: FrameLayout {
                temp_types: local.temp_types.clone(),
                lineage_group_slot: Temp(1),
                lineage_deadline_slot: Temp(2),
            },
            states: vec![State {
                ops: local.body[..2]
                    .iter()
                    .cloned()
                    .map(FlowInst::Mwir)
                    .collect(),
                transition: Transition::Return(Some(Temp(1))),
            }],
        };
        let program = FlowWirProgram {
            fns: BTreeMap::from([("flow".to_string(), function)]),
        };
        let (rewritten, reports) =
            rewrite_flow_program(&program, &struct_layout()).expect("Flow rewrite");
        assert!(matches!(
            reports["flow"].decisions[&Temp(0)],
            Decision::Candidate { .. }
        ));
        assert!(
            rewritten.fns["flow"].states[0]
                .ops
                .iter()
                .all(|op| !matches!(
                    op,
                    FlowInst::Mwir(Inst::MakeAggregate { .. } | Inst::Project { .. })
                ))
        );

        let mut escaping = program;
        escaping.fns.get_mut("flow").unwrap().states[0].transition =
            Transition::Return(Some(Temp(0)));
        let (unchanged, reports) =
            rewrite_flow_program(&escaping, &struct_layout()).expect("rejected Flow rewrite");
        assert!(matches!(
            reports["flow"].decisions[&Temp(0)],
            Decision::Rejected { .. }
        ));
        assert!(
            unchanged.fns["flow"].states[0]
                .ops
                .iter()
                .any(|op| matches!(op, FlowInst::Mwir(Inst::MakeAggregate { .. })))
        );
    }

    #[test]
    fn rejection_propagates_across_aggregate_copies() {
        let pair = Type::Named("Pair".into(), vec![]);
        let mut f = pair_fn();
        f.temp_types.push(pair);
        f.body.insert(
            1,
            Inst::Copy {
                dst: Temp(3),
                src: Temp(0),
            },
        );
        f.body.insert(
            2,
            Inst::Call {
                dst: Temp(1),
                write_backs: Vec::new(),
                key: "consume".into(),
                args: vec![Temp(0)],
            },
        );
        f.body[3] = Inst::Project {
            dst: Temp(1),
            base: Temp(3),
            index: 0,
        };

        let report = analyze(&f, &struct_layout());
        assert!(matches!(
            report.decisions[&Temp(0)],
            Decision::Rejected { .. }
        ));
        assert!(matches!(
            report.decisions[&Temp(3)],
            Decision::Rejected { .. }
        ));
        let (rewritten, _) = rewrite(&f, &struct_layout()).expect("rewrite");
        assert_eq!(
            rewritten, f,
            "a partially rejected identity graph must stay intact"
        );
    }
}
