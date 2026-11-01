# Day 47 — Fixing the rule that kept causing trouble

Three different odd results had the same cause: the hint scheduler's aging rule,
which lifts any thread that waited 100 ms straight to the top priority, where it
collides with the interactive work it was supposed to stay behind. The textbook
alternative is graded aging, one level up per 100 ms waited. It took ten lines
of kernel code and a new `agestep=1` knob.

Predictions first, as always. Three held: the I/O anomaly vanished, correct
hints were unaffected, and with wrong hints the victim waited about twice as long
(206 ms instead of 106) but was never starved. The fourth failed in the direction
nobody minds: with ten competitors, the latency tail I expected to shrink a bit
disappeared entirely (3.6 ms to 4 us). I don't know why yet, and the write-up
says so.

The honest framing: this makes `hint` a better scheduler when hints are right
and a slower-to-rescue one when they're wrong. It doesn't change the main
conclusion. `hfair` doesn't need aging at all.
