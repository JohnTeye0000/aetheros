# Day 40 — Baselines have knobs, and I'd picked them

The hint scheduler's interactive latency was 4 us with a few competitors and
3.6 ms with ten, and I couldn't say why. Rather than guess, I wrote down a
hypothesis and three predictions in the experiment plan's header, then ran
145 runs to test them.

**The cause was my aging rule.** `hint` promotes any thread that has waited
100 ms to the interactive level (an anti-starvation rule). With ten CPU-bound
competitors each waits several hundred ms between turns, so they all get
promoted, tie with the interactive process, and one runs a slice first. With
aging off the latency at ten competitors is 4 us in all five seeds. Prediction
P1: confirmed.

**Two predictions failed.**

* With wrong hints I predicted starvation once aging was off, and saw nothing:
  p95 wake-up latency 813 us, *maximum* 1.8 ms. The starvation was real. The
  interactive process didn't get its first dispatch for 1.29 s, and took 1.75 s
  to finish instead of 0.45 s. My headline metric, wake-up latency, only
  counts samples after a thread's first sleep, so a thread that never gets to
  start is invisible to it. A metric can be right about what it measures and
  silent about what matters. Reports now carry response time and turnaround.
* I predicted that shorter MLFQ boost periods would improve interactive
  latency. The opposite is true: the boost *causes* the latency, by returning
  CPU-bound threads to the interactive level. With the boost at 1 s or off,
  MLFQ's interactive latency is 1 us. That undercuts my earlier headline
  ("MLFQ recovers almost none of the benefit of hints"), which rested on a
  200 ms boost I had picked and never questioned. What MLFQ without the boost
  can't do is serve the I/O-style process (76 ms p95).

None of this changes the result that matters most (a CFS-style scheduler with
no tunable latency/fairness constant matches hints on latency), but it changes
how much I'd trust any comparison where I also chose the baseline's constants.
The new kernel knobs (`bench ... aging=N`, `boost=N`) exist so the sweeps are
reproducible; the earlier measurement-defaults bug (devlog 039) is why each
sweep line states its parameters explicitly.
