//! Bounded lexical resolver for `split verify --extract`: which identifier
//! occurrences of a block (a match arm, a fn body) are bound inside it and
//! which are free, i.e. bound by whatever encloses it (a fn param or `let`,
//! a fn-local item, or module scope).
//!
//! Modelled binding forms, each with a fixture: `let` (patterns, `let …
//! else`: the initializer and the else block are outside the binding's
//! scope), `if let` / `while let` and `&&`-chained `let`s (a binding scopes
//! over the later conditions and the then-body, not the else), `for`
//! patterns, `match` arm patterns and guards (also inside `matches!`),
//! closure params, block-local items and `use`s (visible in their whole
//! block; item bodies do not see locals), nested-fn params, and implicit
//! format-string captures (`"{dev}"`, `{:w$}`) in std format-family and
//! `log` macros, resolved like a plain identifier at the macro site.
//!
//! A pattern identifier that starts with an uppercase letter is a path (a
//! const, unit struct or variant), a lowercase one a binding: the other
//! readings are `non_snake_case` / `non_upper_case_globals` warnings, which
//! the gate denies (residual: a lowercase const imported from another
//! crate).
//!
//! Anything else — another macro, `async`, const blocks, items with
//! generics, impls, `macro_rules!`, glob `use` in a block, macro patterns —
//! is refused: the caller extracts that code by hand.

use std::collections::BTreeSet;

use proc_macro2::TokenStream;
use syn::{
    BinOp, Block, Expr, FnArg, GenericArgument, Item, Pat, PathArguments, ReturnType, Stmt, Token,
    Type, TypeParamBound, UseTree,
    parse::{ParseStream, Parser},
    punctuated::Punctuated,
};

use crate::scope::{TYPE, VALUE};

/// One free identifier occurrence: its name and namespace mask (as
/// `scope::free_names` classifies it: a path's first segment is a type, a
/// single uppercase ident a type or value, a lowercase one a value).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Ref {
    pub name: String,
    pub ns: u8,
}

#[derive(Default, Debug)]
pub struct Scan {
    /// Free occurrences, in walk order.
    pub refs: Vec<Ref>,
    /// Why the code is outside the modelled forms (empty: fully resolved).
    pub refused: Vec<String>,
    /// Paths of the macros invoked, in walk order.
    pub macros: Vec<String>,
}

#[derive(Default)]
struct Frame {
    locals: BTreeSet<String>,
    items: BTreeSet<String>,
    /// An item body: locals of the frames outside it are not visible.
    barrier: bool,
}

#[derive(Default)]
struct Walker {
    frames: Vec<Frame>,
    out: Scan,
}

/// Free names of a block (nothing bound around it).
pub fn block(b: &Block) -> Scan {
    let mut w = Walker::default();
    w.block(&b.stmts);
    w.out
}

/// Free names of statements read as one block.
pub fn stmts(s: &[Stmt]) -> Scan {
    let mut w = Walker::default();
    w.block(s);
    w.out
}

/// Free names of an expression.
pub fn expr(e: &Expr) -> Scan {
    let mut w = Walker::default();
    w.expr(e);
    w.out
}

/// Free names of a type.
pub fn ty(t: &Type) -> Scan {
    let mut w = Walker::default();
    w.ty(t);
    w.out
}

/// A pattern identifier that binds (see the module doc).
pub fn is_binding_name(s: &str) -> bool {
    s.trim_start_matches('_')
        .chars()
        .next()
        .is_none_or(|c| !c.is_uppercase())
}

fn is_ident(s: &str) -> bool {
    let mut c = s.chars();
    c.next().is_some_and(|f| f == '_' || f.is_alphabetic())
        && c.all(|x| x == '_' || x.is_alphanumeric())
        && s != "_"
}

