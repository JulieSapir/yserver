//! `split globs`: the explicit imports that replace a file's `use super::*`
//! (with `--down`, its private `use child::*`); see
//! docs/superpowers/plans/2026-10-12-explicit-imports.md.
//!
//! Every name the file's leaves use (free names, impl headers, macro
//! invocations, first segments of the file's own relative `use` paths) that
//! the glob supplies is imported by a crate-absolute path: an item by its
//! defining module, an import by the importing module's path and alias.
//! Every trait the glob supplies that the file does not name is imported
//! `as _`. The output over-approximates on purpose (a local binding named
//! like an ancestor item, a trait never called): rustc's unused-import
//! report prunes it (`tools/split/prune-unused-imports.py`). Not trusted:
//! rustc is the check.

use std::{
    cell::RefCell,
    collections::{BTreeMap, BTreeSet},
};

use crate::{
    scope::{self, MACRO, TYPE, VALUE, admits, free_names, invocations, scope},
    tree::{self, Disk, Res, Tree, UseItem},
};

fn parent(m: &str) -> &str {
    m.rsplit_once("::").map_or("", |(p, _)| p)
}

fn segs(m: &str) -> Vec<String> {
    m.split("::")
        .filter(|s| !s.is_empty())
        .map(str::to_string)
        .collect()
}

fn abs(module: &str, rest: &[String]) -> String {
    let mut v = vec!["crate".to_string()];
    v.extend(segs(module));
    v.extend(rest.iter().cloned());
    v.join("::")
}

fn item_ns(kind: &str) -> Option<u8> {
    match kind {
        "fn" | "const" | "static" => Some(VALUE),
        "struct" => Some(TYPE | VALUE),
        "enum" | "union" | "trait" | "type" | "extern_crate" => Some(TYPE),
        _ => None,
    }
}

/// `#[cfg…]` attributes of a parsed item or of a `UseItem::attrs` string.
fn cfgs(attrs: &[syn::Attribute]) -> String {
    attrs
        .iter()
        .filter(|a| a.path().is_ident("cfg") || a.path().is_ident("cfg_attr"))
        .map(|a| format!("{}\n", tree::norm(&tree::tok(a))))
        .collect()
}

fn use_cfgs(u: &UseItem) -> String {
    syn::parse_str::<syn::ItemUse>(&format!("{} use x;", u.attrs))
        .map(|i| cfgs(&i.attrs))
        .unwrap_or_default()
}

#[derive(Clone, PartialEq, Eq, PartialOrd, Ord)]
struct Cand {
    /// `crate::a::B`, `std::x::Y as Z`.
    path: String,
    attrs: String,
    def: bool,
}

struct G<'a> {
    t: &'a Tree,
    /// Module-level leaves by module and name; `use` items by module.
    items: BTreeMap<(String, String), Vec<usize>>,
    by_module: BTreeMap<String, Vec<usize>>,
    busy: RefCell<BTreeSet<(String, String)>>,
    opaque: RefCell<BTreeSet<String>>,
}

impl<'a> G<'a> {
    fn new(t: &'a Tree) -> Self {
        let mut items: BTreeMap<(String, String), Vec<usize>> = BTreeMap::new();
        for l in t.leaves.iter().filter(|l| l.owner.is_none()) {
            items
                .entry((l.module.clone(), l.name.clone()))
                .or_default()
                .push(l.idx);
        }
        let mut by_module: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (i, u) in t.uses.iter().enumerate() {
            by_module.entry(u.module.clone()).or_default().push(i);
        }
        G {
            t,
            items,
            by_module,
            busy: RefCell::default(),
            opaque: RefCell::default(),
        }
    }

