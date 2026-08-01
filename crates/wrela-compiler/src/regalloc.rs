use std::cell::Cell;
use std::collections::{BTreeMap, BTreeSet};

pub type RegSet = u32;

pub const ALL_REGS: RegSet = u32::MAX;

pub fn reg_bit(r: u8) -> RegSet {
    debug_assert!(r < 32, "register number {r} out of range");
    1u32 << (r & 31)
}

pub const POOL: &[u8] = &[19, 20, 21, 22, 23, 24, 25, 26, 27];

pub const WIDE_POOL: &[u8] = &[
    0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 16, 17, 19, 20, 21, 22, 23, 24, 25, 26,
    27,
];

thread_local! {
    static REGALLOC: Cell<bool> = const { Cell::new(false) };
    static INTERPROC: Cell<bool> = const { Cell::new(false) };
}

pub fn set_regalloc(enabled: bool) {
    REGALLOC.with(|c| c.set(enabled));
}

pub fn regalloc() -> bool {
    REGALLOC.with(|c| c.get())
}

pub fn set_interproc_regs(enabled: bool) {
    INTERPROC.with(|c| c.set(enabled));
}

pub fn interproc_regs() -> bool {
    INTERPROC.with(|c| c.get())
}

#[derive(Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Debug)]
pub enum Touch {
    Read,
    Write,
    Escape,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CallWord {
    pub word: usize,
    pub callee: Option<String>,
}

#[derive(Clone, Debug, Default)]
pub struct PointFacts {
    pub touches: Vec<(usize, Touch, usize, u8)>,
    pub call_words: Vec<CallWord>,
    pub regs: BTreeSet<u8>,
    pub word_regs: Vec<(usize, u8)>,
}

#[derive(Clone, Debug, Default)]
pub struct FnFacts {
    pub temp_count: usize,
    pub points: Vec<PointFacts>,
    pub back_edges: Vec<(usize, usize)>,
    pub calls: BTreeSet<String>,
    pub opaque_calls: bool,
    pub has_returning_call: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Assignment {
    reg: Vec<Option<u8>>,
}

impl Assignment {
    pub fn none(temp_count: usize) -> Assignment {
        Assignment {
            reg: vec![None; temp_count],
        }
    }

    pub fn of(&self, t: usize) -> Option<u8> {
        self.reg.get(t).copied().flatten()
    }

    pub fn is_empty(&self) -> bool {
        self.reg.iter().all(|r| r.is_none())
    }

    pub fn resident_count(&self) -> usize {
        self.reg.iter().filter(|r| r.is_some()).count()
    }

