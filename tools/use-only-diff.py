#!/usr/bin/env python3
"""Guard for import-only and visibility-only commits; see
docs/superpowers/plans/2026-10-12-explicit-imports.md (section 3).

usage: use-only-diff.py <base> [<head>]   (head: the working tree)
       use-only-diff.py --self-test

1. Every changed .rs file is token-identical before and after once
   module-level `use` items (column 0, with their attribute lines) and
   `pub(...)` are deleted. Indented (fn/block-local) `use` items compare.
2. `drm`: every .rs file with an unqualified `drm::` path binds `drm` to the
   same thing before and after (its own import, else through `use super::*`
   its parent's, else the extern crate).
3. Changed files with `cfg(target_os…)` gates: gate sites, module-level
   imports whose every use is inside a gated item, and `as _` imports (to
   review by eye: the FreeBSD build)."""
import os, re, subprocess, sys

USE = re.compile(r"(pub(\([^)]*\))? )?use ")


def module_uses(text):
    """(start line, end line) of every module-level `use` with its attributes."""
    lines, out, i, attr = text.split("\n"), [], 0, None
    while i < len(lines):
        l = lines[i]
        if l.startswith("#["):
            attr = i if attr is None else attr
        elif USE.match(l):
            j = i
            while not re.search(r";\s*(//.*)?$", lines[j]):
                j += 1
            out.append((i if attr is None else attr, j))
            i, attr = j, None
        else:
            attr = None
        i += 1
    return out


def strip(text):
    lines = text.split("\n")
    drop = {k for s, e in module_uses(text) for k in range(s, e + 1)}
    s = "\n".join(l for k, l in enumerate(lines) if k not in drop)
    return " ".join(re.sub(r"\bpub\((crate|self|super|in [^)]*)\)", " ", s).split())


def flatten(stmt):
    """`use a::{b, c as d, e::{self}}` -> [(bound name, path)]."""
    body = re.sub(r"^\s*(#\[[^\]]*\]\s*)*(pub(\([^)]*\))?)?\s*use\s", "", stmt)
    body = re.sub(r"\s*(::|\{|\}|,)\s*", r"\1", " ".join(body.split())).strip().rstrip(";")

    def walk(prefix, s):
        parts, depth, cur = [], 0, ""
        for ch in s:
            depth += ch == "{"
            depth -= ch == "}"
            if ch == "," and depth == 0:
                parts.append(cur)
                cur = ""
            else:
                cur += ch
        parts.append(cur)
        out = []
        for p in filter(None, parts):
            m = re.match(r"^(.*?)::\{(.*)\}$", p) if p.endswith("}") else None
            if m or p.startswith("{"):
                head, inner = (m.group(1), m.group(2)) if m else ("", p[1:-1])
                out += walk(prefix + head + "::" if head else prefix, inner)
            elif p == "self":
                out.append((prefix.rstrip(":").split("::")[-1], prefix.rstrip(":")))
            else:
                path, _, alias = p.partition(" as ")
                full = prefix + (path if alias else p)
                out.append((alias or full.split("::")[-1], full))
        return out

    return walk("", body)


def module_of(path):
    p = re.sub(r"^crates/[^/]+/src/", "", path)
    p = re.sub(r"(/mod|^lib|^main)\.rs$|\.rs$", "", p)
    return [s for s in p.split("/") if s]


def parent_file(path, read):
    d, b = os.path.split(path)
    name, pdir = (os.path.basename(d), os.path.dirname(d)) if b == "mod.rs" else (b[:-3], d)
    for p in (pdir + ".rs", pdir + "/mod.rs", pdir + "/lib.rs", pdir + "/main.rs"):
        t = read(p)
        if t is not None and re.search(rf"^\s*(pub(\([^)]*\))? )?mod {name};", t, re.M):
            return p
    return None


def drm_binding(path, read):
    text = read(path)
    for s, e in module_uses(text):
        for name, full in flatten("\n".join(text.split("\n")[s:e + 1])):
            if name == "drm":
                segs, mod = full.split("::"), module_of(path)
                if segs[0] == "self":
                    segs = mod + segs[1:]
                elif segs[0] == "super":
                    while segs and segs[0] == "super":
                        mod, segs = mod[:-1], segs[1:]
                    segs = mod + segs
                elif segs[0] == "crate":
                    segs = segs[1:]
                return path.split("/src/")[0] + "::" + "::".join(segs)
    if re.search(r"^use super::\*;", text, re.M):
        p = parent_file(path, read)
        return drm_binding(p, read) if p else "?"
    return "extern"


