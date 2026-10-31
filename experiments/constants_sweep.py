"""Score plans/constants_sweep.txt (K1-K6), mechanically.

    python experiments/constants_sweep.py results/release/constants_sweep --out report_release/constants_sweep.md
"""
import argparse
import collections
import csv
import math
import os

INTERACTIVE = ("wint", "wint2", "wint3")


def load(d):
    lat = collections.defaultdict(list)
    for x in csv.DictReader(open(os.path.join(d, "latency.csv"), encoding="utf-8")):
        lat[(x["run"], x["pid"])].append(int(x["lat_us"]))
    procs = collections.defaultdict(list)
    for r in csv.DictReader(open(os.path.join(d, "results.csv"), encoding="utf-8")):
        procs[r["run"]].append(r)
    cfg = collections.defaultdict(list)
    for s in csv.DictReader(open(os.path.join(d, "summary.csv"), encoding="utf-8")):
        ints = [p for p in procs[s["run"]] if p["prog"] in INTERACTIVE]
        v = sorted(x for p in ints for x in lat[(s["run"], p["pid"])])
        cfg[(s["workload"], s["policy"])].append({
            "p95": v[max(0, math.ceil(0.95 * len(v)) - 1)] if v else None,
            "switches": int(s["ctx_switches"]), "makespan": int(s["makespan_us"])})
    return cfg


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    cfg = load(a.dir)

    def m(w, p, k):
        v = [r[k] for r in cfg.get((w, p), []) if r[k] is not None]
        return sum(v) / len(v) if v else None

    lv = lambda n: "mlfq" if n == 3 else f"mlfq-levels{n}"    # noqa: E731
    fl = lambda n: "fair" if n == 1 else f"fair-floor{n}"     # noqa: E731
    out = ["# The last constants: MLFQ levels and fair's slice floor", "",
           "Means over 5 seeds. Latency = interactive p95 wake-up latency (us).", ""]

    def v(name, ok, text):
        out.append(f"**{name}** [{'HELD' if ok else 'FAILED'}]: {text}")

    k1 = [(w, m(w, lv(1), "p95"), m(w, "rr", "p95")) for w in ("all", "scale10")]
    v("K1", all(x is not None and r and abs(x - r) <= 0.1 * r for _, x, r in k1),
      "levels=1 vs rr, interactive p95: " + ", ".join(f"{w} {x:.0f} vs {r:.0f}" for w, x, r in k1))
    sw = [m("cpu4", lv(n), "switches") for n in range(1, 6)]
    v("K2", all(sw[i] > sw[i + 1] for i in range(4)), "cpu4 switches by levels 1-5: "
      + ", ".join(f"{x:.0f}" for x in sw))
    k3 = [m("all", lv(n), "p95") for n in range(2, 6)]
    base = m("all", lv(3), "p95")
    v("K3", all(abs(x - base) <= 0.2 * base for x in k3), "`all` interactive p95 by levels 2-5: "
      + ", ".join(f"{x:.0f}" for x in k3))
    s1 = m("scale10", fl(1), "switches")
    ratios = {n: m("scale10", fl(n), "switches") / s1 for n in (2, 5)}
    v("K4", all(abs(ratios[n] - 1 / n) <= 0.25 / n for n in ratios),
      "scale10 switches relative to floor=1: " + ", ".join(f"floor={n} {r:.2f}x (predicted {1 / n:.2f})"
                                                            for n, r in ratios.items())
      + f"; floor=3 {m('scale10', fl(3), 'switches') / s1:.2f}x")
    k5 = [(w, n, m(w, fl(n), "p95")) for w in ("all", "scale10") for n in (1, 2, 3, 5)]
    v("K5", all(x is not None and x <= 100 for *_, x in k5), "fair interactive p95 by floor: "
      + ", ".join(f"{w}/{n} {x:.0f}" for w, n, x in k5))

    def spread(ws, pols):
        ms = [m(ws, p, "makespan") for p in pols]
        return (max(ms) - min(ms)) / min(ms)
    spreads = [("all floors", spread("all", [fl(n) for n in (1, 2, 3, 5)])),
               ("scale10 floors", spread("scale10", [fl(n) for n in (1, 2, 3, 5)])),
               ("all levels", spread("all", [lv(n) for n in range(1, 6)])),
               ("cpu4 levels", spread("cpu4", [lv(n) for n in range(1, 6)]))]
    v("K6", all(x <= 0.01 for _, x in spreads), "makespan spread: " + ", ".join(f"{k} {x:.2%}" for k, x in spreads))
    out += ["", "| workload | policy | runs | interactive p95 (us) | switches | makespan (ms) |", "|---|---|---|---|---|---|"]
    for (w, p) in sorted(cfg):
        p95 = m(w, p, "p95")
        out.append(f"| {w} | {p} | {len(cfg[(w, p)])} | {'-' if p95 is None else f'{p95:.0f}'} | "
                   f"{m(w, p, 'switches'):.0f} | {m(w, p, 'makespan') / 1000:.1f} |")
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out) + "\n")
    print("\n".join(out[:10]))


if __name__ == "__main__":
    main()
