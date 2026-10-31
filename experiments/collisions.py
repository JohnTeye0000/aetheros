"""Score the predictions in plans/collisions.txt (F16), mechanically.

    python experiments/collisions.py results/release/collisions --out report_release/collisions.md

For every (workload, policy) it pools the interactive processes' wake-up
latencies over all seeds and reports p50 / p90 / p95 / p99 and the fraction of
wake-ups delayed by more than 100 us. Then it checks predictions C1-C4.
"""
import argparse
import collections
import csv
import math
import os

INTERACTIVE = ("wint", "wint2", "wint3")
THR_US = 100


def pct(v, q):
    return v[max(0, math.ceil(q * len(v)) - 1)] if v else None


def load(d):
    summ = {s["run"]: s for s in csv.DictReader(open(os.path.join(d, "summary.csv"), encoding="utf-8"))}
    prog = {(r["run"], r["pid"]): r["prog"]
            for r in csv.DictReader(open(os.path.join(d, "results.csv"), encoding="utf-8"))}
    lat = collections.defaultdict(list)
    for x in csv.DictReader(open(os.path.join(d, "latency.csv"), encoding="utf-8")):
        if prog.get((x["run"], x["pid"])) in INTERACTIVE:
            s = summ[x["run"]]
            lat[(s["workload"], s["policy"])].append(int(x["lat_us"]))
    return {k: sorted(v) for k, v in lat.items()}


def stats(v):
    return {"n": len(v), "p50": pct(v, .5), "p90": pct(v, .9), "p95": pct(v, .95), "p99": pct(v, .99),
            "over": sum(1 for x in v if x > THR_US) / len(v)}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    st = {k: stats(v) for k, v in load(a.dir).items() if v}
    out = ["# Wake-up collisions and hint's aging (F16)", "",
           "Interactive wake-up latency pooled over seeds, in microseconds; "
           f"'delayed' = share of wake-ups over {THR_US} us.", ""]

    def verdict(name, ok, text):
        out.append(f"**{name}** [{'HELD' if ok else 'FAILED'}]: {text}")

    bands = {1: (0.0, 0.0), 2: (0.015, 0.05), 3: (0.03, 0.08)}
    pols = ("rr", "mlfq", "fair", "hint", "hfair")
    c1, c1_bad, c2_bad = True, [], []
    for i, (lo, hi) in bands.items():
        w = f"mix2:0;0;{i}"
        fr = []
        for p in pols:
            s = st.get((w, p))
            if s is None:
                c1 = False
                c1_bad.append(f"{w} {p}: no data")
                continue
            fr.append(s["over"])
            p95_ok = s["p95"] > THR_US if i == 3 else s["p95"] <= THR_US
            if not (lo <= s["over"] <= hi) or not p95_ok:
                c1 = False
                c1_bad.append(f"{w} {p}: delayed {s['over']:.1%}, p95 {s['p95']} us")
        if fr and max(fr) - min(fr) > 0.02:
            c2_bad.append(f"{w}: delayed fractions span {min(fr):.1%}-{max(fr):.1%}")
    verdict("C1", c1, "no-competitor floor matches the tick-collision bands under every policy."
            + ("" if c1 else " Misses: " + "; ".join(c1_bad)))
    verdict("C2", not c2_bad, "policies' delayed fractions within 2 points of each other at each I."
            + ("" if not c2_bad else " Misses: " + "; ".join(c2_bad)))
    off = "hint-aging100000"
    c3 = [(w, st.get((f"mix2:{w}", off))) for w in ("6;0;2", "7;2;2", "8;0;2", "8;2;1", "8;3;2")]
    verdict("C3", all(s and s["p95"] <= THR_US for _, s in c3),
            "aging off brings hint's p95 <= 100 us in the <=2-interactive, >=6-CPU mixes: "
            + ", ".join(f"{w} {s['p95'] if s else '-'} us" for w, s in c3))
    c4 = [(w, st.get((f"mix2:{w}", off))) for w in ("1;1;3", "4;3;3", "8;1;3")]
    verdict("C4", all(s and s["p95"] > THR_US for _, s in c4),
            "aging off leaves hint above 100 us in three-interactive mixes: "
            + ", ".join(f"{w} {s['p95'] if s else '-'} us" for w, s in c4))
    out += ["", "| workload | policy | samples | p50 | p90 | p95 | p99 | delayed |", "|---|---|---|---|---|---|---|---|"]
    for (w, p), s in sorted(st.items()):
        out.append(f"| {w} | {p} | {s['n']} | {s['p50']} | {s['p90']} | {s['p95']} | {s['p99']} | {s['over']:.1%} |")
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out) + "\n")
    print("\n".join(out[:9]))


if __name__ == "__main__":
    main()