    pub fn residents(&self) -> Vec<(usize, u8)> {
        self.reg
            .iter()
            .enumerate()
            .filter_map(|(t, r)| r.map(|p| (t, p)))
            .collect()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct Interval {
    temps: Vec<usize>,
    key: usize,
    start: usize,
    end: usize,
    forbidden: RegSet,
    hints: Vec<u8>,
}

fn uf_find(parent: &mut [usize], mut x: usize) -> usize {
    while parent[x] != x {
        parent[x] = parent[parent[x]];
        x = parent[x];
    }
    x
}

fn measured_regs(facts: &FnFacts) -> RegSet {
    let mut m: RegSet = 0;
    for p in &facts.points {
        for &r in &p.regs {
            m |= reg_bit(r);
        }
    }
    m
}

pub fn free_pool(facts: &FnFacts, base_pool: &[u8]) -> Vec<u8> {
    let used = measured_regs(facts);
    base_pool
        .iter()
        .copied()
        .filter(|&r| used & reg_bit(r) == 0)
        .collect()
}

pub fn allocate(facts: &FnFacts, scalar_slot: &[bool]) -> Assignment {
    allocate_with(facts, scalar_slot, POOL, None, false)
}

pub fn allocate_with(
    facts: &FnFacts,
    scalar_slot: &[bool],
    base_pool: &[u8],
    callee_clobbers: Option<&BTreeMap<String, RegSet>>,
    pays_for_itself: bool,
) -> Assignment {
    let n = facts.temp_count;
    let mut out = Assignment::none(n);
    if n == 0 || facts.points.is_empty() {
        return out;
    }

    let mut eligible: Vec<bool> = (0..n)
        .map(|t| scalar_slot.get(t).copied().unwrap_or(false))
        .collect();
    for p in &facts.points {
        for &(t, touch, _, _) in &p.touches {
            if t < n && touch == Touch::Escape {
                eligible[t] = false;
            }
        }
    }

    let mut reads: Vec<usize> = vec![0; n];
    for p in &facts.points {
        for &(t, touch, _, _) in &p.touches {
            if t < n && touch == Touch::Read {
                reads[t] += 1;
            }
        }
    }
    if pays_for_itself {
        for t in 0..n {
            if reads[t] < 2 {
                eligible[t] = false;
            }
        }
    }

    let mut first: Vec<Option<usize>> = vec![None; n];
    let mut last: Vec<Option<usize>> = vec![None; n];
    for (i, p) in facts.points.iter().enumerate() {
        for &(t, _, _, _) in &p.touches {
            if t >= n {
                continue;
            }
            if first[t].is_none() {
                first[t] = Some(i);
            }
            last[t] = Some(i);
        }
    }
    let mut intervals: Vec<Interval> = (0..n)
        .filter(|&t| eligible[t])
        .filter_map(|t| match (first[t], last[t]) {
            (Some(a), Some(b)) => Some(Interval {
                temps: vec![t],
                key: t,
                start: a,
                end: b,
                forbidden: 0,
                hints: Vec::new(),
            }),
            _ => None,
        })
        .collect();
    if intervals.is_empty() {
        return out;
    }

    let cap = intervals.len().saturating_mul(facts.points.len()) + 2;
    let mut rounds = 0usize;
    loop {
        let mut changed = false;
        for &(b, h) in &facts.back_edges {
            let (lo, hi) = (h.min(b), h.max(b));
            for iv in intervals.iter_mut() {
                if iv.start <= hi && iv.end >= lo {
                    if iv.start > lo {
                        iv.start = lo;
                        changed = true;
                    }
                    if iv.end < hi {
                        iv.end = hi;
                        changed = true;
                    }
                }
            }
        }
        rounds += 1;
        if !changed || rounds > cap {
            break;
        }
    }

    let pool: Vec<u8> = free_pool(facts, base_pool);
    if pool.is_empty() {
        return out;
    }
    let pool_mask: RegSet = pool.iter().fold(0, |m, &r| m | reg_bit(r));

    for iv in intervals.iter_mut() {
        for i in iv.start..=iv.end.min(facts.points.len().saturating_sub(1)) {
            let p = &facts.points[i];
            if p.call_words.is_empty() {
                continue;
            }
            let mut lo_w: Option<usize> = None;
            let mut hi_w: Option<usize> = None;
            for &(t, _, w, _) in &p.touches {
                if !iv.temps.contains(&t) {
                    continue;
                }
                lo_w = Some(lo_w.map_or(w, |c: usize| c.min(w)));
                hi_w = Some(hi_w.map_or(w, |c: usize| c.max(w)));
            }
            let lo = if iv.start < i {
                0
            } else {
                lo_w.unwrap_or(usize::MAX)
            };
            let hi = if iv.end > i {
                usize::MAX
            } else {
                hi_w.unwrap_or(0)
            };
            if lo > hi {
                continue;
            }
            for cw in &p.call_words {
                if cw.word < lo || cw.word > hi {
                    continue;
                }
                iv.forbidden |= match callee_clobbers {
                    None => ALL_REGS,
                    Some(map) => match &cw.callee {
                        Some(k) => map.get(k).copied().unwrap_or(ALL_REGS),
                        None => ALL_REGS,
                    },
                };
            }
        }
    }
    coalesce(facts, &mut intervals, pool_mask);
    for iv in intervals.iter_mut() {
        iv.hints = hints_for(facts, iv);
    }

    intervals.retain(|iv| {
        let hint_mask: RegSet = iv.hints.iter().fold(0, |m, &r| m | reg_bit(r));
        (pool_mask | hint_mask) & !iv.forbidden != 0
    });
    if intervals.is_empty() {
        return out;
    }

    intervals.sort_by(|a, b| (a.start, a.key).cmp(&(b.start, b.key)));
    let mut free: BTreeSet<u8> = pool.iter().copied().collect();
    let mut active: Vec<(usize, usize, u8)> = Vec::new();
    let mut assigned: BTreeMap<usize, u8> = BTreeMap::new();
    let mut hint_held: BTreeMap<u8, usize> = BTreeMap::new();

    for iv in &intervals {
        let mut still: Vec<(usize, usize, u8)> = Vec::with_capacity(active.len());
        for &(end, temp, reg) in &active {
            if end < iv.start {
                free.insert(reg);
            } else {
                still.push((end, temp, reg));
            }
        }
        active = still;

        let mut pick = None;
        for &r in &iv.hints {
            if iv.forbidden & reg_bit(r) != 0 {
                continue;
            }
            if pool_mask & reg_bit(r) != 0 {
                if free.contains(&r) {
                    pick = Some(r);
                    break;
                }
            } else if hint_held.get(&r).is_none_or(|&e| e < iv.start) {
                hint_held.insert(r, iv.end);
                assigned.insert(iv.key, r);
                pick = Some(r);
                break;
            }
        }
        if let Some(r) = pick {
            if pool_mask & reg_bit(r) == 0 {
                active.sort_by_key(|&(end, temp, _)| (end, temp));
                continue;
            }
        }
        let pick = pick.or_else(|| {
            free.iter()
                .copied()
                .find(|&r| iv.forbidden & reg_bit(r) == 0)
        });
        match pick {
            Some(reg) => {
                free.remove(&reg);
                assigned.insert(iv.key, reg);
                active.push((iv.end, iv.key, reg));
            }
            None => {
                let victim = active
                    .iter()
                    .copied()
                    .filter(|&(_, _, reg)| iv.forbidden & reg_bit(reg) == 0)
                    .max_by_key(|&(end, temp, _)| (end, temp));
                match victim {
                    Some((vend, vkey, vreg)) if vend > iv.end => {
                        assigned.remove(&vkey);
                        active.retain(|&(_, k, _)| k != vkey);
                        assigned.insert(iv.key, vreg);
                        active.push((iv.end, iv.key, vreg));
                    }
                    _ => {}
                }
            }
        }
        active.sort_by_key(|&(end, temp, _)| (end, temp));
    }

    for iv in &intervals {
        if let Some(&r) = assigned.get(&iv.key) {
            for &t in &iv.temps {
                out.reg[t] = Some(r);
            }
        }
    }
    out
}

fn coalesce(facts: &FnFacts, intervals: &mut Vec<Interval>, pool_mask: RegSet) {
    if intervals.len() < 2 {
        return;
    }
    let n = facts.temp_count;
    let mut idx: Vec<Option<usize>> = vec![None; n];
    for (i, iv) in intervals.iter().enumerate() {
        idx[iv.temps[0]] = Some(i);
    }
    let mut parent: Vec<usize> = (0..intervals.len()).collect();
    let mut merged = false;

    for (i, p) in facts.points.iter().enumerate() {
        if !p.call_words.is_empty() || p.touches.len() != 2 {
            continue;
        }
        let (s, sh, sw, _) = p.touches[0];
        let (d, dh, dw, _) = p.touches[1];
        if sh != Touch::Read || dh != Touch::Write || s == d || sw > dw {
            continue;
        }
        let (Some(si), Some(di)) = (idx.get(s).copied().flatten(), idx.get(d).copied().flatten())
        else {
            continue;
        };
        let (cs, cd) = (uf_find(&mut parent, si), uf_find(&mut parent, di));
        if cs == cd {
            continue;
        }
        if intervals[cs].end != i || intervals[cd].start != i {
            continue;
        }
        let forbidden = intervals[cs].forbidden | intervals[cd].forbidden;
        if pool_mask & !forbidden == 0 {
            continue;
        }
        let hints = Vec::new();
        let (start, end) = (intervals[cs].start, intervals[cd].end);
        let mut temps = std::mem::take(&mut intervals[cd].temps);
        temps.append(&mut intervals[cs].temps);
        temps.sort_unstable();
        temps.dedup();
        let key = temps[0];
        intervals[cs] = Interval {
            temps,
            key,
            start,
            end,
            forbidden,
            hints,
        };
        parent[cd] = cs;
        merged = true;
    }

    if merged {
        intervals.retain(|iv| !iv.temps.is_empty());
    }
}

pub const MAX_HINT_REG: u8 = 8;

fn hints_for(facts: &FnFacts, iv: &Interval) -> Vec<u8> {
    let mut counts: BTreeMap<u8, usize> = BTreeMap::new();
    for i in iv.start..=iv.end.min(facts.points.len().saturating_sub(1)) {
        for &(t, how, _, reg) in &facts.points[i].touches {
            if reg <= MAX_HINT_REG && how != Touch::Escape && iv.temps.contains(&t) {
                *counts.entry(reg).or_insert(0) += 1;
            }
        }
    }
    let mut cands: Vec<(usize, u8)> = counts
        .into_iter()
        .filter(|&(r, _)| hint_admissible(facts, iv, r))
        .map(|(r, c)| (c, r))
        .collect();
    cands.sort_by(|a, b| (b.0, a.1).cmp(&(a.0, b.1)));
    cands.into_iter().map(|(_, r)| r).collect()
}

fn hint_admissible(facts: &FnFacts, iv: &Interval, r: u8) -> bool {
    let last = facts.points.len().saturating_sub(1);
    for i in iv.start..=iv.end.min(last) {
        let p = &facts.points[i];
        let mut own: Vec<usize> = Vec::new();
        for &(t, _, w, _) in &p.touches {
            if iv.temps.contains(&t) {
                own.push(w);
            }
        }
        let lo = if iv.start < i {
            0
        } else {
            match own.iter().min() {
                Some(&w) => w,
                None => 0,
            }
        };
        let hi = if iv.end > i {
            usize::MAX
        } else {
            match own.iter().max() {
                Some(&w) => w,
                None => usize::MAX,
            }
        };
        for &(w, reg) in &p.word_regs {
            if reg != r || w < lo || w > hi {
                continue;
            }
            if !own.contains(&w) {
                return false;
            }
        }
    }
    true
}

#[derive(Clone, Debug, Default)]
pub struct FnInput {
    pub facts: FnFacts,
    pub scalar_slot: Vec<bool>,
    pub opaque_body: bool,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Convention {
    pub assignment: Assignment,
    pub pool: Vec<u8>,
    pub clobbers: RegSet,
    pub opaque: bool,
}

pub fn allocate_program(fns: &BTreeMap<String, FnInput>) -> BTreeMap<String, Convention> {
    let mut callees: BTreeMap<&str, BTreeSet<&str>> = BTreeMap::new();
    let mut opaque_callee: BTreeMap<&str, bool> = BTreeMap::new();
    for (key, input) in fns {
        let mut set: BTreeSet<&str> = BTreeSet::new();
        let mut unknown = input.facts.opaque_calls;
        for k in &input.facts.calls {
            match fns.get_key_value(k) {
                Some((held, held_input)) if !held_input.opaque_body => {
                    set.insert(held.as_str());
                }
                _ => unknown = true,
            }
        }
        callees.insert(key.as_str(), set);
        opaque_callee.insert(key.as_str(), unknown);
    }

    let mut clobbers: BTreeMap<String, RegSet> = BTreeMap::new();
    let mut out: BTreeMap<String, Convention> = BTreeMap::new();
    let mut done: BTreeSet<&str> = BTreeSet::new();

    loop {
        let mut progressed = false;
        for (key, input) in fns {
            let k = key.as_str();
            if done.contains(k) {
                continue;
            }
            if !callees[k].iter().all(|c| done.contains(c)) {
                continue;
            }
            let mut conv = allocate_one(input, &clobbers, opaque_callee[k], &callees[k]);
            if input.opaque_body {
                conv.clobbers = ALL_REGS;
                conv.opaque = true;
            }
            clobbers.insert(key.clone(), conv.clobbers);
            out.insert(key.clone(), conv);
            done.insert(k);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }

    let cyclic: Vec<&str> = fns
        .keys()
        .map(|k| k.as_str())
        .filter(|k| !done.contains(k))
        .collect();
    for k in &cyclic {
        clobbers.insert((*k).to_string(), ALL_REGS);
    }
    for k in &cyclic {
        let input = &fns[*k];
        let mut conv = allocate_one(input, &clobbers, true, &callees[*k]);
        conv.clobbers = ALL_REGS;
        conv.opaque = true;
        out.insert((*k).to_string(), conv);
    }
    out
}

fn allocate_one(
    input: &FnInput,
    clobbers: &BTreeMap<String, RegSet>,
    opaque_callee: bool,
    callees: &BTreeSet<&str>,
) -> Convention {
    let pool = free_pool(&input.facts, WIDE_POOL);
    let assignment = allocate_with(
        &input.facts,
        &input.scalar_slot,
        WIDE_POOL,
        Some(clobbers),
        false,
    );
    let mut mask = measured_regs(&input.facts);
    for (_, r) in assignment.residents() {
        mask |= reg_bit(r);
    }
    let mut opaque = opaque_callee;
    for c in callees {
        match clobbers.get(*c) {
            Some(&m) => mask |= m,
            None => {
                mask = ALL_REGS;
                opaque = true;
            }
        }
    }
    if opaque_callee {
        mask = ALL_REGS;
    }
    Convention {
        assignment,
        pool,
        clobbers: mask,
        opaque,
    }
}

pub fn render_reg_set(mask: RegSet) -> String {
    if mask == ALL_REGS {
        return "all".to_string();
    }
    if mask == 0 {
        return "none".to_string();
    }
    let mut out = String::new();
    let mut r = 0u8;
    while r < 32 {
        if mask & reg_bit(r) == 0 {
            r += 1;
            continue;
        }
        let start = r;
        while r < 31 && mask & reg_bit(r + 1) != 0 {
            r += 1;
        }
        if !out.is_empty() {
            out.push(',');
        }
        if start == r {
            out.push_str(&format!("x{start}"));
        } else {
            out.push_str(&format!("x{start}-x{r}"));
        }
        r += 1;
    }
    out
}

#[cfg(test)]
pub(crate) mod tests_support {
    use super::*;

    pub(crate) fn point(touches: &[(usize, Touch)]) -> PointFacts {
        PointFacts {
            touches: touches.iter().map(|&(t, h)| (t, h, 0usize, 9u8)).collect(),
            call_words: Vec::new(),
            regs: BTreeSet::new(),
            word_regs: Vec::new(),
        }
    }

    pub(crate) fn call_point() -> PointFacts {
        call_point_to(None)
    }

    pub(crate) fn call_point_to(callee: Option<&str>) -> PointFacts {
        PointFacts {
            touches: Vec::new(),
            call_words: vec![CallWord {
                word: 0,
                callee: callee.map(str::to_string),
            }],
            regs: BTreeSet::new(),
            word_regs: Vec::new(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn straight_line_scalars_become_resident() {
        let facts = FnFacts {
            temp_count: 2,
            points: vec![
                point(&[(0, Touch::Write), (1, Touch::Write)]),
                point(&[(0, Touch::Read), (1, Touch::Read)]),
                point(&[(0, Touch::Read), (1, Touch::Read)]),
            ],
            back_edges: Vec::new(),
            ..Default::default()
        };
        let a = allocate(&facts, &[true, true]);
        assert_eq!(a.resident_count(), 2);
        assert_ne!(
            a.of(0),
            a.of(1),
            "overlapping temps need distinct registers"
        );
    }

    #[test]
    fn an_escaped_temp_is_never_resident() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Escape)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: Vec::new(),
            ..Default::default()
        };
        assert_eq!(allocate(&facts, &[true]).of(0), None);
    }

    #[test]
    fn a_non_scalar_slot_is_never_resident() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: Vec::new(),
            ..Default::default()
        };
        assert_eq!(allocate(&facts, &[false]).of(0), None);
    }

    #[test]
    fn a_range_spanning_a_call_is_refused() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                call_point(),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: Vec::new(),
            ..Default::default()
        };
        assert_eq!(allocate(&facts, &[true]).of(0), None);

        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
                call_point(),
            ],
            back_edges: Vec::new(),
            ..Default::default()
        };
        assert!(allocate(&facts, &[true]).of(0).is_some());
    }

