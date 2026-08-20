//! Maintainer-only generator for the canonical Pixels P9 transfer tables.
//!
//! Compile and run deliberately:
//! `rustc tools/gen_pixels_tables.rs -o target/gen-pixels-tables &&
//!  target/gen-pixels-tables [--accept]`
//!
//! Without `--accept` candidate files are written beside the canonical files.
//! Canonical bytes are never overwritten accidentally.

use std::fs;
use std::path::{Path, PathBuf};

const ENTRIES: usize = 4097;
const A: f64 = 0.15;
const B: f64 = 0.50;
const C: f64 = 0.10;
const D: f64 = 0.20;
const E: f64 = 0.02;
const F: f64 = 0.30;
const W: f64 = 11.2;

fn h(x: f64) -> f64 {
    ((x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F)) - E / F
}

fn round_ties_even(value: f64) -> u16 {
    let floor = value.floor();
    let fraction = value - floor;
    let rounded = if fraction < 0.5 {
        floor
    } else if fraction > 0.5 || (floor as u64) & 1 != 0 {
        floor + 1.0
    } else {
        floor
    };
    rounded.clamp(0.0, f64::from(u16::MAX)) as u16
}

fn filmic_table() -> Vec<u8> {
    let white = h(W);
    (0..ENTRIES)
        .flat_map(|index| {
            let log2_x = -16.0 + 32.0 * index as f64 / (ENTRIES - 1) as f64;
            let mapped = (h(2.0_f64.powf(log2_x)) / white).clamp(0.0, 1.0);
            round_ties_even(mapped * 65535.0).to_le_bytes()
        })
        .collect()
}

fn srgb_table() -> Vec<u8> {
    (0..ENTRIES)
        .flat_map(|index| {
            let linear = index as f64 / (ENTRIES - 1) as f64;
            let encoded = if linear <= 0.003_130_8 {
                12.92 * linear
            } else {
                1.055 * linear.powf(1.0 / 2.4) - 0.055
            };
            round_ties_even(encoded.clamp(0.0, 1.0) * 65535.0).to_le_bytes()
        })
        .collect()
}

fn destination(root: &Path, name: &str, accept: bool) -> PathBuf {
    let canonical = root.join("stdlib/data/pixels").join(name);
    if accept {
        canonical
    } else {
        canonical.with_extension("bin.candidate")
    }
}

fn main() -> Result<(), String> {
    let accept = std::env::args().skip(1).any(|argument| argument == "--accept");
    let root = std::env::current_dir().map_err(|error| error.to_string())?;
    let output_dir = root.join("stdlib/data/pixels");
    fs::create_dir_all(&output_dir).map_err(|error| error.to_string())?;
    for (name, bytes) in [
        ("filmic_v1_u16.bin", filmic_table()),
        ("srgb_v1_u16.bin", srgb_table()),
    ] {
        let path = destination(&root, name, accept);
        fs::write(&path, bytes).map_err(|error| format!("{}: {error}", path.display()))?;
        println!("wrote {}", path.display());
    }
    if !accept {
        println!("candidate-only run; pass --accept to replace canonical bytes");
    }
    Ok(())
}
