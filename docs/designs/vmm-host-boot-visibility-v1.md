# Wrela vCPU boot and visibility table v1

This table is the complete backend-neutral initial-state contract consumed by
the portable VMM engine. Both host adapters mechanically initialize every
writable row before the first guest instruction. A row marked *sealed-out* is
not assumed equal across hosts: the compiler's executable-section certificate
proves that no permitted instruction can observe it.

| surface | machine-v1 value or visibility rule | enforcement |
|---|---|---|
| `x0` | `MACHINE_INFO_BASE` | initialized by both adapters |
| `x1`–`x30` | zero | all 30 registers initialized by both adapters |
| `PC` | report entry for the owning core | initialized by both adapters |
| `SP_EL0` | zero | initialized by both adapters |
| `SP_EL1` | report-declared top of that core's stack | initialized by both adapters |
| `PSTATE` / `DAIF` | `0x3c5` (EL1h with D/A/I/F masked) | initialized by both adapters |
| `CPACR_EL1` | `0x0030_0000` (FP/SIMD enabled at EL1) | initialized by both adapters |
| `FPCR` | `0x0200_0000` (`DN=1`) | initialized by both adapters |
| `FPSR` | zero | initialized by both adapters |
| `V0`–`V31` | 128 zero bits each | all 32 registers initialized by both adapters |
| `TTBR0_EL1`, `TCR_EL1`, `MAIR_EL1`, `VBAR_EL1`, `SCTLR_EL1` | exact values in the sealed `Stage1` report; `SCTLR_EL1.M=1` and `WXN=1` | validated before VM creation and installed in this order with `SCTLR_EL1` last |
| `MPIDR_EL1`, `MIDR_EL1` | sealed-out | no MRS encoding for either register is certificate-permitted |
| `ID_AA64*` feature registers | sealed-out; KVM validates the FP/ASIMD/Atomics minimum, reduces writable Atomics fields to the baseline, and disables writable SVE, pointer-authentication, and MTE discovery | KVM masks writable fields before any other vCPU register access; immutable FP/ASIMD version supersets remain sealed out, and the certificate is authoritative across both hosts |
| counter, timer, and guest PMU registers | sealed-out and inaccessible to generated code | no MRS/MSR encoding is permitted; validation counters are host perf events around `KVM_RUN`, not a guest PMU ABI |
| `CTR_EL0`, `DCZID_EL0`, and other cache-identification registers | sealed-out | no MRS encoding is permitted |
| cache maintenance and `DC ZVA` | forbidden | no `SYS`/`DC` encoding is permitted; ordinary coherent memory accesses and the two listed DMB forms are sufficient |
| SVE, pointer authentication, and MTE state | disabled and sealed-out | no associated instruction/system-register surface is compiler-permitted; KVM feature masks are defense in depth |
| PSCI, SVC, HVC, SMC, firmware calls, and hypercalls | absent | executable-section certificate rejects their exception-generation encodings |
| debug registers and host breakpoints | sealed-out | no debug-system-register instruction is permitted; compiler-emitted `BRK #imm16` is a fail-closed guest fault only |

## Executable-section certificate

Every sealed report carries `Stage1 system_allowlist_sha256`, the SHA-256 of
the exact, sorted UTF-8 value `wrela_machine::stage1::SYSTEM_INSTRUCTION_ALLOWLIST_V1`.
The compiler scans every word of every executable section, including the
stage-1 vector page, after final layout. It accepts only:

- `BRK #imm16`;
- `DMB ISHLD` and `DMB ISHST`;
- the fault trampoline's exact `MRS ESR_EL1`, `MRS FAR_EL1`, `MRS ELR_EL1`,
  and `MRS SPSR_EL1` encodings.

All other AArch64 exception-generation and system-instruction encodings fail
image construction. The machine report parser independently requires the
current allowlist digest, so an older or forged certificate cannot be paired
with a current image contract. The four permitted MRS instructions exist only
in the read-only stage-1 fault trampoline and feed the original-fault record;
ordinary generated code has no system-register read surface.