    #[test]
    fn more_live_values_than_registers_still_spills_correctly() {
        let n = POOL.len() * 3;
        let mut defs = Vec::new();
        let mut uses = Vec::new();
        for t in 0..n {
            defs.push((t, Touch::Write));
            uses.push((t, Touch::Read));
        }
        let facts = FnFacts {
            temp_count: n,
            points: vec![point(&defs), point(&uses), point(&uses)],
            back_edges: Vec::new(),
            ..Default::default()
        };
        let a = allocate(&facts, &vec![true; n]);
        assert_eq!(
            a.resident_count(),
            POOL.len(),
            "exactly the pool is used, no more"
        );
        let regs: BTreeSet<u8> = a.residents().iter().map(|&(_, r)| r).collect();
        assert_eq!(regs.len(), POOL.len(), "no register is handed out twice");
    }

    #[test]
    fn a_register_the_emitter_already_uses_is_withheld() {
        let mut p0 = point(&[(0, Touch::Write)]);
        p0.regs.extend(POOL.iter().copied().take(POOL.len() - 1));
        let facts = FnFacts {
            temp_count: 2,
            points: vec![
                p0,
                point(&[(0, Touch::Read), (1, Touch::Write), (1, Touch::Read)]),
                point(&[(0, Touch::Read), (1, Touch::Read)]),
            ],
            back_edges: Vec::new(),
            ..Default::default()
        };
        let a = allocate(&facts, &[true, true]);
        assert_eq!(a.resident_count(), 1, "only the untouched register is free");
        assert_eq!(a.of(0), Some(*POOL.last().unwrap()));
    }

