# V0 host-backend prerequisite audit

**Status:** sealed 2026-08-16. This record is the Task V0 prerequisite for
`vmm-host-backends.md`; later host records supersede the volatile probe values,
not this dependency and migration decision.

## Rust/KVM dependency closure

The Linux/aarch64 VMM pins `kvm-ioctls 0.25.0` and `kvm-bindings 0.14.1`
with default features disabled. Both require Rust 1.85; the pinned repository
toolchain is Rust 1.97.1. The audited locked runtime closure is:

| crate | version | license | crates.io checksum |
|---|---:|---|---|
| `kvm-bindings` | 0.14.1 | Apache-2.0 | `11cf0ca75d59e9d298647c59cf6c5286fa048120caa77972a7a504a0824d234f` |
| `kvm-ioctls` | 0.25.0 | Apache-2.0 OR MIT | `06ac372c120eb893b086d1a12027669cf2b478d1f71204021ffa7adf57948d63` |
| `vmm-sys-util` | 0.15.0 | BSD-3-Clause | `506c62fdf617a5176827c2f9afbcf1be155b03a9b4bf9617a60dbc07e3a1642f` |
| `libc` | 0.2.189 | MIT OR Apache-2.0 | `3eaf3ede3fee6db1a4c2ee091bf8a8b4dccdc6d17f656fb07896ee72867612f2` |
| `bitflags` | 1.3.2 and 2.13.1 | MIT OR Apache-2.0 | lockfile-authoritative |

These licenses are compatible with the repository. `Cargo.lock` is the
authoritative closure and integrity record. Any version, feature, source, or
closure change requires an amendment to the approved design and `AGENTS.md`.
The bindings are kernel ABI only: their types are wrapped immediately inside
`wrela-vmm::host::kvm` and cannot enter a machine contract or stable format.

## Cross-build preflight

The Mac has both `aarch64-unknown-linux-gnu` and
`aarch64-unknown-linux-musl` standard libraries installed. The required gate
uses the pinned musl target and the repository-selected linker and performs a
full link, not `cargo check`. Rasputin executes AArch64 ELF artifacts and has
unprivileged read/write access to `/dev/kvm`.

## Prototype migration inventory

The P8-era `boot_kvm.rs` prototype demonstrates the following behaviors that
must survive as named fixtures when it is deleted at V4:

- API-version refusal and a one-vCPU direct boot;
- x0, PC, PSTATE, SP_EL1, CPACR_EL1, and FPCR initialization;
- 1/2/4/8-byte `KVM_EXIT_MMIO` decoding and read completion;
- console, clock, entropy, release, park, exit, display, and replay paths;
- machine-info initialization and exact output/frame digests;
- Linux native-display selection and headless operation.

The manually copied ioctl encodings, UAPI structures, register IDs, raw
`kvm_run` offsets, and libc FFI are explicitly not surviving behavior. Named
portable and synthetic-exit tests replace them. The prototype DRM file
`display/kvm.rs` likewise migrates to presenter-owned `display/drm.rs`; the
portable display model remains the authority.

## Initial Rasputin preflight

The initial stable-interface probe found AArch64 Linux, a 16 KiB host page,
1,038,827,520 bytes of RAM, unprivileged `/dev/kvm` access, and the expected
1 GiB Raspberry Pi product class. Ioctl-only facts are represented as the
canonical value `not-queried` until the rust-vmm-backed KVM probe replaces
them. No capability is inferred from the kernel release.
