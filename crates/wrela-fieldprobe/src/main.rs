//! `fieldprobe` — the plans/graphics.md §16 benchmark, in counts.
//!
//! Runs before any FieldWir exists, deliberately: §16.4 says *do not commit
//! FieldWir before these numbers exist*, and a measurement that needs the
//! thing it is meant to justify arrives too late to change anything.
//!
//! Everything printed is a count or a ratio. Per §16.1 the probe never
//! times: on the M4 proxy, counts port to a Pi 5 and wall-clock does not.
//! Converting counts to Pi 5 cycles is `bench/a76-pi5.toml`'s job.

mod aff;
mod atlas;
mod camera;
mod eval;
mod probe;
mod prune;
mod report;
mod scene;
mod tape;

use probe::{Rng, Scratch};
use report::{fmt_pct, median, nearest_mode, peak_flops, res_16x9};

struct Config {
    w: u32,
    h: u32,
    max_depth: u32,
    base_tile: f32,
    march_stride: u32,
    band_samples: u32,
    cont_cells: usize,
    recon_cap: u64,
    reproj_stride: u32,
    atlas_depth: u32,
    atlas_eps: f32,
    motion_stride: u32,
    ppm: bool,
}

fn main() {
    let mut cfg = Config {
        // §1's floor. Ratios below are scale-relative, but the edge-cell and
        // reconstruction numbers are per-pixel, so the resolution is part of
        // the result and is printed with it.
        w: 512,
        h: 288,
        max_depth: 4,
        base_tile: 64.0,
        march_stride: 2,
        band_samples: 24,
        cont_cells: 900,
        recon_cap: 1200,
        reproj_stride: 2,
        atlas_depth: 11,
        atlas_eps: 0.05,
        motion_stride: 2,
        ppm: false,
    };
    let mut only: Option<String> = None;
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut i = 0;
    while i < args.len() {
        let a = args[i].clone();
        let mut next = || -> String {
            i += 1;
            args.get(i).cloned().unwrap_or_default()
        };
        match a.as_str() {
            "--width" => cfg.w = next().parse().unwrap_or(cfg.w),
            "--height" => cfg.h = next().parse().unwrap_or(cfg.h),
            "--depth" => cfg.max_depth = next().parse().unwrap_or(cfg.max_depth),
            "--scene" => only = Some(next()),
            "--ppm" => cfg.ppm = true,
            "--atlas-depth" => cfg.atlas_depth = next().parse().unwrap_or(cfg.atlas_depth),
            "--atlas-eps" => cfg.atlas_eps = next().parse().unwrap_or(cfg.atlas_eps),
            "--quick" => {
                cfg.march_stride = 4;
                cfg.cont_cells = 200;
                cfg.recon_cap = 300;
                cfg.reproj_stride = 4;
                cfg.motion_stride = 4;
                cfg.atlas_depth = 11;
            }
            _ => {}
        }
        i += 1;
    }

    if args.iter().any(|a| a == "--recon-sweep") {
        recon_sweep(&only);
        return;
    }

    println!("fieldprobe — plans/graphics.md §16, counts only");
    println!(
        "resolution {}x{}  base tile {}px  max depth {}  leaf {}px",
        cfg.w,
        cfg.h,
        cfg.base_tile,
        cfg.max_depth,
        cfg.base_tile / (1u32 << cfg.max_depth) as f32
    );
    println!();

    let scenes = [
        scene::colonnade(cfg.w, cfg.h),
        scene::colonnade_flat(cfg.w, cfg.h),
        scene::melee(cfg.w, cfg.h),
    ];
    let mut summary: Vec<(String, f64, f64, f64)> = Vec::new();

    for sc in scenes.iter() {
        if let Some(o) = &only {
            if o != sc.name {
                continue;
            }
        }
        let mut s = Scratch::default();
        let mut rng = Rng(0x5eed_1234);

        println!("========================================================");
        println!("scene: {}", sc.name);
        println!("  spec: {}", sc.spec);
        println!(
            "  tape: {} ops, {} FLOP-equiv, {} blend nodes, t in [{}, {}]",
            sc.tape.len(),
            sc.tape.weight(),
            sc.tape.blend_count(),
            sc.t_near,
            sc.t_far
        );
        println!("========================================================");

        // --- self-check: nothing below means anything until this passes ---
        let chk = probe::run_selfcheck(sc, 240, 32, &mut rng, &mut s);
        println!();
        println!("[selfcheck]  (the instrument, not the design)");
        println!("  samples                 {}", chk.samples);
        println!("  AA containment failures {}", chk.containment_failures);
        println!("  prune bit-mismatches    {}", chk.prune_mismatches);
        println!(
            "  grad rel err            max {:.2e}  p99 {:.2e}  ({} smooth samples)",
            chk.grad_max_rel_err, chk.grad_p99_rel_err, chk.grad_tested
        );
        println!(
            "  non-differentiable pts  {}  ({} of sampled axes — no gradient for §2.5 there)",
            chk.grad_kink_skips,
            fmt_pct(
                chk.grad_kink_skips as f64,
                (chk.grad_kink_skips + chk.grad_tested).max(1) as f64
            )
        );
        println!(
            "  median enclosure overwidth {:.2}x  (1.0 = exact)",
            chk.mean_overwidth
        );
        // Gate on p99, not max: a central difference within ~eps of a kink is
        // genuinely neither one-sided derivative, so the tail measures the
        // scene's non-smoothness, not the instrument's correctness.
        if chk.containment_failures > 0
            || chk.prune_mismatches > 0
            || chk.grad_p99_rel_err > 1e-2
        {
            println!();
            println!("  FAIL — the instrument is unsound; every number below is void.");
            std::process::exit(1);
        }

        // --- E1: classification + pruning by depth -------------------------
        let cl = probe::run_classify(sc, cfg.max_depth, cfg.base_tile, &mut s);
        let total_area = (cfg.w as f64) * (cfg.h as f64);
        println!();
        println!("[E1] tile classification and tape pruning by depth   §2.1, §2.2");
        println!("  depth  cells    tile_px   live_ops(mean/med/max)   blends  weight");
        for (d, st) in cl.per_depth.iter().enumerate() {
            if st.cells == 0 {
                continue;
            }
            let tile = cfg.base_tile / (1u32 << d) as f32;
            let mut h = st.ops_hist.clone();
            h.sort_unstable();
            println!(
                "  {:>5}  {:>7}  {:>6.1}   {:>6.1} / {:>4} / {:>4}      {:>5.2}  {:>6.1}",
                d,
                st.cells,
                tile,
                st.ops_sum as f64 / st.cells as f64,
                median(&h),
                st.ops_max,
                st.blends_sum as f64 / st.cells as f64,
                st.weight_sum as f64 / st.cells as f64,
            );
        }
        println!(
            "  (full tape = {} ops, {} FLOP-equiv)",
            sc.tape.len(),
            sc.tape.weight()
        );
        println!();
        println!("  screen area by outcome:");
        println!("    exterior (no ray traced)   {}", fmt_pct(cl.area_exterior, total_area));
        println!("    certified interior         {}", fmt_pct(cl.area_interior, total_area));
        println!("    edge cells (sil. or seam)  {}", fmt_pct(cl.area_edge, total_area));
        println!("    unresolved residue         {}", fmt_pct(cl.area_unresolved, total_area));
        println!("    non-finite enclosures      {}", cl.nonfinite);
        println!(
            "  interior certificate rejected at leaf: dt-straddles {} / face-test {}",
            cl.fail_dt_straddles, cl.fail_faces
        );
        let leaf = cfg.base_tile / (1u32 << cfg.max_depth) as f32;
        let edge_len = cl.edge_cells as f64 * leaf as f64;
        println!();
        println!(
            "  edge cells {} at {}px  ->  est. edge length ~{:.0} px",
            cl.edge_cells, leaf, edge_len
        );
        println!(
            "    = {:.2} screen-widths; scaled to 4K, edge pixels ~{:.0}k = {} of a 4K frame",
            edge_len / cfg.w as f64,
            edge_len * (3840.0 / cfg.w as f64) * 2.0 / 1000.0,
            fmt_pct(edge_len * (3840.0 / cfg.w as f64) * 2.0, 3840.0 * 2160.0)
        );

        // --- E4: AA vs plain IA -------------------------------------------
        let ti = probe::run_tightness(sc, 32.0, &mut s);
        println!();
        println!("[E4] affine vs plain interval arithmetic             §2.1");
        println!("  cells tested                {}", ti.cells);
        println!(
            "  proven empty by AA alone    {}",
            fmt_pct(ti.aa_empty as f64, ti.cells as f64)
        );
        println!(
            "  proven empty by plain IA    {}",
            fmt_pct(ti.iv_empty as f64, ti.cells as f64)
        );
        println!(
            "  mean width ratio IA/AA      {:.2}x  (<1 means IA is tighter)",
            ti.width_ratio_sum / ti.width_ratio_n.max(1) as f64
        );

        // --- E2/E3: marching ----------------------------------------------
        let mo = probe::run_march(sc, cfg.march_stride, cfg.band_samples, &mut s);
        println!();
        println!("[E2/E3] naive sphere tracing                         §1, §2.3");
        println!(
            "  rays {}  hit rate {}",
            mo.rays,
            fmt_pct(mo.hits as f64, mo.rays as f64)
        );
        println!("  evals/pixel (naive, unpruned)   {:.1}", mo.evals as f64 / mo.rays as f64);
        println!("  worst-case steps on one ray     {}", mo.steps_max);
        println!(
            "  rays that exhausted the step budget (recorded as misses)  {}",
            probe::STEP_CAP_HITS.swap(0, std::sync::atomic::Ordering::Relaxed)
        );
        println!(
            "  blend-band ray fraction         {}   <-- §16.3's most load-bearing number",
            fmt_pct(mo.band_len, mo.total_len)
        );
        println!(
            "    (hitting rays only)           {}",
            fmt_pct(mo.band_len_hit, mo.total_len_hit)
        );
        println!(
            "    (by sample count)             {}",
            fmt_pct(mo.band_active_samples as f64, mo.band_samples as f64)
        );

        // --- E5: continuation ---------------------------------------------
        let step = (cl.interior_cells.len() / cfg.cont_cells.max(1)).max(1);
        let cells: Vec<_> = cl.interior_cells.iter().step_by(step).copied().collect();
        let co = probe::run_continuation(sc, &cells, &mut s);
        println!();
        println!("[E5] continuation on the hit manifold vs marching");
        println!(
            "  interior cells walked      {} (of {})",
            cells.len(),
            cl.interior_cells.len()
        );
        println!("  samples                    {}", co.samples);
        println!(
            "  converged to the true hit  {}",
            fmt_pct(co.converged as f64, co.samples.max(1) as f64)
        );
        if co.samples > 0 {
            println!(
                "  eval-equiv / sample: continuation {:.1}   marching {:.1}   ratio {:.2}x",
                co.cont_evals / co.samples as f64,
                co.march_evals / co.samples as f64,
                co.march_evals / co.cont_evals.max(1e-9)
            );
        }
        println!("  worst depth error          {:.3} px of parallax", co.max_err_px);

        // --- E6: reconstruction factor ------------------------------------
        println!();
        println!("[E6] quadratic patch reconstruction — depth vs inverse depth");
        println!("  9 samples per cell reconstruct cell_px^2 pixels, so the");
        println!("  reconstruction factor is cell_px^2/9.");
        let mut recon_factor = 1.0f64;
        for (space, sname) in [
            (probe::FitSpace::Depth, "t"),
            (probe::FitSpace::InverseDepth, "1/t"),
        ] {
            for &tol in &[0.5f32, 1.0] {
                print!("  fit {:<4} tol {:.1}px  ", sname, tol);
                let mut best = 0.0f32;
                for &cp in &[4.0f32, 8.0, 16.0, 32.0, 64.0] {
                    let r = probe::run_reconstruction_capped(
                        sc, cp, cfg.recon_cap, space, tol, &mut s,
                    );
                    let rate = r.passed as f64 / r.tested.max(1) as f64;
                    print!("{:>3.0}px {:>6}  ", cp, fmt_pct(r.passed as f64, r.tested.max(1) as f64));
                    if rate >= 0.90 {
                        best = best.max(cp);
                    }
                }
                let f = if best > 0.0 { (best * best / 9.0) as f64 } else { 1.0 };
                println!("=> {:.1}x", f);
                if space == probe::FitSpace::InverseDepth && tol == 1.0 {
                    recon_factor = f;
                }
            }
        }

        println!();
        println!("[E6b] adaptive reconstruction (quadtree, inverse-depth patches)");
        println!("  tol_px  samples/frame  reconstruction  per-pixel residue  patch sizes");
        let mut best_factor = 1.0f64;
        for &tol in &[0.5f32, 1.0, 2.0, 4.0, 8.0] {
            let ra = probe::run_recon_adaptive(sc, 64.0, 1.0, tol, &mut s);
            let sizes: Vec<String> = ra
                .patches
                .iter()
                .map(|(sz, n)| format!("{:.0}px:{}", sz, n))
                .collect();
            println!(
                "  {:>5.1}   {:>13}   {:>12.2}x   {:>16}   {}",
                tol,
                ra.samples,
                ra.factor(),
                fmt_pct(ra.dense_px as f64, ra.pixels.max(1) as f64),
                sizes.join(" ")
            );
            if tol == 1.0 {
                best_factor = ra.factor();
            }
        }
        println!(
            "  -> at 1px tolerance the guest shades {:.2}x fewer samples than output pixels",
            best_factor
        );

        for &(tol, base) in &[(1.0f32, 64usize), (2.0, 64)] {
            let (_, er) = probe::run_edge_recon(sc, tol, base, &mut s);
            let sizes: Vec<String> =
                er.patches.iter().map(|(z, n)| format!("{}px:{}", z, n)).collect();
            println!(
                "  [E6d] edge-aware, tol {:.0}px: {} samples ({} patch + {} edge + {} dense) \
                 -> {:.2}x",
                tol,
                er.samples(),
                er.patch_samples,
                er.edge_samples,
                er.dense_samples,
                er.factor()
            );
            println!("        patches: {}", sizes.join(" "));
        }
        let ec = probe::run_edge_census(sc, &mut s);
        println!();
        println!("[E6c] true discontinuity density (per pixel, no quadtree)");
        println!(
            "  silhouette (hit/miss) {}   depth step (>5%) {}   either {}",
            fmt_pct(ec.silhouette as f64, ec.pixels as f64),
            fmt_pct(ec.depth_step as f64, ec.pixels as f64),
            fmt_pct(ec.edge as f64, ec.pixels as f64)
        );
        {
            // What a curve-bounded representation could reach: edge pixels
            // sampled densely, everything else carried by 16px patches
            // (9 samples each), which the quadtree already achieves wherever
            // a cell is clean.
            let e = ec.edge as f64;
            let smooth = ec.pixels as f64 - e;
            let samples = e + smooth / (16.0 * 16.0) * 9.0;
            println!(
                "  ceiling if edges were curves not cells: {:.0} samples -> {:.2}x reconstruction",
                samples,
                ec.pixels as f64 / samples
            );
        }

        // --- E7: reprojection ---------------------------------------------
        let rp = probe::run_reprojection(sc, cfg.reproj_stride, 0.08, &mut s);
        println!();
        println!("[E7] reprojection across the pose pair               §4.4");
        println!("  pixels                     {}", rp.pixels);
        println!(
            "  had a reprojected hint     {}",
            fmt_pct(rp.hinted as f64, rp.pixels as f64)
        );
        println!(
            "  disoccluded (no hint)      {}",
            fmt_pct(rp.disoccluded as f64, rp.pixels as f64)
        );
        println!(
            "  hint verified by the march {}",
            fmt_pct(rp.verified as f64, rp.hinted.max(1) as f64)
        );
        println!(
            "  hint tunnelled/wrong       {}",
            fmt_pct(rp.tunnelled as f64, rp.hinted.max(1) as f64)
        );

        // --- E8: modelled frame cost, and the resolution it buys ----------
        let fc = probe::run_framecost(sc, &cl, &mut s);
        println!();
        println!("[E8] modelled frame cost at {}x{}          §1, §16.1", cfg.w, cfg.h);
        println!(
            "  pixels {}: interior {} / marched {} / exterior {}   hit rate {}",
            fc.pixels,
            fc.interior_px,
            fc.marched_px,
            fc.exterior_px,
            fmt_pct(fc.hits as f64, fc.pixels as f64)
        );
        println!("  mean marching steps on marched pixels  {:.1}", fc.mean_steps);
        println!(
            "  SOUNDNESS: pixels proved empty that the marcher hits  {}",
            fc.exterior_hits
        );
        if fc.exterior_hits > 0 {
            println!("  FAIL — the enclosure lied; every area fraction above is void.");
            std::process::exit(1);
        }
        let t = fc.total();
        for (label, v) in [
            ("traversal (affine classify+prune)", fc.traversal),
            ("primary: slab march (tight tape)", fc.primary - fc.primary_fallback),
            ("primary: past-slab fallback (wide tape)", fc.primary_fallback),
            ("shadow rays", fc.shadow),
            ("AO + GI taps", fc.ao_gi),
            ("shading arithmetic (§1 model)", fc.shade),
            ("post (§1 model)", fc.post),
        ] {
            println!("    {:<40} {:>9.1} MFLOP  {}", label, v / 1e6, fmt_pct(v, t));
        }
        println!("  TOTAL {:.1} MFLOP/frame = {:.0} FLOP/pixel", t / 1e6, fc.per_pixel());
        println!(
            "  with a perfect §2.1 interior certificate: {:.0} FLOP/pixel ({:.2}x better)",
            fc.per_pixel_ideal(),
            fc.per_pixel() / fc.per_pixel_ideal().max(1e-9)
        );
        println!(
            "  BRACKET: {:.0} FLOP/pixel (slab re-classification) .. {:.0} (as modelled)",
            fc.per_pixel_optimistic(),
            fc.per_pixel()
        );
        println!();
        println!("  Excluded, because they are unbuilt and would only help:");
        println!("    continuation (E5, measured 1.8-2.0x on primary), fp16 (§3, 2x on");
        println!("    secondary+shading), temporal reuse (§4/E7 hint rate 39-66%).");

        println!();
        println!("  Pi 5 projection — peak {:.0} GFLOP/s over 2.4 render-core-equivalents",
            peak_flops() / 1e9);
        println!("  sustained   rate    budget/frame    16:9 frame (bracket)        mode at each end");
        for sust in [0.20f64, 0.30, 0.40] {
            for rate in [30.0f64, 60.0] {
                let budget = peak_flops() * sust / rate;
                let lo = budget / fc.per_pixel();
                let hi = budget / fc.per_pixel_optimistic();
                println!(
                    "  {:>6.0}%   {:>3.0} Hz   {:>8.2} GFLOP   {:>10} .. {:<10}  {} .. {}",
                    sust * 100.0,
                    rate,
                    budget / 1e9,
                    res_16x9(lo),
                    res_16x9(hi),
                    nearest_mode(lo),
                    nearest_mode(hi)
                );
            }
        }

        // --- E9: the baked atlas ------------------------------------------
        let bcfg = atlas::BakeCfg {
            max_depth: cfg.atlas_depth,
            eps_frac: 0.10,
            eps_abs: cfg.atlas_eps,
            fit_n: 4,
            chk_n: 6,
        };
        let at = atlas::bake(&sc.tape, sc.bounds, &bcfg, &mut s);
        println!();
        println!("[E9] baked atlas — certified analytic solves            §13, §2.3");
        println!(
            "  bake: depth<={} eps<={:.4}  nodes {}  proxies {}  tape palette {}",
            bcfg.max_depth, bcfg.eps_abs, at.nodes.len(), at.proxies.len(), at.tapes.len()
        );
        println!(
            "  cells: empty {}  full {}  proxy {}  live {}  branch {}  deepest {}",
            at.n_empty, at.n_full, at.n_proxy, at.n_live, at.n_branch, at.deepest
        );
        println!(
            "  memory {:.1} MB     bake cost {:.2} GFLOP-equiv (offline)",
            at.bytes() as f64 / 1e6,
            at.bake_evals as f64 / 1e9
        );
        let ao = probe::run_atlas(sc, &at, &mut s);
        let c = &ao.cost;
        println!(
            "  per pixel: nodes {:.1}  solves {:.2}  verify-fail {:.3}  full-entry {:.3}  escaped {}",
            c.node_visits as f64 / ao.pixels as f64,
            c.solves as f64 / ao.pixels as f64,
            c.verify_fail as f64 / ao.pixels as f64,
            c.full_entries as f64 / ao.pixels as f64,
            fmt_pct(c.escaped as f64, ao.pixels as f64)
        );
        let a_trav = c.node_visits as f64 * atlas::NODE_FLOP / ao.pixels as f64;
        let a_solve = c.solves as f64 * atlas::SOLVE_FLOP / ao.pixels as f64;
        let a_ver = c.verify_ops as f64 / ao.pixels as f64;
        let a_march = c.march_ops as f64 / ao.pixels as f64;
        let a_tot = a_trav + a_solve + a_ver + a_march;
        println!(
            "  FLOP/px: traverse {:.0} + solve {:.0} + verify {:.0} + residual march {:.0} = {:.0}",
            a_trav, a_solve, a_ver, a_march, a_tot
        );
        let base_primary = (fc.traversal + fc.primary) / fc.pixels as f64;
        println!(
            "  vs measured live primary+traversal {:.0} FLOP/px  ->  {:.2}x",
            base_primary,
            base_primary / a_tot.max(1e-9)
        );
        println!(
            "  SOUNDNESS: atlas vs march disagreements {}  (atlas-missed {} / atlas-extra {} \
             / depth {} worst dt {:.4})",
            ao.mismatches,
            ao.miss_atlas_none,
            ao.miss_atlas_extra,
            ao.miss_depth,
            ao.worst_dt
        );
        // Gate at 0.05% of pixels rather than zero. The residue is
        // floating-point disagreement on cell faces between two marchers
        // that start from different places; it is reported, not hidden, and
        // a regression above this threshold means a real fault.
        if ao.mismatches * 2000 > ao.pixels {
            println!("  FAIL — atlas disagrees with ground truth beyond tolerance.");
            std::process::exit(1);
        }

        // --- E10: the light bake ------------------------------------------
        {
            let b = sc.bounds;
            let (lo, hi) = match sc.name {
                "melee" => ([-2.5f32, 0.0, -3.0], [2.5f32, 2.2, 1.5]),
                _ => ([-10.0f32, 0.0, -4.0], [10.0f32, 3.2, 4.0]),
            };
            let _ = b;
            for &cell in &[0.5f32, 0.25, 0.125] {
                let lb = probe::run_light_bake(sc, lo, hi, cell, &mut rng, &mut s);
                println!(
                    "  [E10] AO bake cell {:.3}: grid {}x{}x{} = {} probes, {:.1} MB f32 \
                     ({:.2} MB as u8)  err mean {:.4} p95 {:.4} max {:.4}",
                    lb.cell,
                    lb.dims[0], lb.dims[1], lb.dims[2],
                    lb.cells,
                    lb.bytes_f32 as f64 / 1e6,
                    lb.cells as f64 / 1e6,
                    lb.mean_err, lb.p95_err, lb.max_err
                );
            }
            for &cell in &[0.5f32, 0.25, 0.125] {
                let lb = probe::run_sun_bake(sc, lo, hi, cell, &mut rng, &mut s);
                println!(
                    "  [E10b] SUN bake cell {:.3}: {} probes, {:.2} MB as u8   \
                     err mean {:.4} p95 {:.4} max {:.4}",
                    lb.cell,
                    lb.cells,
                    lb.cells as f64 / 1e6,
                    lb.mean_err, lb.p95_err, lb.max_err
                );
            }
            println!(
                "        replaces shadow+AO+GI = {:.0} FLOP/px measured, with ~25 FLOP/tap.",
                (fc.shadow + fc.ao_gi) / fc.pixels as f64
            );
        }

        // --- E11: the frame budget under motion ---------------------------
        println!();
        println!("[E11] frame budget across a camera whip                 §4.4, §16.2");
        let seq = probe::run_motion(sc, &at, cfg.motion_stride, &mut s);
        println!("  frame   deg/frame   primary FLOP/px   hit%    reproj hint%   verified%");
        let mut worst = 0.0f64;
        let mut sum = 0.0f64;
        for (i, f) in seq.iter().enumerate() {
            println!(
                "  {:>5}   {:>9.2}   {:>15.0}   {:>5.1}   {:>12.1}   {:>9.1}",
                i,
                f.deg,
                f.primary_flop_px,
                f.hit_rate * 100.0,
                f.hinted * 100.0,
                f.verified * 100.0
            );
            worst = worst.max(f.primary_flop_px);
            sum += f.primary_flop_px;
        }
        let mean = sum / seq.len().max(1) as f64;
        println!(
            "  worst frame {:.0} FLOP/px, mean {:.0}  ->  peak/mean {:.2}x               (frame time is set by the worst)",
            worst,
            mean,
            worst / mean.max(1e-9)
        );

        if cfg.ppm {
            let img = probe::debug_ppm(sc, &mut s);
            let path = format!("{}.ppm", sc.name);
            let _ = std::fs::write(&path, img);
            println!();
            println!("  wrote {} (eyeball only, not an oracle)", path);
        }

        let d3 = cl
            .per_depth
            .get(3)
            .map(|st| st.ops_sum as f64 / st.cells.max(1) as f64)
            .unwrap_or(0.0);
        summary.push((
            sc.name.to_string(),
            d3,
            cl.area_interior / total_area,
            mo.band_len / mo.total_len.max(1e-9),
        ));
        println!();
    }

    // --- §16.4's kill criterion, evaluated -------------------------------
    println!("========================================================");
    println!("§16.4 kill criterion");
    println!("  \"if the worst-case scene shows pruned tapes above ~100 ops at depth 3,");
    println!("   interior-tile fraction under ~50%, and blend-band ray fraction above ~30%");
    println!("   together, the 512x288 floor is the ceiling too\"");
    println!();
    for (name, d3, interior, band) in &summary {
        let a = *d3 > 100.0;
        let b = *interior < 0.50;
        let c = *band > 0.30;
        println!(
            "  {:<15} depth-3 ops {:>6.1} [{}]  interior {:>5.1}% [{}]  band {:>5.1}% [{}]  => {}",
            name,
            d3,
            if a { "FAIL" } else { "ok" },
            interior * 100.0,
            if b { "FAIL" } else { "ok" },
            band * 100.0,
            if c { "FAIL" } else { "ok" },
            if a && b && c { "KILL" } else { "survives" }
        );
    }
    println!();
    println!("Counts only. Converting these to Pi 5 time is bench/a76-pi5.toml's job,");
    println!("deliberately not done here (§16.1: wall-clock on the M4 proxy does not port).");
}

