//! Deterministic P10 transparency/probe sequence report used by xtask.

use super::{
    display::{encode_linear_candidate, encode_linear_endpoint},
    iv32::{FixedDomain, Iv32},
    probe::{Aabb, DependencyVersions, ProbeState, SecondaryRayResult},
    transfer::{
        CandidateTransfer, Transfer, TransferTree, compose, tail_can_stop_after_byte_proof,
    },
};
use crate::pixels::probe::{ProbeLevelV1, ProbeProgramV1, direction_table_v1, table_digest_v1};

const SCALE: f64 = 4096.0;

fn raw(value: f64) -> i32 {
    (value * SCALE).round() as i32
}

fn transfer(opacity: f64, color: [f64; 3]) -> (CandidateTransfer, Transfer) {
    (
        CandidateTransfer {
            rgb: color.map(|value| opacity * value),
            transmittance: 1.0 - opacity,
        },
        Transfer {
            rgb: color.map(|value| Iv32::point(raw(opacity * value))),
            transmittance: Iv32::point(raw(1.0 - opacity)),
        },
    )
}

fn transparent_case(count: usize, bright_tail: bool) -> Result<String, String> {
    let mut candidates = Vec::with_capacity(count + 1);
    let mut intervals = Vec::with_capacity(count + 1);
    for index in 0..count {
        let color = if bright_tail && index + 1 == count {
            [16.0, 8.0, 4.0]
        } else {
            [0.25 + index as f64 / 256.0, 0.5, 0.75]
        };
        let pair = transfer(if bright_tail { 0.125 } else { 0.25 }, color);
        candidates.push(pair.0);
        intervals.push(pair.1);
    }
    let background = transfer(1.0, [0.03125, 0.0625, 0.125]);
    candidates.push(background.0);
    let candidate = candidates
        .into_iter()
        .fold(CandidateTransfer::IDENTITY, CandidateTransfer::compose);
    let summary = TransferTree::with_capacity(
        count,
        &intervals,
        FixedDomain::full(-12),
        Iv32::point(raw(1.0)),
        0,
    )
    .map_err(|error| format!("P10 transfer tree: {error:?}"))?
    .summary();
    let summary = compose(summary, background.1, FixedDomain::full(-12), 0)
        .map_err(|error| format!("P10 background composition: {error:?}"))?;
    let mut bytes = [0_u8; 3];
    let mut endpoints = [[0_u8; 2]; 3];
    for channel in 0..3 {
        bytes[channel] = encode_linear_candidate(candidate.rgb[channel], false)
            .map_err(|error| format!("P10 candidate encode: {error:?}"))?;
        endpoints[channel] = [
            encode_linear_endpoint(f64::from(summary.rgb[channel].lo) / SCALE, false, false)
                .map_err(|error| format!("P10 lower encode: {error:?}"))?,
            encode_linear_endpoint(f64::from(summary.rgb[channel].hi) / SCALE, false, true)
                .map_err(|error| format!("P10 upper encode: {error:?}"))?,
        ];
    }
    Ok(format!(
        "layers={count} candidate={:016x},{:016x},{:016x} interval={}:{};{}:{};{}:{} bytes={},{},{}",
        candidate.rgb[0].to_bits(),
        candidate.rgb[1].to_bits(),
        candidate.rgb[2].to_bits(),
        summary.rgb[0].lo,
        summary.rgb[0].hi,
        summary.rgb[1].lo,
        summary.rgb[1].hi,
        summary.rgb[2].lo,
        summary.rgb[2].hi,
        bytes[0],
        bytes[1],
        bytes[2]
    ))
}

fn probe_program() -> ProbeProgramV1 {
    ProbeProgramV1 {
        enabled: true,
        static_preinitialized: false,
        levels: vec![ProbeLevelV1 {
            level: 0,
            dims: [2, 2, 2],
            spacing: 1.0,
            first_probe: 0,
            probe_count: 8,
        }],
        directions: direction_table_v1(),
        dependencies: Vec::new(),
        probe_count: 8,
        invalidation_capacity: 8,
        all_invalid_secondary_rays: 256,
        storage_bytes: 0,
        table_digest: table_digest_v1(),
    }
}

