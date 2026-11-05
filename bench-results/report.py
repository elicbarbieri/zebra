#!/usr/bin/env python3
"""Render critcmp JSON exports (arm-{a,b,c}-t{1,4}.json) as one markdown table per thread count.

- row = benchmark id (window/replay or window/block/pXX-n); columns = median ms ± CI95/2 per variant, ratios
"""
import json
import sys
from pathlib import Path

ARMS = ["a", "b", "c"]


def load(path):
    out = {}
    for bench in json.loads(Path(path).read_text())["benchmarks"].values():
        key = bench["fullname"].split("/", 1)[1]
        med = bench["criterion_estimates_v1"]["median"]
        lo, hi = med["confidence_interval"]["lower_bound"], med["confidence_interval"]["upper_bound"]
        out[key] = (med["point_estimate"] / 1e6, (hi - lo) / 2e6)
    return out


def fmt(ms):
    return f"{ms:,.0f}" if ms >= 100 else f"{ms:,.2f}"


def main(results_dir):
    for threads in (1, 4):
        runs = {a: load(p) for a in ARMS if (p := Path(results_dir) / f"arm-{a}-t{threads}.json").exists()}
        if not runs:
            continue
        arms = list(runs)
        pairs = [(x, y) for i, x in enumerate(arms) for y in arms[i + 1:]]
        print(f"\n### RAYON_NUM_THREADS={threads} (median ms ± CI95/2)\n")
        head = ["benchmark"] + [f"{a.upper()}" for a in arms] + [f"{x.upper()}/{y.upper()}" for x, y in pairs]
        print("| " + " | ".join(head) + " |")
        print("|---|" + "---:|" * (len(head) - 1))
        for key in sorted({k for r in runs.values() for k in r}):
            vals = {a: runs[a].get(key) for a in arms}
            row = [key]
            row += [f"{fmt(v[0])} ± {fmt(v[1])}" if v else "—" for v in vals.values()]
            row += [f"{vals[x][0] / vals[y][0]:.2f}×" if vals[x] and vals[y] else "—" for x, y in pairs]
            print("| " + " | ".join(row) + " |")


if __name__ == "__main__":
    main(sys.argv[1] if len(sys.argv) > 1 else "/home/elicb/DevProjects/ZingoLabs/bench-results")