def gated_lines(lines):
    out, i = set(), 0
    while i < len(lines):
        if re.search(r"#\[cfg\((any\(|all\()?\s*target_os", lines[i].strip()) or \
           re.search(r"#\[cfg\(not\(.*target_os", lines[i]):
            depth, j, opened = 0, i + 1, False
            while j < len(lines):
                depth += lines[j].count("{") - lines[j].count("}")
                opened |= "{" in lines[j]
                out.add(j)
                if (opened and depth <= 0) or (not opened and lines[j].rstrip().endswith(";")):
                    break
                j += 1
        i += 1
    return out


def report_target_os(path, text):
    lines = text.split("\n")
    sites = sum("target_os" in l and "cfg" in l for l in lines)
    if not sites:
        return
    gated, uses = gated_lines(lines), module_uses(text)
    in_use = {k for s, e in uses for k in range(s, e + 1)}
    only, anon = [], []
    for s, e in uses:
        for name, full in flatten("\n".join(lines[s:e + 1])):
            if name == "_":
                anon.append(full)
                continue
            hits = [k for k, l in enumerate(lines) if k not in in_use and re.search(rf"\b{name}\b", l)]
            if hits and all(k in gated for k in hits):
                only.append(name)
    print(f"target_os {path}: {sites} sites; gated-only names {only or 0}; `as _` {anon or 0}")


def git_reader(rev):
    def read(p):
        r = subprocess.run(["git", "show", f"{rev}:{p}"], capture_output=True, text=True)
        return r.stdout if r.returncode == 0 else None
    return read


def disk(p):
    return open(p).read() if os.path.exists(p) else None


def main(base, head):
    before = git_reader(base)
    after = git_reader(head) if head else disk
    rng = [base, head] if head else [base]
    st = subprocess.run(["git", "diff", "--name-status", "--no-renames", *rng, "--", "*.rs"],
                        capture_output=True, text=True, check=True).stdout.split("\n")
    bad = 0
    for line in filter(None, st):
        kind, path = line.split("\t")
        if kind != "M":
            print(f"FAIL {path}: {kind} (only modified files)")
            bad += 1
        elif strip(before(path)) != strip(after(path)):
            print(f"FAIL {path}: tokens differ outside module-level use items and pub(...)")
            bad += 1
        else:
            report_target_os(path, after(path))
    files = subprocess.run(["git", "ls-files", "crates/*.rs"], capture_output=True, text=True).stdout.split()
    drm = 0
    for path in files:
        text = after(path)
        if text and re.search(r"(?<![\w:])drm::", text):
            b, a = drm_binding(path, before), drm_binding(path, after)
            drm += 1
            if a != b:
                print(f"FAIL drm {path}: {b} -> {a}")
                bad += 1
    changed = sum(1 for l in st if l)
    print(f"{changed} files compared, {drm} drm:: files, {bad} failures")
    return 1 if bad else 0


def self_test():
    body = "fn f() -> u32 {{\n    use std::cmp::{} as pick;\n    pick(1, 2)\n}}\n"
    cases = [
        ("use super::*;\n" + body.format("min"), "use crate::a::{B, c};\n" + body.format("min"), True),
        (body.format("min"), body.format("max"), False),
        ("#[cfg(test)]\nuse x::Y;\npub(crate) fn g() {}\n", "pub(super) fn g() {}\n", True),
        ("#[cfg(test)]\nfn g() {}\n", "fn g() {}\n", False),
        ("fn g() { a + b }\n", "fn g() { a - b }\n", False),
    ]
    ok = all((strip(a) == strip(b)) == want for a, b, want in cases)
    ok &= flatten("use crate::{a::{self, B as C}, d};") == [("a", "crate::a"), ("C", "crate::a::B"), ("d", "crate::d")]
    print("self-test", "ok" if ok else "FAILED")
    return 0 if ok else 1


if __name__ == "__main__":
    a = sys.argv[1:]
    sys.exit(self_test() if a == ["--self-test"] else main(a[0], a[1] if len(a) > 1 else None))
