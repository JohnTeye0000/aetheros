"""Aggregate AetherOS benchmark results into tables and graphs.

    python experiments/analyze.py results/main [results/sweep ...] --out results/report

Reads results.csv / summary.csv / latency.csv written by aetherbench.py (one or
more result directories, combined), and writes:

  report.md          tables: per configuration, mean +/- 95% CI across seeds
  *.png              graphs

Statistics, stated plainly:
  * Each (workload, policy, hints, q) configuration is run once per seed. The
    unit of replication is the RUN, so means and confidence intervals are
    computed over per-run values (not over pooled processes or samples).
  * 95% CI = mean +/- t * sd / sqrt(n), with the t value for n-1 degrees of
    freedom. With small n this is wide on purpose.
  * Latency percentiles are first computed within each run (over all wake-up
    samples of that process class in that run), then averaged across runs.
  * Jain's fairness index over the CPU shares of the CPU-bound processes in a
    run: 1.0 = perfectly equal, 1/n = one process got everything.
"""

import argparse
import csv
import math
import os
from collections import defaultdict

import matplotlib
matplotlib.use("Agg")
import matplotlib.pyplot as plt  # noqa: E402

POLICY_ORDER = ["rr", "mlfq", "hint", "hmlfq"]
HINT_ORDER = ["on", "none", "wrong"]
# what each program REALLY is (ground truth by construction), whatever hint it carries
CLASS_OF = {"wcpu": "cpu", "wio": "io", "wint": "interactive",
            "p_cpu.axe": "cpu", "p_io.axe": "io", "p_int.axe": "interactive",
            "p_phase.axe": "phase", "p_trick.axe": "io"}
CLASSES = ("cpu", "io", "interactive", "phase")


def workload_name(w):
    """Short names for the PlainLang `files:` workloads."""
    if not w.startswith("files:"):
        return w
    progs = w[len("files:"):].split("+")
    if "p_phase.axe" in progs:
        return "pl_phase"
    if "p_trick.axe" in progs:
        return "pl_trick"
    return "pl_mixed"

# two-sided 95% t critical values, df = 1..30
T95 = [12.706, 4.303, 3.182, 2.776, 2.571, 2.447, 2.365, 2.306, 2.262, 2.228,
       2.201, 2.179, 2.160, 2.145, 2.131, 2.120, 2.110, 2.101, 2.093, 2.086,
       2.080, 2.074, 2.069, 2.064, 2.060, 2.056, 2.052, 2.048, 2.045, 2.042]


def mean_ci(xs):
    n = len(xs)
    if n == 0:
        return float("nan"), float("nan"), 0
    m = sum(xs) / n
    if n == 1:
        return m, float("nan"), 1
    sd = math.sqrt(sum((x - m) ** 2 for x in xs) / (n - 1))
    t = T95[min(n - 1, len(T95)) - 1] if n - 1 <= len(T95) else 1.96
    return m, t * sd / math.sqrt(n), n


def pct(sorted_xs, p):
    if not sorted_xs:
        return float("nan")
    rank = max(1, math.ceil(p / 100 * len(sorted_xs)))
    return sorted_xs[rank - 1]


def read_csv(path):
    if not os.path.exists(path):
        return []
    with open(path, encoding="utf-8") as f:
        return list(csv.DictReader(f))


def load(dirs):
    results, summaries, lat = [], [], defaultdict(list)
    for i, d in enumerate(dirs):
        tag = f"d{i}:"
        for r in read_csv(os.path.join(d, "results.csv")):
            r["run"] = tag + r["run"]
            results.append(r)
        for s in read_csv(os.path.join(d, "summary.csv")):
            s["run"] = tag + s["run"]
            summaries.append(s)
        for row in read_csv(os.path.join(d, "latency.csv")):
            lat[(tag + row["run"], row["pid"])].append(int(row["lat_us"]))
    return results, summaries, lat


def config_key(r):
    return (workload_name(r["workload"]), r["policy"], r["hints"], int(r["q"]))


