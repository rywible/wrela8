//! Tape pruning: Keeter 2020, as described in plans/graphics.md §2.2.
//!
//! Given affine enclosures of every slot over a region, any `min`/`max`
//! whose branches are separable over that region collapses to the winner,
//! and everything feeding the loser becomes dead. §2.2 calls this "the one
//! mechanism in the design with a shape other than a constant factor", so
//! the number it produces — pruned tape length by subdivision depth — is the
//! probe's headline output.
//!
//! The compacted tape is a real, evaluable tape rather than just a count,
//! because a count nobody can execute is a count nobody can check. The
//! bit-identity gate in `probe::selfcheck` runs the pruned tape against the
//! full one, which is §7's `diff-eval` clause applied to the instrument.

use crate::aff::Iv;
use crate::tape::{Op, Tape};

pub struct Pruned {
    pub tape: Tape,
    /// Live op count over the original tape (equals `tape.len()`).
    pub ops: usize,
    /// Live FLOP-equivalent weight.
    pub weight: u32,
    /// Blend nodes that survived. A blend that survives is a blend whose
    /// band the region straddles — §16.3's blend-band question, asked
    /// spatially instead of along a ray.
    pub blends: usize,
}

/// Which branch of a select node provably wins over the region.
#[derive(Clone, Copy, PartialEq)]
enum Sel {
    Both,
    A,
    B,
}

fn decide(op: &Op, r: &[Iv]) -> Sel {
    let (a, b, k, is_min) = match *op {
        Op::Min(a, b) => (a, b, 0.0, true),
        Op::Max(a, b) => (a, b, 0.0, false),
        // `smin` equals `min` only outside the blend band, so separability
        // needs an extra `k` of clearance. This is exactly why a cluster of
        // wide blends resists pruning (§16.2's worst-case scene).
        Op::SMin(a, b, k) => (a, b, k, true),
        Op::SMax(a, b, k) => (a, b, k, false),
        _ => return Sel::Both,
    };
    let (alo, ahi) = (r[a as usize].lo, r[a as usize].hi);
    let (blo, bhi) = (r[b as usize].lo, r[b as usize].hi);
    if !(alo.is_finite() && ahi.is_finite() && blo.is_finite() && bhi.is_finite()) {
        return Sel::Both;
    }
    if is_min {
        if ahi + k <= blo {
            return Sel::A;
        }
        if bhi + k <= alo {
            return Sel::B;
        }
    } else {
        if alo >= bhi + k {
            return Sel::A;
        }
        if blo >= ahi + k {
            return Sel::B;
        }
    }
    Sel::Both
}

/// Prune `tape` against per-slot enclosures `ranges` and return a compacted,
/// independently evaluable tape.
pub fn prune(tape: &Tape, ranges: &[Iv]) -> Pruned {
    let n = tape.ops.len();
    debug_assert_eq!(ranges.len(), n);

    let mut sel = vec![Sel::Both; n];
    for i in 0..n {
        sel[i] = decide(&tape.ops[i], ranges);
    }

    // Follow select chains to the slot that actually supplies the value.
    // Bounded by n because every hop strictly decreases the index.
    let mut repr: Vec<u32> = (0..n as u32).collect();
    for i in 0..n {
        let r = match (sel[i], tape.ops[i]) {
            (Sel::A, Op::Min(a, _) | Op::Max(a, _) | Op::SMin(a, _, _) | Op::SMax(a, _, _)) => {
                repr[a as usize]
            }
            (Sel::B, Op::Min(_, b) | Op::Max(_, b) | Op::SMin(_, b, _) | Op::SMax(_, b, _)) => {
                repr[b as usize]
            }
            _ => i as u32,
        };
        repr[i] = r;
    }

    // Backward liveness from the resolved root.
    let mut live = vec![false; n];
    let root = repr[tape.root as usize];
    live[root as usize] = true;
    for i in (0..n).rev() {
        if !live[i] {
            continue;
        }
        let (ins, cnt) = tape.ops[i].inputs();
        for j in 0..cnt {
            live[repr[ins[j] as usize] as usize] = true;
        }
    }

    // Forward compaction with index remapping.
    let mut map = vec![u32::MAX; n];
    let mut ops: Vec<Op> = Vec::new();
    let mut weight = 0u32;
    let mut blends = 0usize;
    for i in 0..n {
        if !live[i] {
            continue;
        }
        let m = |s: u32| -> u32 { map[repr[s as usize] as usize] };
        let op = match tape.ops[i] {
            Op::X => Op::X,
            Op::Y => Op::Y,
            Op::Z => Op::Z,
            Op::Const(v) => Op::Const(v),
            Op::Neg(a) => Op::Neg(m(a)),
            Op::Add(a, b) => Op::Add(m(a), m(b)),
            Op::Sub(a, b) => Op::Sub(m(a), m(b)),
            Op::Mul(a, b) => Op::Mul(m(a), m(b)),
            Op::Square(a) => Op::Square(m(a)),
            Op::Sqrt(a) => Op::Sqrt(m(a)),
            Op::Abs(a) => Op::Abs(m(a)),
            Op::Min(a, b) => Op::Min(m(a), m(b)),
            Op::Max(a, b) => Op::Max(m(a), m(b)),
            Op::SMin(a, b, k) => Op::SMin(m(a), m(b), k),
            Op::SMax(a, b, k) => Op::SMax(m(a), m(b), k),
            Op::Clamp01(a) => Op::Clamp01(m(a)),
            Op::Sin(a) => Op::Sin(m(a)),
            Op::AddC(a, v) => Op::AddC(m(a), v),
            Op::MulC(a, v) => Op::MulC(m(a), v),
            Op::Rep(a, v) => Op::Rep(m(a), v),
            Op::Len2(a, b) => Op::Len2(m(a), m(b)),
            Op::Len3(a, b, c) => Op::Len3(m(a), m(b), m(c)),
        };
        map[i] = ops.len() as u32;
        weight += op.weight();
        if op.is_blend() {
            blends += 1;
        }
        ops.push(op);
    }

    let new_root = map[root as usize];
    let ops_len = ops.len();
    Pruned {
        tape: Tape {
            ops,
            root: new_root,
        },
        ops: ops_len,
        weight,
        blends,
    }
}
