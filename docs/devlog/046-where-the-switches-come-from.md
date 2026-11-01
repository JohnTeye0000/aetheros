# Day 46 — Where the switches come from

The hint scheduler had two constants nobody had questioned. CPU-hinted
threads got 4x longer slices (Q << level), and I/O-hinted threads sat one level
below interactive ones. Two new bench knobs, `hscale=0` and `iotop=1`, and
four predictions committed before running.

Three held. The one that matters most: with flat slices, the hint scheduler's
context switches on four CPU-bound programs went from 85 to 267, the same as round
robin (268). So the "hints cut switches 14x" result was never about priority.
It was about giving long slices to the threads that want them. Put that next to
yesterday's result (fair with a 5 ms floor also cuts switches 5x) and the story
gets simpler and more honest: switch counts follow slice lengths, and a hint's
real job is to say *which* threads deserve long ones. That's exactly what hfair
keeps and nothing more.

Promoting I/O to the top level was a straight trade. The I/O process went from
0.69 ms to 2 us, and the interactive process from 2 us to 818 us, because now it
waits behind 5 ms I/O bursts at equal priority. In the workload with two I/O
processes and no interactive one, I/O latency got *worse* (1.9 to 2.7 ms), and I
can't explain that yet, so it stays recorded as a failed prediction.

A surprise: flat slices cut the hint scheduler's worst-case latency with ten
competitors from 3.6 ms to 0.6 ms. That fits the aging mechanism from F11.
When aged CPU threads tie with the interactive one, a 10 ms slice ahead of it
is better than a 40 ms one.

**Addendum.** The unexplained result got its own test, with predictions
committed first. Turn aging off and the anomaly disappears: I/O latency is
1.75 ms with or without promoting I/O to the top level. So aging, the same rule
behind F11 and the scale10 surprise above, was the cause again. That makes
three separate results traced to one anti-starvation rule I chose without
testing.
