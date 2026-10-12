//! `split extract` and `split verify --extract`: the arms of one dispatcher
//! `match` become one fn per arm in the same module (phase 2c), and the
//! dispatcher's fn-local items move to module scope first (prep).
//!
//! Generated (`split extract`, untrusted) and checked (`verify --extract`)
//! against `<rev>^`:
//! 1. Dispatcher: old fn = new fn after removing the prelude `let` and the
//!    post-match statements, making the match the tail and replacing each
//!    listed arm body by its call; kept arms equal up to a leading `return`
//!    and trailing `;`, or with the old post-match statements appended.
//! 2. Call shape: same pattern and guard; `callee(a1, …)` with plain ident
//!    arguments equal to the callee's params in order; the callee is new
//!    (absent from every `.rs` of `<rev>^`), in the same file and module,
//!    same cfg context, private, same return type.
//! 3. Binding equivalence (`bind`, bounded): every free identifier of the
//!    arm (and of the tail) names the same binding before and after — an
//!    outer param/`let` ↔ the callee param of that name with the outer type
//!    (the manifest's `locals` for un-annotated `let`s), the prelude `let` ↔
//!    the callee's copy of it, anything else ↔ the same module-scope
//!    resolution (`scope::Names`). Params are exactly the outer names so
//!    resolved, minus the prelude. Arms `bind` refuses must be listed under
//!    `by_hand` (checked without 3).
//! 4. Body: `<arm block>`, `{ <arm block> } <tail>` or `<prelude> { <arm
//!    block> } <tail>`, where `<tail>` is `<tail fn>(…, buf | Vec::new())`
//!    or the old post-match statements; the prelude is present iff the arm
//!    uses its binding; the only extra attribute allowed is
//!    `#[allow(clippy::needless_return)]`, and only on the first form.
//! 5. Tail fn: body = the old post-match statements; params = the outer
//!    names they use, with outer types.
//! 6. Comments: the arm's lead-in and inner comments = the callee's; the
//!    dispatcher keeps the rest.
//! 7. Hoist (prep): the listed fn-local items reappear as module items with
//!    identical tokens; the dispatcher is otherwise unchanged; names that
//!    meant them now resolve to them, every other name as before (other
//!    leaves: the regular per-leaf check, a hoisted name skipped only where
//!    `bind` shows the leaf binds it itself).
//!
//! Also: macro invocations reach the same definitions; no position-sensitive
//! code (`line!`, `Location`, `#[track_caller]`) moves.

use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};

use quote::ToTokens;
use serde::Deserialize;
use syn::{Block, Expr, ExprMatch, FnArg, Item, ItemFn, Pat, Stmt, Type, visit_mut::VisitMut};

use crate::{
    bind,
    scope::{self, Names, VALUE},
    tree::{self, Commas, Leaf, Res, Source, SrcFile, Tree, range, tok},
};

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ExtractManifest {
    /// Root of the module tree loaded for name resolution.
    pub source: String,
    /// Crate-relative module path of `source`.
    pub module: String,
    /// File holding the dispatcher fn.
    pub file: String,
    pub extract: Extract,
    #[serde(skip)]
    pub path: PathBuf,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Extract {
    /// The dispatcher fn (a module-level fn of `file`).
    #[serde(rename = "fn")]
    pub func: String,
    /// The ident the dispatching `match` is on.
    pub scrutinee: String,
    /// Types of the dispatcher's un-annotated `let`s that become params.
    #[serde(default)]
    pub locals: BTreeMap<String, String>,
    /// The `let` before the match that moves into the arms that use it.
    #[serde(default)]
    pub prelude: Option<String>,
    /// New fn holding the post-match statements; arms that fall through
    /// call it. Without one, they get the statements themselves.
    #[serde(default)]
    pub tail: Option<String>,
    /// Prep: fn-local item keys (`split items` style) moved to module scope.
    #[serde(default)]
    pub hoist: Vec<String>,
    /// Arms that stay in the dispatcher (pattern keys).
    #[serde(default)]
    pub keep: Vec<String>,
    /// Pattern key (tokens without whitespace, ` if <guard>` appended) →
    /// new fn.
    #[serde(default)]
    pub arms: BTreeMap<String, String>,
    /// Pattern key → why `bind` refuses it: extracted by hand, reviewed.
    #[serde(default)]
    pub by_hand: BTreeMap<String, String>,
}

impl ExtractManifest {
    pub fn load(path: &Path) -> Res<Self> {
        let text = std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))?;
        let mut m: ExtractManifest =
            toml::from_str(&text).map_err(|e| format!("{}: {e}", path.display()))?;
        m.path = path.to_path_buf();
        if !m.extract.hoist.is_empty() && !m.extract.arms.is_empty() {
            return Err("a manifest either hoists (prep) or extracts arms, not both".into());
        }
        if m.extract.prelude.is_some() && m.extract.tail.is_none() {
            return Err("a prelude needs a tail fn".into());
        }
        Ok(m)
    }
}

/// Token text with the spaces `quote` puts between tokens dropped, except
/// between two identifier characters (`randr as x11randr`).
fn squash(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let word = |x: Option<&char>| x.is_some_and(|x| x.is_alphanumeric() || *x == '_');
    c.iter()
        .enumerate()
        .filter(|(i, ch)| **ch != ' ' || (word(c.get(i.wrapping_sub(1))) && word(c.get(i + 1))))
        .map(|(_, ch)| *ch)
        .collect()
}

/// Key of a fn-local item: `use <tree>` or `<kind> <name>`.
pub fn item_key(i: &Item) -> String {
    match i {
        Item::Use(u) => format!("use {}", squash(&tok(&u.tree))),
        _ => tree::item_parts(i).map_or_else(|| "?".into(), |p| format!("{} {}", p.kind, p.name)),
    }
}

/// Manifest key of an arm: pattern (and ` if <guard>`) tokens, squashed.
pub fn arm_key(a: &syn::Arm) -> String {
    let mut k = squash(&tok(&a.pat));
    if let Some((_, g)) = &a.guard {
        k = format!("{k} if {}", squash(&tok(g)));
    }
    k
}

fn norm_stmt(s: &Stmt) -> String {
    let mut s = s.clone();
    Commas.visit_stmt_mut(&mut s);
    tok(&s)
}

fn norm_stmts(s: &[Stmt]) -> Vec<String> {
    s.iter().map(norm_stmt).collect()
}

fn norm_expr(e: &Expr) -> String {
    let mut e = e.clone();
    Commas.visit_expr_mut(&mut e);
    tok(&e)
}

/// An arm with `body`, normalized (`{ e }` compares as `e`).
fn norm_arm(a: &syn::Arm, body: &Expr) -> String {
    let mut a = a.clone();
    *a.body = body.clone();
    Commas.visit_arm_mut(&mut a);
    tok(&a)
}

fn ident_of(p: &Pat) -> Option<String> {
    match p {
        Pat::Ident(i) if i.by_ref.is_none() && i.subpat.is_none() => Some(i.ident.to_string()),
        _ => None,
    }
}

/// The dispatcher before the change.
struct Old<'a> {
    f: &'a ItemFn,
    /// Params, then the `let`s before the match, in order, with their type
    /// when written (or given by `locals`; `false`: not from the source).
    outer: Vec<(String, Option<Type>, bool)>,
    /// Fn-local items before the match: key → statement index.
    items: BTreeMap<String, usize>,
    item_names: BTreeSet<String>,
    at: usize,
    mat: &'a ExprMatch,
    prelude: Option<usize>,
}

impl Old<'_> {
    fn post(&self) -> &[Stmt] {
        &self.f.block.stmts[self.at + 1..]
    }

    fn outer_names(&self) -> BTreeSet<String> {
        self.outer.iter().map(|(n, ..)| n.clone()).collect()
    }

    fn order(&self, names: &BTreeSet<String>) -> Vec<String> {
        let mut seen = BTreeSet::new();
        self.outer
            .iter()
            .rev()
            .filter(|(n, ..)| names.contains(n) && seen.insert(n.clone()))
            .map(|(n, ..)| n.clone())
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .collect()
    }

    fn ty(&self, n: &str) -> Option<&Type> {
        self.outer
            .iter()
            .rev()
            .find(|(x, ..)| x == n)
            .and_then(|(_, t, _)| t.as_ref())
    }
}

