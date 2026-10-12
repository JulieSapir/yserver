#!/usr/bin/env python3
"""Diff XTS verdicts against a reference (Xorg) baseline.

Real Xorg on a TTY does NOT pass the XTS suites 100% (the XI suite alone
fails ~47/316 cases on stock Xorg — see memory). So the meaningful target
for yserver is *matching Xorg's per-case verdict profile*, not an absolute
pass count. This tool extracts per-(case, test-purpose) verdicts from a TET
journal and diffs a candidate run against a baseline run, surfacing only the
cases where the two disagree — which is the actual yserver bug list.

Usage:
  # Snapshot a reference run (e.g. Xorg on a TTY) into a baseline file:
  tools/xts-vs-baseline.py extract <journal> > baselines/xorg-XI.tsv

  # Diff a candidate run (yserver) against that baseline:
  tools/xts-vs-baseline.py diff baselines/xorg-XI.tsv <yserver-journal>

  # Or diff two journals directly:
  tools/xts-vs-baseline.py diff <baseline-journal> <candidate-journal>

  # Before/after gate for a refactor: both raw journals, complete, the same
  # (case, test purpose) keys and the same verdict for every one:
  tools/xts-vs-baseline.py diff --strict <before-journal> <after-journal>

`diff` exit code is 0 when there are no REGRESSIONs (baseline PASS that the
candidate does not pass), 1 otherwise — so it is usable as a CI gate.
`diff --strict` exits 1 on any verdict difference (either direction) or a
key-set difference, 3 when either journal is incomplete (the run is
inconclusive: rerun it, it is never a pass), 0 only when the two agree on
every purpose.
"""
from __future__ import annotations

import sys


def parse_journal(path: str) -> dict[tuple[str, str], str]:
    """journal -> {(case_path, tp): verdict_string}.

    `10|<tc> /Scenario/Case <time>|TC Start...` binds a tc number to a case
    path; `220|<tc> <tp> <code> <time>|VERDICT` is the verdict for one test
    purpose. We key on (case_path, tp) so results are independent of the
    sequential tc index (which varies with scenario/run ordering).
    """
    tc_to_case: dict[str, str] = {}
    verdicts: dict[tuple[str, str], str] = {}
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("|")
            if len(parts) < 3:
                continue
            code = parts[0]
            if code == "10":
                # parts[1] = "<tc> /Scenario/Case <time>"
                fields = parts[1].split()
                if len(fields) >= 2:
                    tc_to_case[fields[0]] = fields[1]
            elif code == "220":
                fields = parts[1].split()
                if len(fields) >= 2:
                    tc, tp = fields[0], fields[1]
                    case = tc_to_case.get(tc, f"tc{tc}")
                    verdicts[(case, tp)] = parts[2].strip()
    return verdicts


def load(path: str) -> dict[tuple[str, str], str]:
    """Load a baseline TSV (case<TAB>tp<TAB>verdict) or a raw journal."""
    with open(path, encoding="utf-8", errors="replace") as fh:
        head = fh.readline()
    if "\t" in head:
        out: dict[tuple[str, str], str] = {}
        with open(path, encoding="utf-8", errors="replace") as fh:
            for line in fh:
                cols = line.rstrip("\n").split("\t")
                if len(cols) == 3:
                    out[(cols[0], cols[1])] = cols[2]
        return out
    return parse_journal(path)


def cmd_extract(journal: str) -> int:
    v = parse_journal(journal)
    for (case, tp), verdict in sorted(v.items()):
        print(f"{case}\t{tp}\t{verdict}")
    return 0


# Verdicts that count as "the implementation handled it correctly". Anything
# else from the candidate where the baseline PASSed is a regression.
_PASSING = {"PASS", "UNSUPPORTED", "NOTINUSE", "UNTESTED", "NORESULT"}


