"""Score the predictions written in plans/random_mix*_*.txt, mechanically.

    python experiments/mix_summary.py --version v1 results/release/random_mix_{a,b,c} --out report_release/random_mix.md
    python experiments/mix_summary.py --version v2 results/release/random_mix2_{a,b,c} --out report_release/random_mix2.md

v1 = `mix:` workloads (all interactive processes sleep exactly 15 ms, so several
wake in lockstep), v2 = `mix2:` (staggered 13/15/17 ms periods) with predictions
rewritten after v1's failures were traced to that lockstep.

Per (mix, policy, hints) it averages over the seeds: interactive p95 wake-up
latency (pooled over that mix's interactive processes), the interactive
processes' worst RESPONSE time (first dispatch; catches starvation that wake-up
latency cannot see), context switches and makespan. Then it checks every
prediction across all mixes and lists each violation.
"""
import argparse
import collections
import csv
import math
import os

INTERACTIVE = ("wint", "wint2", "wint3")


def load(dirs):
    rows, summ, lat = [], [], collections.defaultdict(list)
    for i, d in enumerate(dirs):
        tag = f"{i}:"
        for r in csv.DictReader(open(os.path.join(d, "results.csv"), encoding="utf-8")):
            r["run"] = tag + r["run"]
            rows.append(r)
        for s in csv.DictReader(open(os.path.join(d, "summary.csv"), encoding="utf-8")):
            s["run"] = tag + s["run"]
            summ.append(s)
        for x in csv.DictReader(open(os.path.join(d, "latency.csv"), encoding="utf-8")):
            lat[(tag + x["run"], x["pid"])].append(int(x["lat_us"]))
    return rows, summ, lat


def p95(v):
    v = sorted(v)
    return v[max(0, math.ceil(0.95 * len(v)) - 1)] if v else None