fn find_match<'a>(f: &'a ItemFn, scrutinee: &str) -> Res<(usize, &'a ExprMatch)> {
    let hits: Vec<(usize, &ExprMatch)> = f
        .block
        .stmts
        .iter()
        .enumerate()
        .filter_map(|(i, s)| match s {
            Stmt::Expr(Expr::Match(m), _)
                if matches!(&*m.expr, Expr::Path(p) if p.path.is_ident(scrutinee)) =>
            {
                Some((i, m))
            }
            _ => None,
        })
        .collect();
    match hits.as_slice() {
        [one] => Ok(*one),
        _ => Err(format!(
            "fn {}: {} top-level `match {scrutinee}` statements, need one",
            f.sig.ident,
            hits.len()
        )),
    }
}

fn model<'a>(f: &'a ItemFn, ex: &Extract) -> Res<Old<'a>> {
    let (at, mat) = find_match(f, &ex.scrutinee)?;
    let mut outer = Vec::new();
    for a in &f.sig.inputs {
        let FnArg::Typed(t) = a else {
            return Err("dispatcher with a `self` param".into());
        };
        let n = ident_of(&t.pat).ok_or("dispatcher param that is not a plain ident")?;
        outer.push((n, Some((*t.ty).clone()), true));
    }
    let mut items = BTreeMap::new();
    let mut item_names = BTreeSet::new();
    let mut prelude = None;
    for (i, s) in f.block.stmts[..at].iter().enumerate() {
        match s {
            Stmt::Local(l) => {
                let (n, ty) = match &l.pat {
                    Pat::Type(t) => (ident_of(&t.pat), Some((*t.ty).clone())),
                    p => (ident_of(p), None),
                };
                let n = n.ok_or("dispatcher `let` before the match that is not a plain ident")?;
                let src = ty.is_some();
                let ty = match ty {
                    Some(t) => Some(t),
                    None => ex
                        .locals
                        .get(&n)
                        .map(|t| syn::parse_str::<Type>(t))
                        .transpose()
                        .map_err(|e| format!("locals.{n}: {e}"))?,
                };
                if ex.prelude.as_deref() == Some(n.as_str()) {
                    prelude = Some(i);
                }
                outer.push((n, ty, src));
            }
            Stmt::Item(it) => {
                items.insert(item_key(it), i);
                let mut v = Vec::new();
                item_binds(it, &mut v);
                item_names.extend(v);
            }
            _ => {}
        }
    }
    if ex.prelude.is_some() && prelude.is_none() {
        return Err(format!(
            "prelude `{}` is not a `let` before the match",
            ex.prelude.as_deref().unwrap_or_default()
        ));
    }
    Ok(Old {
        f,
        outer,
        items,
        item_names,
        at,
        mat,
        prelude,
    })
}

/// Names an item binds where it is declared.
fn item_binds(i: &Item, out: &mut Vec<String>) {
    fn walk(t: &syn::UseTree, out: &mut Vec<String>) {
        match t {
            syn::UseTree::Path(p) => walk(&p.tree, out),
            syn::UseTree::Name(n) => out.push(n.ident.to_string()),
            syn::UseTree::Rename(r) => out.push(r.rename.to_string()),
            syn::UseTree::Glob(_) => out.push("*".into()),
            syn::UseTree::Group(g) => g.items.iter().for_each(|i| walk(i, out)),
        }
    }
    match i {
        Item::Use(u) => walk(&u.tree, out),
        _ => {
            if let Some(p) = tree::item_parts(i) {
                out.push(p.name);
            }
        }
    }
}

fn is_diverging_mac(m: &syn::Macro) -> bool {
    m.path.segments.last().is_some_and(|s| {
        matches!(
            s.ident.to_string().as_str(),
            "panic" | "unreachable" | "todo" | "unimplemented"
        )
    })
}

fn diverges_stmts(s: &[Stmt]) -> bool {
    match s.last() {
        Some(Stmt::Expr(e, _)) => diverges(e),
        Some(Stmt::Macro(m)) => is_diverging_mac(&m.mac),
        _ => false,
    }
}

/// Syntactically cannot fall through (the generator's choice of form; the
/// compiler checks it).
fn diverges(e: &Expr) -> bool {
    match e {
        Expr::Return(_) => true,
        Expr::Block(b) => diverges_stmts(&b.block.stmts),
        Expr::Match(m) => !m.arms.is_empty() && m.arms.iter().all(|a| diverges(&a.body)),
        Expr::If(i) => {
            i.else_branch.as_ref().is_some_and(|(_, x)| diverges(x))
                && diverges_stmts(&i.then_branch.stmts)
        }
        Expr::Macro(m) => is_diverging_mac(&m.mac),
        _ => false,
    }
}