    fn uses<'b>(&'b self, m: &str) -> impl Iterator<Item = &'b UseItem> + 'b {
        self.by_module
            .get(m)
            .into_iter()
            .flatten()
            .map(|&i| &self.t.uses[i])
    }

    fn leaves<'b>(&'b self, m: &str, name: &str) -> impl Iterator<Item = &'b tree::Leaf> + 'b {
        self.items
            .get(&(m.to_string(), name.to_string()))
            .into_iter()
            .flatten()
            .map(|&i| &self.t.leaves[i])
    }

    fn mod_vis(&self, m: &str) -> Option<&str> {
        self.t.mods.get(m).map(|i| i.vis.as_str())
    }

    /// In-tree module a glob `use <path>` in `m` reaches.
    fn glob_module(&self, m: &str, path: &str) -> Option<String> {
        let p = path.strip_suffix(" :: *")?;
        let target = self.absolute(m, p).strip_prefix("crate")?.to_string();
        let target = target.trim_start_matches("::").to_string();
        self.t.mods.contains_key(&target).then_some(target)
    }

    /// `path` (spaced `use` tokens, alias included) written in `m`, made
    /// crate-absolute where it starts in the tree; extern paths unchanged.
    fn absolute(&self, m: &str, path: &str) -> String {
        let (p, alias) = path
            .split_once(" as ")
            .map_or((path, String::new()), |(p, a)| (p, format!(" as {a}")));
        if let Some(e) = p.strip_prefix(":: ") {
            return format!("::{}{alias}", e.replace(" :: ", "::"));
        }
        let v: Vec<String> = p.split(" :: ").map(str::to_string).collect();
        let out = match v[0].as_str() {
            "crate" => v.join("::"),
            "self" => abs(m, &v[1..]),
            "super" => {
                let mut base = segs(m);
                let mut i = 0;
                while v.get(i).is_some_and(|s| s == "super") {
                    base.pop();
                    i += 1;
                }
                abs(&base.join("::"), &v[i..])
            }
            s if self.t.mods.contains_key(&tree::child(m, s))
                || self.leaves(m, s).next().is_some() =>
            {
                abs(m, &v)
            }
            s => {
                let key = (m.to_string(), s.to_string());
                let found = if self.busy.borrow_mut().insert(key.clone()) {
                    let r = self.lookup(m, s, TYPE, m, &mut Vec::new());
                    self.busy.borrow_mut().remove(&key);
                    r
                } else {
                    Vec::new()
                };
                match found.as_slice() {
                    [one] => {
                        let base = one.path.split(" as ").next().unwrap_or(&one.path);
                        std::iter::once(base.to_string())
                            .chain(v[1..].iter().cloned())
                            .collect::<Vec<_>>()
                            .join("::")
                    }
                    _ => v.join("::"),
                }
            }
        };
        format!("{out}{alias}")
    }

    /// What `name` (namespaces `ns`) is bound to in module `m` as seen
    /// from `viewer`: local items, child modules and named imports first,
    /// then `m`'s globs.
    fn lookup(
        &self,
        m: &str,
        name: &str,
        ns: u8,
        viewer: &str,
        stack: &mut Vec<String>,
    ) -> Vec<Cand> {
        if stack.iter().any(|s| s == m) {
            return Vec::new();
        }
        let mut local = Vec::new();
        for l in self.leaves(m, name) {
            if item_ns(l.kind).is_some_and(|k| k & ns != 0) && admits(&scope(&l.vis, m), viewer) {
                let attrs = syn::parse_str::<syn::Item>(&l.tokens)
                    .map(|i| match i {
                        syn::Item::Fn(i) => cfgs(&i.attrs),
                        syn::Item::Const(i) => cfgs(&i.attrs),
                        syn::Item::Static(i) => cfgs(&i.attrs),
                        syn::Item::Struct(i) => cfgs(&i.attrs),
                        syn::Item::Enum(i) => cfgs(&i.attrs),
                        syn::Item::Union(i) => cfgs(&i.attrs),
                        syn::Item::Trait(i) => cfgs(&i.attrs),
                        syn::Item::Type(i) => cfgs(&i.attrs),
                        _ => String::new(),
                    })
                    .unwrap_or_default();
                local.push(Cand {
                    path: abs(m, &[name.to_string()]),
                    attrs,
                    def: true,
                });
            }
        }
        let sub = tree::child(m, name);
        if ns & TYPE != 0
            && let Some(vis) = self.mod_vis(&sub)
            && admits(&scope(vis, m), viewer)
        {
            local.push(Cand {
                path: abs(&sub, &[]),
                attrs: String::new(),
                def: true,
            });
        }
        for u in self.uses(m).filter(|u| u.name.as_deref() == Some(name)) {
            if admits(&scope(&u.vis, m), viewer) {
                local.push(Cand {
                    path: self.absolute(m, &u.path),
                    attrs: use_cfgs(u),
                    def: false,
                });
            }
        }
        if !local.is_empty() {
            return local;
        }
        stack.push(m.to_string());
        let mut out = Vec::new();
        for u in self
            .uses(m)
            .filter(|u| u.name.is_none() && u.path.ends_with('*'))
        {
            if !admits(&scope(&u.vis, m), viewer) {
                continue;
            }
            match self.glob_module(m, &u.path) {
                Some(t) => out.extend(self.lookup(&t, name, ns, viewer, stack).into_iter().map(
                    |mut c| {
                        // An item behind a module `viewer` cannot name is
                        // imported through this glob's module instead.
                        if c.def && !self.reachable_path(&c.path, viewer) {
                            c.path = abs(m, &[name.to_string()]);
                        }
                        c
                    },
                )),
                None => {
                    self.opaque
                        .borrow_mut()
                        .insert(format!("use {} in {m}", u.path));
                }
            }
        }
        stack.pop();
        out.sort();
        out.dedup_by(|a, b| a.path == b.path);
        out
    }

    /// Every in-tree module on `crate::…` `path` (before its last segment)
    /// that is not an ancestor of `viewer` is visible to `viewer`.
    fn reachable_path(&self, path: &str, viewer: &str) -> bool {
        let v = segs(path.strip_prefix("crate").unwrap_or(path));
        (1..v.len()).all(|i| {
            let m = v[..i].join("::");
            scope::under(viewer, &m)
                || self
                    .mod_vis(&m)
                    .is_none_or(|vis| admits(&scope(vis, parent(&m)), viewer))
        })
    }

    /// Modules whose bindings a glob into `m` can supply, `m` included.
    fn reachable(&self, m: &str, viewer: &str, out: &mut Vec<String>) {
        if out.iter().any(|x| x == m) {
            return;
        }
        out.push(m.to_string());
        for u in self
            .uses(m)
            .filter(|u| u.name.is_none() && u.path.ends_with('*'))
        {
            if admits(&scope(&u.vis, m), viewer)
                && let Some(t) = self.glob_module(m, &u.path)
            {
                self.reachable(&t, viewer, out);
            }
        }
    }
}

