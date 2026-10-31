# Who pays for a wrong compiler hint? (F19)

p95 wake-up latency in microseconds, mean over 3 seeds. 'neighbour' = p_int (a real interactive program); 'own' = the mislabelled corpus program. Hint source: c = counted analysis, s = safe.

**Y1** [FAILED]: LIGHT programs, counted (too-light) hint, under hint: neighbour p95 > 1 ms -- h018 15, h074 178, h085 3148, h117 1, h126 1859
**Y2** [HELD]: HEAVY programs, safe (too-heavy) hint, under hint: own p95 > 1 ms and neighbour <= 100 us -- h025 own 100001 / nb 1, h050 own 100001 / nb 1, h073 own 100001 / nb 1, h076 own 100001 / nb 1, h082 own 100001 / nb 1
**Y3** [FAILED]: under hfair, neighbour p95 <= 100 us in 16/20 cells: misses h074_c 537, h074_s 537, h126_c 1861, h126_s 1241

| group | program | hint source | its hint | policy | neighbour p95 | own p95 |
|---|---|---|---|---|---|---|
| LIGHT | h018 | c | interactive | hint | 15 | 1 |
| LIGHT | h018 | s | io | hint | 1 | 1 |
| LIGHT | h018 | c | interactive | hfair | 44 | 1335 |
| LIGHT | h018 | s | io | hfair | 44 | 1335 |
| LIGHT | h074 | c | interactive | hint | 178 | 1 |
| LIGHT | h074 | s | io | hint | 1 | 11 |
| LIGHT | h074 | c | interactive | hfair | 537 | 2 |
| LIGHT | h074 | s | io | hfair | 537 | 2 |
| LIGHT | h085 | c | interactive | hint | 3148 | 1 |
| LIGHT | h085 | s | cpu | hint | 1 | 100001 |
| LIGHT | h085 | c | interactive | hfair | 2 | 2 |
| LIGHT | h085 | s | cpu | hfair | 2 | 2 |
| LIGHT | h117 | c | interactive | hint | 1 | 178 |
| LIGHT | h117 | s | cpu | hint | 1 | 33510 |
| LIGHT | h117 | c | interactive | hfair | 2 | 511 |
| LIGHT | h117 | s | cpu | hfair | 2 | 511 |
| LIGHT | h126 | c | interactive | hint | 1859 | 1 |
| LIGHT | h126 | s | cpu | hint | 1 | 100001 |
| LIGHT | h126 | c | interactive | hfair | 1861 | 2 |
| LIGHT | h126 | s | cpu | hfair | 1241 | 12 |
| HEAVY | h025 | c | interactive | hint | 1 | 177 |
| HEAVY | h025 | s | cpu | hint | 1 | 100001 |
| HEAVY | h025 | c | interactive | hfair | 2 | 511 |
| HEAVY | h025 | s | cpu | hfair | 2 | 511 |
| HEAVY | h050 | c | interactive | hint | 29 | 1 |
| HEAVY | h050 | s | cpu | hint | 1 | 100001 |
| HEAVY | h050 | c | interactive | hfair | 2 | 2 |
| HEAVY | h050 | s | cpu | hfair | 30 | 12 |
| HEAVY | h073 | c | interactive | hint | 1 | 1 |
| HEAVY | h073 | s | cpu | hint | 1 | 100001 |
| HEAVY | h073 | c | interactive | hfair | 2 | 2 |
| HEAVY | h073 | s | cpu | hfair | 2 | 2 |
| HEAVY | h076 | c | interactive | hint | 1 | 1 |
| HEAVY | h076 | s | cpu | hint | 1 | 100001 |
| HEAVY | h076 | c | interactive | hfair | 2 | 12 |
| HEAVY | h076 | s | cpu | hfair | 2 | 12 |
| HEAVY | h082 | c | interactive | hint | 1 | 177 |
| HEAVY | h082 | s | cpu | hint | 1 | 100001 |
| HEAVY | h082 | c | interactive | hfair | 2 | 511 |
| HEAVY | h082 | s | cpu | hfair | 2 | 511 |