fn arm_block(a: &syn::Arm) -> Option<&Block> {
    match &*a.body {
        Expr::Block(b) if b.attrs.is_empty() && b.label.is_none() => Some(&b.block),
        _ => None,
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
enum Form {
    /// The arm cannot fall through: its block is the fn body.
    Return,
    /// `{ <arm> } <tail>` (prelude unused).
    Void,
    /// `<prelude> { <arm> } <tail>`.
    Prelude,
}

/// Outer names a scan uses (value namespace).
fn outer_used(old: &Old, s: &bind::Scan) -> BTreeSet<String> {
    let outer = old.outer_names();
    s.refs
        .iter()
        .filter(|r| r.ns & VALUE != 0 && outer.contains(&r.name))
        .map(|r| r.name.clone())
        .collect()
}

/// Params of the tail fn: the outer names the post-match statements use.
fn tail_params(old: &Old) -> Vec<String> {
    old.order(&outer_used(old, &bind::stmts(old.post())))
}

struct Plan {
    form: Form,
    params: Vec<String>,
}

fn plan(old: &Old, ex: &Extract, block: &Block, by_hand: bool) -> Res<Plan> {
    let scan = bind::block(block);
    if !scan.refused.is_empty() && !by_hand {
        return Err(format!("refused: {}", scan.refused.join("; ")));
    }
    let used = outer_used(old, &scan);
    if let Some(n) = scan.refs.iter().find(|r| old.item_names.contains(&r.name)) {
        return Err(format!(
            "uses fn-local item `{}` of the dispatcher: hoist it first",
            n.name
        ));
    }
    let uses_prelude = ex.prelude.as_ref().is_some_and(|p| used.contains(p));
    let form = if diverges_stmts(&block.stmts) {
        if uses_prelude {
            return Err("uses the prelude binding but cannot fall through".into());
        }
        Form::Return
    } else if uses_prelude {
        Form::Prelude
    } else {
        Form::Void
    };
    let mut want = used;
    if form != Form::Return {
        match &ex.tail {
            Some(_) => want.extend(tail_params(old)),
            None => want.extend(outer_used(old, &bind::stmts(old.post()))),
        }
    }
    if let Some(p) = &ex.prelude {
        want.remove(p);
    }
    Ok(Plan {
        form,
        params: old.order(&want),
    })
}

// ───────────────────────────── generator ─────────────────────────────

/// Lines inside multi-line tokens (string literals): never reindented.
fn frozen(f: &SrcFile) -> Res<BTreeSet<usize>> {
    let mut v = Vec::new();
    tree::spans(f.text.parse().map_err(|e| format!("lex: {e}"))?, &mut v);
    let mut out = BTreeSet::new();
    for (sp, multi) in v {
        if multi {
            out.extend(sp.start().line + 1..=sp.end().line);
        }
    }
    Ok(out)
}

/// `text` (starting at file line `first`) with `n` leading spaces removed
/// from every line but the first and frozen ones.
fn dedent(text: &str, n: usize, first: usize, frozen: &BTreeSet<usize>) -> String {
    text.split('\n')
        .enumerate()
        .map(|(k, line)| {
            if k == 0 || frozen.contains(&(first + k)) {
                line.to_string()
            } else {
                let lead = line.len() - line.trim_start_matches(' ').len();
                line[lead.min(n)..].to_string()
            }
        })
        .collect::<Vec<_>>()
        .join("\n")
}

fn line_start(text: &str, off: usize) -> usize {
    text[..off].rfind('\n').map_or(0, |i| i + 1)
}

fn line_end(text: &str, off: usize) -> usize {
    text[off..].find('\n').map_or(text.len(), |i| off + i + 1)
}

fn indent_at(text: &str, off: usize) -> usize {
    let s = line_start(text, off);
    text[s..].len() - text[s..].trim_start_matches(' ').len()
}

fn find_fn<'a>(ast: &'a syn::File, name: &str) -> Res<(usize, &'a ItemFn)> {
    ast.items
        .iter()
        .enumerate()
        .find_map(|(i, it)| match it {
            Item::Fn(f) if f.sig.ident == name => Some((i, f)),
            _ => None,
        })
        .ok_or_else(|| format!("fn {name} not found"))
}

fn ty_text(f: &SrcFile, t: &Type) -> String {
    let (a, b) = range(f, t.to_token_stream());
    f.text[a..b].to_string()
}

fn sig_params(f: &SrcFile, old: &Old, params: &[String]) -> Res<String> {
    params
        .iter()
        .map(|p| {
            let t = match old.outer.iter().rev().find(|(n, ..)| n == p) {
                Some((_, Some(t), true)) => ty_text(f, t),
                Some((_, Some(t), false)) => tok(t),
                _ => return Err(format!("no type for `{p}`: add it to locals")),
            };
            Ok(format!("    {p}: {t},\n"))
        })
        .collect()
}

/// `split extract <manifest>`: rewrites `file` in the working tree.
pub fn run(m: &ExtractManifest, repo: &Path) -> Res<()> {
    let path = repo.join(&m.file);
    let text = std::fs::read_to_string(&path).map_err(|e| format!("{}: {e}", m.file))?;
    let out = rewrite(m, text)?;
    std::fs::write(&path, out).map_err(|e| format!("{}: {e}", m.file))?;
    println!(
        "{}: rewritten; run cargo +nightly fmt, then verify --extract",
        m.file
    );
    Ok(())
}

/// The text of `file` after `split extract` (unformatted).
pub fn rewrite(m: &ExtractManifest, text: String) -> Res<String> {
    let f = SrcFile::new(text);
    let ast = syn::parse_file(&f.text).map_err(|e| format!("{}: {e}", m.file))?;
    if m.extract.hoist.is_empty() {
        generate(m, &f, &ast)
    } else {
        hoist(m, &f, &ast)
    }
}

fn hoist(m: &ExtractManifest, f: &SrcFile, ast: &syn::File) -> Res<String> {
    let ex = &m.extract;
    let (idx, func) = find_fn(ast, &ex.func)?;
    let old = model(func, ex)?;
    let fz = frozen(f)?;
    let t = &f.text;
    let mut cuts: Vec<(usize, usize)> = Vec::new();
    let (mut uses, mut items) = (Vec::new(), Vec::new());
    for key in &ex.hoist {
        let &i = old
            .items
            .get(key)
            .ok_or_else(|| format!("hoist: no fn-local item {key}"))?;
        let s = &func.block.stmts[i];
        let (a, b) = range(f, s.to_token_stream());
        let prev = if i == 0 {
            f.off(func.block.brace_token.span.open().end())
        } else {
            range(f, func.block.stmts[i - 1].to_token_stream()).1
        };
        let first = prev
            + t[prev..a]
                .find(|c: char| !c.is_whitespace())
                .unwrap_or(a - prev);
        let from = line_start(t, first);
        let to = line_end(t, b);
        let body = dedent(t[from..to].trim_end(), 4, f.line_of(from), &fz);
        let body = body.trim_start().to_string();
        cuts.push((from, to));
        if matches!(s, Stmt::Item(Item::Use(_))) {
            uses.push(body);
        } else {
            items.push(body);
        }
    }
    let fn_start = {
        let (s, _) = range(f, ast.items[idx].to_token_stream());
        let prev = if idx == 0 {
            0
        } else {
            range(f, ast.items[idx - 1].to_token_stream()).1
        };
        let gap = &t[prev..s];
        let first = gap.find(|c: char| !c.is_whitespace()).unwrap_or(gap.len());
        prev + gap[..first].rfind('\n').map_or(0, |i| i + 1)
    };
    let use_at = ast
        .items
        .iter()
        .take(idx)
        .rfind(|i| matches!(i, Item::Use(_)))
        .map(|u| line_end(t, range(f, u.to_token_stream()).1));
    let mut edits: Vec<(usize, usize, String)> = cuts
        .into_iter()
        .map(|(a, b)| (a, b, String::new()))
        .collect();
    let mut block = String::new();
    for i in &items {
        block.push_str(i);
        block.push_str("\n\n");
    }
    let mut ublock = String::new();
    for u in &uses {
        ublock.push_str(u);
        ublock.push('\n');
    }
    match use_at {
        Some(p) => {
            edits.push((p, p, ublock));
            edits.push((fn_start, fn_start, block));
        }
        None => edits.push((
            fn_start,
            fn_start,
            format!(
                "{ublock}{}{block}",
                if ublock.is_empty() { "" } else { "\n" }
            ),
        )),
    }
    Ok(apply_edits(t, edits))
}

fn apply_edits(t: &str, mut edits: Vec<(usize, usize, String)>) -> String {
    edits.sort_by_key(|e| (e.0, e.1));
    let mut out = t.to_string();
    for (a, b, s) in edits.into_iter().rev() {
        out.replace_range(a..b, &s);
    }
    out
}

fn generate(m: &ExtractManifest, f: &SrcFile, ast: &syn::File) -> Res<String> {
    let ex = &m.extract;
    let (idx, func) = find_fn(ast, &ex.func)?;
    let old = model(func, ex)?;
    let fz = frozen(f)?;
    let t = &f.text;
    let ret = match &func.sig.output {
        syn::ReturnType::Type(_, ty) => ty_text(f, ty),
        syn::ReturnType::Default => return Err("dispatcher without a return type".into()),
    };
    let post = old.post();
    let tparams = tail_params(&old);
    let post_text = match (post.first(), post.last()) {
        (Some(a), Some(b)) => {
            let s = line_start(t, range(f, a.to_token_stream()).0);
            t[s..range(f, b.to_token_stream()).1].to_string()
        }
        _ => String::new(),
    };
    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut fns: Vec<String> = Vec::new();
    if let Some(tail) = &ex.tail {
        fns.push(format!(
            "fn {tail}(\n{}) -> {ret} {{\n{post_text}\n}}",
            sig_params(f, &old, &tparams)?
        ));
    }
    if !post.is_empty() {
        let mend = range(f, old.mat.to_token_stream()).1;
        let close = f.off(func.block.brace_token.span.close().start());
        edits.push((mend, close, "\n".into()));
    }
    if let Some(p) = old.prelude {
        let (a, b) = range(f, func.block.stmts[p].to_token_stream());
        edits.push((line_start(t, a), line_end(t, b), String::new()));
    }
    let prelude_text = old.prelude.map(|p| {
        let (a, b) = range(f, func.block.stmts[p].to_token_stream());
        t[a..b].to_string()
    });
    let open = f.off(old.mat.brace_token.span.open().end());
    let mut prev = open;
    let mut seen = BTreeSet::new();
    for arm in &old.mat.arms {
        let (a, b) = range(f, arm.to_token_stream());
        let key = arm_key(arm);
        seen.insert(key.clone());
        let lead_from = prev;
        prev = b;
        let fat = f.off(arm.fat_arrow_token.spans[0].start());
        let pat_text = t[a..fat].trim_end().to_string();
        let ind = indent_at(t, a);
        if let Some(callee) = ex.arms.get(&key) {
            let block = arm_block(arm).ok_or_else(|| format!("arm {key}: body is not a block"))?;
            let p = plan(&old, ex, block, ex.by_hand.contains_key(&key))
                .map_err(|e| format!("arm {key}: {e}"))?;
            let (ba, bb) = range(f, block.to_token_stream());
            let first = f.line_of(ba);
            let lead = t[lead_from..line_start(t, a)].trim_matches('\n');
            let lead = dedent(lead, ind, 0, &BTreeSet::new());
            let lead = lead
                .lines()
                .map(|l| l.strip_prefix(&" ".repeat(ind)).unwrap_or(l).trim_end())
                .collect::<Vec<_>>()
                .join("\n");
            let lead = lead.trim().to_string();
            let tail_call = |buf: &str| -> String {
                match &ex.tail {
                    Some(tail) => {
                        let args: Vec<String> = tparams
                            .iter()
                            .map(|x| {
                                if Some(x) == ex.prelude.as_ref() {
                                    buf.to_string()
                                } else {
                                    x.clone()
                                }
                            })
                            .collect();
                        format!("{tail}({})", args.join(", "))
                    }
                    None => post_text.trim().to_string(),
                }
            };
            let body = match p.form {
                Form::Return => dedent(&t[ba..bb], ind, first, &fz),
                Form::Void => format!(
                    "{{\n    {}\n    {}\n}}",
                    dedent(&t[ba..bb], ind.saturating_sub(4), first, &fz),
                    tail_call("Vec::new()")
                ),
                Form::Prelude => format!(
                    "{{\n    {}\n    {}\n    {}\n}}",
                    prelude_text.clone().unwrap_or_default(),
                    dedent(&t[ba..bb], ind.saturating_sub(4), first, &fz),
                    tail_call(ex.prelude.as_deref().unwrap_or_default())
                ),
            };
            let attr = if p.form == Form::Return {
                "#[allow(clippy::needless_return)]\n"
            } else {
                ""
            };
            let lead = if lead.is_empty() {
                String::new()
            } else {
                format!("{lead}\n")
            };
            fns.push(format!(
                "{lead}{attr}fn {callee}(\n{}) -> {ret} {body}",
                sig_params(f, &old, &p.params)?
            ));
            edits.push((
                lead_from,
                b,
                format!(
                    "\n{}{pat_text} => {callee}({}),",
                    " ".repeat(ind),
                    p.params.join(", ")
                ),
            ));
        } else if ex.keep.contains(&key) {
            match &*arm.body {
                Expr::Return(r) => {
                    let e = r.expr.as_ref().ok_or("bare `return` arm")?;
                    let (ea, eb) = range(f, e.to_token_stream());
                    let (ra, _) = range(f, r.to_token_stream());
                    edits.push((ra, eb, t[ea..eb].to_string()));
                }
                Expr::Block(bl) => match bl.block.stmts.last() {
                    Some(Stmt::Expr(Expr::Return(r), Some(semi))) if r.expr.is_some() => {
                        let e = r.expr.as_ref().expect("expr");
                        let (ea, eb) = range(f, e.to_token_stream());
                        let (ra, _) = range(f, r.to_token_stream());
                        edits.push((ra, f.off(semi.spans[0].end()), t[ea..eb].to_string()));
                    }
                    Some(last) if !diverges_stmts(&bl.block.stmts) => {
                        if ex.tail.is_some() {
                            return Err(format!(
                                "kept arm {key} falls through: unsupported with a tail fn"
                            ));
                        }
                        let e = range(f, last.to_token_stream()).1;
                        edits.push((
                            e,
                            e,
                            format!("\n{}{}", " ".repeat(ind + 4), post_text.trim()),
                        ));
                    }
                    _ => {}
                },
                _ => {}
            }
        } else {
            return Err(format!("arm {key} is neither in arms nor in keep"));
        }
    }
    for k in ex.arms.keys().chain(&ex.keep) {
        if !seen.contains(k) {
            return Err(format!("manifest arm {k} matches no arm"));
        }
    }
    let (_, fend) = range(f, ast.items[idx].to_token_stream());
    let mut out_edits = edits;
    let mut tailtext = String::new();
    for x in &fns {
        tailtext.push_str("\n\n");
        tailtext.push_str(x);
    }
    out_edits.push((fend, fend, tailtext));
    Ok(apply_edits(t, out_edits))
}

// ───────────────────────────── verifier ─────────────────────────────

/// What `verify --extract` reads from the regular verify.
pub struct Cx<'a> {
    pub m: &'a ExtractManifest,
    pub base: &'a dyn Source,
    pub head: &'a dyn Source,
    pub before: &'a Tree,
    pub after: &'a Tree,
    pub names: &'a [Names; 2],
    pub sites: &'a [Vec<scope::Site>; 2],
    /// First file of `<rev>^` where an identifier occurs.
    pub prior: &'a dyn Fn(&str) -> Option<String>,
    /// `line!`/`Location`/`#[track_caller]` use in tokens, if any.
    pub positional: &'a dyn Fn(&str) -> Option<String>,
}

