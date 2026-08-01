use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use crate::{blk_conformance_image, build_and_sign_vmm, fail_closed, root};

const VIRT_UART: u64 = 0x0900_0000;

pub(crate) fn qemu_path() -> Result<PathBuf, String> {
    let name = "qemu-system-aarch64";
    let Some(path) = std::env::var_os("PATH") else {
        return fail_closed(
            "diff-blk",
            "PATH is not set; cannot locate qemu-system-aarch64",
        );
    };
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(name);
        if candidate.is_file() {
            return Ok(candidate);
        }
    }
    fail_closed("diff-blk", "qemu-system-aarch64 is not on PATH")
}

pub(crate) fn diff_blk() -> Result<(), String> {
    qemu_path()?;
    let dir = root().join("target/diff-blk-tmp");
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).map_err(|e| format!("create {}: {e}", dir.display()))?;

    let smoke_path = dir.join("smoke.bin");
    std::fs::write(&smoke_path, build_qemu_smoke_guest()).map_err(|e| format!("write: {e}"))?;
    let smoke = run_qemu(&smoke_path, None)?;
    if !smoke.contains("WRELA-SMOKE") {
        return Err(format!(
            "diff-blk: QEMU did not run this harness's own smallest guest at all (got {smoke:?}) — \
             the oracle refuses to report agreement it never established"
        ));
    }

    let guest_path = dir.join("guest.bin");
    std::fs::write(&guest_path, build_qemu_blk_guest()).map_err(|e| format!("write guest: {e}"))?;
    let disk_path = dir.join("disk.img");
    std::fs::write(&disk_path, vec![0u8; 16 * 512]).map_err(|e| format!("write disk: {e}"))?;
    let qemu_out = run_qemu(&guest_path, Some(&disk_path))?;
    let fields: Vec<u64> = match qemu_out.split_whitespace().collect::<Vec<_>>().as_slice() {
        ["R", rest @ ..] if rest.len() == 7 => rest
            .iter()
            .map(|h| u64::from_str_radix(h, 16).map_err(|e| format!("bad qemu hex {h:?}: {e}")))
            .collect::<Result<_, _>>()?,
        _ => {
            return Err(format!(
                "diff-blk: the QEMU guest did not complete its own two operations (it prints \
                 `NODEV`/`FEAT`/`TMO1`/`TMO2` for each way bring-up can fail): {qemu_out:?}"
            ));
        }
    };
    let (used_w0, used_w1, used_w2) = (fields[0], fields[1], fields[2]);
    let qemu = BlkAnswer {
        used_idx: ((used_w0 >> 16) & 0xFFFF) as u32,
        head0: (used_w0 >> 32) as u32,
        len0: (used_w1 & 0xFFFF_FFFF) as u32,
        head1: (used_w1 >> 32) as u32,
        len1: (used_w2 & 0xFFFF_FFFF) as u32,
        status0: fields[3] as u32,
        status1: fields[4] as u32,
        digest0: format!("{:016x}", fields[5]),
        digest1: format!("{:016x}", fields[6]),
    };

    let vmm = build_and_sign_vmm()?;
    let (img_bytes, report_text) = blk_conformance_image();
    let img_path = dir.join("wrela.img");
    let report_path = dir.join("wrela.report.txt");
    let record_path = dir.join("wrela.record.txt");
    std::fs::write(&img_path, &img_bytes).map_err(|e| format!("write img: {e}"))?;
    std::fs::write(&report_path, &report_text).map_err(|e| format!("write report: {e}"))?;
    let out = Command::new(&vmm)
        .arg(&report_path)
        .arg(&img_path)
        .arg("--record")
        .arg(&record_path)
        .output()
        .map_err(|e| format!("run wrela-vmm: {e}"))?;
    if out.status.code() != Some(0) {
        return Err(format!(
            "diff-blk: the wrela side's own boot failed (exit {:?}):\n{}",
            out.status.code(),
            String::from_utf8_lossy(&out.stderr)
        ));
    }
    let record_text = std::fs::read_to_string(&record_path)
        .map_err(|e| format!("read {}: {e}", record_path.display()))?;
    let completions: Vec<BTreeMap<String, String>> = record_text
        .lines()
        .filter(|l| l.contains("=DeviceCompletion "))
        .map(|l| {
            l.split_whitespace()
                .filter_map(|p| p.split_once('='))
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect()
        })
        .collect();
    if completions.len() != 2 {
        return Err(format!(
            "diff-blk: the wrela side recorded {} completion(s), expected 2",
            completions.len()
        ));
    }
    let field = |i: usize, k: &str| -> Result<String, String> {
        completions[i]
            .get(k)
            .cloned()
            .ok_or_else(|| format!("diff-blk: recorded completion #{i} has no `{k}` field"))
    };
    let num = |i: usize, k: &str| -> Result<u32, String> {
        field(i, k)?
            .parse()
            .map_err(|e| format!("diff-blk: completion #{i} field `{k}`: {e}"))
    };
    let wrela = BlkAnswer {
        used_idx: completions.len() as u32,
        head0: num(0, "head")?,
        len0: num(0, "len")?,
        head1: num(1, "head")?,
        len1: num(1, "len")?,
        status0: num(0, "status")?,
        status1: num(1, "status")?,
        digest0: field(0, "digest")?,
        digest1: field(1, "digest")?,
    };

    let mut payload_and_status = blk_shape::payload();
    payload_and_status.push(0);
    let expected_sha256 = wrela_compiler::report::sha256_hex(&payload_and_status);
    let expected_fnv64 = format!("{:016x}", fnv1a64(&payload_and_status));
    let canonical_payload = "512-byte canonical payload followed by status=0";
    let digest_fact = |actual: &str, expected: &str, algorithm: &str| {
        if actual == expected {
            canonical_payload.to_string()
        } else {
            format!("{algorithm}={actual} (expected {expected})")
        }
    };

    let facts: Vec<(&str, String, String)> = vec![
        (
            "used.idx",
            wrela.used_idx.to_string(),
            qemu.used_idx.to_string(),
        ),
        (
            "write: used id",
            wrela.head0.to_string(),
            qemu.head0.to_string(),
        ),
        (
            "write: used len",
            wrela.len0.to_string(),
            qemu.len0.to_string(),
        ),
        (
            "write: status",
            wrela.status0.to_string(),
            qemu.status0.to_string(),
        ),
        (
            "write: payload + status bytes",
            digest_fact(&wrela.digest0, &expected_sha256, "sha256"),
            digest_fact(&qemu.digest0, &expected_fnv64, "fnv1a64"),
        ),
        (
            "read: used id",
            wrela.head1.to_string(),
            qemu.head1.to_string(),
        ),
        (
            "read: used len",
            wrela.len1.to_string(),
            qemu.len1.to_string(),
        ),
        (
            "read: status",
            wrela.status1.to_string(),
            qemu.status1.to_string(),
        ),
        (
            "read: payload + status bytes",
            digest_fact(&wrela.digest1, &expected_sha256, "sha256"),
            digest_fact(&qemu.digest1, &expected_fnv64, "fnv1a64"),
        ),
    ];
    let mut disagreements = Vec::new();
    for (what, w, q) in &facts {
        if w != q {
            disagreements.push(format!("  {what}: wrela says `{w}`, QEMU says `{q}`"));
        }
    }
    if !disagreements.is_empty() {
        return Err(format!(
            "diff-blk: the two virtio-blk implementations disagree on {} of {} compared fact(s):\n{}",
            disagreements.len(),
            facts.len(),
            disagreements.join("\n")
        ));
    }
    for (what, w, _) in &facts {
        println!("diff-blk:   {what} = {w} (both)");
    }
    println!(
        "diff-blk: {} fact(s) agree between wrela-vmm's own virtio-blk model and `{}` over \
         identical descriptor chains",
        facts.len(),
        qemu_version()?
    );
    Ok(())
}

