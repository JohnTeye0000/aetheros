# Day 43 — The cheap fix that wasn't

Yesterday's classifier result had an obvious-looking fix: every mistake was
"too light", so make the classifier guess heavy when it's unsure. I wrote the
predictions down and committed them to git *before* writing the analysis, so
the commit history proves the order. Then I wrote it: unknown loops assumed
to run 10,000 times instead of 100, and an `if` counted as taking its
*fewest* waits.

Three of four predictions failed. Too-light misses fell from 36 to 11
(I'd said 9 or fewer). Accuracy fell from 76% to 59% (I'd said no lower than 61%).
And 50 new too-heavy mistakes appeared, mostly from the 10,000 assumption
overshooting, not from the gated waits I'd expected. The one that held: no CPU
hog was labelled interactive any more.

The lesson is about what a constant can do. The programs' loop bounds span
four orders of magnitude, so any single guess for an unknown bound is wrong
for most of them. The bias just chooses who pays. A too-light hint makes
*neighbours* pay under a priority scheduler, while a too-heavy one makes the
program itself pay. That's an argument for the heavy bias when protecting
others matters, and an even better argument for a scheduler design (hfair)
where neither kind of mistake can hurt anyone else.

Small but real: the `if` rule also made some programs *lighter*. A program
whose only waits are inside an `if` now counts zero waits, and if it does
little work it lands in `none`. One "safe" rule broke another category.