/// The dispatcher's leaf in a tree.
pub fn dispatcher<'t>(t: &'t Tree, m: &ExtractManifest) -> Res<&'t Leaf> {
    let v: Vec<&Leaf> = t
        .leaves
        .iter()
        .filter(|l| {
            l.file == m.file && l.kind == "fn" && l.owner.is_none() && l.name == m.extract.func
        })
        .collect();
    match v.as_slice() {
        [one] => Ok(one),
        _ => Err(format!(
            "{}: {} fns {} in the tree",
            m.file,
            v.len(),
            m.extract.func
        )),
    }
}

/// `use` items hoisted (prep), as written in the module: allowed new imports.
pub fn hoisted_uses(
    m: &ExtractManifest,
    base: &dyn Source,
    module: &str,
) -> Res<Vec<tree::UseItem>> {
    let mut out = Vec::new();
    if m.extract.hoist.is_empty() {
        return Ok(out);
    }
    let (_, ast) = parse(base, &m.file)?;
    let (_, f) = find_fn(&ast, &m.extract.func)?;
    for s in &f.block.stmts {
        if let Stmt::Item(Item::Use(u)) = s
            && m.extract.hoist.contains(&item_key(&Item::Use(u.clone())))
        {
            tree::flatten_use(u, module, &mut out);
        }
    }
    Ok(out)
}

fn parse(src: &dyn Source, file: &str) -> Res<(SrcFile, syn::File)> {
    let bytes = src.read(file).ok_or_else(|| format!("{file}: not found"))?;
    let f = SrcFile::new(String::from_utf8(bytes).map_err(|e| format!("{file}: {e}"))?);
    let ast = syn::parse_file(&f.text).map_err(|e| format!("{file}: {e}"))?;
    Ok((f, ast))
}

fn sub(a: &[String], parts: &[&Vec<String>]) -> Option<Vec<String>> {
    let mut bag: BTreeMap<&str, i64> = BTreeMap::new();
    for x in a {
        *bag.entry(x).or_default() += 1;
    }
    for p in parts {
        for x in p.iter() {
            let e = bag.entry(x).or_default();
            *e -= 1;
            if *e < 0 {
                return None;
            }
        }
    }
    Some(
        bag.into_iter()
            .flat_map(|(k, n)| std::iter::repeat_n(k.to_string(), n.max(0) as usize))
            .collect(),
    )
}

fn sorted(v: &[String]) -> Vec<String> {
    let mut v = v.to_vec();
    v.sort();
    v
}

#[derive(Debug, PartialEq, Eq)]
enum Binding {
    Outer(String),
    Param(String),
    Prelude,
    Module(BTreeSet<String>),
}

struct Stats {
    outer: usize,
    prelude: usize,
    module: usize,
}

impl<'a> Cx<'a> {
    fn resolve(&self, side: usize, module: &str, r: &bind::Ref) -> BTreeSet<String> {
        self.names[side].resolve(module, &r.name, r.ns)
    }