def dims(w):
    """'mix2:4;3;1' -> (4, 3, 1)"""
    return tuple(int(x) for x in w.split(":")[1].split(";"))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--out", required=True)
    ap.add_argument("--version", choices=("v1", "v2"), default="v2")
    a = ap.parse_args()
    v2 = a.version == "v2"
    rows, summ, lat = load(a.dirs)
    by_run = collections.defaultdict(list)
    for r in rows:
        by_run[r["run"]].append(r)

    cfg = collections.defaultdict(list)
    for s in summ:
        procs = by_run[s["run"]]
        ints = [r for r in procs if r["prog"] in INTERACTIVE]
        samples = [v for r in ints for v in lat.get((s["run"], r["pid"]), [])]
        cfg[(s["workload"], s["policy"], s["hints"])].append({
            "int_p95": p95(samples),
            "int_resp_max": max((int(r["response_us"]) for r in ints), default=None),
            "switches": int(s["ctx_switches"]),
            "makespan": int(s["makespan_us"]),
        })

    def m(mix, pol, hints, key):
        runs = cfg.get((mix, pol, hints))
        if not runs:
            return None
        vals = [r[key] for r in runs if r[key] is not None]
        return sum(vals) / len(vals) if vals else None

    mixes = sorted({k[0] for k in cfg}, key=dims)
    has_int = [w for w in mixes if dims(w)[2] > 0]
    has_both = [w for w in has_int if dims(w)[0] > 0]
    thr = 100 if v2 else 50
    out = [f"# Random-workload validation ({a.version}: "
           f"{'staggered interactive periods, mix2' if v2 else 'lockstep interactive periods, mix'})", "",
           f"{len(mixes)} random mixes (C cpu-bound; W io-style; I interactive), {len(has_int)} with at least one "
           "interactive process. Means over 3 seeds. Latencies in microseconds.", ""]
    viol = []

    def frac(name, cond_list, need, label):
        ok = sum(1 for _, c in cond_list if c)
        verdict = "HELD" if ok >= need * len(cond_list) else "FAILED"
        out.append(f"**{name}** [{verdict}]: {ok}/{len(cond_list)} mixes (needed {need:.0%}). {label}")
        for mix, c in cond_list:
            if not c:
                viol.append((name, mix))

    f_ok = lambda w, p, h: (m(w, p, h, "int_p95") is not None) and m(w, p, h, "int_p95") <= thr  # noqa: E731
    frac("M1" + ("'" if v2 else ""), [(w, f_ok(w, "fair", "on") and f_ok(w, "hfair", "on")) for w in has_int],
         0.9 if v2 else 1.0, f"fair and hfair interactive p95 <= {thr} us.")
    c = []
    for w in mixes:
        sh, sf, sg = (m(w, p, "on", "switches") for p in ("hint", "fair", "hfair"))
        c.append((w, None not in (sh, sf, sg) and sh <= sg + 1 and sg <= sf * 1.05))
    frac("M2", c, 1.0, "hint <= hfair <= 1.05 x fair in context switches.")
    frac("M3a" + ("'" if v2 else ""), [(w, f_ok(w, "hfair", "wrong")) for w in has_int], 0.9 if v2 else 1.0,
         f"hfair with WRONG hints: interactive p95 <= {thr} us.")
    n_bad = sum(1 for w in has_both if (m(w, "hint", "wrong", "int_p95") or 0) > 10000)
    verdict = "HELD" if n_bad >= len(has_both) / 3 else "FAILED"
    out.append(f"**M3b** [{verdict}]: hint with WRONG hints exceeds 10 ms in {n_bad}/{len(has_both)} mixes that have "
               f"both CPU-bound and interactive processes (needed a third).")
    spreads = {}
    for w in mixes:
        ms = [m(w, p, "on", "makespan") for p in ("rr", "mlfq", "fair", "hint", "hfair", "hfair16")]
        ms = [x for x in ms if x]
        spreads[w] = (max(ms) - min(ms)) / min(ms) if ms else 1
    lim = 0.015 if v2 else 0.01
    frac("M4" + ("'" if v2 else ""), [(w, spreads[w] <= lim) for w in mixes], 1.0,
         f"makespan within {lim:.1%} across rr/mlfq/fair/hint/hfair/hfair16 (largest spread {max(spreads.values()):.2%}).")
    if v2:
        c = []
        for w in has_int:
            r_, ml, f = (m(w, p, "on", "int_p95") for p in ("rr", "mlfq", "fair"))
            c.append((w, None not in (r_, ml, f) and r_ > f and ml > f))
        frac("M5'", c, 1.0, "rr's AND mlfq's interactive p95 are worse than fair's.")
        low = [w for w in has_int if dims(w)[0] <= 5]
        high = [w for w in has_int if dims(w)[0] >= 7]
        frac("M6a", [(w, f_ok(w, "hint", "on")) for w in low], 0.9,
             f"hint with correct hints: p95 <= {thr} us in mixes with <= 5 CPU-bound processes.")
        worse = [w for w in high if (m(w, "hint", "on", "int_p95") or 0) > thr]
        verdict = "HELD" if worse else "FAILED"
        out.append(f"**M6b** [{verdict}]: hint's p95 exceeds {thr} us in {len(worse)}/{len(high)} mixes with >= 7 "
                   "CPU-bound processes (needed at least one).")
    else:
        c = []
        for w in has_int:
            r_, ml, f = (m(w, p, "on", "int_p95") for p in ("rr", "mlfq", "fair"))
            c.append((w, None not in (r_, ml, f) and ml >= r_ and ml >= f))
        frac("M5", c, 1.0, "mlfq's interactive p95 is the worst of rr / mlfq / fair.")

    out += ["", "## Violations", ""]
    out += [f"- {n}: `{w}`" for n, w in viol] or ["None."]
    out += ["", "## Starvation check (worst interactive response time, ms, any seed)", ""]
    worst = []
    for w in has_int:
        for p, h in (("rr", "on"), ("fair", "on"), ("hint", "on"), ("hfair", "on"), ("hint", "wrong"), ("hfair", "wrong")):
            r = m(w, p, h, "int_resp_max")
            if r is not None:
                worst.append((r, w, p, h))
    worst.sort(reverse=True)
    out += [f"- {r / 1000:8.1f} ms  `{w}` {p} (hints={h})" for r, w, p, h in worst[:10]]
    cols = (("rr", "on"), ("mlfq", "on"), ("fair", "on"), ("hint", "on"), ("hfair", "on"), ("hfair16", "on"),
            ("hint", "wrong"), ("hfair", "wrong"))
    out += ["", "## Per-mix table: interactive p95 (us), by policy", "",
            "| mix | rr | mlfq | fair | hint | hfair | hfair16 | hint wrong | hfair wrong |", "|---|---|---|---|---|---|---|---|---|"]
    for w in mixes:
        cells = []
        for p, h in cols:
            ip = m(w, p, h, "int_p95")
            cells.append("-" if ip is None else f"{ip:.0f}")
        out.append(f"| {w.split(':')[1]} | " + " | ".join(cells) + " |")
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out) + "\n")
    print("\n".join(out[:14]))


if __name__ == "__main__":
    main()