/// Identifiers a format string captures implicitly or names as arguments:
/// `{name}`, `{name:…}`, and `name$` widths/precisions in a spec.
pub fn format_names(s: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut rest = s;
    while let Some(i) = rest.find(['{', '}']) {
        let c = rest.as_bytes()[i];
        if rest.as_bytes().get(i + 1) == Some(&c) {
            rest = &rest[i + 2..];
            continue;
        }
        if c == b'}' {
            rest = &rest[i + 1..];
            continue;
        }
        let Some(j) = rest[i..].find('}') else { break };
        let inner = &rest[i + 1..i + j];
        let (arg, spec) = inner.split_once(':').unwrap_or((inner, ""));
        if is_ident(arg.trim()) {
            out.push(arg.trim().to_string());
        }
        let mut word = String::new();
        for ch in spec.chars() {
            if ch == '_' || ch.is_alphanumeric() {
                word.push(ch);
                continue;
            }
            if ch == '$' && is_ident(&word) && !word.starts_with(|d: char| d.is_ascii_digit()) {
                out.push(word.clone());
            }
            word.clear();
        }
        rest = &rest[i + j + 1..];
    }
    out
}

const FORMAT_MACROS: [&str; 10] = [
    "format",
    "format_args",
    "print",
    "println",
    "eprint",
    "eprintln",
    "panic",
    "unreachable",
    "todo",
    "unimplemented",
];

const LOG_MACROS: [&str; 5] = ["trace", "debug", "info", "warn", "error"];

impl Walker {
    fn refuse(&mut self, why: impl Into<String>) {
        self.out.refused.push(why.into());
    }

    fn bound(&self, n: &str, ns: u8) -> bool {
        let mut crossed = false;
        for f in self.frames.iter().rev() {
            if f.items.contains(n) {
                return true;
            }
            if ns & VALUE != 0 && !crossed && f.locals.contains(n) {
                return true;
            }
            crossed |= f.barrier;
        }
        false
    }

    fn refer(&mut self, name: String, ns: u8) {
        if !self.bound(&name, ns) {
            self.out.refs.push(Ref { name, ns });
        }
    }

    fn top(&mut self) -> &mut Frame {
        if self.frames.is_empty() {
            self.frames.push(Frame::default());
        }
        self.frames.last_mut().expect("frame")
    }

    fn bind(&mut self, names: Vec<String>) {
        self.top().locals.extend(names);
    }

    fn scoped(&mut self, barrier: bool, f: impl FnOnce(&mut Self)) {
        self.frames.push(Frame {
            barrier,
            ..Frame::default()
        });
        f(self);
        self.frames.pop();
    }

    fn path(&mut self, p: &syn::Path, single: u8) {
        for seg in &p.segments {
            self.args(&seg.arguments);
        }
        if p.leading_colon.is_some() {
            return;
        }
        let first = p.segments[0].ident.to_string();
        if matches!(first.as_str(), "crate" | "self" | "super") {
            return;
        }
        if first == "Self" {
            self.refuse("`Self` path");
            return;
        }
        let ns = if p.segments.len() == 1 { single } else { TYPE };
        self.refer(first, ns);
    }

    fn qself(&mut self, q: Option<&syn::QSelf>) {
        if let Some(q) = q {
            self.ty(&q.ty);
        }
    }

    fn value_path(&mut self, q: Option<&syn::QSelf>, p: &syn::Path) {
        self.qself(q);
        let one = p.segments.len() == 1
            && p.segments[0]
                .ident
                .to_string()
                .starts_with(char::is_uppercase);
        self.path(p, if one { TYPE | VALUE } else { VALUE });
    }

    fn args(&mut self, a: &PathArguments) {
        match a {
            PathArguments::None => {}
            PathArguments::AngleBracketed(g) => {
                for arg in &g.args {
                    match arg {
                        GenericArgument::Type(t) => self.ty(t),
                        GenericArgument::Const(e) => self.expr(e),
                        GenericArgument::AssocType(t) => self.ty(&t.ty),
                        GenericArgument::Lifetime(_) => {}
                        _ => self.refuse("generic argument form"),
                    }
                }
            }
            PathArguments::Parenthesized(p) => {
                for t in &p.inputs {
                    self.ty(t);
                }
                if let ReturnType::Type(_, t) = &p.output {
                    self.ty(t);
                }
            }
        }
    }