    /// Pairs the free refs of the same code before (in the dispatcher) and
    /// after (in a new fn with `params`, `prelude` bound around it).
    #[allow(clippy::too_many_arguments)]
    fn equivalent(
        &self,
        what: &str,
        old: &Old,
        module: &str,
        before: &bind::Scan,
        after: &bind::Scan,
        params: &BTreeSet<String>,
        prelude: Option<&str>,
        st: &mut Stats,
        errs: &mut Vec<String>,
    ) {
        if before.refs != after.refs {
            errs.push(format!(
                "{what}: free names differ between the arm and the new fn body"
            ));
            return;
        }
        let outer = old.outer_names();
        for (r, n) in before.refs.iter().zip(&after.refs) {
            let b = if r.ns & VALUE != 0 && outer.contains(&r.name) {
                Binding::Outer(r.name.clone())
            } else if old.item_names.contains(&r.name) {
                errs.push(format!(
                    "{what}: `{}` names a fn-local item of the dispatcher",
                    r.name
                ));
                continue;
            } else {
                Binding::Module(self.resolve(0, module, r))
            };
            let a = if n.ns & VALUE != 0 && params.contains(&n.name) {
                Binding::Param(n.name.clone())
            } else if n.ns & VALUE != 0 && prelude == Some(n.name.as_str()) {
                Binding::Prelude
            } else {
                Binding::Module(self.resolve(1, module, n))
            };
            match (&b, &a) {
                (Binding::Outer(x), Binding::Param(y)) if x == y => st.outer += 1,
                (Binding::Outer(x), Binding::Prelude) if Some(x.as_str()) == prelude => {
                    st.prelude += 1;
                }
                (Binding::Module(x), Binding::Module(y)) if x == y => st.module += 1,
                _ => errs.push(format!(
                    "{what}: `{}` was bound to {b:?}, now to {a:?}",
                    r.name
                )),
            }
        }
    }

    fn macros_same(&self, what: &str, old_leaf: &Leaf, new_leaf: &Leaf, errs: &mut Vec<String>) {
        let mut seen: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
        for (n, t) in scope::invocations(&old_leaf.tokens)
            .into_iter()
            .zip(&self.sites[0][old_leaf.idx].text)
        {
            seen.entry(n).or_default().insert(t.clone());
        }
        for (n, t) in scope::invocations(&new_leaf.tokens)
            .into_iter()
            .zip(&self.sites[1][new_leaf.idx].text)
        {
            match seen.get(&n) {
                Some(s) if s.len() == 1 && s.contains(t) => {}
                _ => errs.push(format!(
                    "{what}: macro {n}! resolves differently\n      before: {:?}\n      after:  {t:?}",
                    seen.get(&n)
                )),
            }
        }
    }

    /// The new leaf named `name` in the dispatcher's file and module: claimed
    /// from `news`; private, same cfg context, new to the repo.
    fn claim<'n>(
        &self,
        name: &str,
        disp: &Leaf,
        news: &mut Vec<&'n Leaf>,
        errs: &mut Vec<String>,
    ) -> Option<&'n Leaf> {
        let hits: Vec<usize> = news
            .iter()
            .enumerate()
            .filter(|(_, l)| l.owner.is_none() && l.name == name)
            .map(|(i, _)| i)
            .collect();
        let [i] = hits.as_slice() else {
            errs.push(format!(
                "new fn {name}: {} new leaves of that name",
                hits.len()
            ));
            return None;
        };
        let l = news.remove(*i);
        if l.kind != "fn" || l.file != disp.file || l.module != disp.module {
            errs.push(format!(
                "new fn {name}: not a fn in {} ({})",
                disp.file, disp.module
            ));
        }
        if !l.vis.is_empty() {
            errs.push(format!("new fn {name}: not private ({})", l.vis));
        }
        if l.ctx != disp.ctx {
            errs.push(format!(
                "new fn {name}: cfg/attribute context differs from the dispatcher's"
            ));
        }
        if let Some(f) = (self.prior)(name) {
            errs.push(format!(
                "new fn {name}: the name already occurs before the change ({f})"
            ));
        }
        if let Some(p) = (self.positional)(&l.tokens) {
            errs.push(format!(
                "new fn {name}: position-sensitive code ({p}) moves to a new position"
            ));
        }
        Some(l)
    }
}

fn sig_without(f: &ItemFn) -> String {
    let mut f = f.clone();
    f.block.stmts.clear();
    Commas.visit_item_fn_mut(&mut f);
    tok(&f)
}

/// The params of a new fn: plain `name: Type`.
fn params_of(f: &ItemFn) -> Option<Vec<(String, Type)>> {
    f.sig
        .inputs
        .iter()
        .map(|a| match a {
            FnArg::Typed(t) => match &*t.pat {
                Pat::Ident(i)
                    if i.by_ref.is_none() && i.mutability.is_none() && i.subpat.is_none() =>
                {
                    Some((i.ident.to_string(), (*t.ty).clone()))
                }
                _ => None,
            },
            FnArg::Receiver(_) => None,
        })
        .collect()
}

fn plain_sig(f: &ItemFn, ret: &syn::ReturnType) -> bool {
    let s = &f.sig;
    s.constness.is_none()
        && s.asyncness.is_none()
        && s.unsafety.is_none()
        && s.abi.is_none()
        && s.generics.params.is_empty()
        && s.generics.where_clause.is_none()
        && s.variadic.is_none()
        && tok(&s.output) == tok(ret)
}

/// Runs the extract (or hoist) checks; claims the new leaves it accounts
/// for from `news`.
pub fn check(cx: &Cx, news: &mut Vec<&Leaf>) -> Res<(Vec<String>, Vec<String>)> {
    let m = cx.m;
    let ex = &m.extract;
    let (of, oast) = parse(cx.base, &m.file)?;
    let (_, nast) = parse(cx.head, &m.file)?;
    let (_, ofn) = find_fn(&oast, &ex.func)?;
    let (_, nfn) = find_fn(&nast, &ex.func)?;
    let od = dispatcher(cx.before, m)?;
    let nd = dispatcher(cx.after, m)?;
    let mut errs = Vec::new();
    let mut info = Vec::new();
    if sig_without(ofn) != sig_without(nfn) {
        errs.push(format!(
            "{}: attributes, visibility or signature changed",
            ex.func
        ));
    }
    if od.ctx != nd.ctx || od.module != nd.module {
        errs.push(format!("{}: module or cfg context changed", ex.func));
    }
    if ex.hoist.is_empty() {
        extract_check(cx, &of, ofn, nfn, od, nd, news, &mut errs, &mut info)?;
    } else {
        hoist_check(cx, ofn, &nast, nfn, od, nd, news, &mut errs, &mut info)?;
    }
    Ok((info, errs))
}

