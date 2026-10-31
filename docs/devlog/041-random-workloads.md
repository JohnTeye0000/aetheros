# Day 41 — Workloads I didn't pick

Every workload so far was one I chose, and `hfair` was designed after looking
at the data it was meant to explain. That's the setup in which you fool
yourself. So I drew 30 workloads at random from a fixed seed
(`gen_random_mix.py`), wrote predictions into the plan headers, and ran each
under eight policy configurations, 3 seeds each: 720 runs. `mix_summary.py`
scores the predictions without me in the loop.

**New kernel pieces.** `bench mix:C,W,I` builds a workload of C CPU-bound,
W io-style and I interactive processes. `mix2:` does the same with the
interactive processes on staggered 13/15/17 ms periods (`wint2`, `wint3` are
new built-in programs). Workload labels in the CSVs use `;` instead of `,`
because the commas broke the CSV.

**First batch: four of six predictions failed.** Two of the failures were my
fault. Every interactive process slept exactly 15 ms, so with two or three of
them they woke together and queued behind each other, and no scheduler on one
CPU can fix that. The mlfq-is-worst prediction was ill-posed: rr and mlfq are
both tens of ms behind, and which is worse is noise.

**Second batch: staggered periods, predictions rewritten before running.**
Five of eight held, including the structural ones: switch ordering
hint <= hfair <= fair in all 30 mixes again, wrong hints breaking
hint-as-priority in every mix with both CPU-bound and interactive processes, makespan within 1.14%.
Three failed, and every M1' failure is a mix with three interactive
processes.

Why three and not two? Staggered periods don't help when wake-ups are
quantized to a 1 ms tick: two processes that wake in the same tick queue
regardless of their periods. Back-of-envelope, about 6% of wake-ups collide
with three interactive processes and 3% with two. Measured: 5.2% and 3.3%
(0.0% with one). The p95 threshold sits exactly at that edge. This was
worked out after the failure, so it's a hypothesis, and the obvious test is a
run with jittered wake-ups or a finer tick.

The honest takeaway: "a few microseconds of latency" is true for *one*
interactive process. With several, every scheduler is bounded below by one
burst of a sibling, and p95-against-a-threshold is the wrong yardstick.
The hint scheduler with correct hints also looked worse here (0.33-2.7 ms in
15 of 21 mixes) than in my hand-picked workloads, which is exactly why this
run was worth doing.