pub struct FileImports {
    pub file: String,
    /// The glob is the file's own (column 0), not an inline module's.
    pub col0: bool,
    pub glob: String,
    pub imports: Vec<String>,
    pub names: usize,
    pub traits: usize,
    pub ambiguous: Vec<String>,
}

fn is_test_module(module: &str) -> bool {
    module
        .split("::")
        .any(|s| s == "tests" || (s.ends_with("_tests") && s != "for_tests"))
}

/// Imports for every non-test file under `root` (module `module`) holding a
/// column-0 `use super::*;` (`down`: private `use <child>::*;`), skipping
/// the `skip` modules and everything under them.
pub fn generate(
    t: &Tree,
    down: bool,
    skip: &[String],
    src: &dyn tree::Source,
) -> Res<Vec<FileImports>> {
    let g = G::new(t);
    let mut out = Vec::new();
    for (file, m) in &t.file_module {
        if is_test_module(m) || skip.iter().any(|s| scope::under(m, s)) {
            continue;
        }
        let text = String::from_utf8(src.read(file).ok_or_else(|| format!("{file}: not found"))?)
            .map_err(|e| e.to_string())?;
        let globs: Vec<(String, String)> = if down {
            g.uses(m)
                .filter(|u| u.vis.is_empty() && u.attrs.is_empty() && u.path.ends_with(" :: *"))
                .filter(|u| !u.path.starts_with("super") && !u.path.starts_with("crate"))
                .filter_map(|u| {
                    let line = format!("use {}::*;", u.path.trim_end_matches(" :: *"));
                    let t = g.glob_module(m, &u.path)?;
                    (text.lines().any(|l| l == line)).then_some((line, t))
                })
                .collect()
        } else if text.lines().any(|l| l == "use super::*;") {
            vec![("use super::*;".to_string(), parent(m).to_string())]
        } else {
            Vec::new()
        };
        for (line, target) in globs {
            out.push(one_file(&g, file, m, &line, &target));
        }
    }
    let opaque = g.opaque.borrow();
    if !opaque.is_empty() {
        eprintln!("opaque globs on the lookup path: {opaque:?}");
    }
    Ok(out)
}

