# The last constants: MLFQ levels and fair's slice floor

Means over 5 seeds. Latency = interactive p95 wake-up latency (us).

**K1** [HELD]: levels=1 vs rr, interactive p95: all 9818 vs 9817, scale10 85002 vs 85002
**K2** [FAILED]: cpu4 switches by levels 1-5: 268, 164, 140, 140, 140
**K3** [FAILED]: `all` interactive p95 by levels 2-5: 4415, 6818, 18815, 18815
**K4** [HELD]: scale10 switches relative to floor=1: floor=2 0.50x (predicted 0.50), floor=5 0.21x (predicted 0.20); floor=3 0.34x
**K5** [HELD]: fair interactive p95 by floor: all/1 3, all/2 3, all/3 3, all/5 3, scale10/1 6, scale10/2 6, scale10/3 6, scale10/5 6
**K6** [HELD]: makespan spread: all floors 0.02%, scale10 floors 0.64%, all levels 0.01%, cpu4 levels 0.02%

| workload | policy | runs | interactive p95 (us) | switches | makespan (ms) |
|---|---|---|---|---|---|
| all | fair | 5 | 3 | 420 | 1446.2 |
| all | fair-floor2 | 5 | 3 | 420 | 1446.2 |
| all | fair-floor3 | 5 | 3 | 417 | 1446.2 |
| all | fair-floor5 | 5 | 3 | 362 | 1446.0 |
| all | mlfq | 5 | 6818 | 180 | 1443.5 |
| all | mlfq-levels1 | 5 | 9818 | 210 | 1443.6 |
| all | mlfq-levels2 | 5 | 4415 | 181 | 1443.5 |
| all | mlfq-levels4 | 5 | 18815 | 183 | 1443.5 |
| all | mlfq-levels5 | 5 | 18815 | 183 | 1443.5 |
| all | rr | 5 | 9817 | 200 | 1443.1 |
| cpu4 | mlfq | 5 | - | 140 | 2567.7 |
| cpu4 | mlfq-levels1 | 5 | - | 268 | 2568.2 |
| cpu4 | mlfq-levels2 | 5 | - | 164 | 2567.8 |
| cpu4 | mlfq-levels4 | 5 | - | 140 | 2567.7 |
| cpu4 | mlfq-levels5 | 5 | - | 140 | 2567.7 |
| scale10 | fair | 5 | 6 | 6564 | 6511.2 |
| scale10 | fair-floor2 | 5 | 6 | 3314 | 6485.5 |
| scale10 | fair-floor3 | 5 | 6 | 2216 | 6476.8 |
| scale10 | fair-floor5 | 5 | 6 | 1353 | 6470.0 |
| scale10 | mlfq | 5 | 45001 | 560 | 6443.3 |
| scale10 | mlfq-levels1 | 5 | 85002 | 708 | 6445.2 |
| scale10 | mlfq-levels5 | 5 | 45001 | 560 | 6443.3 |
| scale10 | rr | 5 | 85002 | 706 | 6440.2 |