def per_run_metrics(results, summaries, lat):
    """-> {config: [ {metric: value} per run ]}"""
    by_run = defaultdict(list)
    for r in results:
        by_run[r["run"]].append(r)
    summ = {s["run"]: s for s in summaries}
    out = defaultdict(list)
    for run, procs in by_run.items():
        if run not in summ:
            continue
        s = summ[run]
        m = {"makespan_ms": int(s["makespan_us"]) / 1000,
             "ctx_switches": int(s["ctx_switches"]),
             "sched_us": int(s["sched_us"])}
        for cls in CLASSES:
            members = [p for p in procs if CLASS_OF.get(p["prog"]) == cls]
            if not members:
                continue
            m[f"{cls}_turnaround_ms"] = sum(int(p["turnaround_us"]) for p in members) / len(members) / 1000
            m[f"{cls}_wait_ms"] = sum(int(p["wait_us"]) for p in members) / len(members) / 1000
            m[f"{cls}_response_ms"] = sum(int(p["response_us"]) for p in members) / len(members) / 1000
            samples = sorted(v for p in members for v in lat.get((run, p["pid"]), []))
            if samples:
                for q in (50, 95, 99):
                    m[f"{cls}_lat_p{q}_ms"] = pct(samples, q) / 1000
        cpus = [int(p["cpu_us"]) / max(1, int(p["turnaround_us"]))
                for p in procs if CLASS_OF.get(p["prog"]) == "cpu"]
        if len(cpus) >= 2:
            m["jain_cpu"] = sum(cpus) ** 2 / (len(cpus) * sum(c * c for c in cpus))
        out[config_key(procs[0])].append(m)
    return out


def agg(runs, metric):
    return mean_ci([r[metric] for r in runs if metric in r])


def fmt(m, ci, digits=2):
    if math.isnan(m):
        return "-"
    if math.isnan(ci):
        return f"{m:.{digits}f}"
    return f"{m:.{digits}f} ± {ci:.{digits}f}"


def sort_configs(keys):
    def k(c):
        w, p, h, q = c
        return (w, q, POLICY_ORDER.index(p) if p in POLICY_ORDER else 9,
                HINT_ORDER.index(h) if h in HINT_ORDER else 9)
    return sorted(keys, key=k)


