# Day 45 — A clock nobody reset

To sweep the last fixed constants (MLFQ's number of levels, fair's minimum
slice) I turned them into `bench` knobs, `levels=` and `floor=`. Their defaults
equal the old constants, so nothing should have changed. To check that, I reran
21 runs and compared each one, seed for seed, with the same run on the old
kernel image (`compare_runs.py`).

Twenty matched to within 0.6%. One didn't: hmlfq, seed 1, with an io-style
p95 3 ms off. Before blaming the new code I ran the same lines on the *saved
old image*, in a different order. The old image disagreed with its own earlier
results by up to **29 ms**. So the new code wasn't the cause, and the old data
had an uncontrolled variable all along.

The cause: MLFQ boosts every thread to the top level every 200 ms, timed by
`LAST_BOOST`. Nothing reset that clock when a benchmark run started, so where
the first boost landed in a run depended on when the *previous* run had ended.
The seed didn't control it. The order of lines in the plan file did. Across 10
seeds it behaved like random noise, so it widened confidence intervals rather
than biasing them. But "identical runs give identical results" was false for
the two MLFQ policies, and I'd been saying that for weeks.

Fix: `reset_counters()` (called at the start of every run) now restarts the
boost clock. The same lines run in two different orders now agree to within one
tick (at most 2 ms on one seed, against 29 ms before).

Because this changes MLFQ-family numbers everywhere, the whole dataset is being
rerun on one final kernel image rather than patching individual plans. Every
finding gets re-checked against the new numbers, and anything that moves will
be reported as having moved.