def journal_gaps(path: str) -> list[str]:
    """Why a raw TET journal is incomplete (empty when it is complete).

    Every TC start (`10`) needs its TC end (`80`), every TP start (`200`) its
    result (`220`), and the journal needs exactly one TCC end (`900`).
    """
    tc_start: dict[str, int] = {}
    tc_end: dict[str, int] = {}
    tp_start: dict[tuple[str, str], int] = {}
    tp_result: dict[tuple[str, str], int] = {}
    ends = 0
    with open(path, encoding="utf-8", errors="replace") as fh:
        for line in fh:
            parts = line.rstrip("\n").split("|")
            if len(parts) < 2:
                continue
            code, fields = parts[0], parts[1].split()
            if code == "900":
                ends += 1
            elif code in ("10", "80") and fields:
                d = tc_start if code == "10" else tc_end
                d[fields[0]] = d.get(fields[0], 0) + 1
            elif code in ("200", "220") and len(fields) >= 2:
                d = tp_start if code == "200" else tp_result
                key = (fields[0], fields[1])
                d[key] = d.get(key, 0) + 1
    gaps = []
    if ends != 1:
        gaps.append(f"{ends} TCC end (900) records, want 1")
    for tc in sorted(tc_start.keys() | tc_end.keys()):
        if tc_start.get(tc, 0) != tc_end.get(tc, 0):
            gaps.append(f"tc {tc}: {tc_start.get(tc, 0)} TC start(s), "
                        f"{tc_end.get(tc, 0)} TC end(s)")
    for key in sorted(tp_start.keys() | tp_result.keys()):
        if tp_start.get(key, 0) != tp_result.get(key, 0):
            gaps.append(f"tc {key[0]} tp {key[1]}: {tp_start.get(key, 0)} TP "
                        f"start(s), {tp_result.get(key, 0)} result(s)")
    return gaps


def is_tsv(path: str) -> bool:
    with open(path, encoding="utf-8", errors="replace") as fh:
        return "\t" in fh.readline()


def cmd_strict(before_path: str, after_path: str) -> int:
    inconclusive = False
    for path in (before_path, after_path):
        if is_tsv(path):
            print(f"{path}: a baseline TSV, not a raw journal; "
                  "--strict needs both journals")
            return 2
        gaps = journal_gaps(path)
        if gaps:
            inconclusive = True
            print(f"INCOMPLETE {path} ({len(gaps)}):")
            for g in gaps[:20]:
                print(f"  {g}")
    if inconclusive:
        print("inconclusive: rerun (an incomplete run is never a pass)")
        return 3
    before = parse_journal(before_path)
    after = parse_journal(after_path)
    only_before = sorted(before.keys() - after.keys())
    only_after = sorted(after.keys() - before.keys())
    differ = sorted(k for k in before.keys() & after.keys()
                    if before[k] != after[k])
    print(f"before={before_path}  after={after_path}")
    print(f"purposes: {len(before)} before, {len(after)} after, "
          f"{len(differ)} verdicts differ")
    for (case, tp) in only_before:
        print(f"  only before: {case} tp{tp} ({before[(case, tp)]})")
    for (case, tp) in only_after:
        print(f"  only after: {case} tp{tp} ({after[(case, tp)]})")
    for (case, tp) in differ:
        print(f"  {case} tp{tp}: before={before[(case, tp)]} "
              f"after={after[(case, tp)]}")
    return 1 if only_before or only_after or differ else 0


def cmd_diff(baseline_path: str, candidate_path: str) -> int:
    base = load(baseline_path)
    cand = load(candidate_path)
    common = sorted(base.keys() & cand.keys())
    only_base = sorted(base.keys() - cand.keys())
    only_cand = sorted(cand.keys() - base.keys())

    regressions = []  # baseline PASS, candidate not -> real yserver bug
    stricter = []     # baseline FAIL, candidate PASS -> we pass what Xorg fails
    other = []        # any other verdict mismatch
    for key in common:
        b, c = base[key], cand[key]
        if b == c:
            continue
        if b == "PASS" and c not in _PASSING:
            regressions.append((key, b, c))
        elif b != "PASS" and c == "PASS":
            stricter.append((key, b, c))
        else:
            other.append((key, b, c))

    def show(title, rows):
        print(f"\n== {title} ({len(rows)}) ==")
        for (case, tp), b, c in rows:
            print(f"  {case} tp{tp}: baseline={b} candidate={c}")

    print(f"baseline={baseline_path}  candidate={candidate_path}")
    print(f"common purposes: {len(common)}  agree: "
          f"{sum(1 for k in common if base[k] == cand[k])}")
    show("REGRESSIONS (baseline PASS, candidate not) — real bug list", regressions)
    show("candidate stricter (baseline !PASS, candidate PASS)", stricter)
    show("other verdict mismatches", other)
    if only_base:
        print(f"\n(only in baseline: {len(only_base)} purposes)")
    if only_cand:
        print(f"(only in candidate: {len(only_cand)} purposes)")
    return 1 if regressions else 0


def main(argv: list[str]) -> int:
    if len(argv) >= 3 and argv[1] == "extract":
        return cmd_extract(argv[2])
    if len(argv) == 5 and argv[1] == "diff" and argv[2] == "--strict":
        return cmd_strict(argv[3], argv[4])
    if len(argv) >= 4 and argv[1] == "diff":
        return cmd_diff(argv[2], argv[3])
    print(__doc__)
    return 2


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