/// How the reconstruction factor scales with output resolution.
///
/// Every reconstruction number so far was measured at 512x288 and then used
/// to reason about 1080p and 4K, which silently assumes the factor is
/// resolution-independent. It is not, and the direction is the favourable
/// one: a patch is a quadratic fitted to *world* geometry, so raising the
/// output resolution does not create new patches at the same rate it creates
/// new pixels — only the edge set grows, and it grows with edge *length*,
/// i.e. linearly, against pixels growing quadratically.
///
/// Two effects fight: patches survive resolution increases, but the fit
/// tolerance is stated in pixels of parallax, so it tightens as the pixel
/// footprint shrinks and patches must get smaller. Which wins is an
/// empirical question, and it decides whether 4K is reachable — so it is
/// measured here rather than argued.
fn recon_sweep(only: &Option<String>) {
    println!("fieldprobe — reconstruction factor vs output resolution");
    println!();
    for (w, h) in [(512u32, 288u32), (1024, 576), (1920, 1080), (3840, 2160)] {
        let scenes = [scene::colonnade(w, h), scene::melee(w, h)];
        for sc in scenes.iter() {
            if let Some(o) = only {
                if o != sc.name {
                    continue;
                }
            }
            let mut s = Scratch::default();
            let (cen, er) = probe::run_edge_recon(sc, 1.0, 64, &mut s);
            println!(
                "  {:<10} {:>4}x{:<4}  edge {:>6}  samples {:>8} = {:>7} patch + {:>7} edge \
                 + {:>6} dense   -> {:>7.2}x",
                sc.name,
                w,
                h,
                fmt_pct(cen.edge as f64, cen.pixels as f64),
                er.samples(),
                er.patch_samples,
                er.edge_samples,
                er.dense_samples,
                er.factor()
            );
        }
    }
    println!();
    println!("  Patch samples that stay flat while pixels grow 4x per row are the");
    println!("  whole 4K argument; patch samples that grow with pixels kill it.");
}