fn fnv1a64(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for &byte in bytes {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100_0000_01b3);
    }
    hash
}

struct BlkAnswer {
    used_idx: u32,
    head0: u32,
    len0: u32,
    head1: u32,
    len1: u32,
    status0: u32,
    status1: u32,
    digest0: String,
    digest1: String,
}

fn qemu_version() -> Result<String, String> {
    let out = Command::new(qemu_path()?)
        .arg("--version")
        .output()
        .map_err(|e| format!("run qemu --version: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout)
        .lines()
        .next()
        .unwrap_or("qemu-system-aarch64")
        .trim()
        .to_string())
}

fn run_qemu(guest: &Path, disk: Option<&Path>) -> Result<String, String> {
    let mut cmd = Command::new(qemu_path()?);
    cmd.args([
        "-M",
        "virt",
        "-cpu",
        "cortex-a72",
        "-m",
        "256",
        "-nographic",
        "-no-reboot",
        "-global",
        "virtio-mmio.force-legacy=false",
    ]);
    cmd.arg("-device");
    cmd.arg(format!(
        "loader,file={},addr=0x40100000,force-raw=on",
        guest.display()
    ));
    cmd.arg("-device").arg("loader,addr=0x40100000,cpu-num=0");
    if let Some(disk) = disk {
        cmd.arg("-drive")
            .arg(format!("if=none,file={},format=raw,id=d0", disk.display()));
        cmd.arg("-device").arg("virtio-blk-device,drive=d0");
    }
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());
    cmd.stdin(std::process::Stdio::null());
    let mut child = cmd.spawn().map_err(|e| format!("spawn qemu: {e}"))?;
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        match child.try_wait().map_err(|e| format!("wait qemu: {e}"))? {
            Some(_) => break,
            None => {
                if Instant::now() > deadline {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(
                        "diff-blk: the QEMU guest never reached its own SYSTEM_OFF within 20s"
                            .to_string(),
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
    let out = child
        .wait_with_output()
        .map_err(|e| format!("collect qemu output: {e}"))?;
    Ok(String::from_utf8_lossy(&out.stdout).to_string())
}

fn qemu_load_imm(reg: u8, value: u64) -> Vec<u32> {
    use wrela_compiler::encode;
    vec![
        encode::enc_movz(reg, (value & 0xFFFF) as u16, 0, true),
        encode::enc_movk(reg, ((value >> 16) & 0xFFFF) as u16, 16, true),
        encode::enc_movk(reg, ((value >> 32) & 0xFFFF) as u16, 32, true),
        encode::enc_movk(reg, ((value >> 48) & 0xFFFF) as u16, 48, true),
    ]
}

const ENC_HVC0: u32 = 0xD400_0002;

fn qemu_system_off(w: &mut Vec<u32>) {
    w.extend(qemu_load_imm(0, 0x8400_0008));
    w.push(ENC_HVC0);
}

fn build_qemu_smoke_guest() -> Vec<u8> {
    use wrela_compiler::encode;
    let mut w = Vec::new();
    w.extend(qemu_load_imm(9, VIRT_UART));
    for b in b"WRELA-SMOKE\n" {
        w.push(encode::enc_movz(10, *b as u16, 0, false));
        w.push(encode::enc_str_w_imm(10, 9, 0));
    }
    qemu_system_off(&mut w);
    w.iter().flat_map(|x| x.to_le_bytes()).collect()
}

const MMIO_MAGIC: u16 = 0x000;
const MMIO_VERSION: u16 = 0x004;
const MMIO_DEVICE_ID: u16 = 0x008;
const MMIO_DEVICE_FEATURES: u16 = 0x010;
const MMIO_DEVICE_FEATURES_SEL: u16 = 0x014;
const MMIO_DRIVER_FEATURES: u16 = 0x020;
const MMIO_DRIVER_FEATURES_SEL: u16 = 0x024;
const MMIO_QUEUE_SEL: u16 = 0x030;
const MMIO_QUEUE_NUM: u16 = 0x038;
const MMIO_QUEUE_READY: u16 = 0x044;
const MMIO_QUEUE_NOTIFY: u16 = 0x050;
const MMIO_STATUS: u16 = 0x070;
const MMIO_QUEUE_DESC_LOW: u16 = 0x080;
const MMIO_QUEUE_DESC_HIGH: u16 = 0x084;
const MMIO_QUEUE_DRIVER_LOW: u16 = 0x090;
const MMIO_QUEUE_DRIVER_HIGH: u16 = 0x094;
const MMIO_QUEUE_DEVICE_LOW: u16 = 0x0A0;
const MMIO_QUEUE_DEVICE_HIGH: u16 = 0x0A4;

const VIRT_MMIO_BASE: u64 = 0x0A00_0000;
const VIRT_MMIO_STRIDE: u64 = 0x200;
const VIRT_MMIO_SLOTS: u64 = 32;
const QEMU_LOAD_ADDR: u64 = 0x4010_0000;

pub(crate) mod blk_shape {
    pub const QUEUE_SIZE: u64 = 8;
    pub const DESC_SIZE: u64 = 16;
    pub const DESC_F_NEXT: u16 = 1;
    pub const DESC_F_WRITE: u16 = 2;
    pub const T_IN: u32 = 0;
    pub const T_OUT: u32 = 1;
    pub const OFF_DESC: u64 = 0x000;
    pub const OFF_AVAIL: u64 = 0x080;
    pub const OFF_USED: u64 = 0x0C0;
    pub const OFF_HDR1: u64 = 0x150;
    pub const OFF_HDR2: u64 = 0x160;
    pub const OFF_STATUS1: u64 = 0x170;
    pub const OFF_STATUS2: u64 = 0x178;
    pub const OFF_SRC: u64 = 0x200;
    pub const OFF_DST: u64 = 0x400;
    pub const DATA_REGION_SIZE: u64 = 0x600;

    pub fn payload() -> Vec<u8> {
        (0..512u32).map(|i| ((i * 7 + 3) % 256) as u8).collect()
    }
}

pub(crate) fn fill_blk_ring(img: &mut [u8], data_off: usize, data_base: u64) {
    use blk_shape::*;
    let put = |img: &mut [u8], off: u64, bytes: &[u8]| {
        let at = data_off + off as usize;
        img[at..at + bytes.len()].copy_from_slice(bytes);
    };
    let desc = |img: &mut [u8], i: u64, addr: u64, len: u32, flags: u16, next: u16| {
        let at = OFF_DESC + i * DESC_SIZE;
        put(img, at, &addr.to_le_bytes());
        put(img, at + 8, &len.to_le_bytes());
        put(img, at + 12, &flags.to_le_bytes());
        put(img, at + 14, &next.to_le_bytes());
    };
    put(img, OFF_HDR1, &T_OUT.to_le_bytes());
    put(img, OFF_HDR1 + 8, &0u64.to_le_bytes());
    desc(img, 0, data_base + OFF_HDR1, 16, DESC_F_NEXT, 1);
    desc(img, 1, data_base + OFF_SRC, 512, DESC_F_NEXT, 2);
    desc(img, 2, data_base + OFF_STATUS1, 1, DESC_F_WRITE, 0);
    put(img, OFF_HDR2, &T_IN.to_le_bytes());
    put(img, OFF_HDR2 + 8, &0u64.to_le_bytes());
    desc(img, 3, data_base + OFF_HDR2, 16, DESC_F_NEXT, 4);
    desc(
        img,
        4,
        data_base + OFF_DST,
        512,
        DESC_F_NEXT | DESC_F_WRITE,
        5,
    );
    desc(img, 5, data_base + OFF_STATUS2, 1, DESC_F_WRITE, 0);
    put(img, OFF_STATUS1, &[0xEE]);
    put(img, OFF_STATUS2, &[0xEE]);
    put(img, OFF_SRC, &blk_shape::payload());
}

fn build_qemu_blk_guest() -> Vec<u8> {
    use blk_shape::*;
    use wrela_compiler::encode;
    use wrela_compiler::encode::Cond;

    let build = |data_base: u64| -> Vec<u32> {
        let mut w: Vec<u32> = Vec::new();
        let li = |w: &mut Vec<u32>, reg: u8, v: u64| w.extend(qemu_load_imm(reg, v));

        li(&mut w, 22, VIRT_UART);
        li(&mut w, 21, data_base);

        let putc = |w: &mut Vec<u32>, b: u8| {
            w.push(encode::enc_movz(10, b as u16, 0, false));
            w.push(encode::enc_str_w_imm(10, 22, 0));
        };
        let puts = |w: &mut Vec<u32>, s: &[u8]| {
            for b in s {
                putc(w, *b);
            }
        };

        li(&mut w, 20, VIRT_MMIO_BASE);
        li(&mut w, 19, VIRT_MMIO_SLOTS);
        let scan_top = w.len();
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_MAGIC));
        li(&mut w, 10, 0x7472_6976);
        w.push(encode::enc_cmp_reg(9, 10, false));
        let magic_ne = w.len();
        w.push(0);
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_VERSION));
        w.push(encode::enc_cmp_imm(9, 2, false));
        let version_ne = w.len();
        w.push(0);
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_DEVICE_ID));
        w.push(encode::enc_cmp_imm(9, 2, false));
        let id_eq = w.len();
        w.push(0);
        let next_slot = w.len();
        li(&mut w, 10, VIRT_MMIO_STRIDE);
        w.push(encode::enc_add_reg(20, 20, 10, true));
        w.push(encode::enc_subs_imm(19, 19, 1, true));
        {
            let this = w.len();
            w.push(encode::enc_cbnz(
                19,
                ((scan_top as i64 - this as i64) * 4) as i32,
                true,
            ));
        }
        puts(&mut w, b"NODEV\n");
        qemu_system_off(&mut w);
        let found = w.len();
        for (at, cond) in [(magic_ne, Cond::Ne), (version_ne, Cond::Ne)] {
            w[at] = encode::enc_b_cond(cond, ((next_slot as i64 - at as i64) * 4) as i32);
        }
        w[id_eq] = encode::enc_b_cond(Cond::Eq, ((found as i64 - id_eq as i64) * 4) as i32);

        let status = |w: &mut Vec<u32>, bits: u16| {
            w.push(encode::enc_movz(10, bits, 0, false));
            w.push(encode::enc_str_w_imm(10, 20, MMIO_STATUS));
        };
        status(&mut w, 0);
        status(&mut w, 1);
        status(&mut w, 3);

        w.push(encode::enc_movz(10, 1, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DEVICE_FEATURES_SEL));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DRIVER_FEATURES_SEL));
        w.push(encode::enc_movz(10, 1, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DRIVER_FEATURES));
        w.push(encode::enc_movz(10, 0, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DEVICE_FEATURES_SEL));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DRIVER_FEATURES_SEL));
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_DEVICE_FEATURES));
        w.push(encode::enc_movz(10, 1 << 9, 0, false));
        w.push(encode::enc_and_reg(10, 10, 9, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_DRIVER_FEATURES));

        status(&mut w, 3 | 8);
        w.push(encode::enc_ldr_w_imm(9, 20, MMIO_STATUS));
        w.push(encode::enc_movz(10, 8, 0, false));
        w.push(encode::enc_and_reg(9, 9, 10, false));
        let feat_ok = w.len();
        w.push(0);
        puts(&mut w, b"FEAT\n");
        qemu_system_off(&mut w);
        {
            let target = w.len();
            w[feat_ok] = encode::enc_cbnz(9, ((target as i64 - feat_ok as i64) * 4) as i32, true);
        }

        w.push(encode::enc_movz(10, 0, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_QUEUE_SEL));
        w.push(encode::enc_movz(10, QUEUE_SIZE as u16, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_QUEUE_NUM));
        for (lo, hi, addr) in [
            (
                MMIO_QUEUE_DESC_LOW,
                MMIO_QUEUE_DESC_HIGH,
                data_base + OFF_DESC,
            ),
            (
                MMIO_QUEUE_DRIVER_LOW,
                MMIO_QUEUE_DRIVER_HIGH,
                data_base + OFF_AVAIL,
            ),
            (
                MMIO_QUEUE_DEVICE_LOW,
                MMIO_QUEUE_DEVICE_HIGH,
                data_base + OFF_USED,
            ),
        ] {
            li(&mut w, 10, addr & 0xFFFF_FFFF);
            w.push(encode::enc_str_w_imm(10, 20, lo));
            li(&mut w, 10, addr >> 32);
            w.push(encode::enc_str_w_imm(10, 20, hi));
        }
        w.push(encode::enc_movz(10, 1, 0, false));
        w.push(encode::enc_str_w_imm(10, 20, MMIO_QUEUE_READY));
        status(&mut w, 3 | 8 | 4);

        let mut timeout_markers: Vec<(usize, &[u8])> = Vec::new();
        for (round, (avail_idx, want_used)) in [(1u64, 1u32), (2u64, 2u32)].iter().enumerate() {
            li(&mut w, 9, data_base + OFF_AVAIL);
            li(&mut w, 10, (avail_idx << 16) | (3 << 48));
            w.push(encode::enc_str_x_imm(10, 9, 0));
            w.push(encode::enc_movz(10, 0, 0, false));
            w.push(encode::enc_str_w_imm(10, 20, MMIO_QUEUE_NOTIFY));
            li(&mut w, 12, 200_000_000);
            li(&mut w, 9, data_base + OFF_USED);
            let poll_top = w.len();
            w.push(encode::enc_ldr_w_imm(10, 9, 0));
            w.push(encode::enc_lsr_imm(10, 10, 16, false));
            w.push(encode::enc_cmp_imm(10, *want_used as u16, false));
            let done = w.len();
            w.push(0);
            w.push(encode::enc_subs_imm(12, 12, 1, true));
            {
                let this = w.len();
                w.push(encode::enc_cbnz(
                    12,
                    ((poll_top as i64 - this as i64) * 4) as i32,
                    true,
                ));
            }
            let marker: &[u8] = if round == 0 { b"TMO1\n" } else { b"TMO2\n" };
            timeout_markers.push((w.len(), marker));
            puts(&mut w, marker);
            qemu_system_off(&mut w);
            let target = w.len();
            w[done] = encode::enc_b_cond(Cond::Eq, ((target as i64 - done as i64) * 4) as i32);
        }
        let _ = timeout_markers;

        li(&mut w, 9, data_base + OFF_USED);
        w.push(encode::enc_ldr_x_imm(23, 9, 0));
        w.push(encode::enc_ldr_x_imm(24, 9, 8));
        w.push(encode::enc_ldr_x_imm(25, 9, 16));
        li(&mut w, 9, data_base + OFF_STATUS1);
        w.push(encode::enc_ldrb_imm(19, 9, 0));
        li(&mut w, 9, data_base + OFF_STATUS2);
        w.push(encode::enc_ldrb_imm(28, 9, 0));

        let fnv = |w: &mut Vec<u32>, start: u64, len: u64, status_at: u64, out: u8| {
            li(w, 13, 0xcbf2_9ce4_8422_2325);
            li(w, 14, 0x0000_0100_0000_01b3);
            li(w, 11, start);
            li(w, 15, start + len);
            let top = w.len();
            w.push(encode::enc_ldrb_imm(16, 11, 0));
            w.push(encode::enc_eor_reg(13, 13, 16, true));
            w.push(encode::enc_mul(13, 13, 14, true));
            w.push(encode::enc_add_imm(11, 11, 1, true));
            w.push(encode::enc_cmp_reg(11, 15, true));
            {
                let this = w.len();
                w.push(encode::enc_b_cond(
                    Cond::Ne,
                    ((top as i64 - this as i64) * 4) as i32,
                ));
            }
            li(w, 11, status_at);
            w.push(encode::enc_ldrb_imm(16, 11, 0));
            w.push(encode::enc_eor_reg(13, 13, 16, true));
            w.push(encode::enc_mul(13, 13, 14, true));
            w.push(encode::enc_mov_reg(out, 13, true));
        };
        fnv(
            &mut w,
            data_base + OFF_SRC,
            512,
            data_base + OFF_STATUS1,
            26,
        );
        fnv(
            &mut w,
            data_base + OFF_DST,
            512,
            data_base + OFF_STATUS2,
            27,
        );

        let print_hex = |w: &mut Vec<u32>, src: u8| {
            w.push(encode::enc_movz(11, 60, 0, true));
            let top = w.len();
            w.push(encode::enc_lsr_reg(12, src, 11, true));
            w.push(encode::enc_movz(13, 0xF, 0, true));
            w.push(encode::enc_and_reg(12, 12, 13, true));
            w.push(encode::enc_cmp_imm(12, 10, true));
            w.push(encode::enc_movz(13, b'0' as u16, 0, true));
            w.push(encode::enc_movz(14, (b'a' - 10) as u16, 0, true));
            w.push(encode::enc_csel(13, 13, 14, Cond::Cc, true));
            w.push(encode::enc_add_reg(12, 12, 13, true));
            w.push(encode::enc_str_w_imm(12, 22, 0));
            w.push(encode::enc_subs_imm(11, 11, 4, true));
            {
                let this = w.len();
                w.push(encode::enc_b_cond(
                    Cond::Ge,
                    ((top as i64 - this as i64) * 4) as i32,
                ));
            }
        };
        puts(&mut w, b"R ");
        for reg in [23u8, 24, 25, 19, 28, 26, 27] {
            print_hex(&mut w, reg);
            putc(&mut w, b' ');
        }
        putc(&mut w, b'\n');
        qemu_system_off(&mut w);
        w
    };

    let probe_len = build(0).len();
    let data_base = {
        let after_code = QEMU_LOAD_ADDR + (probe_len as u64) * 4;
        after_code.div_ceil(16) * 16
    };
    let words = build(data_base);
    assert_eq!(words.len(), probe_len, "guest length must not move");
    let mut img: Vec<u8> = words.iter().flat_map(|x| x.to_le_bytes()).collect();
    img.resize((data_base - QEMU_LOAD_ADDR + DATA_REGION_SIZE) as usize, 0);
    let data_off = (data_base - QEMU_LOAD_ADDR) as usize;
    fill_blk_ring(&mut img, data_off, data_base);
    img
}
