# Rasputin target profile

Rasputin is the one physical Wrela product and measurement target. Values in
this profile were observed on the board on 2026-08-16; generic Raspberry Pi 5
capacity is not a substitute for this record.

## Hardware

| Property | Canonical value |
|---|---|
| Board | Raspberry Pi 5 Model B Rev 1.1 |
| SoC / CPU | BCM2712, four Cortex-A76 r4p1 cores |
| Online physical cores | `0-3` |
| Stock core frequency | fixed 2.4 GHz |
| Physical RAM model | 1 GiB |
| Linux `MemTotal` | 1,038,827,520 bytes |
| Guest vCPUs | 3, assigned to physical cores `1-3` |
| Host housekeeping | physical core `0` |
| Guest DRAM | 512 MiB, `0x4000_0000..0x6000_0000` |

The remaining roughly 479 MiB visible to Linux is host capacity for the
kernel, KVM, VMM, display, and measurement tools. Swap is diagnostic-only and
never enlarges either the guest profile or a product memory verdict.

## Installed host profile

The board runs Debian 13 with the Raspberry Pi `6.18.34+rpt-rpi-2712` arm64
kernel. The persistent measurement setup is:

- `linux-perf` matching the running kernel;
- `kernel.perf_event_paranoid=1`, allowing the unprivileged VMM to create
  per-thread guest/EL1 counters;
- the `performance` governor with minimum and maximum frequency both
  2,400,000 kHz;
- boot parameters `isolcpus=domain,managed_irq,1-3 irqaffinity=0`;
- unbound workqueues and default IRQ affinity restricted to core 0; and
- Raspberry Pi swap disabled with `Mechanism=none`, so zram and disk-backed
  swap cannot enlarge the physical-memory envelope; and
- `/dev/kvm` available to the `kvm` group without privilege escalation.

The configuration lives in `/etc/sysctl.d/90-wrela-perf.conf`,
`/usr/local/sbin/wrela-apply-host-profile`,
`/etc/systemd/system/wrela-host-profile.service`, and
`/etc/rpi/swap.conf.d/90-wrela-no-swap.conf`, and
`/boot/firmware/cmdline.txt`. The pre-Wrela command line is retained on the
host as `cmdline.txt.pre-wrela-profile`.

The kernel supports CPU isolation but has `CONFIG_NO_HZ_FULL` and
`CONFIG_HUGETLBFS` disabled. Results must not claim full-tickless or HugeTLB
isolation. Product memory commitment therefore uses prefaulted, locked normal
pages; until that VMM path lands, runs are diagnostic rather than
product-conforming.

## Measurement capability

The `armv8_cortex_a76` PMU exposes the complete validation group:
`cpu_cycles`, `inst_retired`, `l1d_cache_refill`, `l2d_cache_refill`,
`br_mis_pred`, `stall_frontend`, and `stall_backend`. All seven events fit one
pinned group without multiplexing. Guest-only `exclude_host` counters open as
the ordinary SSH user; validation still refuses any sample whose
`time_enabled` and `time_running` differ.

The VMM must bind its three vCPU threads to cores 1, 2, and 3 and keep device,
display, and supervisory work on core 0. Host isolation prepares that layout;
it does not replace explicit VMM affinity.