#[allow(clippy::too_many_arguments)]
fn hoist_check(
    cx: &Cx,
    ofn: &ItemFn,
    nast: &syn::File,
    nfn: &ItemFn,
    od: &Leaf,
    nd: &Leaf,
    news: &mut Vec<&Leaf>,
    errs: &mut Vec<String>,
    info: &mut Vec<String>,
) -> Res<()> {
    let ex = &cx.m.extract;
    let old = model(ofn, ex)?;
    let module = &od.module;
    let mut hoisted: BTreeSet<usize> = BTreeSet::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let mut moved_comments: Vec<Vec<String>> = Vec::new();
    for key in &ex.hoist {
        let Some(&i) = old.items.get(key) else {
            errs.push(format!("hoist {key}: no such fn-local item before"));
            continue;
        };
        hoisted.insert(i);
        let Stmt::Item(item) = &ofn.block.stmts[i] else {
            continue;
        };
        let mut v = Vec::new();
        item_binds(item, &mut v);
        names.extend(v);
        match item {
            Item::Use(u) => {
                let found = nast
                    .items
                    .iter()
                    .any(|x| matches!(x, Item::Use(n) if tok(n) == tok(u)));
                if !found {
                    errs.push(format!(
                        "hoist {key}: no module-level `use` with the same tokens"
                    ));
                }
            }
            _ => {
                let p = tree::item_parts(item).ok_or("hoisted item kind")?;
                let hit = news.iter().position(|l| {
                    l.owner.is_none()
                        && l.kind == p.kind
                        && l.name == p.name
                        && l.module == *module
                        && l.file == od.file
                });
                match hit {
                    Some(h) => {
                        let l = news.remove(h);
                        if l.tokens != p.tokens || !l.vis.is_empty() || !p.vis.is_empty() {
                            errs.push(format!(
                                "hoist {key}: tokens or visibility differ{}",
                                crate::verify::first_diff(&p.tokens, &l.tokens)
                            ));
                        }
                        if l.ctx != od.ctx {
                            errs.push(format!("hoist {key}: cfg context differs"));
                        }
                        moved_comments.push(l.comments.clone());
                        let (fb, ..) = scope::free_names(&l.tokens, module);
                        for (n, ns) in fb {
                            if names.contains(&n) || old.item_names.contains(&n) {
                                continue;
                            }
                            let (b, a) = (
                                cx.names[0].resolve(module, &n, ns),
                                cx.names[1].resolve(module, &n, ns),
                            );
                            if b != a {
                                errs.push(format!(
                                    "hoist {key}: name `{n}` resolves differently\n      before: {b:?}\n      after:  {a:?}"
                                ));
                            }
                        }
                    }
                    None => errs.push(format!("hoist {key}: not found as a new module item")),
                }
            }
        }
    }
    let expect: Vec<String> = ofn
        .block
        .stmts
        .iter()
        .enumerate()
        .filter(|(i, _)| !hoisted.contains(i))
        .map(|(_, s)| norm_stmt(s))
        .collect();
    if expect != norm_stmts(&nfn.block.stmts) {
        errs.push(format!(
            "{}: body is not the old body minus the hoisted items",
            ex.func
        ));
    }
    let parts: Vec<&Vec<String>> = moved_comments.iter().collect();
    match sub(&od.comments, &parts) {
        Some(rest) if sorted(&rest) == sorted(&nd.comments) => {}
        _ => errs.push(format!("{}: comments changed by the hoist", ex.func)),
    }
    // Names the hoisted items bound inside the fn now resolve at module
    // scope (to them: a module item beats a glob, a duplicate is E0428);
    // every other name of the dispatcher as before.
    let (fa, ..) = scope::free_names(&nd.tokens, module);
    for (n, ns) in fa {
        let a = cx.names[1].resolve(module, &n, ns);
        if names.contains(&n) {
            if a.is_empty() || a.iter().any(|x| x.starts_with("glob ")) {
                errs.push(format!(
                    "hoisted `{n}` does not resolve to a module binding: {a:?}"
                ));
            }
            continue;
        }
        let b = cx.names[0].resolve(module, &n, ns);
        if b != a {
            errs.push(format!(
                "{}: name `{n}` resolves differently\n      before: {b:?}\n      after:  {a:?}",
                ex.func
            ));
        }
    }
    info.push(format!(
        "hoist: {} fn-local items of {} moved to module scope ({} names)",
        hoisted.len(),
        ex.func,
        names.len()
    ));
    Ok(())
}

