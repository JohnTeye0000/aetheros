"""Score the hint classifier against measured behaviour (F17), mechanically.

    python experiments/hint_accuracy.py results/release/hint_corpus --out report_release/hint_accuracy.md

Reads the static hints recorded by gen_hint_corpus.py (plainlang/corpus/hints.csv)
and the kernel's measurements of each program run alone, assigns every program
a MEASURED class with the thresholds in plans/hint_corpus.txt, and checks
predictions A1-A4. It also re-classifies every program with the heavy-biased
`safe` analysis (needs ../plainlang) and checks F18's predictions S1-S4
(plans/hint_safe_predictions.txt) against the same measurements.
"""
import argparse
import collections
import csv
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
CLASSES = ("cpu", "io", "interactive", "none")
INT_MAX_US, IO_MAX_US, CPU_MIN_US = 100, 4000, 2000
BLIND = ("function", "aliased", "gated")
ORDER = {"none": 0, "interactive": 1, "io": 2, "cpu": 3}   # light -> heavy
PLAINLANG = os.path.normpath(os.path.join(HERE, "..", "..", "plainlang"))


def safe_hints(progs):
    sys.path.insert(0, PLAINLANG)
    import plainaxe
    import plainhint
    for name, p in progs.items():
        src = open(os.path.join(HERE, "plainlang", "corpus", name.replace(".axe", ".plang")), encoding="utf-8").read()
        p["safe"] = plainhint.infer(plainaxe.parse(src), "safe")[0]


def direction(p, key):
    """'light' if the guess is lighter than measured, 'heavy' if heavier, '' if right."""
    if p[key] == p["truth"]:
        return ""
    return "light" if ORDER[p[key]] < ORDER[p["truth"]] else "heavy"


def measured(cpu_us, blocks):
    if blocks == 0:
        return "cpu" if cpu_us >= CPU_MIN_US else "none"
    per = cpu_us / blocks
    return "interactive" if per <= INT_MAX_US else "io" if per <= IO_MAX_US else "cpu"


