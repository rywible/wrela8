//! TEMPORARY — item M measurement harness. Deleted before the item lands.

use wrela_compiler::opts::win::{
    CostTier, compare_opt_lists_over_box_for_case, discover_cost_cases, report_path_under_opts,
};
use wrela_compiler::opts::{OptId, RELEASE_OPTS};

#[test]
#[ignore]
fn item_m_measure() {
    let cases = discover_cost_cases();
    println!("== dev vs release, product tier ==");
    println!(
        "{:<28} {:>8} {:>8} {:>8} {:>8} {:>8} {:>8}",
        "case", "devCyc", "relCyc", "dCyc%", "devWrd", "relWrd", "dWrd%"
    );
    for c in cases.iter().filter(|c| c.tier == CostTier::Product) {
        let d = report_path_under_opts(&c.input, &[]);
        let r = report_path_under_opts(&c.input, RELEASE_OPTS);
        println!(
            "{:<28} {:>8} {:>8} {:>7.1}% {:>8} {:>8} {:>7.1}%",
            c.name,
            d.total_proxy_cycles,
            r.total_proxy_cycles,
            100.0 * (r.total_proxy_cycles as f64 - d.total_proxy_cycles as f64)
                / d.total_proxy_cycles as f64,
            d.total_words,
            r.total_words,
            100.0 * (r.total_words as f64 - d.total_words as f64) / d.total_words as f64,
        );
    }

    println!("\n== each RELEASE_OPTS member alone, dev -> [opt] ==");
    print!("{:<16}", "opt");
    let prod: Vec<_> = cases
        .iter()
        .filter(|c| c.tier == CostTier::Product)
        .collect();
    for c in &prod {
        print!(" {:>22}", c.name.replace("cost-product-", ""));
    }
    println!();
    for c in &prod {
        let d = report_path_under_opts(&c.input, &[]);
        print!("{:<16}", "");
        let _ = d;
    }
    println!();
    for &opt in RELEASE_OPTS {
        print!("{:<16}", format!("{opt:?}"));
        for c in &prod {
            let d = report_path_under_opts(&c.input, &[]);
            let o = report_path_under_opts(&c.input, &[opt]);
            let same = d.total_proxy_cycles == o.total_proxy_cycles && d.total_words == o.total_words;
            print!(
                " {:>22}",
                format!(
                    "{}/{}{}",
                    o.total_proxy_cycles,
                    o.total_words,
                    if same { "=" } else { "" }
                )
            );
        }
        println!();
    }
    print!("{:<16}", "dev");
    for c in &prod {
        let d = report_path_under_opts(&c.input, &[]);
        print!(" {:>22}", format!("{}/{}", d.total_proxy_cycles, d.total_words));
    }
    println!();
    print!("{:<16}", "release");
    for c in &prod {
        let r = report_path_under_opts(&c.input, RELEASE_OPTS);
        print!(" {:>22}", format!("{}/{}", r.total_proxy_cycles, r.total_words));
    }
    println!();

    println!("\n== app/runtime/driver split, dev and release ==");
    for c in cases.iter().filter(|c| c.tier == CostTier::Product) {
        for (label, opts) in [("dev", &[][..]), ("release", RELEASE_OPTS)] {
            let r = report_path_under_opts(&c.input, opts);
            let mut owners: Vec<(String, u64)> = r
                .owner_totals
                .iter()
                .map(|(k, v)| (k.to_string(), *v))
                .collect();
            owners.sort();
            println!("{:<28} {:<8} {:?}", c.name, label, owners);
        }
    }
}

#[test]
#[ignore]
fn item_m_sweep() {
    for case in ["cost-product-compositor", "cost-product-appliance"] {
        match compare_opt_lists_over_box_for_case(&[], RELEASE_OPTS, case) {
            Ok(cmp) => {
                for s in &cmp.cases {
                    println!(
                        "case {} tier={} box_dims={} swept_k={} corners={} swept={:?}",
                        s.name,
                        s.tier,
                        s.box_dims,
                        s.swept.len(),
                        s.corners(),
                        s.swept
                    );
                }
                println!("{case}: wins={} reasons={:?}", cmp.wins(), cmp.reasons);
            }
            Err(e) => println!("{case}: ERR {e}"),
        }
    }
}

#[test]
#[ignore]
fn item_m_bounds_elide_alone_over_box() {
    for case in [
        "cost-product-compositor",
        "cost-product-appliance",
        "cost-product-actors",
        "cost-product-blk",
        "cost-product-receipt",
    ] {
        let r = compare_opt_lists_over_box_for_case(&[], &[OptId::BoundsElide], case);
        match r {
            Ok(cmp) => println!("{case}: BoundsElide wins={} reasons={:?}", cmp.wins(), cmp.reasons),
            Err(e) => println!("{case}: ERR {e}"),
        }
    }
}

#[test]
#[ignore]
fn item_m_product_tier_pins() {
    use wrela_compiler::opts::win::compare_opt_lists_over_box_in_tier;
    for &opt in RELEASE_OPTS {
        let cmp = compare_opt_lists_over_box_in_tier(&[], &[opt], CostTier::Product).expect("sweep");
        println!(
            "{opt:?}: product={} points={} reasons={:?}",
            if cmp.wins() { "wins" } else { "veto" },
            cmp.scored_points(),
            cmp.reasons
        );
    }
    let cmp = compare_opt_lists_over_box_in_tier(&[], RELEASE_OPTS, CostTier::Product).expect("s");
    println!(
        "release: product={} points={}",
        if cmp.wins() { "wins" } else { "veto" },
        cmp.scored_points()
    );
}
