# Day 44 — Who pays, and what p95 was hiding

**Who pays for a wrong hint.** Yesterday I claimed in writing that a too-light
hint hurts other programs and a too-heavy one hurts only the program carrying
it. That was an argument, not a measurement, so today it became one: ten
mislabelled programs from the corpus, each run next to a real interactive
program and a CPU hog, 120 runs, predictions committed first.

The heavy side came out as cleanly as anything in this study. Give an
interactive program a "cpu" hint and its own p95 latency goes from ~1 us to
exactly 100 ms, the aging threshold, which is how long `hint` makes it wait
before rescuing it. Its neighbour doesn't notice (1 us, every time).

The light side was weaker than I predicted. A CPU-heavy program wearing an
"interactive" hint delayed its neighbour by more than 1 ms in only 2 of 5
cases. Short bursts at top priority don't hurt much. Long ones do.

And my hfair prediction failed in a way I should have seen coming. Two
programs pushed the neighbour's latency to 0.5-1.9 ms under hfair *with
either hint*. If swapping the hint changes nothing, the hint isn't the cause.
It's the collision floor from F16 again: these programs wake often with
millisecond bursts, and sometimes the interactive program wakes right behind one.

**What p95 was hiding.** The main report now has a starvation audit: every
configuration where an interactive process waited over 50 ms even once,
before its first dispatch or on any wake-up. 31 of 215 qualify. The good news:
no fair, hfair, or correctly hinted hint configuration at the default quantum
is among them. The correction: "MLFQ without the boost matches fair" (F13)
is only true after an ~88 ms start-up wait that every MLFQ variant pays and
p95 never showed. F13 now says so.