def plausibility(results, summaries):
    """Automated sanity checks: the kind of thing that exposed real bugs in
    this study (devlog 035). Returns a list of human-readable warnings."""
    warn = []
    by_cfg = defaultdict(list)
    for s in summaries:
        by_cfg[(workload_name(s["workload"]), s["policy"], s["hints"], s["q"])].append(s)
    for cfg, runs in by_cfg.items():
        ms = sorted(int(r["makespan_us"]) for r in runs)
        med = ms[len(ms) // 2]
        for r in runs:
            dev = abs(int(r["makespan_us"]) - med) / med
            if dev > 0.05:
                warn.append(f"{cfg}: run {r['run']} (seed {r['seed']}) makespan "
                            f"{int(r['makespan_us']) / 1000:.0f} ms is {dev:.0%} from the median "
                            f"{med / 1000:.0f} ms")
    summ = {s["run"]: s for s in summaries}
    for r in results:
        s = summ.get(r["run"])
        if s and int(r["turnaround_us"]) > int(s["makespan_us"]) * 1.01:
            warn.append(f"run {r['run']} pid {r['pid']}: turnaround exceeds the run's makespan")
        if r["status"] != "0":
            warn.append(f"run {r['run']} pid {r['pid']} ({r['prog']}): exit status {r['status']}")
    return warn


def write_report(data, out, warnings=()):
    lines = ["# AetherOS scheduling experiments — results", "",
             "Values are mean ± 95% CI across runs (one run per seed). Times in ms.",
             "Latency = time from a process being woken to it actually running.", ""]
    metrics = [
        ("n", None),
        ("interactive p95 lat", "interactive_lat_p95_ms"),
        ("interactive p99 lat", "interactive_lat_p99_ms"),
        ("io p95 lat", "io_lat_p95_ms"),
        ("io turnaround", "io_turnaround_ms"),
        ("phase p95 lat", "phase_lat_p95_ms"),
        ("phase turnaround", "phase_turnaround_ms"),
        ("cpu turnaround", "cpu_turnaround_ms"),
        ("makespan", "makespan_ms"),
        ("ctx switches", "ctx_switches"),
        ("sched time (us)", "sched_us"),
        ("Jain (cpu)", "jain_cpu"),
    ]
    workloads = sorted({c[0] for c in data})
    for w in workloads:
        lines += [f"## workload `{w}`", ""]
        cfgs = [c for c in sort_configs(data.keys()) if c[0] == w]
        lines.append("| policy | hints | q | " + " | ".join(m[0] for m in metrics) + " |")
        lines.append("|" + "---|" * (3 + len(metrics)))
        for c in cfgs:
            runs = data[c]
            cells = []
            for label, key in metrics:
                if key is None:
                    cells.append(str(len(runs)))
                else:
                    m, ci, _ = agg(runs, key)
                    cells.append(fmt(m, ci, 3 if key == "jain_cpu" else 2))
            lines.append(f"| {c[1]} | {c[2]} | {c[3]} | " + " | ".join(cells) + " |")
        lines.append("")
    lines += ["## Plausibility checks", ""]
    if warnings:
        lines += [f"{len(warnings)} warning(s):", ""] + [f"- {w}" for w in warnings[:200]]
    else:
        lines += ["No run's makespan deviates more than 5% from its configuration's median; "
                  "no process outlived its run; every process exited with status 0."]
    lines.append("")
    with open(os.path.join(out, "report.md"), "w", encoding="utf-8") as f:
        f.write("\n".join(lines) + "\n")


def bar_by_policy(data, out, metric, ylabel, fname, title, hints="on", log=False):
    workloads = sorted({c[0] for c in data})
    pols = [p for p in POLICY_ORDER if any(c[1] == p for c in data)]
    if not workloads or not pols:
        return
    fig, ax = plt.subplots(figsize=(8, 4.2))
    width = 0.8 / len(pols)
    drew = False
    for j, p in enumerate(pols):
        xs, ms, cis = [], [], []
        for i, w in enumerate(workloads):
            cfg = [c for c in data if c[0] == w and c[1] == p and (c[2] == hints or p in ("rr", "mlfq"))]
            cfg = [c for c in cfg if c[3] == 10] or cfg
            if not cfg:
                continue
            m, ci, _ = agg(data[cfg[0]], metric)
            if math.isnan(m):
                continue
            xs.append(i + j * width)
            ms.append(m)
            cis.append(0 if math.isnan(ci) else ci)
        if xs:
            ax.bar(xs, ms, width, yerr=cis, capsize=3, label=p)
            drew = True
    if not drew:
        plt.close(fig)
        return
    ax.set_xticks([i + width * (len(pols) - 1) / 2 for i in range(len(workloads))])
    ax.set_xticklabels(workloads)
    ax.set_ylabel(ylabel)
    ax.set_title(title)
    if log:
        ax.set_yscale("log")
    ax.legend()
    ax.grid(axis="y", alpha=0.3)
    fig.tight_layout()
    fig.savefig(os.path.join(out, fname), dpi=130)
    plt.close(fig)


def hint_quality_plot(data, out):
    cfgs = [c for c in data if c[1] in ("hint", "hmlfq")]
    if not any(c[2] != "on" for c in cfgs):
        return
    workloads = sorted({c[0] for c in cfgs})
    fig, axes = plt.subplots(1, 2, figsize=(10, 4))
    for ax, metric, label in ((axes[0], "interactive_lat_p95_ms", "interactive p95 latency (ms)"),
                              (axes[1], "cpu_turnaround_ms", "cpu-bound turnaround (ms)")):
        width = 0.8 / 6
        k = 0
        for p in ("hint", "hmlfq"):
            for h in HINT_ORDER:
                xs, ms, cis = [], [], []
                for i, w in enumerate(workloads):
                    c = [c for c in cfgs if c[0] == w and c[1] == p and c[2] == h and c[3] == 10]
                    if not c:
                        continue
                    m, ci, _ = agg(data[c[0]], metric)
                    if math.isnan(m):
                        continue
                    xs.append(i + k * width)
                    ms.append(m)
                    cis.append(0 if math.isnan(ci) else ci)
                if xs:
                    ax.bar(xs, ms, width, yerr=cis, capsize=2, label=f"{p} hints={h}")
                k += 1
        ax.set_xticks([i + width * 2.5 for i in range(len(workloads))])
        ax.set_xticklabels(workloads)
        ax.set_ylabel(label)
        ax.grid(axis="y", alpha=0.3)
    axes[0].set_yscale("log")
    axes[0].legend(fontsize=7)
    fig.suptitle("What happens when the hints are missing or wrong")
    fig.tight_layout()
    fig.savefig(os.path.join(out, "hint_quality.png"), dpi=130)
    plt.close(fig)


def sweep_plot(data, out, prefix, xkey, fname, title, xlabel):
    """Line plot of interactive p95 latency vs workload size or quantum."""
    pts = defaultdict(list)
    for c, runs in data.items():
        w, p, h, q = c
        if h not in ("on",) and p in ("hint", "hmlfq"):
            continue
        if xkey == "scale":
            if not w.startswith("scale"):
                continue
            x = int(w[5:])
        else:
            if w != prefix:
                continue
            x = q
        m, ci, _ = agg(runs, "interactive_lat_p95_ms")
        if not math.isnan(m):
            pts[p].append((x, m, 0 if math.isnan(ci) else ci))
    if not pts or all(len(v) < 2 for v in pts.values()):
        return
    fig, ax = plt.subplots(figsize=(7, 4))
    for p in POLICY_ORDER:
        if p in pts:
            v = sorted(pts[p])
            ax.errorbar([a for a, _, _ in v], [b for _, b, _ in v], yerr=[c for _, _, c in v],
                        marker="o", capsize=3, label=p)
    ax.set_xlabel(xlabel)
    ax.set_ylabel("interactive p95 wake-up latency (ms)")
    ax.set_yscale("log")
    ax.set_title(title)
    ax.grid(alpha=0.3)
    ax.legend()
    fig.tight_layout()
    fig.savefig(os.path.join(out, fname), dpi=130)
    plt.close(fig)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dirs", nargs="+")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    os.makedirs(a.out, exist_ok=True)
    results, summaries, lat = load(a.dirs)
    data = per_run_metrics(results, summaries, lat)
    data_q10 = {c: v for c, v in data.items()
                if not c[0].startswith("scale") and not c[0].startswith("pl_")}
    data_pl = {c: v for c, v in data.items() if c[0].startswith("pl_")}
    warnings = plausibility(results, summaries)
    write_report(data, a.out, warnings)
    bar_by_policy(data_q10, a.out, "interactive_lat_p95_ms", "ms (log scale)", "interactive_latency.png",
                  "Interactive process: p95 wake-up latency", log=True)
    bar_by_policy(data_q10, a.out, "io_lat_p95_ms", "ms (log scale)", "io_latency.png",
                  "I/O-bound process: p95 wake-up latency", log=True)
    bar_by_policy(data_q10, a.out, "cpu_turnaround_ms", "ms", "cpu_turnaround.png",
                  "CPU-bound processes: mean turnaround (the cost side)")
    bar_by_policy(data_q10, a.out, "ctx_switches", "context switches per run", "ctx_switches.png",
                  "Scheduling overhead: context switches")
    if data_pl:
        bar_by_policy(data_pl, a.out, "interactive_lat_p95_ms", "ms (log scale)", "pl_interactive_latency.png",
                      "PlainLang programs, compiler hints: interactive p95 latency", log=True)
        bar_by_policy(data_pl, a.out, "io_lat_p95_ms", "ms (log scale)", "pl_io_latency.png",
                      "PlainLang programs, compiler hints: I/O p95 latency", log=True)
        bar_by_policy(data_pl, a.out, "phase_lat_p95_ms", "ms (log scale)", "pl_phase_latency.png",
                      "Phase-changing program: p95 latency during its interactive phase", log=True)
        bar_by_policy(data_pl, a.out, "cpu_turnaround_ms", "ms", "pl_cpu_turnaround.png",
                      "PlainLang CPU-bound programs: mean turnaround")
    hint_quality_plot(data, a.out)
    sweep_plot(data, a.out, None, "scale", "scale.png",
               "Latency as CPU-bound competitors are added", "number of CPU-bound processes")
    sweep_plot(data, a.out, "all", "q", "quantum.png",
               "Latency vs base quantum (workload 'all')", "base quantum (ms)")
    print(f"{sum(len(v) for v in data.values())} runs in {len(data)} configurations -> {a.out}")
    print(f"{len(warnings)} plausibility warning(s)" + ("" if not warnings else ":"))
    for w in warnings[:15]:
        print("  " + w)


if __name__ == "__main__":
    main()