def near_boundary(cpu_us, blocks, guess, truth):
    """Is the measurement within 3x of the boundary between guess and truth?"""
    if blocks == 0 or "none" in (guess, truth):
        edge, val = CPU_MIN_US, cpu_us
    else:
        per = cpu_us / blocks
        pair = {guess, truth}
        edge = INT_MAX_US if pair == {"interactive", "io"} else IO_MAX_US if pair == {"io", "cpu"} else None
        if edge is None:     # interactive <-> cpu skips a class: never "near"
            return False
        val = per
    return edge / 3 <= val <= edge * 3


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    static = {r["prog"]: r for r in csv.DictReader(open(os.path.join(HERE, "plainlang", "corpus", "hints.csv"),
                                                         encoding="utf-8"))}
    progs = {}
    for r in csv.DictReader(open(os.path.join(a.dir, "results.csv"), encoding="utf-8")):
        if r["status"] != "0":
            continue
        cpu, blocks = int(r["cpu_us"]), int(r["blocks"])
        progs[r["prog"]] = {**static[r["prog"]], "cpu": cpu, "blocks": blocks, "truth": measured(cpu, blocks)}
    n = len(progs)
    missing = sorted(set(static) - set(progs))
    out = ["# Hint-classifier accuracy on random PlainLang programs (F17)", "",
           f"{n} of {len(static)} programs ran to completion" + (f" (missing: {', '.join(missing)})" if missing else "")
           + ". Measured class from CPU time per wait, thresholds in plans/hint_corpus.txt.", ""]

    def acc(key):
        return sum(p[key] == p["truth"] for p in progs.values()) / n

    def balanced(key):
        rec = []
        for c in CLASSES:
            members = [p for p in progs.values() if p["truth"] == c]
            if members:
                rec.append(sum(p[key] == c for p in members) / len(members))
        return sum(rec) / len(rec)

    ac, ab = acc("counted"), acc("basic")
    gross = [p for p in progs.values() if {p["counted"], p["truth"]} == {"cpu", "interactive"}]
    blind = [p for p in gross if any(b in p["shape"] for b in BLIND)]
    misses = [p for p in progs.values() if p["counted"] != p["truth"]]
    near = [p for p in misses if near_boundary(p["cpu"], p["blocks"], p["counted"], p["truth"])]

    def v(name, ok, text):
        out.append(f"**{name}** [{'HELD' if ok else 'FAILED'}]: {text}")

    v("A1", ac >= 0.70, f"counted analysis agrees on {ac:.0%} (balanced accuracy {balanced('counted'):.0%}).")
    v("A2", ac - ab >= 0.10, f"counted {ac:.0%} vs basic {ab:.0%} (balanced {balanced('basic'):.0%}).")
    v("A3", len(gross) / n < 0.10 and (not gross or len(blind) * 2 >= len(gross)),
      f"{len(gross)} gross errors ({len(gross) / n:.0%}); {len(blind)} of them from blind-spot shapes.")
    v("A4", not misses or len(near) * 2 >= len(misses),
      f"{len(near)} of {len(misses)} misses are within 3x of the boundary crossed.")

    safe_hints(progs)
    light = {k: [p for p in progs.values() if direction(p, k) == "light"] for k in ("basic", "counted", "safe")}
    heavy = {k: [p for p in progs.values() if direction(p, k) == "heavy"] for k in ("basic", "counted", "safe")}
    asafe = acc("safe")
    out += ["", "## F18: the safety-biased analysis (predictions in plans/hint_safe_predictions.txt)", ""]
    v("S1", len(light["safe"]) <= 9, f"too-light misses: counted {len(light['counted'])}, safe {len(light['safe'])}.")
    v("S2", asafe >= 0.61, f"safe agrees on {asafe:.0%} (balanced {balanced('safe'):.0%}); counted {ac:.0%}.")
    hg = [p for p in heavy["safe"] if "gated" in p["shape"]]
    v("S3", len(heavy["safe"]) >= 5 and len(hg) * 2 >= len(heavy["safe"]),
      f"too-heavy misses under safe: {len(heavy['safe'])} ({len(hg)} with gated waits).")
    gl = [p for p in progs.values() if p["truth"] == "cpu" and p["safe"] == "interactive"]
    v("S4", not gl, f"measured cpu guessed interactive under safe: {len(gl)}.")
    out += ["", "| analysis | agreement | balanced | too light | too heavy |", "|---|---|---|---|---|"]
    for k in ("basic", "counted", "safe"):
        out.append(f"| {k} | {acc(k):.0%} | {balanced(k):.0%} | {len(light[k])} | {len(heavy[k])} |")

    for key in ("counted", "safe", "basic"):
        out += ["", f"## Confusion matrix, {key} analysis (rows: measured, columns: static guess)", "",
                "| measured \\ guess | " + " | ".join(CLASSES) + " |", "|---" * (len(CLASSES) + 1) + "|"]
        cm = collections.Counter((p["truth"], p[key]) for p in progs.values())
        for t in CLASSES:
            out.append(f"| {t} | " + " | ".join(str(cm[(t, g)]) for g in CLASSES) + " |")

    out += ["", "## Accuracy by loop shape (counted analysis; a program can have two phases)", "",
            "| shape | programs | correct |", "|---|---|---|"]
    by = collections.defaultdict(list)
    for p in progs.values():
        for part in set(p["shape"].replace("/", "+").split("+")):
            by[part].append(p["counted"] == p["truth"])
    for k in sorted(by):
        out.append(f"| {k} | {len(by[k])} | {sum(by[k]) / len(by[k]):.0%} |")

    out += ["", "## Every miss (counted analysis)", "", "| program | shape | guess | measured | CPU ms | waits |",
            "|---|---|---|---|---|---|"]
    for p in sorted(misses, key=lambda p: p["prog"]):
        out.append(f"| {p['prog']} | {p['shape']} | {p['counted']} | {p['truth']} | {p['cpu'] / 1000:.2f} | {p['blocks']} |")
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out) + "\n")
    print("\n".join(out[:10]))


if __name__ == "__main__":
    main()