/// What the glob `line` (into `target`) supplies to the leaves `leaf` of
/// module `m`, keyed by name (`as _` imports by their text), and the names
/// it supplies ambiguously.
fn glob_imports(
    g: &G,
    m: &str,
    line: &str,
    target: &str,
    leaf: &dyn Fn(&tree::Leaf) -> bool,
) -> (BTreeMap<String, String>, Vec<String>) {
    let t = g.t;
    let mut names: BTreeMap<String, u8> = BTreeMap::new();
    for l in t.leaves.iter().filter(|l| leaf(l)) {
        for (n, ns) in free_names(&l.tokens, &l.module).0 {
            *names.entry(n).or_default() |= ns;
        }
        if let Some(o) = &l.owner {
            for (n, ns) in free_names(&o.header, &l.module).0 {
                *names.entry(n).or_default() |= ns;
            }
        }
        for n in invocations(&l.tokens) {
            *names.entry(n).or_default() |= MACRO;
        }
    }
    for u in g.uses(m) {
        let first = u.path.split(" :: ").next().unwrap_or_default();
        if !matches!(first, "crate" | "self" | "super" | "" | "*") && !u.path.starts_with(":: ") {
            *names.entry(first.to_string()).or_default() |= TYPE;
        }
    }
    let globbed = |u: &UseItem| u.name.is_none() && u.path.ends_with('*');
    let own_line = |u: &UseItem| {
        u.path
            .strip_suffix(" :: *")
            .is_some_and(|p| line == format!("use {}::*;", p.replace(" :: ", "::")))
    };
    let bound_locally = |n: &str| {
        g.leaves(m, n).next().is_some()
            || t.mods.contains_key(&tree::child(m, n))
            || g.uses(m).any(|u| u.name.as_deref() == Some(n))
            || g.uses(m)
                .filter(|u| globbed(u) && !own_line(u))
                .filter_map(|u| g.glob_module(m, &u.path))
                .any(|o| {
                    !g.lookup(&o, n, TYPE | VALUE | MACRO, m, &mut vec![m.to_string()])
                        .is_empty()
                })
    };
    let mut out = BTreeMap::new();
    let mut ambiguous = Vec::new();
    for (n, ns) in &names {
        if bound_locally(n) {
            continue;
        }
        match g
            .lookup(target, n, *ns, m, &mut vec![m.to_string()])
            .as_slice()
        {
            [] => {}
            [one] => {
                out.insert(n.clone(), format!("{}use {};", one.attrs, one.path));
            }
            many => ambiguous.push(format!(
                "{n}: {}",
                many.iter()
                    .map(|c| c.path.as_str())
                    .collect::<Vec<_>>()
                    .join(" | ")
            )),
        }
    }
    let mut mods = Vec::new();
    g.reachable(target, m, &mut mods);
    for x in &mods {
        let mut cands: BTreeSet<String> = t
            .leaves
            .iter()
            .filter(|l| l.owner.is_none() && &l.module == x && l.kind == "trait")
            .map(|l| l.name.clone())
            .collect();
        for u in g.uses(x) {
            match &u.name {
                Some(n) if n.starts_with(char::is_uppercase) => {
                    cands.insert(n.clone());
                }
                None if u.path.ends_with(" as _") && admits(&scope(&u.vis, x), m) => {
                    let a = format!("{}use {};", use_cfgs(u), g.absolute(x, &u.path));
                    out.insert(format!("_ {a}"), a);
                }
                _ => {}
            }
        }
        for n in cands {
            if names.contains_key(&n) || bound_locally(&n) {
                continue;
            }
            if let [one] = g
                .lookup(target, &n, TYPE, m, &mut vec![m.to_string()])
                .as_slice()
            {
                let base = one.path.split(" as ").next().unwrap_or(&one.path);
                let a = format!("{}use {base} as _;", one.attrs);
                out.insert(format!("_ {a}"), a);
            }
        }
    }
    (out, ambiguous)
}

fn one_file(g: &G, file: &str, m: &str, line: &str, target: &str) -> FileImports {
    let (map, ambiguous) = glob_imports(g, m, line, target, &|l| l.file == file);
    let traits = map.keys().filter(|k| k.starts_with("_ ")).count();
    let imports: BTreeSet<String> = map.into_values().collect();
    FileImports {
        file: file.to_string(),
        col0: true,
        glob: line.to_string(),
        names: imports.len() - traits.min(imports.len()),
        traits,
        imports: imports.into_iter().collect(),
        ambiguous,
    }
}