    fn bounds<'a>(&mut self, b: impl IntoIterator<Item = &'a TypeParamBound>) {
        for b in b {
            match b {
                TypeParamBound::Trait(t) => self.path(&t.path, TYPE),
                TypeParamBound::Lifetime(_) => {}
                _ => self.refuse("bound form"),
            }
        }
    }

    fn ty(&mut self, t: &Type) {
        match t {
            Type::Path(p) => {
                self.qself(p.qself.as_ref());
                self.path(&p.path, TYPE);
            }
            Type::Reference(r) => self.ty(&r.elem),
            Type::Slice(s) => self.ty(&s.elem),
            Type::Array(a) => {
                self.ty(&a.elem);
                self.expr(&a.len);
            }
            Type::Tuple(t) => t.elems.iter().for_each(|e| self.ty(e)),
            Type::Ptr(p) => self.ty(&p.elem),
            Type::Paren(p) => self.ty(&p.elem),
            Type::Group(g) => self.ty(&g.elem),
            Type::Never(_) | Type::Infer(_) => {}
            Type::ImplTrait(i) => self.bounds(&i.bounds),
            Type::TraitObject(o) => self.bounds(&o.bounds),
            Type::BareFn(f) => {
                for i in &f.inputs {
                    self.ty(&i.ty);
                }
                if let ReturnType::Type(_, t) = &f.output {
                    self.ty(t);
                }
            }
            _ => self.refuse("type form"),
        }
    }

    /// Walks the paths, types and expressions in `p`; its bindings go to
    /// `binds`.
    fn pat(&mut self, p: &Pat, binds: &mut Vec<String>) {
        match p {
            Pat::Ident(i) => {
                let s = i.ident.to_string();
                if is_binding_name(&s) {
                    binds.push(s);
                } else if i.by_ref.is_some() || i.mutability.is_some() || i.subpat.is_some() {
                    self.refuse(format!("uppercase binding `{s}`"));
                } else {
                    self.refer(s, TYPE | VALUE);
                }
                if let Some((_, sub)) = &i.subpat {
                    self.pat(sub, binds);
                }
            }
            Pat::Lit(_) | Pat::Rest(_) | Pat::Wild(_) => {}
            Pat::Path(pp) => self.value_path(pp.qself.as_ref(), &pp.path),
            Pat::Or(o) => o.cases.iter().for_each(|c| self.pat(c, binds)),
            Pat::Paren(x) => self.pat(&x.pat, binds),
            Pat::Range(r) => {
                if let Some(e) = &r.start {
                    self.expr(e);
                }
                if let Some(e) = &r.end {
                    self.expr(e);
                }
            }
            Pat::Reference(r) => self.pat(&r.pat, binds),
            Pat::Slice(s) => s.elems.iter().for_each(|e| self.pat(e, binds)),
            Pat::Struct(s) => {
                self.qself(s.qself.as_ref());
                self.path(&s.path, TYPE | VALUE);
                s.fields.iter().for_each(|f| self.pat(&f.pat, binds));
            }
            Pat::Tuple(t) => t.elems.iter().for_each(|e| self.pat(e, binds)),
            Pat::TupleStruct(t) => {
                self.qself(t.qself.as_ref());
                self.path(&t.path, TYPE | VALUE);
                t.elems.iter().for_each(|e| self.pat(e, binds));
            }
            Pat::Type(t) => {
                self.pat(&t.pat, binds);
                self.ty(&t.ty);
            }
            _ => self.refuse("pattern form"),
        }
    }

    /// Binds `p` in the innermost frame (after walking its paths).
    fn bind_pat(&mut self, p: &Pat) {
        let mut b = Vec::new();
        self.pat(p, &mut b);
        self.bind(b);
    }

    /// A condition: `let`s bind in the innermost frame for what follows
    /// (later `&&` operands, then the caller's then-body).
    fn cond(&mut self, e: &Expr) {
        match e {
            Expr::Let(l) => {
                self.expr(&l.expr);
                self.bind_pat(&l.pat);
            }
            Expr::Binary(b) if matches!(b.op, BinOp::And(_)) => {
                self.cond(&b.left);
                self.cond(&b.right);
            }
            _ => self.expr(e),
        }
    }

    fn block(&mut self, stmts: &[Stmt]) {
        self.scoped(false, |w| {
            for s in stmts {
                if let Stmt::Item(i) = s {
                    w.declare(i);
                }
            }
            for s in stmts {
                match s {
                    Stmt::Local(l) => {
                        if let Some(init) = &l.init {
                            w.expr(&init.expr);
                            if let Some((_, d)) = &init.diverge {
                                w.expr(d);
                            }
                        }
                        w.bind_pat(&l.pat);
                    }
                    Stmt::Item(i) => w.item(i),
                    Stmt::Expr(e, _) => w.expr(e),
                    Stmt::Macro(m) => w.mac(&m.mac),
                }
            }
        });
    }

    /// Names a block-local item binds in its whole block.
    fn declare(&mut self, i: &Item) {
        let name = match i {
            Item::Const(x) => Some(&x.ident),
            Item::Static(x) => Some(&x.ident),
            Item::Fn(x) => Some(&x.sig.ident),
            Item::Struct(x) => Some(&x.ident),
            Item::Enum(x) => Some(&x.ident),
            Item::Type(x) => Some(&x.ident),
            Item::Use(u) => {
                let mut v = Vec::new();
                if !use_names(&u.tree, &mut v) {
                    self.refuse("glob `use` in a block");
                }
                self.top().items.extend(v);
                None
            }
            _ => {
                self.refuse("block-local item form");
                None
            }
        };
        if let Some(n) = name {
            let n = n.to_string();
            self.top().items.insert(n);
        }
    }

    fn generics(&mut self, g: &syn::Generics) {
        if !g.params.is_empty() || g.where_clause.is_some() {
            self.refuse("block-local item with generics");
        }
    }

    fn fields(&mut self, f: &syn::Fields) {
        for x in f {
            self.ty(&x.ty);
        }
    }

    fn item(&mut self, i: &Item) {
        self.scoped(true, |w| match i {
            Item::Const(x) => {
                w.generics(&x.generics);
                w.ty(&x.ty);
                w.expr(&x.expr);
            }
            Item::Static(x) => {
                w.ty(&x.ty);
                w.expr(&x.expr);
            }
            Item::Fn(f) => {
                w.generics(&f.sig.generics);
                if f.sig.asyncness.is_some() || f.sig.variadic.is_some() {
                    w.refuse("async or variadic fn");
                }
                for a in &f.sig.inputs {
                    match a {
                        FnArg::Typed(t) => {
                            w.ty(&t.ty);
                            w.bind_pat(&t.pat);
                        }
                        FnArg::Receiver(_) => w.refuse("`self` param"),
                    }
                }
                if let ReturnType::Type(_, t) = &f.sig.output {
                    w.ty(t);
                }
                w.block(&f.block.stmts);
            }
            Item::Struct(s) => {
                w.generics(&s.generics);
                w.fields(&s.fields);
            }
            Item::Enum(e) => {
                w.generics(&e.generics);
                for v in &e.variants {
                    w.fields(&v.fields);
                    if let Some((_, d)) = &v.discriminant {
                        w.expr(d);
                    }
                }
            }
            Item::Type(t) => {
                w.generics(&t.generics);
                w.ty(&t.ty);
            }
            Item::Use(u) => {
                if u.leading_colon.is_none()
                    && let UseTree::Path(p) = &u.tree
                {
                    let first = p.ident.to_string();
                    if !matches!(first.as_str(), "crate" | "self" | "super") {
                        w.refer(first, TYPE);
                    }
                }
            }
            _ => {}
        });
    }

    fn expr(&mut self, e: &Expr) {
        match e {
            Expr::Array(a) => a.elems.iter().for_each(|x| self.expr(x)),
            Expr::Assign(a) => {
                self.expr(&a.left);
                self.expr(&a.right);
            }
            Expr::Await(a) => self.expr(&a.base),
            Expr::Binary(b) => {
                self.expr(&b.left);
                self.expr(&b.right);
            }
            Expr::Block(b) => self.block(&b.block.stmts),
            Expr::Break(b) => {
                if let Some(x) = &b.expr {
                    self.expr(x);
                }
            }
            Expr::Call(c) => {
                self.expr(&c.func);
                c.args.iter().for_each(|x| self.expr(x));
            }
            Expr::Cast(c) => {
                self.expr(&c.expr);
                self.ty(&c.ty);
            }
            Expr::Closure(c) => {
                if c.asyncness.is_some() || c.lifetimes.is_some() {
                    self.refuse("async or higher-ranked closure");
                }
                self.scoped(false, |w| {
                    for p in &c.inputs {
                        w.bind_pat(p);
                    }
                    if let ReturnType::Type(_, t) = &c.output {
                        w.ty(t);
                    }
                    w.expr(&c.body);
                });
            }
            Expr::Continue(_) | Expr::Infer(_) | Expr::Lit(_) => {}
            Expr::Field(f) => self.expr(&f.base),
            Expr::ForLoop(f) => {
                self.expr(&f.expr);
                self.scoped(false, |w| {
                    w.bind_pat(&f.pat);
                    w.block(&f.body.stmts);
                });
            }
            Expr::Group(g) => self.expr(&g.expr),
            Expr::If(i) => {
                self.scoped(false, |w| {
                    w.cond(&i.cond);
                    w.block(&i.then_branch.stmts);
                });
                if let Some((_, x)) = &i.else_branch {
                    self.expr(x);
                }
            }
            Expr::Index(i) => {
                self.expr(&i.expr);
                self.expr(&i.index);
            }
            Expr::Let(_) => self.refuse("`let` outside an if/while condition"),
            Expr::Loop(l) => self.block(&l.body.stmts),
            Expr::Macro(m) => self.mac(&m.mac),
            Expr::Match(m) => {
                self.expr(&m.expr);
                for arm in &m.arms {
                    self.scoped(false, |w| {
                        w.bind_pat(&arm.pat);
                        if let Some((_, g)) = &arm.guard {
                            w.cond(g);
                        }
                        w.expr(&arm.body);
                    });
                }
            }
            Expr::MethodCall(m) => {
                self.expr(&m.receiver);
                if let Some(t) = &m.turbofish {
                    for a in &t.args {
                        match a {
                            GenericArgument::Type(t) => self.ty(t),
                            GenericArgument::Const(e) => self.expr(e),
                            _ => self.refuse("turbofish form"),
                        }
                    }
                }
                m.args.iter().for_each(|x| self.expr(x));
            }
            Expr::Paren(p) => self.expr(&p.expr),
            Expr::Path(p) => self.value_path(p.qself.as_ref(), &p.path),
            Expr::Range(r) => {
                if let Some(x) = &r.start {
                    self.expr(x);
                }
                if let Some(x) = &r.end {
                    self.expr(x);
                }
            }
            Expr::RawAddr(r) => self.expr(&r.expr),
            Expr::Reference(r) => self.expr(&r.expr),
            Expr::Repeat(r) => {
                self.expr(&r.expr);
                self.expr(&r.len);
            }
            Expr::Return(r) => {
                if let Some(x) = &r.expr {
                    self.expr(x);
                }
            }
            Expr::Struct(s) => {
                self.qself(s.qself.as_ref());
                self.path(&s.path, TYPE);
                s.fields.iter().for_each(|f| self.expr(&f.expr));
                if let Some(r) = &s.rest {
                    self.expr(r);
                }
            }
            Expr::Try(t) => self.expr(&t.expr),
            Expr::Tuple(t) => t.elems.iter().for_each(|x| self.expr(x)),
            Expr::Unary(u) => self.expr(&u.expr),
            Expr::Unsafe(u) => self.block(&u.block.stmts),
            Expr::While(w) => self.scoped(false, |s| {
                s.cond(&w.cond);
                s.block(&w.body.stmts);
            }),
            _ => self.refuse("expression form"),
        }
    }

    fn mac(&mut self, m: &syn::Macro) {
        let segs: Vec<String> = m
            .path
            .segments
            .iter()
            .map(|s| s.ident.to_string())
            .collect();
        let qualified_ok = matches!(
            segs.first().map(String::as_str),
            Some("log" | "std" | "core")
        ) && segs.len() == 2;
        if m.path.leading_colon.is_some() || (segs.len() > 1 && !qualified_ok) {
            self.refuse(format!("macro path `{}`", segs.join("::")));
            return;
        }
        let name = segs.last().cloned().unwrap_or_default();
        self.out.macros.push(segs.join("::"));
        let ts = m.tokens.clone();
        let log = LOG_MACROS.contains(&name.as_str()) && (segs.len() == 1 || segs[0] == "log");
        match name.as_str() {
            n if FORMAT_MACROS.contains(&n) && segs.len() == 1 => self.fmt(ts, 0, &name),
            "write" | "writeln" | "assert" | "debug_assert" if segs.len() == 1 => {
                self.fmt(ts, 1, &name);
            }
            "assert_eq" | "assert_ne" | "debug_assert_eq" | "debug_assert_ne"
                if segs.len() == 1 =>
            {
                self.fmt(ts, 2, &name);
            }
            _ if log => {
                let first = ts.clone().into_iter().next().map(|t| t.to_string());
                if first.as_deref() == Some("target") {
                    self.refuse(format!("{name}! with `target:`"));
                } else {
                    self.fmt(ts, 0, &name);
                }
            }
            "vec" if segs.len() == 1 => match syn::parse2::<Expr>(quote::quote!([#ts])) {
                Ok(e) => self.expr(&e),
                Err(_) => self.refuse("vec! arguments"),
            },
            "matches" if segs.len() == 1 => self.matches(ts),
            _ => self.refuse(format!("macro `{}!`", segs.join("::"))),
        }
    }

    fn matches(&mut self, ts: TokenStream) {
        let parsed = (|p: ParseStream| {
            let e = p.parse::<Expr>()?;
            p.parse::<Token![,]>()?;
            let pat = Pat::parse_multi_with_leading_vert(p)?;
            let guard = if p.peek(Token![if]) {
                p.parse::<Token![if]>()?;
                Some(p.parse::<Expr>()?)
            } else {
                None
            };
            if !p.is_empty() {
                p.parse::<Token![,]>()?;
            }
            Ok((e, pat, guard))
        })
        .parse2(ts);
        let Ok((e, pat, guard)) = parsed else {
            self.refuse("matches! arguments");
            return;
        };
        self.expr(&e);
        self.scoped(false, |w| {
            w.bind_pat(&pat);
            if let Some(g) = &guard {
                w.cond(g);
            }
        });
    }

    /// Format-family arguments: `lead` plain expressions, then an optional
    /// literal format string and its arguments (`name = expr` names one).
    fn fmt(&mut self, ts: TokenStream, lead: usize, name: &str) {
        let Ok(args) = Punctuated::<Expr, Token![,]>::parse_terminated.parse2(ts) else {
            self.refuse(format!("{name}! arguments"));
            return;
        };
        let args: Vec<&Expr> = args.iter().collect();
        for a in args.iter().take(lead) {
            self.expr(a);
        }
        let rest = args.get(lead..).unwrap_or_default();
        let Some((first, more)) = rest.split_first() else {
            return;
        };
        let Expr::Lit(syn::ExprLit {
            lit: syn::Lit::Str(s),
            ..
        }) = first
        else {
            self.refuse(format!("{name}! without a literal format string"));
            return;
        };
        let mut named = BTreeSet::new();
        for a in more {
            match a {
                Expr::Assign(x) if matches!(&*x.left, Expr::Path(p) if p.path.get_ident().is_some()) =>
                {
                    if let Expr::Path(p) = &*x.left {
                        named.insert(p.path.segments[0].ident.to_string());
                    }
                    self.expr(&x.right);
                }
                _ => self.expr(a),
            }
        }
        for n in format_names(&s.value()) {
            if !named.contains(&n) {
                self.refer(n, VALUE);
            }
        }
    }
}

/// Names a `use` tree binds; false when it has a glob.
fn use_names(t: &UseTree, out: &mut Vec<String>) -> bool {
    match t {
        UseTree::Path(p) => use_names(&p.tree, out),
        UseTree::Name(n) => {
            out.push(n.ident.to_string());
            true
        }
        UseTree::Rename(r) => {
            if r.rename != "_" {
                out.push(r.rename.to_string());
            }
            true
        }
        UseTree::Glob(_) => false,
        UseTree::Group(g) => g.items.iter().all(|i| use_names(i, out)),
    }
}
