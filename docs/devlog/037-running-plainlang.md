# Day 37 — PlainLang programs run on AetherOS

My other project, PlainLang (an English-like beginner language), now has a
backend that compiles to native x86-64 for this kernel: `plainaxe.py` in the
PlainLang repo. It hand-encodes the machine code (SSE2 doubles, rel32 fixups, a
small runtime) and emits AXE1 program files. This is the kernel side.

## What the kernel needed

* **`printnum` (syscall 13).** PlainLang has exact rules for printing numbers:
  whole numbers with no decimal point (and exact digits however large: 2^70 prints
  `1180591620717411303424`), everything else rounded half-away-from-zero to 10
  places, never scientific notation. The kernel implements them
  (`format_plain_number`), including exact decimal expansion of large integers in
  base 10^9 limbs. The kernel is soft-float, but its float operations are
  IEEE-exact, so it agrees with Python's.
* **Bigger programs and stacks.** Compiled programs can exceed 16 KiB, so code is
  now up to 64 KiB. The user stack grew from 8 to 32 KiB for recursion.
* **`exec <file>` in autorun scripts**, which runs a program to completion with
  `PROG,<file>,begin` / `PROG,<file>,end,<status>` markers around its output, and
  a kernel-side `wait_child`.
* **A bigger on-disk filesystem.** AFS1 now holds up to 1 MiB (it was 128 KiB),
  so a whole test suite fits on one disk image.
* **`bench files:a.axe+b.axe+...`**, which launches program files from the
  filesystem, each with the hint in its own header.

## The test

PlainLang's `test_axe.py` compiles 30 hand-written programs plus 200 random
programs from the WebAssembly fuzzer's generator. It boots this kernel in QEMU,
runs them all, and checks every output against PlainLang's reference
interpreter, including the exact point where a runtime error stops a program.
All 232 pass. The fuzzer found one real compiler bug on the way: the backend
printed `total ` before discovering an error later in the same `say` line. The
interpreter builds the whole line first.

## Compiler-derived hints

`plainhint.py` (also in the PlainLang repo) reads a program and estimates how
much work it does between `wait`s, from its loop structure. It then labels the
program `cpu`, `io` or `interactive` and writes the label and its reasoning into
the `.axe` file. Five PlainLang workloads in `experiments/plainlang/` test it.
Two are designed to break it:

| program | behaviour | compiler's hint |
|---|---|---|
| `p_cpu` | counts primes, never waits | cpu ✓ |
| `p_io` | bursts of work, then `wait 0.004` | io ✓ |
| `p_int` | `wait 0.015`, then a tiny bit of work | interactive ✓ |
| `p_phase` | interactive for 15 events, then a long computation | cpu (right for half of it) |
| `p_trick` | io-like, but its burst is a `while` loop the classifier can't size | interactive ✗ (assumed 100 iterations; really 10,000) |

The experiments measure what those hints do to scheduling (`experiments/RESEARCH.md`).
