# Day 42 — Checking my excuse, then grading the compiler

**The excuse.** Yesterday's random workloads failed three predictions, and I
explained the failures afterwards: interactive processes that wake in the same
1 ms tick queue behind each other, whatever the scheduler does. An explanation
invented after a failure is cheap, so it got its own run with predictions
written first (99 runs). All four held. With *no* competing processes at all,
two interactive programs have 3.3% of wake-ups delayed and three have about
5.7%, identically under rr, mlfq, fair, hint and hfair. And the hint
scheduler's poor random-mix latency with two or fewer interactive processes was
entirely its aging rule: turning aging off took it from 0.33-2.7 ms to 3-4 us.

One thing I got wrong in the explanation: I'd done "probability" arithmetic,
but the collisions are deterministic, exactly 1 wake-up in 30 per colliding pair
in every seed, because the seed doesn't move those processes' phases. The
mechanism was right. The arithmetic was a lucky approximation.

**The compiler's report card.** Until now the hint classifier had been tested
on five programs I wrote. Now it's 150 generated ones (fixed seed), each run
alone in the kernel so its real CPU-time-per-wait can grade the static guess.
Counted analysis: 76% right (the older basic analysis: 62%), and only 2
"gross" errors (cpu <-> interactive). It does well on the loops it was built
for (counted while 97%) and worse on the blind spots it documents: aliased
counters 53%, gated waits 62%, function-bound loops 67%.

The surprise: **all 36 wrong guesses were too light.** Not one program was
guessed heavier than it was. That's the dangerous direction for a scheduler
that uses hints as priority, since a CPU hog labelled interactive is exactly the
failure from F4. It's harmless for hfair, which only uses hints to size
slices. So the classifier's errors land precisely where the priority design is
fragile. Next experiment: a classifier biased to guess heavy when unsure, to
see what that costs in accuracy and buys in safety.

The prediction that failed (A4): I expected most misses to sit near a
threshold. Only 17 of 36 did. Many are misses by a whole factor of 10 or more
from a loop bound the classifier simply can't see.
