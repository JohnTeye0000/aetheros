# Day 39 — hfair, the release build, and two corrections

## hfair: let the hint size the slice, never pick the thread

The previous entry ended with a design question: if measurement alone gives
interactive processes good latency, what can a hint still add? Something
measurement can't know: *the future*. "This process will want the CPU for a long
time, so don't switch it out every millisecond."

`hfair` is `fair` with one change. A thread hinted `cpu` gets a time slice
HFAIR_MULT times longer. That's all a hint does. Who runs next is still decided
by virtual runtime alone, so a wrong hint can cost some efficiency but can't delay
or starve anyone, and an interactive wake-up still preempts on virtual runtime.

Results (optimized kernel): at a x16 multiplier, `hfair` is within 3-18% of the
hint scheduler's context-switch count (`cpu4`: 92 vs 86; `all`: 157 vs 153;
`scale6`: 293 vs 248) while keeping fair queuing's 3-4 us interactive latency.
With deliberately wrong hints, latency is unchanged (3 us; the hint-as-priority
scheduler gave 106 ms) and switches rise to plain `fair`'s level, which is the
correct way to degrade. It was designed after seeing the earlier data, so treat
it as a hypothesis that needs fresh workloads, not a confirmed result.

## Correction 1: I was benchmarking an unoptimized kernel

While checking why `hfair` still cost ~3% makespan, I noticed the per-switch cost
was ~48 us, which is thousands of instructions for a handful of comparisons. The
kernel was a debug build (no optimization). I rebuilt with `--release` and reran
everything (1,155 runs). Scheduler decision time fell to 2.5-3.4 us per switch, and
policy makespans converged to within ~1%. So:

* the claim that `fair` costs 4-21% was an artifact of the build;
* the claim that hints give better CPU-bound *throughput* was wrong twice over:
  makespan is equal across policies, and the lower mean *turnaround* under `hint`
  is just completion ordering (jobs finish one after another instead of all
  together);
* what survives is the context-switch *count* gap, which is real but probably
  understated here, since the emulator has no cache or TLB effects.

The latency results were the same on both builds, because they depend on
decisions, not speed. Devlog 038 now carries an addendum pointing here.

## Correction 2: a benchmark line didn't mean the same thing everywhere

The `hfair` multiplier sweep ran at q = 50 ms instead of 10. `bench` used the
*previous run's* quantum as its default, and the sweep's lines followed the
quantum-sweep lines in the same plan. I only noticed because the sweep's switch
counts didn't match the main matrix. Defaults are constants now. I checked that
nothing else was affected (no other plan had `q=` lines before lines without
one), rebuilt, confirmed the fix reproduces earlier numbers (makespan identical to
the microsecond, switch counts identical), and reran the sweep.

## A better static analysis, and a blind spot

(PlainLang repo.) `plainhint.py` assumed every `while` loop runs 100 times, which
is why `p_trick`, a loop that really runs 10,000 times, got the wrong hint. It now
does constant propagation and recognises counted loops, conservatively: it
refuses when the counter is written twice or inside an `if`, when a `skip` could
jump past the increment, when the bound changes, or when the step moves away
from the bound. Calls are never evaluated, so a bound computed by a function
(`p_opaque`) is still misclassified. The old analysis is kept as
`--analysis basic`, and I checked it reproduces every committed program
bit-for-bit, so the earlier experiments stay valid.

With a wrong hint, the genuinely interactive neighbour sees 0.65-0.73 ms under
`hint` (0.002 ms when the hint is right), and 0.003 ms under `hfair` either way.
