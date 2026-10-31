# Hint-classifier accuracy on random PlainLang programs (F17)

150 of 150 programs ran to completion. Measured class from CPU time per wait, thresholds in plans/hint_corpus.txt.

**A1** [HELD]: counted analysis agrees on 76% (balanced accuracy 76%).
**A2** [HELD]: counted 76% vs basic 62% (balanced 63%).
**A3** [HELD]: 2 gross errors (1%); 2 of them from blind-spot shapes.
**A4** [FAILED]: 17 of 36 misses are within 3x of the boundary crossed.

## Confusion matrix, counted analysis (rows: measured, columns: static guess)

| measured \ guess | cpu | io | interactive | none |
|---|---|---|---|---|
| cpu | 10 | 4 | 2 | 5 |
| io | 0 | 33 | 25 | 0 |
| interactive | 0 | 0 | 59 | 0 |
| none | 0 | 0 | 0 | 12 |

## Confusion matrix, basic analysis (rows: measured, columns: static guess)

| measured \ guess | cpu | io | interactive | none |
|---|---|---|---|---|
| cpu | 4 | 4 | 7 | 6 |
| io | 0 | 19 | 39 | 0 |
| interactive | 0 | 1 | 58 | 0 |
| none | 0 | 0 | 0 | 12 |

## Accuracy by loop shape (counted analysis; a program can have two phases)

| shape | programs | correct |
|---|---|---|
| aliased | 36 | 53% |
| always | 100 | 81% |
| count | 34 | 82% |
| function | 43 | 67% |
| gated | 34 | 62% |
| never | 33 | 73% |
| repeat | 32 | 91% |
| while | 31 | 97% |

## Every miss (counted analysis)

| program | shape | guess | measured | CPU ms | waits |
|---|---|---|---|---|---|
| h002.axe | aliased/never | none | cpu | 2.97 | 0 |
| h004.axe | aliased/never | none | cpu | 3.90 | 0 |
| h008.axe | aliased/always+aliased/always | interactive | io | 14.03 | 56 |
| h018.axe | aliased/always | interactive | io | 11.54 | 11 |
| h023.axe | function/always | interactive | io | 8.87 | 15 |
| h024.axe | aliased/never | none | cpu | 33.75 | 0 |
| h033.axe | count/gated | io | cpu | 84.83 | 6 |
| h036.axe | repeat/gated | interactive | io | 1.05 | 10 |
| h037.axe | function/never | none | cpu | 16.37 | 0 |
| h040.axe | count/gated | interactive | io | 0.28 | 1 |
| h043.axe | aliased/never | none | cpu | 4.02 | 0 |
| h046.axe | aliased/always+function/never | io | cpu | 83.95 | 6 |
| h055.axe | function/always | interactive | io | 11.35 | 32 |
| h064.axe | aliased/gated | interactive | io | 1.16 | 9 |
| h065.axe | aliased/always | interactive | io | 7.65 | 29 |
| h074.axe | function/always+count/gated | interactive | io | 59.44 | 40 |
| h075.axe | function/always | interactive | io | 5.82 | 39 |
| h079.axe | while/always+function/always | interactive | io | 41.33 | 50 |
| h085.axe | aliased/never+aliased/gated | interactive | cpu | 20.00 | 3 |
| h086.axe | repeat/gated | interactive | io | 0.31 | 3 |
| h091.axe | aliased/always | interactive | io | 8.44 | 26 |
| h095.axe | function/always | interactive | io | 22.56 | 35 |
| h099.axe | count/gated | interactive | io | 1.24 | 5 |
| h113.axe | count/gated | io | cpu | 30.74 | 5 |
| h117.axe | repeat/gated+aliased/never | interactive | io | 4.69 | 2 |
| h119.axe | function/always | interactive | io | 12.42 | 36 |
| h124.axe | function/always | interactive | io | 2.33 | 10 |
| h126.axe | function/gated | interactive | io | 10.12 | 5 |
| h128.axe | aliased/always | interactive | cpu | 92.64 | 22 |
| h129.axe | aliased/always | interactive | io | 4.32 | 3 |
| h132.axe | count/gated | interactive | io | 1.90 | 13 |
| h136.axe | aliased/gated | interactive | io | 0.42 | 3 |
| h143.axe | function/never+function/always | io | cpu | 97.04 | 9 |
| h145.axe | function/always | interactive | io | 19.05 | 20 |
| h146.axe | aliased/always+aliased/always | interactive | io | 33.62 | 72 |
| h149.axe | aliased/always+function/always | interactive | io | 10.16 | 43 |
