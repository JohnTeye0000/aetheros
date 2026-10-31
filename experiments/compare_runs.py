"""Regression check: do runs on a new kernel image match the same runs on the old one?

    python experiments/compare_runs.py results/release/regression_knobs \
        results/release/main_all results/release/fair_main results/release/hfair_main

Matches runs on (workload, policy, q, hints, seed) and compares makespan,
context switches and every process's turnaround and p95 wake-up latency.
Prints the largest relative differences. Under -icount, identical scheduling
decisions give near-identical numbers; a changed decision shows up as a jump.
"""
import collections
import csv
import os
import sys


def load(d):
    summ = {s["run"]: s for s in csv.DictReader(open(os.path.join(d, "summary.csv"), encoding="utf-8"))}
    procs = collections.defaultdict(list)
    for r in csv.DictReader(open(os.path.join(d, "results.csv"), encoding="utf-8")):
        procs[r["run"]].append(r)
    out = {}
    for run, s in summ.items():
        key = (s["workload"], s["policy"], s["q"], s["hints"], s["seed"])
        out[key] = (s, sorted(procs[run], key=lambda p: int(p["pid"])))
    return out


def rel(a, b):
    a, b = float(a), float(b)
    return abs(a - b) / max(abs(b), 1.0)


def main():
    new, olds = sys.argv[1], sys.argv[2:]
    old = {}
    for d in olds:
        old.update(load(d))
    diffs, matched = [], 0
    for key, (s, ps) in load(new).items():
        if key not in old:
            print("no old run for", key)
            continue
        matched += 1
        os_, ops = old[key]
        diffs.append((rel(s["makespan_us"], os_["makespan_us"]), key, "makespan"))
        diffs.append((rel(s["ctx_switches"], os_["ctx_switches"]), key, "switches"))
        for p, q in zip(ps, ops):
            diffs.append((rel(p["turnaround_us"], q["turnaround_us"]), key, f"{p['prog']} turnaround"))
            # latencies are tiny numbers; compare in absolute microseconds
            diffs.append((abs(int(p["lat_p95"]) - int(q["lat_p95"])) / 1000.0, key, f"{p['prog']} p95 (ms abs)"))
    diffs.sort(reverse=True)
    print(f"{matched} runs matched")
    for d, key, what in diffs[:8]:
        print(f"  {d:8.4f}  {what:28s} {key}")


if __name__ == "__main__":
    main()
