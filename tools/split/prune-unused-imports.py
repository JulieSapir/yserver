#!/usr/bin/env python3
"""Drop the imports rustc reports unused in every unit that compiles the file
(`cargo check --workspace --lib`, normal and `--profile test`, x three
feature configs); list the ones unused in only some (PARTIAL).

usage: prune-unused-imports.py [--dry-run] [--roots <f.rs,...>] <file-or-dir>...

With --roots, an import unused in every non-test unit but used under
`cfg(test)` is deleted from those root files (decision 2: `split globs
--tests` then moves it into the test modules) and gets `#[cfg(test)]`
anywhere else.

The module-level `use` items of the given files are first exploded to one
path per item, so every unused name is a whole item; `cargo +nightly fmt`
regroups them afterwards. See
docs/superpowers/plans/2026-10-12-explicit-imports.md."""
import importlib.util, json, os, re, subprocess, sys
from collections import defaultdict

FEATURES = [[], ["--features", "tcp-transport"], ["--features", "xdmcp"]]
UNITS = [(p, f) for p in ([], ["--profile", "test"]) for f in FEATURES]
ENV = {k: v for k, v in os.environ.items() if k != "RUSTC_WRAPPER"}
_spec = importlib.util.spec_from_file_location("uod", os.path.join(os.path.dirname(__file__), "..", "use-only-diff.py"))
uod = importlib.util.module_from_spec(_spec)
_spec.loader.exec_module(uod)


def explode(path):
    lines = open(path).read().split("\n")
    out, last = [], 0
    for s, e in uod.module_uses("\n".join(lines)):
        out += lines[last:s]
        block = "\n".join(lines[s:e + 1])
        attrs = re.findall(r"^#\[.*\]$", block, re.M)
        vis = re.search(r"^(pub(\([^)]*\))? )?use ", block, re.M).group(1) or ""
        for name, full in uod.flatten(block):
            alias = "" if full.split("::")[-1] == name or name == "*" else f" as {name}"
            out += attrs + [f"{vis}use {full}{alias};"]
        last = e + 1
    out += lines[last:]
    open(path, "w").write("\n".join(out))


def check(args):
    cmd = ["cargo", "check", "--workspace", "--lib", "--locked", "--message-format=json", *args]
    out = subprocess.run(cmd, capture_output=True, text=True, env=ENV).stdout
    found, errors = {}, []
    for line in out.splitlines():
        m = json.loads(line) if line.startswith("{") else {}
        d = m.get("message") if m.get("reason") == "compiler-message" else None
        if not d:
            continue
        if d["level"] == "error":
            errors.append(d["rendered"])
        elif (d.get("code") or {}).get("code") == "unused_imports":
            for s in d["spans"]:
                found[(s["file_name"], s["line_start"], s["column_start"])] = s["text"][0]["text"].strip()
    return found, errors


def test_only(path):
    """Declared under `#[cfg(test)]` (here or in an enclosing module)."""
    p = uod.parent_file(path, lambda f: open(f).read() if os.path.exists(f) else None)
    if not p:
        return False
    d, b = os.path.split(path)
    name = os.path.basename(d) if b == "mod.rs" else b[:-3]
    lines = open(p).read().split("\n")
    i = next(i for i, l in enumerate(lines) if re.match(rf"\s*(pub(\([^)]*\))? )?mod {name};", l))
    j = i - 1
    while j >= 0 and lines[j].strip().startswith("#["):
        if "cfg(test)" in lines[j].replace(" ", ""):
            return True
        j -= 1
    return test_only(p)


def main():
    args = [a for a in sys.argv[1:] if a != "--dry-run"]
    dry = len(args) < len(sys.argv) - 1
    roots = None
    if "--roots" in args:
        i = args.index("--roots")
        roots = set(args[i + 1].split(","))
        del args[i:i + 2]
    targets = [os.path.join(r, f) for a in args for r, _, fs in (os.walk(a) if os.path.isdir(a) else [("", None, [a])]) for f in fs if f.endswith(".rs")]
    if not dry:
        for t in targets:
            explode(t)
    seen, texts = defaultdict(set), {}
    for i, (p, f) in enumerate(UNITS):
        found, errors = check(p + f)
        if errors:
            print("".join(errors[:20]))
            sys.exit(f"unit {' '.join(p + f) or 'default'}: {len(errors)} errors")
        for k, t in found.items():
            seen[k].add(i)
            texts[k] = t
    drop, gate, partial = defaultdict(set), defaultdict(set), []
    for k, units in seen.items():
        file, line, col = k
        items = {s: e for s, e in uod.module_uses(open(file).read())}
        item = next((s for s, e in items.items() if s <= line - 1 <= e), None)
        tonly = test_only(file)
        need = {3, 4, 5} if tonly else set(range(6))
        if need <= units and item is not None:
            drop[file].add(item)
        elif roots is not None and units == {0, 1, 2} and item is not None and not tonly:
            (drop if file in roots else gate)[file].add(item)
        else:
            partial.append((file, line, sorted(units), texts[k]))
    n = 0
    for file in sorted(set(drop) | set(gate)):
        lines = open(file).read().split("\n")
        spans = {s: e for s, e in uod.module_uses("\n".join(lines))}
        gone = {k for s in drop[file] for k in range(s, spans[s] + 1)}
        n += len(drop[file])
        out = []
        for k, l in enumerate(lines):
            if k in gate[file]:
                out.append("#[cfg(test)]")
                print(f"cfg(test) {file}: {l}")
            if k not in gone:
                out.append(l)
        if not dry:
            open(file, "w").write("\n".join(out))
    if roots is not None:
        for file in sorted(roots & set(drop)):
            print(f"moved out of {file} (test-only): {len(drop[file])} imports")
    print(f"{'would remove' if dry else 'removed'} {n} unused imports in {len(drop)} files")
    for file, line, units, text in sorted(partial):
        print(f"PARTIAL {file}:{line} units {units}: {text}")
    return 1 if partial else 0


if __name__ == "__main__":
    sys.exit(main())
