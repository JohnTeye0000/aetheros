# Day 38 — The baseline that changed the answer

The study's weakest point was that the hint scheduler only beat textbook
schedulers (round robin, MLFQ). A reviewer's first question would be "what about
CFS?", the scheduler Linux actually uses. So I built one.

## `fair`: a CFS-style scheduler (no hints)

Each thread accumulates *virtual runtime*, the CPU time it has used. The runnable
thread with the least runs. A slice is the base quantum divided among the
runnable threads (at least one 1 ms tick). A thread waking from sleep is credited
for having slept (placed at `min_vruntime - Q/2`, bounded so sleep can't be
banked forever) and preempts the running thread if it is more than 1 ms behind.
It's an approximation: no weights, no nice levels, no group scheduling, no EEVDF
deadlines, and one CPU.

## What it did to the findings

Against `fair`, the hint scheduler's headline advantage mostly disappears:

| `all` workload, q = 10 ms | interactive p95 latency |
|---|---|
| rr | 10.1 ms |
| mlfq | 6.1 ms |
| fair (no hints) | 0.05 ms |
| hint (correct hints) | 0.03 ms |

`fair` also handles the programs that broke hints: the phase-changing program
(70 ms under `hint`, 0.05 ms under `fair`) and the misclassified one. Hints still
win on cost (2-11x fewer context switches, 4-21% better CPU-bound turnaround) and on
I/O-bound latency at large quanta (0.35 ms vs 9 ms at q = 50). Full numbers and
the revised conclusion are in `experiments/RESEARCH.md` (F8-F10).

This is a more honest and more interesting result than the one I had: it says
where hints actually add value, and it suggests the design worth building next
(hints to cut `fair`'s switching, measurement to catch wrong hints).

## A reproducibility check that caught my own mistake

After adding `fair`, I verified `rr` and `hint` still reproduced the earlier
results. They didn't, quite. Scheduler decision time per switch had risen from
31.5 to 45.4 us. I had added a scan over all threads (to maintain `fair`'s
`min_vruntime`) that ran under *every* policy. Scheduling decisions were
unchanged, but cost was not, so old and new results would have been measured on
different kernels. The scan now runs only under `fair`. The fix restores 33.5 us
per switch, within 2 us of the committed kernel. All `fair` runs were redone on the
final binary.

Bit-exact comparison across runs turned out *not* to be a valid test: two disk
images with different contents change boot-time instruction counts, which shifts
where benchmark start falls relative to the 1 ms timer by a few microseconds. The
valid test was the scheduler's own measured cost and the switch counts.
