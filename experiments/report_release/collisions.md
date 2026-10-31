# Wake-up collisions and hint's aging (F16)

Interactive wake-up latency pooled over seeds, in microseconds; 'delayed' = share of wake-ups over 100 us.

**C1** [HELD]: no-competitor floor matches the tick-collision bands under every policy.
**C2** [HELD]: policies' delayed fractions within 2 points of each other at each I.
**C3** [HELD]: aging off brings hint's p95 <= 100 us in the <=2-interactive, >=6-CPU mixes: 6;0;2 3 us, 7;2;2 4 us, 8;0;2 3 us, 8;2;1 4 us, 8;3;2 4 us
**C4** [HELD]: aging off leaves hint above 100 us in three-interactive mixes: 1;1;3 486 us, 4;3;3 491 us, 8;1;3 493 us

| workload | policy | samples | p50 | p90 | p95 | p99 | delayed |
|---|---|---|---|---|---|---|---|
| mix2:0;0;1 | fair | 150 | 0 | 0 | 0 | 0 | 0.0% |
| mix2:0;0;1 | hfair | 150 | 0 | 0 | 0 | 0 | 0.0% |
| mix2:0;0;1 | hint | 150 | 0 | 0 | 0 | 0 | 0.0% |
| mix2:0;0;1 | mlfq | 150 | 0 | 0 | 0 | 0 | 0.0% |
| mix2:0;0;1 | rr | 150 | 0 | 0 | 0 | 0 | 0.0% |
| mix2:0;0;2 | fair | 300 | 1 | 1 | 1 | 485 | 3.3% |
| mix2:0;0;2 | hfair | 300 | 1 | 1 | 1 | 485 | 3.3% |
| mix2:0;0;2 | hint | 300 | 0 | 0 | 0 | 484 | 3.3% |
| mix2:0;0;2 | mlfq | 300 | 0 | 0 | 0 | 484 | 3.3% |
| mix2:0;0;2 | rr | 300 | 0 | 0 | 0 | 484 | 3.3% |
| mix2:0;0;3 | fair | 450 | 1 | 1 | 485 | 485 | 5.6% |
| mix2:0;0;3 | hfair | 450 | 1 | 1 | 485 | 485 | 5.6% |
| mix2:0;0;3 | hint | 450 | 0 | 1 | 484 | 485 | 5.8% |
| mix2:0;0;3 | mlfq | 450 | 1 | 1 | 484 | 484 | 5.8% |
| mix2:0;0;3 | rr | 450 | 0 | 1 | 484 | 484 | 5.8% |
| mix2:1;1;3 | hint-aging100000 | 270 | 1 | 2 | 486 | 490 | 5.2% |
| mix2:4;3;3 | hint-aging100000 | 270 | 3 | 3 | 491 | 494 | 5.9% |
| mix2:6;0;2 | hint-aging100000 | 180 | 3 | 3 | 3 | 491 | 3.3% |
| mix2:7;2;2 | hint-aging100000 | 180 | 4 | 4 | 4 | 494 | 3.3% |
| mix2:8;0;2 | hint-aging100000 | 180 | 3 | 3 | 3 | 493 | 3.3% |
| mix2:8;1;3 | hint-aging100000 | 270 | 4 | 4 | 493 | 494 | 5.9% |
| mix2:8;2;1 | hint-aging100000 | 90 | 4 | 4 | 4 | 4 | 0.0% |
| mix2:8;3;2 | hint-aging100000 | 180 | 4 | 4 | 4 | 495 | 3.3% |