    #[test]
    fn a_back_edge_widens_intervals_so_loop_carried_temps_do_not_share() {
        let facts = FnFacts {
            temp_count: 2,
            points: vec![
                point(&[]),
                point(&[(0, Touch::Write)]),
                point(&[(1, Touch::Write), (1, Touch::Read)]),
                point(&[(1, Touch::Read), (0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: vec![(3, 1)],
            ..Default::default()
        };
        let a = allocate(&facts, &[true, true]);
        assert_eq!(a.resident_count(), 2);
        assert_ne!(
            a.of(0),
            a.of(1),
            "a loop-carried temp must not share a register with a body temp"
        );
    }

    #[test]
    fn disjoint_intervals_share_one_register() {
        let facts = FnFacts {
            temp_count: 2,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
                point(&[(1, Touch::Write)]),
                point(&[(1, Touch::Read)]),
                point(&[(1, Touch::Read)]),
            ],
            back_edges: Vec::new(),
            ..Default::default()
        };
        let a = allocate(&facts, &[true, true]);
        assert_eq!(a.of(0), a.of(1));
        assert!(a.of(0).is_some());
    }

    #[test]
    fn allocation_is_deterministic() {
        let facts = FnFacts {
            temp_count: 4,
            points: vec![
                point(&[(0, Touch::Write), (1, Touch::Write)]),
                point(&[(2, Touch::Write), (3, Touch::Write)]),
                point(&[
                    (0, Touch::Read),
                    (1, Touch::Read),
                    (2, Touch::Read),
                    (3, Touch::Read),
                ]),
                point(&[
                    (0, Touch::Read),
                    (1, Touch::Read),
                    (2, Touch::Read),
                    (3, Touch::Read),
                ]),
            ],
            back_edges: Vec::new(),
            ..Default::default()
        };
        let first = allocate(&facts, &[true; 4]);
        for _ in 0..8 {
            assert_eq!(allocate(&facts, &[true; 4]), first);
        }
    }

    #[test]
    fn the_pool_avoids_every_register_codegen_hardcodes() {
        for r in POOL {
            assert!((19..=27).contains(r), "pool register x{r} is out of range");
        }
        assert_eq!(POOL.len(), 9);
    }
}

#[cfg(test)]
mod pays_for_itself_tests {
    use super::tests_support::*;
    use super::*;

    #[test]
    fn a_single_read_temp_is_now_promoted_because_its_copy_is_free() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![point(&[(0, Touch::Write)]), point(&[(0, Touch::Read)])],
            back_edges: Vec::new(),
            ..Default::default()
        };
        assert!(
            allocate(&facts, &[true]).of(0).is_some(),
            "with coalescing a single-read temp's copy is free, so residency pays"
        );
    }

    #[test]
    fn a_second_read_makes_the_register_pay() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![
                point(&[(0, Touch::Write)]),
                point(&[(0, Touch::Read)]),
                point(&[(0, Touch::Read)]),
            ],
            back_edges: Vec::new(),
            ..Default::default()
        };
        assert!(allocate(&facts, &[true]).of(0).is_some());
    }

    #[test]
    fn a_write_only_temp_is_now_promoted_and_its_stores_deleted() {
        let facts = FnFacts {
            temp_count: 1,
            points: vec![point(&[(0, Touch::Write)]), point(&[(0, Touch::Write)])],
            back_edges: Vec::new(),
            ..Default::default()
        };
        assert!(allocate(&facts, &[true]).of(0).is_some());
    }
}