/// Test modules (`use super::*` kept): the imports that their glob supplied
/// in `before` and no longer supplies, or supplies differently, in `after`.
fn test_imports(before: &Tree, after: &Tree) -> Res<Vec<FileImports>> {
    let (gb, ga) = (G::new(before), G::new(after));
    let mut out = Vec::new();
    for m in after.mods.keys().filter(|m| is_test_module(m)) {
        if !ga.uses(m).any(|u| u.path == "super :: *") {
            continue;
        }
        let pred = |l: &tree::Leaf| &l.module == m;
        let (b, amb) = glob_imports(&gb, m, "use super::*;", parent(m), &pred);
        let (a, _) = glob_imports(&ga, m, "use super::*;", parent(m), &pred);
        let add: Vec<String> = b
            .iter()
            .filter(|(k, v)| a.get(*k) != Some(v))
            .map(|(_, v)| v.clone())
            .collect();
        for (k, v) in &a {
            if !b.contains_key(k) && !k.starts_with("_ ") {
                eprintln!("NEW {m}: {v} (supplied after, not before)");
            }
        }
        if add.is_empty() {
            continue;
        }
        let own = after.file_module.iter().find(|(_, x)| *x == m);
        let file = match own {
            Some((f, _)) => f.clone(),
            None => after
                .leaves
                .iter()
                .find(|l| &l.module == m)
                .map(|l| l.file.clone())
                .ok_or_else(|| format!("{m}: no file"))?,
        };
        out.push(FileImports {
            file,
            col0: own.is_some(),
            glob: "use super::*;".into(),
            names: add.len(),
            traits: 0,
            imports: add,
            ambiguous: amb,
        });
    }
    Ok(out)
}

/// `split globs <root> <module> [--down] [--skip <module>]… [--write]`.
pub fn run(args: &[String], repo: &std::path::Path) -> Res<bool> {
    let usage = "usage: split globs <root.rs> <module> [--down | --tests <rev>] [--skip <module>]... [--write]";
    let (mut pos, mut down, mut write, mut skip) = (Vec::new(), false, false, Vec::new());
    let mut tests = None;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "--down" => down = true,
            "--write" => write = true,
            "--tests" => {
                i += 1;
                tests = Some(args.get(i).ok_or(usage)?.clone());
            }
            "--skip" => {
                i += 1;
                skip.push(args.get(i).ok_or(usage)?.clone());
            }
            s if !s.starts_with('-') => pos.push(s.to_string()),
            _ => return Err(usage.into()),
        }
        i += 1;
    }
    let [root, module] = pos.as_slice() else {
        return Err(usage.into());
    };
    let src = Disk(repo.to_path_buf());
    let t = tree::load(&src, root, module)?;
    let files = match &tests {
        Some(rev) => test_imports(&tree::load(&tree::Git(rev.clone()), root, module)?, &t)?,
        None => generate(&t, down, &skip, &src)?,
    };
    let mut ok = true;
    for f in &files {
        println!(
            "{}: {} — {} names, {} `as _`",
            f.file, f.glob, f.names, f.traits
        );
        for a in &f.ambiguous {
            ok = false;
            println!("  AMBIGUOUS {a}");
        }
        if !write {
            for l in &f.imports {
                println!("  {}", l.replace('\n', " "));
            }
        }
    }
    if write {
        let mut by_file: BTreeMap<&str, Vec<&FileImports>> = BTreeMap::new();
        for f in &files {
            by_file.entry(&f.file).or_default().push(f);
        }
        for (file, fs) in by_file {
            let path = repo.join(file);
            let mut text = std::fs::read_to_string(&path).map_err(|e| e.to_string())?;
            for f in fs {
                let mut lines: Vec<String> = text.lines().map(str::to_string).collect();
                let at: Vec<usize> = (0..lines.len())
                    .filter(|&k| {
                        let l = &lines[k];
                        l.trim_start() == f.glob && (l == &f.glob) == f.col0
                    })
                    .collect();
                let [at] = at[..] else {
                    return Err(format!("{file}: `{}` found {} times", f.glob, at.len()));
                };
                if tests.is_some() {
                    let indent = &lines[at][..lines[at].len() - f.glob.len()].to_string();
                    let add = f.imports.iter().flat_map(|i| {
                        i.lines()
                            .map(|l| format!("{indent}{l}"))
                            .collect::<Vec<_>>()
                    });
                    lines.splice(at + 1..at + 1, add.collect::<Vec<_>>());
                } else if f.imports.is_empty() {
                    lines.remove(at);
                } else {
                    lines[at] = f.imports.join("\n");
                }
                text = lines.join("\n") + "\n";
            }
            std::fs::write(&path, text).map_err(|e| e.to_string())?;
        }
    }
    let n: usize = files.iter().map(|f| f.names).sum();
    let tr: usize = files.iter().map(|f| f.traits).sum();
    println!("{} globs, {n} names, {tr} `as _`", files.len());
    Ok(ok)
}
