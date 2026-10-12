#!/usr/bin/env python3
"""Regression tests for `xts-vs-baseline.py diff --strict`.

Run: python3 -m unittest tools/test_xts_vs_baseline.py
"""
from __future__ import annotations

import contextlib
import importlib.util
import io
import os
import tempfile
import unittest

_spec = importlib.util.spec_from_file_location(
    "xts_vs_baseline", os.path.join(os.path.dirname(__file__), "xts-vs-baseline.py"))
xvb = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(xvb)

END = "900|1 00:00:00|TCC End\n"


def case(tc: str, path: str, verdicts: dict[str, str]) -> str:
    lines = [f"10|{tc} {path} 00:00:00|TC Start\n"]
    for tp, verdict in verdicts.items():
        lines.append(f"200|{tc} {tp} 00:00:00|TP Start\n")
        lines.append(f"220|{tc} {tp} 0 00:00:00|{verdict}\n")
    lines.append(f"80|{tc} 0 00:00:00|TC End\n")
    return "".join(lines)


class StrictDiff(unittest.TestCase):
    def strict(self, before: str, after: str) -> int:
        with tempfile.TemporaryDirectory() as d:
            paths = []
            for name, text in (("before", before), ("after", after)):
                path = os.path.join(d, name)
                with open(path, "w", encoding="utf-8") as fh:
                    fh.write(text)
                paths.append(path)
            with contextlib.redirect_stdout(io.StringIO()):
                return xvb.cmd_strict(*paths)

    def test_identical_runs_pass(self) -> None:
        run = case("1", "/XI/Foo", {"1": "PASS", "2": "FAIL"}) + END
        self.assertEqual(self.strict(run, run), 0)

    def test_runs_without_test_purposes_are_inconclusive(self) -> None:
        self.assertEqual(self.strict(END, END), 3)

    def test_missing_purpose_fails(self) -> None:
        before = case("1", "/XI/Foo", {"1": "PASS", "2": "PASS"}) + END
        after = case("1", "/XI/Foo", {"1": "PASS"}) + END
        self.assertEqual(self.strict(before, after), 1)

    def test_verdict_change_fails(self) -> None:
        before = case("1", "/XI/Foo", {"1": "PASS"}) + END
        after = case("1", "/XI/Foo", {"1": "FAIL"}) + END
        self.assertEqual(self.strict(before, after), 1)

    def test_unfinished_purpose_is_inconclusive(self) -> None:
        before = case("1", "/XI/Foo", {"1": "PASS"}) + END
        after = ("10|1 /XI/Foo 00:00:00|TC Start\n"
                 "200|1 1 00:00:00|TP Start\n" + END)
        self.assertEqual(self.strict(before, after), 3)


if __name__ == "__main__":
    unittest.main()