#[allow(clippy::too_many_arguments, clippy::too_many_lines)]
fn extract_check(
    cx: &Cx,
    of: &SrcFile,
    ofn: &ItemFn,
    nfn: &ItemFn,
    od: &Leaf,
    nd: &Leaf,
    news: &mut Vec<&Leaf>,
    errs: &mut Vec<String>,
    info: &mut Vec<String>,
) -> Res<()> {
    let ex = &cx.m.extract;
    let old = model(ofn, ex)?;
    let module = od.module.as_str();
    let ret = &ofn.sig.output;
    let post = old.post();
    let mut st = Stats {
        outer: 0,
        prelude: 0,
        module: 0,
    };

    // 1. Dispatcher: preamble (minus the prelude) unchanged, the match is
    // the tail.
    let pre: Vec<String> = ofn.block.stmts[..old.at]
        .iter()
        .enumerate()
        .filter(|(i, _)| Some(*i) != old.prelude)
        .map(|(_, s)| norm_stmt(s))
        .collect();
    let nst = &nfn.block.stmts;
    let Some((Stmt::Expr(Expr::Match(nmat), None), npre)) = nst.split_last() else {
        errs.push(format!("{}: the match is not the tail expression", ex.func));
        return Ok(());
    };
    if norm_stmts(npre) != pre {
        errs.push(format!(
            "{}: statements before the match changed (only the prelude may go)",
            ex.func
        ));
    }
    if norm_expr(&nmat.expr) != norm_expr(&old.mat.expr) {
        errs.push(format!("{}: scrutinee changed", ex.func));
    }
    if let Some(p) = &ex.prelude {
        let mut rest: Vec<Stmt> =
            ofn.block.stmts[old.prelude.expect("prelude") + 1..old.at].to_vec();
        rest.push(Stmt::Expr((*old.mat.expr).clone(), None));
        let users = bind::stmts(&rest);
        if users.refs.iter().any(|r| &r.name == p) {
            errs.push(format!(
                "{}: code left in the dispatcher uses prelude `{p}`",
                ex.func
            ));
        }
    }
    if nmat.arms.len() != old.mat.arms.len() {
        errs.push(format!(
            "{}: {} arms before, {} after",
            ex.func,
            old.mat.arms.len(),
            nmat.arms.len()
        ));
        return Ok(());
    }

    // 5. Tail fn.
    let tparams = tail_params(&old);
    let comments = tree::comments(of)?;
    let mstart = range(of, old.mat.to_token_stream()).1;
    let fclose = of.off(ofn.block.brace_token.span.close().start());
    let post_comments: Vec<String> = comments
        .iter()
        .filter(|c| c.at >= mstart && c.at < fclose)
        .map(|c| c.text.clone())
        .collect();
    let mut moved: Vec<Vec<String>> = Vec::new();
    let mut claimed: BTreeSet<String> = BTreeSet::new();
    let new_fn = |name: &str| find_fn_in(cx.head, &cx.m.file, name);
    if let Some(tail) = &ex.tail {
        claimed.insert(tail.clone());
        if let Some(l) = cx.claim(tail, nd, news, errs) {
            let tf = new_fn(tail)?;
            let want: Vec<(String, String)> = tparams
                .iter()
                .map(|p| (p.clone(), old.ty(p).map(tok).unwrap_or_default()))
                .collect();
            let have: Option<Vec<(String, String)>> =
                params_of(&tf).map(|v| v.into_iter().map(|(n, t)| (n, tok(&t))).collect());
            if have.as_ref() != Some(&want) {
                errs.push(format!("tail fn {tail}: params {have:?}, want {want:?}"));
            }
            if !plain_sig(&tf, ret) || !tf.attrs.is_empty() {
                errs.push(format!("tail fn {tail}: signature or attributes not plain"));
            }
            if norm_stmts(&tf.block.stmts) != norm_stmts(post) {
                errs.push(format!(
                    "tail fn {tail}: body is not the old post-match statements"
                ));
            }
            let pset: BTreeSet<String> = tparams.iter().cloned().collect();
            cx.equivalent(
                &format!("tail fn {tail}"),
                &old,
                module,
                &bind::stmts(post),
                &bind::stmts(&tf.block.stmts),
                &pset,
                None,
                &mut st,
                errs,
            );
            if l.comments != post_comments {
                errs.push(format!(
                    "tail fn {tail}: comments changed\n      before: {post_comments:?}\n      after:  {:?}",
                    l.comments
                ));
            }
            moved.push(l.comments.clone());
            cx.macros_same(&format!("tail fn {tail}"), od, l, errs);
        }
    }

    // 2–4, 6. Arms.
    let open = of.off(old.mat.brace_token.span.open().end());
    let mut prev = open;
    let (mut n_ret, mut n_void, mut n_pre, mut kept, mut hand) = (0, 0, 0, 0, 0);
    let mut seen = BTreeSet::new();
    for (oa, na) in old.mat.arms.iter().zip(&nmat.arms) {
        let (_, b) = range(of, oa.to_token_stream());
        let lead_from = prev;
        prev = b;
        let key = arm_key(oa);
        seen.insert(key.clone());
        if tok(&oa.pat) != tok(&na.pat)
            || tok(&oa.guard.as_ref().map(|g| &g.1)) != tok(&na.guard.as_ref().map(|g| &g.1))
        {
            errs.push(format!("arm {key}: pattern or guard changed"));
            continue;
        }
        let arm_comments: Vec<String> = comments
            .iter()
            .filter(|c| c.at >= lead_from && c.at < b)
            .map(|c| c.text.clone())
            .collect();
        if ex.keep.contains(&key) {
            kept += 1;
            if let Some(p) = &ex.prelude
                && bind::expr(&oa.body).refs.iter().any(|r| &r.name == p)
            {
                errs.push(format!("kept arm {key}: uses prelude `{p}`"));
            }
            let stripped = strip_return(&oa.body);
            let appended = append(&oa.body, post);
            let got = norm_arm(na, &na.body);
            if got != norm_arm(oa, &stripped) && appended.is_none_or(|x| got != norm_arm(oa, &x)) {
                errs.push(format!(
                    "kept arm {key}: body is neither the old one without its tail `return` nor with the post-match statements appended"
                ));
            }
            continue;
        }
        let Some(callee) = ex.arms.get(&key) else {
            errs.push(format!("arm {key}: neither in arms nor in keep"));
            continue;
        };
        claimed.insert(callee.clone());
        let Some(block) = arm_block(oa) else {
            errs.push(format!("arm {key}: body is not a block"));
            continue;
        };
        // 2. Call shape.
        let mut nb = na.clone();
        Commas.visit_arm_mut(&mut nb);
        let args: Option<Vec<String>> = match &*nb.body {
            Expr::Call(c) if matches!(&*c.func, Expr::Path(p) if p.qself.is_none() && p.path.is_ident(callee.as_str())) => {
                c.args
                    .iter()
                    .map(|x| match x {
                        Expr::Path(p) if p.qself.is_none() => {
                            p.path.get_ident().map(ToString::to_string)
                        }
                        _ => None,
                    })
                    .collect()
            }
            _ => None,
        };
        let Some(args) = args else {
            errs.push(format!("arm {key}: body is not `{callee}(<idents>)`"));
            continue;
        };
        let Some(leaf) = cx.claim(callee, nd, news, errs) else {
            continue;
        };
        let cf = new_fn(callee)?;
        let Some(params) = params_of(&cf) else {
            errs.push(format!("{callee}: a param is not a plain `name: Type`"));
            continue;
        };
        let pnames: Vec<String> = params.iter().map(|(n, _)| n.clone()).collect();
        if pnames != args {
            errs.push(format!(
                "arm {key}: arguments {args:?} are not {callee}'s params {pnames:?}"
            ));
        }
        if !plain_sig(&cf, ret) {
            errs.push(format!(
                "{callee}: not a plain fn returning the dispatcher's type"
            ));
        }
        // 4. Body form.
        let cs = &cf.block.stmts;
        let blk = |s: &Stmt| match s {
            Stmt::Expr(Expr::Block(x), None) if x.attrs.is_empty() && x.label.is_none() => {
                Some(x.block.clone())
            }
            _ => None,
        };
        let same_block = |x: &Block| norm_stmts(&x.stmts) == norm_stmts(&block.stmts);
        let tail_expr = |buf: &str| -> Option<Vec<String>> {
            match &ex.tail {
                Some(t) => {
                    let args: Vec<String> = tparams
                        .iter()
                        .map(|x| {
                            if Some(x) == ex.prelude.as_ref() {
                                buf.to_string()
                            } else {
                                x.clone()
                            }
                        })
                        .collect();
                    let e: Expr = syn::parse_str(&format!("{t}({})", args.join(", "))).ok()?;
                    Some(vec![norm_stmt(&Stmt::Expr(e, None))])
                }
                None => Some(norm_stmts(post)),
            }
        };
        let prelude_stmt = old.prelude.map(|p| &ofn.block.stmts[p]);
        let form = if norm_stmts(cs) == norm_stmts(&block.stmts) {
            Some(Form::Return)
        } else if let Some(first) = cs.first().and_then(blk)
            && same_block(&first)
            && Some(norm_stmts(&cs[1..])) == tail_expr("Vec::new()")
        {
            Some(Form::Void)
        } else if let (Some(ps), Some(p)) = (prelude_stmt, &ex.prelude)
            && cs.len() >= 2
            && norm_stmt(&cs[0]) == norm_stmt(ps)
            && blk(&cs[1]).is_some_and(|x| same_block(&x))
            && Some(norm_stmts(&cs[2..])) == tail_expr(p)
        {
            Some(Form::Prelude)
        } else {
            None
        };
        let Some(form) = form else {
            errs.push(format!(
                "{callee}: body is none of `<arm>`, `{{ <arm> }} <tail>`, `<prelude> {{ <arm> }} <tail>`"
            ));
            continue;
        };
        let allow =
            cf.attrs.len() == 1 && tok(&cf.attrs[0]) == "# [allow (clippy :: needless_return)]";
        if !(cf.attrs.is_empty() || (allow && form == Form::Return)) {
            errs.push(format!(
                "{callee}: attributes other than needless_return on a returning body"
            ));
        }
        // 3. Binding equivalence.
        let by_hand = ex.by_hand.get(&key);
        let before = bind::block(block);
        let inner = match form {
            Form::Return => cf.block.as_ref().clone(),
            Form::Void => blk(&cs[0]).expect("block"),
            Form::Prelude => blk(&cs[1]).expect("block"),
        };
        let after = bind::block(&inner);
        let pset: BTreeSet<String> = pnames.iter().cloned().collect();
        let used = outer_used(&old, &before);
        let uses_prelude = ex.prelude.as_ref().is_some_and(|p| used.contains(p));
        if uses_prelude != (form == Form::Prelude) {
            errs.push(format!(
                "{callee}: the prelude must be present exactly when the arm uses `{}`",
                ex.prelude.as_deref().unwrap_or_default()
            ));
        }
        match (before.refused.is_empty(), by_hand) {
            (true, Some(_)) => errs.push(format!("arm {key}: listed under by_hand but resolvable")),
            (false, None) => errs.push(format!(
                "arm {key}: binding forms outside the resolver ({}): extract by hand and list it under by_hand",
                before.refused.join("; ")
            )),
            (false, Some(_)) => {
                hand += 1;
                info.push(format!("by hand (not binding-checked): arm {key} → {callee}"));
            }
            (true, None) => {
                cx.equivalent(
                    &format!("arm {key} → {callee}"),
                    &old,
                    module,
                    &before,
                    &after,
                    &pset,
                    (form == Form::Prelude).then_some(ex.prelude.as_deref()).flatten(),
                    &mut st,
                    errs,
                );
                let mut want: BTreeSet<String> = used.clone();
                if form != Form::Return {
                    match &ex.tail {
                        Some(_) => want.extend(tparams.iter().cloned()),
                        None => want.extend(outer_used(&old, &bind::stmts(post))),
                    }
                }
                if let Some(p) = &ex.prelude {
                    want.remove(p);
                }
                let want = old.order(&want);
                if want != pnames {
                    errs.push(format!("{callee}: params {pnames:?}, want {want:?}"));
                }
            }
        }
        for (n, t) in &params {
            match old.ty(n) {
                Some(o) if tok(o) == tok(t) => {}
                o => errs.push(format!(
                    "{callee}: param `{n}: {}` does not have the outer type {:?}",
                    tok(t),
                    o.map(tok)
                )),
            }
            for r in bind::ty(t).refs {
                if cx.resolve(0, module, &r) != cx.resolve(1, module, &r) {
                    errs.push(format!(
                        "{callee}: type name `{}` resolves differently",
                        r.name
                    ));
                }
            }
        }
        if form != Form::Return {
            let tail_code: Vec<Stmt> = cs[if form == Form::Void { 1 } else { 2 }..].to_vec();
            let tb = bind::stmts(&tail_code);
            if ex.tail.is_none() {
                cx.equivalent(
                    &format!("{callee} tail"),
                    &old,
                    module,
                    &bind::stmts(post),
                    &tb,
                    &pset,
                    None,
                    &mut st,
                    errs,
                );
            } else {
                for r in tb.refs.iter().filter(|r| {
                    !pset.contains(&r.name)
                        && Some(&r.name) != ex.prelude.as_ref()
                        && Some(&r.name) != ex.tail.as_ref()
                }) {
                    if cx.resolve(0, module, r) != cx.resolve(1, module, r) {
                        errs.push(format!(
                            "{callee}: tail name `{}` resolves differently",
                            r.name
                        ));
                    }
                }
            }
            if form == Form::Prelude {
                for r in bind::stmts(&cs[..1]).refs {
                    if cx.resolve(0, module, &r) != cx.resolve(1, module, &r) {
                        errs.push(format!(
                            "{callee}: prelude name `{}` resolves differently",
                            r.name
                        ));
                    }
                }
            }
        }
        match form {
            Form::Return => n_ret += 1,
            Form::Void => n_void += 1,
            Form::Prelude => n_pre += 1,
        }
        // 6. Comments.
        if leaf.comments != arm_comments {
            errs.push(format!(
                "{callee}: comments differ from arm {key}'s\n      before: {arm_comments:?}\n      after:  {:?}",
                leaf.comments
            ));
        }
        moved.push(leaf.comments.clone());
        cx.macros_same(callee, od, leaf, errs);
    }
    for k in ex.arms.keys().chain(&ex.keep) {
        if !seen.contains(k) {
            errs.push(format!("manifest arm {k} matches no arm"));
        }
    }
    for k in ex.by_hand.keys() {
        if !ex.arms.contains_key(k) {
            errs.push(format!("by_hand {k}: not an extracted arm"));
        }
    }
    let parts: Vec<&Vec<String>> = moved.iter().collect();
    match sub(&od.comments, &parts) {
        Some(rest) if sorted(&rest) == sorted(&nd.comments) => {}
        _ => errs.push(format!(
            "{}: comments lost or added (the dispatcher keeps what no new fn took)",
            ex.func
        )),
    }
    // The dispatcher's other names resolve as before; the callees to the
    // claimed leaves.
    let (fa, ..) = scope::free_names(&nd.tokens, module);
    for (n, ns) in fa {
        if claimed.contains(&n) {
            continue;
        }
        let (b, a) = (
            cx.names[0].resolve(module, &n, ns),
            cx.names[1].resolve(module, &n, ns),
        );
        if b != a {
            errs.push(format!(
                "{}: name `{n}` resolves differently\n      before: {b:?}\n      after:  {a:?}",
                ex.func
            ));
        }
    }
    cx.macros_same(&ex.func, od, nd, errs);
    info.push(format!(
        "extract: {} arms of {} → new fns ({n_ret} returning, {n_void} void + tail, {n_pre} prelude + tail), {kept} kept, {hand} by hand{}; free names checked: {} outer → param, {} prelude, {} module scope",
        n_ret + n_void + n_pre,
        ex.func,
        if ex.tail.is_some() { ", tail fn" } else { "" },
        st.outer,
        st.prelude,
        st.module
    ));
    Ok(())
}