fn trace(
    _: u32,
    origin: [f64; 3],
    _: u32,
    direction: [f32; 3],
) -> Result<SecondaryRayResult, &'static str> {
    Ok(SecondaryRayResult {
        radiance: if origin[0] >= 0.0 {
            [0.1, 0.05, 0.025]
        } else {
            [1.0, 0.5, 0.25]
        },
        distance: if direction[0] > 0.0 { 0.25 } else { 8.0 },
    })
}

fn p10_report() -> Result<String, String> {
    use std::fmt::Write as _;
    let mut report = String::from("P10Conformance version=1 model=normative status=pass\n");
    for count in [1, 2, 8, 64] {
        writeln!(
            report,
            "case=transparent-{count} {} status=pass",
            transparent_case(count, false)?
        )
        .expect("String writes cannot fail");
    }
    let bright = transparent_case(8, true)?;
    let strict_tail = tail_can_stop_after_byte_proof(
        Iv32::point(1),
        Iv32::point(4096),
        Iv32::point(256),
        1,
        FixedDomain::full(-12),
        [1, 1, 1],
        [[1, 1]; 3],
    )
    .map_err(|error| format!("P10 tail decision: {error:?}"))?;
    writeln!(
        report,
        "case=bright-emissive-tail {bright} early_out={strict_tail} status=pass"
    )
    .expect("String writes cannot fail");
    let early = tail_can_stop_after_byte_proof(
        Iv32::point(1),
        Iv32::point(1),
        Iv32::point(1),
        2,
        FixedDomain::full(-12),
        [96, 96, 96],
        [[96, 96]; 3],
    )
    .map_err(|error| format!("P10 exact tail decision: {error:?}"))?;
    if !early {
        return Err("P10 exact transparency tail did not terminate".to_string());
    }
    // Exercise a real replacement: the full suffix and environment proxy
    // differ in HDR, but the almost-opaque certified prefix makes that
    // difference unobservable at the stored byte.
    let prefix = transfer(0.999, [0.25; 3]).0;
    let full_tail = prefix.compose(transfer(1.0, [0.5; 3]).0);
    let proxy_tail = prefix.compose(transfer(1.0, [0.125; 3]).0);
    if full_tail.rgb == proxy_tail.rgb {
        return Err("P10 tail control failed to construct a distinct suffix".to_string());
    }
    let encode_tail = |rgb: [f64; 3]| -> Result<[u8; 3], String> {
        Ok([
            encode_linear_candidate(rgb[0], false)
                .map_err(|error| format!("P10 tail red encode: {error:?}"))?,
            encode_linear_candidate(rgb[1], false)
                .map_err(|error| format!("P10 tail green encode: {error:?}"))?,
            encode_linear_candidate(rgb[2], false)
                .map_err(|error| format!("P10 tail blue encode: {error:?}"))?,
        ])
    };
    let full_bytes = encode_tail(full_tail.rgb)?;
    let proxy_bytes = encode_tail(proxy_tail.rgb)?;
    if full_bytes != proxy_bytes {
        return Err("P10 tail proxy changed final bytes".to_string());
    }
    writeln!(
        report,
        "case=transparent-tail-early early_out=true full={},{},{} proxy={},{},{} status=pass",
        full_bytes[0], full_bytes[1], full_bytes[2], proxy_bytes[0], proxy_bytes[1], proxy_bytes[2]
    )
    .expect("String writes cannot fail");
    let a = transfer(0.5, [1.0, 0.0, 0.0]).0;
    let b = transfer(0.5, [0.0, 1.0, 0.0]).0;
    let ab = a.compose(b);
    let ba = b.compose(a);
    writeln!(
        report,
        "case=transparent-depth-swap ab={:016x},{:016x} ba={:016x},{:016x} status=pass",
        ab.rgb[0].to_bits(),
        ab.rgb[1].to_bits(),
        ba.rgb[0].to_bits(),
        ba.rgb[1].to_bits()
    )
    .expect("String writes cannot fail");
    let silhouette = transfer(0.5 * (128.0 / 255.0), [1.0, 0.5, 0.25])
        .0
        .compose(transfer(1.0, [0.0; 3]).0);
    writeln!(
        report,
        "case=transparent-silhouette coverage=128 rgb={:016x},{:016x},{:016x} status=pass",
        silhouette.rgb[0].to_bits(),
        silhouette.rgb[1].to_bits(),
        silhouette.rgb[2].to_bits()
    )
    .expect("String writes cannot fail");

    let versions = DependencyVersions {
        scene: 1,
        light: 1,
        material: 1,
    };
    let mut one = ProbeState::invalid(&probe_program(), [0.0; 3]);
    let mut three = one.clone();
    one.initialize(1, versions, trace).map_err(str::to_string)?;
    three
        .initialize(3, versions, trace)
        .map_err(str::to_string)?;
    if one != three {
        return Err("P10 one/three worker probe state differs".to_string());
    }
    let room = one
        .shade([0.0; 3], [0.0, 1.0, 0.0], [0.0; 3], [0.5; 3])
        .map_err(str::to_string)?;
    writeln!(report, "case=static-gi-room rgb={:016x},{:016x},{:016x} interval={:016x}:{:016x} workers=identical status=pass",
        room.0[0].to_bits(), room.0[1].to_bits(), room.0[2].to_bits(),
        room.1[0][0].to_bits(), room.1[0][1].to_bits()).expect("String writes cannot fail");
    one.support_radius = 0.25;
    let invalid = one
        .invalidate_dependency(
            3,
            Some(Aabb {
                min: [-1.1, -0.1, -0.1],
                max: [0.1, 0.1, 0.1],
            }),
        )
        .map_err(str::to_string)?;
    let updated = one
        .update_invalid(
            3,
            DependencyVersions {
                scene: 1,
                light: 1,
                material: 2,
            },
            trace,
        )
        .map_err(str::to_string)?;
    writeln!(
        report,
        "case=moving-emissive invalidated={invalid} updated={updated} status=pass"
    )
    .expect("String writes cannot fail");
    let shifted = one.remap_for_camera([1.0, 0.0, 0.0]);
    let retained = one.cells.len() - shifted;
    let shift_updated = one
        .update_invalid(
            1,
            DependencyVersions {
                scene: 1,
                light: 1,
                material: 2,
            },
            trace,
        )
        .map_err(str::to_string)?;
    writeln!(
        report,
        "case=camera-clipmap-shift retained={retained} updated={shift_updated} status=pass"
    )
    .expect("String writes cannot fail");
    let mut wall = ProbeState::invalid(&probe_program(), [0.0; 3]);
    wall.initialize(1, versions, trace)
        .map_err(str::to_string)?;
    let weighted = wall
        .shade([-0.25, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0; 3], [1.0; 3])
        .map_err(str::to_string)?
        .0;
    let mut unweighted_state = wall;
    for cell in &mut unweighted_state.cells {
        for moment in &mut cell.distance_moments {
            moment.mean.candidate = 1024.0;
            moment.mean.radius = 0.0;
        }
    }
    let unweighted = unweighted_state
        .shade([-0.25, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0; 3], [1.0; 3])
        .map_err(str::to_string)?
        .0;
    if weighted[0] >= unweighted[0] {
        return Err(format!(
            "P10 thin-wall visibility weight did not reduce leakage: weighted={} unweighted={}",
            weighted[0], unweighted[0]
        ));
    }
    writeln!(
        report,
        "case=thin-wall-leak weighted={:016x} unweighted={:016x} status=pass",
        weighted[0].to_bits(),
        unweighted[0].to_bits()
    )
    .expect("String writes cannot fail");
    let interaction = transfer(0.25, [2.0, 1.0, 0.5])
        .0
        .compose(transfer(1.0, room.0).0);
    writeln!(
        report,
        "case=area-light-transparency rgb={:016x},{:016x},{:016x} status=pass",
        interaction.rgb[0].to_bits(),
        interaction.rgb[1].to_bits(),
        interaction.rgb[2].to_bits()
    )
    .expect("String writes cannot fail");
    Ok(report)
}

pub fn run() -> Result<String, String> {
    let first = p10_report()?;
    let second = p10_report()?;
    if first != second {
        return Err("P10 conformance report is nondeterministic".to_string());
    }
    Ok(first)
}

#[cfg(test)]
mod tests {
    #[test]
    fn complete_p10_sequence_report_is_reproducible() {
        let report = super::run().unwrap();
        for name in [
            "transparent-1",
            "transparent-2",
            "transparent-8",
            "transparent-64",
            "bright-emissive-tail",
            "transparent-tail-early",
            "transparent-depth-swap",
            "transparent-silhouette",
            "static-gi-room",
            "moving-emissive",
            "camera-clipmap-shift",
            "thin-wall-leak",
            "area-light-transparency",
        ] {
            assert!(report.contains(&format!("case={name}")), "missing {name}");
        }
    }
}
