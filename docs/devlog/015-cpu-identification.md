# Devlog 015 — CPU identification (`cpuid`)

**Milestone:** the kernel can ask the processor to describe itself. `cpuinfo` prints the vendor, the human-readable brand string, family/model/stepping, and which feature flags are set — the same mechanism `lscpu` and `cat /proc/cpuinfo` are built on.

## A single instruction, self-describing hardware

`cpuid` is unusual among x86 instructions: instead of operating on memory or doing arithmetic, it asks the CPU to report facts about *itself*. You put a "leaf" number in `EAX`, execute `cpuid`, and the processor overwrites `EAX`/`EBX`/`ECX`/`EDX` with the answer for that leaf. Different leaves mean different questions:

- **Leaf 0** — the maximum supported leaf, plus a 12-character **vendor string** spread across `EBX`/`EDX`/`ECX` (in that odd order — it's historical).
- **Leaf 1** — version info (`family`/`model`/`stepping`, packed into bitfields of `EAX`) and two 32-bit **feature bitfields** in `EDX` and `ECX` — one bit per capability (SSE, MMX, an APIC, etc.).
- **Leaves `0x80000002`–`0x80000004`** — the "extended" leaves. Together they spell out a 48-byte human-readable brand string like `QEMU Virtual CPU version 2.5+`, 16 bytes per leaf, in the same packed-register layout as the vendor string.

None of this needs an interrupt, a driver, or I/O ports — it's a single CPU instruction, and Rust exposes it directly as `core::arch::x86_64::__cpuid`, no external crate needed.

## Reading a bitfield of features

The feature registers are the interesting part: each bit position means something specific (bit 0 of `EDX` is "has an FPU", bit 26 is SSE2, and so on), and Intel and AMD have agreed on most of the low bits over decades of compatibility. Rather than hardcode a `println!` for each one, I built a small table — `(is_edx, bit, name)` — and looped over it, printing whichever names are actually set: `info.has(is_edx, bit)` just tests `register & (1 << bit) != 0`. Add a row to the table and you're checking a new feature.

## Real output, real hardware quirks

The demo shows this isn't guesswork — it's genuinely reading the emulated CPU:

```
vendor: AuthenticAMD
brand:  QEMU Virtual CPU version 2.5+
family: 15  model: 11  stepping: 1
features: fpu tsc apic mmx sse sse2 sse3 hypervisor
```

Two details are worth noticing. First, the **vendor and brand disagree** (`AuthenticAMD` but a QEMU brand string) — that's QEMU's default CPU model reporting itself as an old AMD Opteron by ID while still stamping its own name in the brand string, a real quirk of virtualized hardware you only see by actually reading it. Second, **`hypervisor`** shows up in the feature list — bit 31 of `ECX` is the standard signal a CPU is running inside a virtual machine, and ours correctly reports true, because it is.

## Next

The two large remaining milestones — preemptive scheduling and a real disk driver — stay queued for slower, multi-session work.