fn find_fn_in(src: &dyn Source, file: &str, name: &str) -> Res<ItemFn> {
    let (_, ast) = parse(src, file)?;
    find_fn(&ast, name).map(|(_, f)| f.clone())
}

/// `body` with a final `return e;` (or a `return e` body) as `e`.
fn strip_return(body: &Expr) -> Expr {
    match body {
        Expr::Return(r) if r.expr.is_some() => *r.expr.clone().expect("expr"),
        Expr::Block(b) => {
            let mut b = b.clone();
            if let Some(last) = b.block.stmts.last_mut()
                && let Stmt::Expr(Expr::Return(r), Some(_)) = last
                && let Some(e) = &r.expr
            {
                *last = Stmt::Expr((**e).clone(), None);
            }
            Expr::Block(b)
        }
        e => e.clone(),
    }
}

/// A block `body` with `post` appended.
fn append(body: &Expr, post: &[Stmt]) -> Option<Expr> {
    let Expr::Block(b) = body else { return None };
    if post.is_empty() {
        return None;
    }
    let mut b = b.clone();
    b.block.stmts.extend(post.iter().cloned());
    Some(Expr::Block(b))
}

/// Names bound by the items a prep manifest hoists (empty otherwise).
pub fn hoisted_names(m: &ExtractManifest, base: &dyn Source) -> Res<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    if m.extract.hoist.is_empty() {
        return Ok(out);
    }
    let (_, ast) = parse(base, &m.file)?;
    let (_, f) = find_fn(&ast, &m.extract.func)?;
    for s in &f.block.stmts {
        if let Stmt::Item(i) = s
            && m.extract.hoist.contains(&item_key(i))
        {
            let mut v = Vec::new();
            item_binds(i, &mut v);
            out.extend(v);
        }
    }
    Ok(out)
}

/// A fn leaf binds `name` itself: an item at the top level of its body
/// (visible in the whole body, nested blocks, items and macro expansions
/// included) and `name` absent from its signature. Every use of `name` in
/// the leaf then means that local item (or something nested inside), never
/// a module-scope binding.
pub fn binds_locally(tokens: &str, name: &str) -> bool {
    let Ok(Item::Fn(f)) = syn::parse_str::<Item>(tokens) else {
        return false;
    };
    let mut v = Vec::new();
    for s in &f.block.stmts {
        if let Stmt::Item(i) = s {
            item_binds(i, &mut v);
        }
    }
    let mut sig = f.clone();
    sig.block.stmts.clear();
    let sig = tok(&sig);
    v.iter().any(|n| n == name) && !sig.split_whitespace().any(|w| w == name)
}

/// Trait-scope ids (`use <path>`) of the hoisted imports that provably name
/// no trait: the path starts at a workspace crate and walks its in-tree
/// modules to a module, or to a struct/enum/union/type alias (the type
/// namespace then holds no trait of that name there).
pub fn non_trait_hoists(
    m: &ExtractManifest,
    base: &dyn Source,
    module: &str,
) -> Res<BTreeSet<String>> {
    let mut out = BTreeSet::new();
    let uses = hoisted_uses(m, base, module)?;
    if uses.is_empty() {
        return Ok(out);
    }
    let toml_at = |p: &str| -> Option<toml::Value> {
        toml::from_str(&String::from_utf8(base.read(p)?).ok()?).ok()
    };
    let members: Vec<String> = toml_at("Cargo.toml")
        .and_then(|v| v.get("workspace")?.get("members")?.as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|x| x.as_str().map(str::to_string))
        .collect();
    let mut crates: BTreeMap<String, Option<Tree>> = BTreeMap::new();
    for u in uses {
        let path = u.path.split(" as ").next().unwrap_or_default();
        let segs: Vec<&str> = path.split(" :: ").collect();
        let [krate, inner @ ..] = segs.as_slice() else {
            continue;
        };
        let Some((name, parents)) = inner.split_last() else {
            continue;
        };
        let t = crates.entry((*krate).to_string()).or_insert_with(|| {
            let dir = members.iter().find(|d| {
                toml_at(&format!("{d}/Cargo.toml"))
                    .and_then(|v| {
                        v.get("package")?
                            .get("name")?
                            .as_str()
                            .map(|n| n.replace('-', "_"))
                    })
                    .as_deref()
                    == Some(*krate)
            })?;
            tree::load(base, &format!("{dir}/src/lib.rs"), "").ok()
        });
        let Some(t) = t else { continue };
        let parent = parents.join("::");
        let item = tree::child(&parent, name);
        let typed = t.leaves.iter().any(|l| {
            l.owner.is_none()
                && l.module == parent
                && l.name == *name
                && matches!(l.kind, "struct" | "enum" | "union" | "type")
        });
        if t.mods.contains_key(&item) || typed {
            out.insert(format!("use {}", u.path));
        }
    }
    Ok(out)
}
