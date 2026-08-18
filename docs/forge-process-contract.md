# Forge cold-run process contract

Forge uses the repository's real compiler and VMM as separate processes. The
maintainer harness is:

```text
cargo xtask forge run path/to/main.wr --display native
cargo xtask forge restart
```

`run` authenticates a file or complete project source snapshot, compiles a
release test image into a new candidate directory, retains
the compiler's stdout and stderr, signs a fresh diagnostic HVF VMM, and starts
that VMM with the sealed report/image pair. Native output therefore consists
only of complete frames validated by the guest display boundary; Forge does
not contain a renderer or alternate scene representation. Headless mode uses
the same image and display validation and is useful for automated clients.

After a successful child exit, `target/wrela-forge/current.txt` records the
versioned `wrela-forge-run-v1` identity of the source, report, image, VMM, and
retained run directory. A compiler failure or VMM crash retains its candidate
diagnostics without changing `current.txt`, so the last successful run remains
unambiguous. `restart` reads that manifest and performs a full rebuild and cold
boot. It never mutates or replaces state in a running guest.

Project digests are path-sorted and exclude only `.git` and `target`; symlinks
and special files are refused. The source digest is checked again after the
guest exits. If source changed while compilation or boot was in progress, the
candidate remains diagnostic evidence but is never published as current.
`current.txt` is replaced by atomic rename, so a process crash cannot expose a
partial manifest.

The retained run directory contains compiler diagnostics, the VMM transcript,
VMM diagnostics, metrics, and the Wrela choice record. Host errors remain
process failures. A future GUI may supervise this command or implement an
equivalent versioned local transport, but may not bypass the sealed
report/image pairing, frame validation, or normalized machine input devices.
macOS and Linux host key codes translate to the portable
`wrela_machine::input::InputEventV1` FIFO before device state. Host timestamps,
controller identifiers, and repeat policy do not enter the normalized event;
replay suppresses live host input and accepts only the recorded contiguous
sequence.

For the v1 harness, `target/wrela-forge/<candidate>/input.events` is the
append-only input transport passed with `--input-events`. Each LF-terminated
line has exactly `kind=mac|linux code=<u16> pressed=0|1 repeat=0|1 player=<u8>`;
malformed UTF-8, fields, values, truncation, or a player outside 0–3 terminates
the run. A GUI may append translated host events to this file while the child
runs. It may not write a replay run, inject packed guest words directly, or
replace the file in place.

This harness uses the explicitly nonconforming diagnostic host profile. A
packaged Forge child must instead be signed with
`packaging/macos/product-entitlements.plist` and pass the product sandbox
checks before it can produce product-conforming evidence.
