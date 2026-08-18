# Wrela product host profiles

`linux/wrela-vmm.service` is the minimum Rasputin launch boundary. It runs a
dedicated unprivileged user, a private PID and network namespace, a closed
device policy admitting only KVM and DRM, a read-only filesystem view, a
seccomp syscall filter, cgroup CPU/memory limits, no swap, and a 512 MiB
memlock allowance. The VMM independently refuses `WRELA_HOST_PROFILE=product`
unless the observable namespace, seccomp, capability, and cgroup properties
match, then prefaults and locks the entire guest reservation before creating a
vCPU.

`macos/product-entitlements.plist` is the product signing input. It combines
App Sandbox with the Hypervisor entitlement and grants only user-selected
read-only image inputs. The ordinary `crates/wrela-vmm/entitlements.plist`
remains the explicitly nonconforming developer/HVF test identity.

Package acceptance must verify the final signature and entitlements with
`codesign --verify --strict` and `codesign -d --entitlements :-`, then run the
negative file, network, device, and unsigned-child tests. Diagnostic binaries
never produce product-conforming evidence.

## Rasputin lab provisioning and fallback builds

Validation SSH sessions use the dedicated `wrela` account. It belongs only to
the `kvm`, `video`, and `render` device groups, owns `/var/tmp/wrela-lab`, and
has a user manager whose memlock ceiling is exactly 536870912 bytes. The pinned
boot command line isolates CPUs 1–3 and forces the acceptance connector mode;
the probe records the resulting device tree, kernel, modules, profile, and
per-run display mode. A missing control makes product runs refuse themselves.

The ordinary `pi prepare` path cross-builds a static musl VMM on the Mac. A Pi
source build is available only through the conspicuous maintainer command
`cargo xtask pi remote-build-fallback wrela@rasputin.local`. It transfers one
source archive to a unique directory, verifies the archive digest and paths,
uses only provisioned `/usr/bin/cargo`, `/usr/bin/rustc`, and `/usr/bin/cc`
with an isolated Cargo home, builds `--locked --release` with
`native-presentation`, and retains `wrela-remote-build-v1` provenance including
the archive, lockfile, toolchain, build target/profile/features, agent, and
binary identities. That gnu build is explicitly non-release fallback
provenance and never replaces the Mac-built musl binary in a validation report.
