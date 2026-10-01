# Day 36 — Floating point that survives a context switch

PlainLang numbers are 64-bit floats, so compiled PlainLang programs live in the
SSE registers (`xmm0`–`xmm15`). The context switch saved six general-purpose
registers and nothing else. Two float-using processes would therefore overwrite
each other's values every time the scheduler switched between them, silently.

## The fix

* **Turn SSE on for user mode**: CR0.EM = 0, CR0.MP = 1, CR4.OSFXSR = 1,
  CR4.OSXMMEXCPT = 1.
* **Save and restore it on every switch.** Each thread gets a 512-byte,
  16-byte-aligned area. `schedule()` runs `fxsave64` into the outgoing thread's
  area before `switch_context`, and `fxrstor64` from it when that thread is
  later switched back in.
* **New processes start clean.** `enter_user` runs `fninit`, loads the default
  MXCSR (`0x1F80`, all FP exceptions masked) and zeroes `xmm0`–`xmm15` before
  `iretq`. A new program never sees the previous program's registers. That's a
  correctness requirement and a security one (it would be an information leak).

The kernel itself is built soft-float (no SSE in kernel code), so the only
floating-point state in the registers ever belongs to a user program. That's why
saving it at the switch point is sufficient.

## Proving it, including that the test can fail

`run fpu` parks a value unique to its process in `xmm0`, and doubles a value in
`xmm1` 40 times, sleeping 3 ms between rounds so other processes run in between.
Its exit status is the number of times its registers were corrupted.
`bench fpu` runs three of them plus a CPU hog.

| kernel | corruptions per fpu process (18 processes, 6 runs) |
|---|---|
| with fxsave/fxrstor | **0** (all 18) |
| fxsave/fxrstor removed (negative control) | **40–41** (all 18) |

The negative control matters. A test that never fails proves nothing. Here,
removing the fix makes every single process detect corruption on (nearly) every
round.
