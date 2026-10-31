"""Score plans/asymmetry.txt (F19): who pays for a wrong compiler hint?

    python experiments/asymmetry.py results/release/asymmetry --out report_release/asymmetry.md

For every (corpus program, hint source, policy) it averages over seeds the p95
wake-up latency of the neighbour p_int and of the mislabelled program itself,
then checks predictions Y1-Y3.
"""
import argparse
import collections
import csv
import os
import re

HERE = os.path.dirname(os.path.abspath(__file__))


def groups():
    """{'LIGHT': [...], 'HEAVY': [...]} from the plan's comment lines."""
    g = {}
    for line in open(os.path.join(HERE, "plans", "asymmetry.txt"), encoding="utf-8"):
        for key in ("LIGHT", "HEAVY"):
            if line.startswith(f"# {key}: "):
                g[key] = re.findall(r"(h\d{3}) \(", line)
    return g


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("dir")
    ap.add_argument("--out", required=True)
    a = ap.parse_args()
    by_run = collections.defaultdict(list)
    for r in csv.DictReader(open(os.path.join(a.dir, "results.csv"), encoding="utf-8")):
        by_run[r["run"]].append(r)
    cell = collections.defaultdict(lambda: {"nb": [], "own": [], "own_hint": None})
    for rows in by_run.values():
        prog = next(r for r in rows if r["prog"].startswith("h"))
        nb = next(r for r in rows if r["prog"] == "p_int.axe")
        name, tag = prog["prog"][:-4].split("_")
        c = cell[(name, tag, prog["policy"])]
        c["nb"].append(int(nb["lat_p95"]))
        c["own"].append(int(prog["lat_p95"]))
        c["own_hint"] = prog["hint"]

    def mean(v):
        return sum(v) / len(v) if v else None

    g = groups()
    out = ["# Who pays for a wrong compiler hint? (F19)", "",
           "p95 wake-up latency in microseconds, mean over 3 seeds. 'neighbour' = p_int (a real interactive "
           "program); 'own' = the mislabelled corpus program. Hint source: c = counted analysis, s = safe.", ""]

    def v(name, ok, text):
        out.append(f"**{name}** [{'HELD' if ok else 'FAILED'}]: {text}")

    y1 = [(n, mean(cell[(n, "c", "hint")]["nb"])) for n in g["LIGHT"]]
    v("Y1", all(x is not None and x > 1000 for _, x in y1),
      "LIGHT programs, counted (too-light) hint, under hint: neighbour p95 > 1 ms -- "
      + ", ".join(f"{n} {x:.0f}" for n, x in y1))
    y2 = [(n, mean(cell[(n, "s", "hint")]["own"]), mean(cell[(n, "s", "hint")]["nb"])) for n in g["HEAVY"]]
    v("Y2", all(o is not None and o > 1000 and nb <= 100 for _, o, nb in y2),
      "HEAVY programs, safe (too-heavy) hint, under hint: own p95 > 1 ms and neighbour <= 100 us -- "
      + ", ".join(f"{n} own {o:.0f} / nb {nb:.0f}" for n, o, nb in y2))
    y3 = [(k, mean(c["nb"])) for k, c in cell.items() if k[2] == "hfair"]
    bad = [f"{k[0]}_{k[1]} {x:.0f}" for k, x in y3 if x > 100]
    v("Y3", not bad, f"under hfair, neighbour p95 <= 100 us in {len(y3) - len(bad)}/{len(y3)} cells"
      + (": misses " + ", ".join(bad) if bad else "."))
    out += ["", "| group | program | hint source | its hint | policy | neighbour p95 | own p95 |",
            "|---|---|---|---|---|---|---|"]
    for grp in ("LIGHT", "HEAVY"):
        for n in g[grp]:
            for pol in ("hint", "hfair"):
                for tag in ("c", "s"):
                    c = cell[(n, tag, pol)]
                    out.append(f"| {grp} | {n} | {tag} | {c['own_hint']} | {pol} | {mean(c['nb']):.0f} | "
                               f"{mean(c['own']):.0f} |")
    with open(a.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("\n".join(out) + "\n")
    print("\n".join(out[:9]))


if __name__ == "__main__":
    main()
