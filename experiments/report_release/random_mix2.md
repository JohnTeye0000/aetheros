# Random-workload validation (v2: staggered interactive periods, mix2)

30 random mixes (C cpu-bound; W io-style; I interactive), 21 with at least one interactive process. Means over 3 seeds. Latencies in microseconds.

**M1'** [FAILED]: 13/21 mixes (needed 90%). fair and hfair interactive p95 <= 100 us.
**M2** [HELD]: 30/30 mixes (needed 100%). hint <= hfair <= 1.05 x fair in context switches.
**M3a'** [FAILED]: 17/21 mixes (needed 90%). hfair with WRONG hints: interactive p95 <= 100 us.
**M3b** [HELD]: hint with WRONG hints exceeds 10 ms in 21/21 mixes that have both CPU-bound and interactive processes (needed a third).
**M4'** [HELD]: 30/30 mixes (needed 100%). makespan within 1.5% across rr/mlfq/fair/hint/hfair/hfair16 (largest spread 1.14%).
**M5'** [HELD]: 21/21 mixes (needed 100%). rr's AND mlfq's interactive p95 are worse than fair's.
**M6a** [FAILED]: 6/11 mixes (needed 90%). hint with correct hints: p95 <= 100 us in mixes with <= 5 CPU-bound processes.
**M6b** [HELD]: hint's p95 exceeds 100 us in 6/8 mixes with >= 7 CPU-bound processes (needed at least one).

## Violations

- M1': `mix2:1;1;3`
- M1': `mix2:2;3;3`
- M1': `mix2:4;3;3`
- M1': `mix2:5;2;3`
- M1': `mix2:5;3;3`
- M1': `mix2:6;3;3`
- M1': `mix2:7;2;3`
- M1': `mix2:8;1;3`
- M3a': `mix2:5;2;3`
- M3a': `mix2:5;3;3`
- M3a': `mix2:6;3;3`
- M3a': `mix2:8;1;3`
- M6a: `mix2:1;1;3`
- M6a: `mix2:2;3;3`
- M6a: `mix2:4;3;3`
- M6a: `mix2:5;2;3`
- M6a: `mix2:5;3;3`

## Starvation check (worst interactive response time, ms, any seed)

-    164.0 ms  `mix2:8;3;2` hint (hints=wrong)
-    162.6 ms  `mix2:8;1;3` hint (hints=wrong)
-    161.6 ms  `mix2:8;0;2` hint (hints=wrong)
-    161.6 ms  `mix2:8;2;1` hint (hints=wrong)
-    159.6 ms  `mix2:8;0;1` hint (hints=wrong)
-    149.7 ms  `mix2:5;0;2` hint (hints=wrong)
-    143.6 ms  `mix2:7;2;3` hint (hints=wrong)
-    142.6 ms  `mix2:7;2;2` hint (hints=wrong)
-    142.6 ms  `mix2:7;3;1` hint (hints=wrong)
-    126.0 ms  `mix2:4;3;3` hint (hints=wrong)

## Per-mix table: interactive p95 (us), by policy

| mix | rr | mlfq | fair | hint | hfair | hfair16 | hint wrong | hfair wrong |
|---|---|---|---|---|---|---|---|---|
| 1;1;3 | 14814 | 2040 | 10 | 166 | 326 | 326 | 107002 | 13 |
| 1;2;2 | 7126 | 4131 | 3 | 2 | 3 | 165 | 108488 | 3 |
| 1;3;0 | - | - | - | - | - | - | - | - |
| 2;0;2 | 7488 | 5001 | 3 | 2 | 3 | 3 | 107325 | 3 |
| 2;2;0 | - | - | - | - | - | - | - | - |
| 2;3;0 | - | - | - | - | - | - | - | - |
| 2;3;2 | 21951 | 15195 | 4 | 3 | 4 | 4 | 110491 | 4 |
| 2;3;3 | 21958 | 16863 | 329 | 489 | 329 | 329 | 110492 | 31 |
| 4;1;0 | - | - | - | - | - | - | - | - |
| 4;3;1 | 39469 | 45898 | 4 | 3 | 4 | 4 | 108001 | 5 |
| 4;3;3 | 41962 | 40795 | 330 | 491 | 168 | 168 | 111491 | 33 |
| 5;0;2 | 37490 | 29164 | 4 | 2 | 4 | 4 | 137003 | 4 |
| 5;2;0 | - | - | - | - | - | - | - | - |
| 5;2;3 | 48140 | 46537 | 330 | 491 | 168 | 168 | 140001 | 189 |
| 5;3;1 | 49474 | 57157 | 5 | 3 | 5 | 5 | 141001 | 5 |
| 5;3;3 | 51970 | 53129 | 169 | 492 | 169 | 169 | 142001 | 192 |
| 6;0;2 | 47490 | 45001 | 4 | 499 | 4 | 4 | 107493 | 4 |
| 6;3;0 | - | - | - | - | - | - | - | - |
| 6;3;3 | 61972 | 56985 | 169 | 496 | 6 | 6 | 111492 | 194 |
| 7;2;2 | 67144 | 49831 | 6 | 331 | 6 | 6 | 129491 | 6 |
| 7;2;3 | 68153 | 50987 | 169 | 663 | 6 | 6 | 130492 | 33 |
| 7;3;1 | 69482 | 48812 | 6 | 4 | 6 | 6 | 128001 | 6 |
| 8;0;1 | 65001 | 45002 | 5 | 3 | 5 | 5 | 145003 | 5 |
| 8;0;2 | 67491 | 45002 | 5 | 500 | 5 | 5 | 147496 | 5 |
| 8;1;0 | - | - | - | - | - | - | - | - |
| 8;1;3 | 73322 | 48816 | 169 | 664 | 6 | 6 | 149492 | 181 |
| 8;2;0 | - | - | - | - | - | - | - | - |
| 8;2;1 | 74650 | 43669 | 6 | 2670 | 6 | 6 | 147001 | 6 |
| 8;3;0 | - | - | - | - | - | - | - | - |
| 8;3;2 | 81980 | 45610 | 6 | 332 | 7 | 7 | 150493 | 7 |
