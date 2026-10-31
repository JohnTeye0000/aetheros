# Random-workload validation (v1: lockstep interactive periods, mix)

30 random mixes (C cpu-bound; W io-style; I interactive), 21 with at least one interactive process. Means over 3 seeds. Latencies in microseconds.

**M1** [FAILED]: 5/21 mixes (needed 100%). fair and hfair interactive p95 <= 50 us.
**M2** [HELD]: 30/30 mixes (needed 100%). hint <= hfair <= 1.05 x fair in context switches.
**M3a** [FAILED]: 5/21 mixes (needed 100%). hfair with WRONG hints: interactive p95 <= 50 us.
**M3b** [HELD]: hint with WRONG hints exceeds 10 ms in 21/21 mixes that have both CPU-bound and interactive processes (needed a third).
**M4** [FAILED]: 23/30 mixes (needed 100%). makespan within 1.0% across rr/mlfq/fair/hint/hfair/hfair16 (largest spread 1.14%).
**M5** [FAILED]: 4/21 mixes (needed 100%). mlfq's interactive p95 is the worst of rr / mlfq / fair.

## Violations

- M1: `mix:1;1;3`
- M1: `mix:1;2;2`
- M1: `mix:2;0;2`
- M1: `mix:2;3;2`
- M1: `mix:2;3;3`
- M1: `mix:4;3;3`
- M1: `mix:5;0;2`
- M1: `mix:5;2;3`
- M1: `mix:5;3;3`
- M1: `mix:6;0;2`
- M1: `mix:6;3;3`
- M1: `mix:7;2;2`
- M1: `mix:7;2;3`
- M1: `mix:8;0;2`
- M1: `mix:8;1;3`
- M1: `mix:8;3;2`
- M3a: `mix:1;1;3`
- M3a: `mix:1;2;2`
- M3a: `mix:2;0;2`
- M3a: `mix:2;3;2`
- M3a: `mix:2;3;3`
- M3a: `mix:4;3;3`
- M3a: `mix:5;0;2`
- M3a: `mix:5;2;3`
- M3a: `mix:5;3;3`
- M3a: `mix:6;0;2`
- M3a: `mix:6;3;3`
- M3a: `mix:7;2;2`
- M3a: `mix:7;2;3`
- M3a: `mix:8;0;2`
- M3a: `mix:8;1;3`
- M3a: `mix:8;3;2`
- M4: `mix:6;3;3`
- M4: `mix:7;2;3`
- M4: `mix:7;3;1`
- M4: `mix:8;1;3`
- M4: `mix:8;2;1`
- M4: `mix:8;3;0`
- M4: `mix:8;3;2`
- M5: `mix:1;1;3`
- M5: `mix:1;2;2`
- M5: `mix:2;0;2`
- M5: `mix:2;3;2`
- M5: `mix:2;3;3`
- M5: `mix:4;3;3`
- M5: `mix:5;0;2`
- M5: `mix:6;0;2`
- M5: `mix:6;3;3`
- M5: `mix:7;2;2`
- M5: `mix:7;2;3`
- M5: `mix:7;3;1`
- M5: `mix:8;0;1`
- M5: `mix:8;0;2`
- M5: `mix:8;1;3`
- M5: `mix:8;2;1`
- M5: `mix:8;3;2`

## Starvation check (worst interactive response time, ms, any seed)

-    164.0 ms  `mix:8;3;2` hint (hints=wrong)
-    162.6 ms  `mix:8;1;3` hint (hints=wrong)
-    161.6 ms  `mix:8;0;2` hint (hints=wrong)
-    161.6 ms  `mix:8;2;1` hint (hints=wrong)
-    159.6 ms  `mix:8;0;1` hint (hints=wrong)
-    149.7 ms  `mix:5;0;2` hint (hints=wrong)
-    143.6 ms  `mix:7;2;3` hint (hints=wrong)
-    142.6 ms  `mix:7;2;2` hint (hints=wrong)
-    142.6 ms  `mix:7;3;1` hint (hints=wrong)
-    126.0 ms  `mix:4;3;3` hint (hints=wrong)

## Per-mix table: interactive p95 (us), by policy

| mix | rr | mlfq | fair | hint | hfair | hfair16 | hint wrong | hfair wrong |
|---|---|---|---|---|---|---|---|---|
| 1;1;3 | 13814 | 2256 | 492 | 489 | 491 | 492 | 106488 | 492 |
| 1;2;2 | 5125 | 4179 | 492 | 328 | 491 | 492 | 107002 | 491 |
| 1;3;0 | - | - | - | - | - | - | - | - |
| 2;0;2 | 5488 | 5001 | 491 | 327 | 491 | 491 | 105325 | 491 |
| 2;2;0 | - | - | - | - | - | - | - | - |
| 2;3;0 | - | - | - | - | - | - | - | - |
| 2;3;2 | 20463 | 13486 | 493 | 165 | 494 | 493 | 108492 | 494 |
| 2;3;3 | 20469 | 15440 | 493 | 167 | 494 | 493 | 109003 | 495 |
| 4;1;0 | - | - | - | - | - | - | - | - |
| 4;3;1 | 39468 | 43549 | 4 | 3 | 4 | 4 | 108001 | 5 |
| 4;3;3 | 40472 | 36534 | 332 | 494 | 496 | 497 | 109491 | 496 |
| 5;0;2 | 35490 | 35002 | 494 | 327 | 493 | 494 | 135003 | 494 |
| 5;2;0 | - | - | - | - | - | - | - | - |
| 5;2;3 | 46144 | 46877 | 333 | 495 | 496 | 497 | 139001 | 496 |
| 5;3;1 | 49473 | 57157 | 5 | 3 | 5 | 5 | 141001 | 5 |
| 5;3;3 | 50478 | 61137 | 333 | 334 | 498 | 498 | 141001 | 498 |
| 6;0;2 | 45491 | 45334 | 331 | 500 | 495 | 495 | 105494 | 332 |
| 6;3;0 | - | - | - | - | - | - | - | - |
| 6;3;3 | 60477 | 55978 | 498 | 498 | 499 | 500 | 109492 | 498 |
| 7;2;2 | 65146 | 51002 | 334 | 665 | 498 | 668 | 127491 | 334 |
| 7;2;3 | 66147 | 50646 | 498 | 512 | 500 | 500 | 128492 | 499 |
| 7;3;1 | 69478 | 49146 | 6 | 4 | 6 | 6 | 128001 | 6 |
| 8;0;1 | 65001 | 45002 | 5 | 3 | 5 | 5 | 145003 | 5 |
| 8;0;2 | 65492 | 45002 | 169 | 512 | 497 | 497 | 145495 | 169 |
| 8;1;0 | - | - | - | - | - | - | - | - |
| 8;1;3 | 71321 | 52154 | 498 | 1176 | 499 | 500 | 147492 | 498 |
| 8;2;0 | - | - | - | - | - | - | - | - |
| 8;2;1 | 74651 | 43669 | 6 | 2670 | 6 | 6 | 147001 | 6 |
| 8;3;0 | - | - | - | - | - | - | - | - |
| 8;3;2 | 80484 | 44725 | 336 | 335 | 501 | 670 | 148495 | 501 |
