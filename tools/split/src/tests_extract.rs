//! Fixtures for `bind` (one per modelled binding form, plus refusals) and
//! for `split extract` / `verify --extract` end to end.

use std::{collections::BTreeMap, path::PathBuf};

use crate::{
    bind,
    extract::{self, Extract, ExtractManifest},
    tests::mem,
    verify::{Spec, check},
};

fn free(code: &str) -> Vec<String> {
    let b: syn::Block = syn::parse_str(&format!("{{ {code} }}")).expect("parse");
    let s = bind::block(&b);
    assert!(s.refused.is_empty(), "refused: {:?}", s.refused);
    s.refs.into_iter().map(|r| r.name).collect()
}

fn refused(code: &str) -> Vec<String> {
    let b: syn::Block = syn::parse_str(&format!("{{ {code} }}")).expect("parse");
    bind::block(&b).refused
}

#[test]
fn bind_let_plain_and_pattern() {
    assert_eq!(
        free("let (a, b) = (x, y); let c2 = a; a + b + c2 + c"),
        ["x", "y", "c"]
    );
}

#[test]
fn bind_let_else_scopes_after_the_statement() {
    assert_eq!(
        free("let Some(v) = opt(v) else { return w(v); }; v"),
        ["opt", "v", "w", "v", "Some"]
    );
}

#[test]
fn bind_if_let_chain_scopes_over_later_conditions_and_then_only() {
    assert_eq!(
        free("if let Some(a) = x && let Some(b) = a && b > c { f(a, b) } else { g(a) }"),
        ["x", "Some", "Some", "c", "f", "g", "a"]
    );
}

#[test]
fn bind_while_let() {
    assert_eq!(
        free("while let Some(a) = it(a) { h(a) }"),
        ["it", "a", "Some", "h"]
    );
}

#[test]
fn bind_for_pattern() {
    assert_eq!(free("for (i, _) in 0..n { k(i) } i"), ["n", "k", "i"]);
}

#[test]
fn bind_match_pattern_and_guard() {
    assert_eq!(
        free(
            "match v { Some(a) if a > lim => a, Some(_) | None => b, LIMIT => 0, other => other }"
        ),
        ["v", "Some", "lim", "Some", "None", "b", "LIMIT"]
    );
}

#[test]
fn bind_matches_macro_pattern_and_guard() {
    assert_eq!(
        free("matches!(body.get(5), Some(&p) if p != q)"),
        ["body", "Some", "q"]
    );
}

#[test]
fn bind_closure_params_shadow() {
    assert_eq!(
        free("let f = |state: &ServerState, buf| { buf.push(state.x); c }; f(state, buf)"),
        ["ServerState", "c", "state", "buf"]
    );
}

#[test]
fn bind_block_items_and_uses_cover_their_block_but_not_item_bodies() {
    assert_eq!(
        free("let loc = 1; k(); fn k() -> u8 { q + loc } const Q: u8 = 1; use m::z; z + Q + loc"),
        ["u8", "q", "loc", "u8", "m"]
    );
}

#[test]
fn bind_nested_fn_params() {
    assert_eq!(
        free("fn part(a: u8) -> u8 { a + b } part(1)"),
        ["u8", "u8", "b"]
    );
}

#[test]
fn bind_format_captures_and_named_args() {
    assert_eq!(
        free(r#"debug!("{a} {b:?} {:w$} {{c}} {z:>pad$}", x, w = width); a"#),
        ["x", "width", "a", "b", "z", "pad", "a"]
    );
    assert_eq!(
        free(r#"let a = 1; log::warn!("{a}");"#),
        Vec::<String>::new()
    );
}

#[test]
fn bind_struct_shorthand_and_paths() {
    assert_eq!(
        free("let s = S { field, other: o }; x11::error::BAD + T::new()"),
        ["S", "field", "o", "x11", "T"]
    );
}

#[test]
fn bind_refuses_other_forms() {
    for code in [
        "foo!(x)",
        "async { x }",
        "let v = (let a = b);",
        "macro_rules! m { () => {} }",
        "use m::*;",
        "fn g<T>(t: T) {}",
        "impl X {}",
        "a::b!(x)",
        r#"debug!(target: "t", "x")"#,
    ] {
        assert!(!refused(code).is_empty(), "{code}");
    }
}

#[test]
fn format_names_parse() {
    assert_eq!(
        bind::format_names("{} {0} {a} {b:?} {c:>w$.p$} {{d}} }} {e:1$}"),
        ["a", "b", "c", "w", "p", "e"]
    );
}

// ─── split extract / verify --extract end to end ───

const BASE: &str = r#"
EXTRA
pub fn sink(_: &mut u32, buf: &[u8]) -> Result<u32, ()> {
    Ok(buf.len() as u32)
}

pub fn dispatch(state: &mut u32, body: &[u8], minor: u8) -> Result<u32, ()> {
    LOCAL
    let order = minor + 1;
    let mut buf: Vec<u8> = Vec::with_capacity(8);
    match minor {
        // lead-in one
        1 => {
            ARM1
        }
        2 => {
            // inner two
            return Ok(u32::from(body[0]));
        }
        _ => {
            return Err(());
        }
    }
    let n = sink(state, &buf)?;
    Ok(n)
}
"#;

fn base(arm: &str, extra: &str, local: &str) -> String {
    BASE.replace("ARM1", arm)
        .replace("EXTRA", extra)
        .replace("LOCAL", local)
}

fn manifest(hoist: &[&str], by_hand: bool) -> ExtractManifest {
    let s = |v: &[(&str, &str)]| -> BTreeMap<String, String> {
        v.iter().map(|(a, b)| ((*a).into(), (*b).into())).collect()
    };
    let (arms, keep, locals) = if hoist.is_empty() {
        (
            s(&[("1", "arm_one"), ("2", "arm_two")]),
            vec!["_".to_string()],
            s(&[("order", "u8")]),
        )
    } else {
        Default::default()
    };
    ExtractManifest {
        source: "src/d.rs".into(),
        module: "d".into(),
        file: "src/d.rs".into(),
        extract: Extract {
            func: "dispatch".into(),
            scrutinee: "minor".into(),
            locals,
            prelude: hoist.is_empty().then(|| "buf".into()),
            tail: hoist.is_empty().then(|| "write_out".into()),
            hoist: hoist.iter().map(|h| (*h).to_string()).collect(),
            keep,
            arms,
            by_hand: if by_hand {
                s(&[("1", "fixture: unknown macro")])
            } else {
                BTreeMap::new()
            },
        },
        path: PathBuf::new(),
    }
}

fn verify_with(
    m: &ExtractManifest,
    before: &str,
    after: &str,
    other: &str,
) -> (Vec<String>, Vec<String>) {
    let spec = Spec {
        manifest: None,
        old_root: m.source.clone(),
        new_root: m.source.clone(),
        module: m.module.clone(),
        delegate: None,
        extract: Some(m),
    };
    let b = mem(&[("src/d.rs", before), ("src/other.rs", other)]);
    let a = mem(&[("src/d.rs", after), ("src/other.rs", other)]);
    check(&spec, &b, &a, &[]).expect("check")
}

fn verify(m: &ExtractManifest, before: &str, after: &str) -> Vec<String> {
    verify_with(m, before, after, "").1
}

fn generated(m: &ExtractManifest, before: &str) -> String {
    extract::rewrite(m, before.to_string()).expect("rewrite")
}

fn has(errs: &[String], needle: &str) {
    assert!(
        errs.iter().any(|e| e.contains(needle)),
        "no {needle:?} in {errs:#?}"
    );
}

/// `text` with `from` replaced; the fixture must actually change it.
fn edit(text: &str, from: &str, to: &str) -> String {
    assert!(text.contains(from), "{from:?} not in\n{text}");
    text.replacen(from, to, 1)
}

#[test]
fn extract_round_trip_passes() {
    let m = manifest(&[], false);
    let b = base("buf.push(order);", "", "");
    let a = generated(&m, &b);
    assert!(a.contains("1 => arm_one(state, order),"), "{a}");
    assert!(
        a.contains("#[allow(clippy::needless_return)]\nfn arm_two("),
        "{a}"
    );
    assert!(
        a.contains("fn write_out(\n    state: &mut u32,\n    buf: Vec<u8>,\n)"),
        "{a}"
    );
    assert!(a.contains("    write_out(state, buf)\n}"), "{a}");
    assert_eq!(verify(&m, &b, &a), Vec::<String>::new());
}

#[test]
fn void_arm_gets_the_empty_tail_write_and_a_tail_only_param() {
    let m = manifest(&[], false);
    let b = base("let _ = body;", "", "");
    let a = generated(&m, &b);
    assert!(
        a.contains("fn arm_one(\n    state: &mut u32,\n    body: &[u8],\n)"),
        "{a}"
    );
    assert!(a.contains("write_out(state, Vec::new())"), "{a}");
    assert!(
        !a[a.find("fn arm_one").unwrap()..].contains("with_capacity"),
        "{a}"
    );
    assert_eq!(verify(&m, &b, &a), Vec::<String>::new());
    let extra = edit(
        &a,
        "    body: &[u8],\n)",
        "    body: &[u8],\n    minor: u8,\n)",
    );
    let extra = edit(
        &extra,
        "arm_one(state, body)",
        "arm_one(state, body, minor)",
    );
    has(&verify(&m, &b, &extra), "want");
}

#[test]
fn closure_param_shadowing_needs_no_prelude() {
    let m = manifest(&[], false);
    let b = base(
        "let f = |buf: &mut Vec<u8>, state: u8| buf.push(state); let mut v = Vec::new(); f(&mut v, order);",
        "",
        "",
    );
    let a = generated(&m, &b);
    let callee = &a[a.find("fn arm_one").unwrap()..];
    assert!(!callee.contains("with_capacity"), "{callee}");
    assert!(callee.contains("write_out(state, Vec::new())"), "{callee}");
    assert_eq!(verify(&m, &b, &a), Vec::<String>::new());
}

#[test]
fn forgotten_param_against_a_module_item_fails() {
    let m = manifest(&[], false);
    let b = base("buf.push(order);", "static order: u8 = 0;", "");
    let a = generated(&m, &b);
    assert_eq!(verify(&m, &b, &a), Vec::<String>::new());
    let a = edit(&a, "    order: u8,\n", "");
    let a = edit(&a, "arm_one(state, order)", "arm_one(state)");
    has(
        &verify(&m, &b, &a),
        "`order` was bound to Outer(\"order\"), now to Module",
    );
}

#[test]
fn arm_local_items_pass() {
    let m = manifest(&[], false);
    let b = base(
        "const K: u8 = 3; use std::cmp::max; buf.push(max(K, order));",
        "",
        "",
    );
    let a = generated(&m, &b);
    assert_eq!(verify(&m, &b, &a), Vec::<String>::new());
}

#[test]
fn unsupported_form_is_refused_unless_by_hand() {
    let b = base("foo!(order); buf.push(1);", "", "");
    let err = extract::rewrite(&manifest(&[], false), b.clone()).unwrap_err();
    assert!(err.contains("refused"), "{err}");
    let hand = manifest(&[], true);
    let a = generated(&hand, &b);
    has(
        &verify(&manifest(&[], false), &b, &a),
        "binding forms outside the resolver",
    );
    let (info, errs) = verify_with(&hand, &b, &a, "");
    assert_eq!(errs, Vec::<String>::new());
    assert!(info.iter().any(|i| i.contains("by hand")), "{info:?}");
}

#[test]
fn tail_inside_the_arm_block_fails() {
    let m = manifest(&[], false);
    let b = base("buf.push(order);", "", "");
    let a = generated(&m, &b);
    let moved = edit(
        &a,
        "buf.push(order);\n    }\n    write_out(state, buf)\n}",
        "buf.push(order);\n    write_out(state, buf)\n    }\n}",
    );
    has(&verify(&m, &b, &moved), "body is none of");
    let flat = edit(
        &a,
        "    {\n        buf.push(order);\n    }\n",
        "    buf.push(order);\n",
    );
    has(&verify(&m, &b, &flat), "body is none of");
}

#[test]
fn dropped_lead_in_comment_fails() {
    let m = manifest(&[], false);
    let b = base("buf.push(order);", "", "");
    let a = generated(&m, &b);
    has(
        &verify(&m, &b, &edit(&a, "// lead-in one\n", "")),
        "comments differ",
    );
}

#[test]
fn changed_kept_arm_or_extra_attribute_fails() {
    let m = manifest(&[], false);
    let b = base("buf.push(order);", "", "");
    let a = generated(&m, &b);
    has(
        &verify(&m, &b, &edit(&a, "Err(())", "Err(()).or(Ok(0))")),
        "kept arm _",
    );
    has(
        &verify(
            &m,
            &b,
            &edit(&a, "\nfn arm_one(", "\n#[inline]\nfn arm_one("),
        ),
        "attributes other than needless_return",
    );
}

#[test]
fn callee_named_before_the_change_fails() {
    let m = manifest(&[], false);
    let b = base("buf.push(order);", "", "");
    let a = generated(&m, &b);
    has(
        &verify_with(&m, &b, &a, "fn arm_one() {}").1,
        "already occurs before the change",
    );
}

#[test]
fn fn_local_helper_hoists_then_extracts() {
    let local = "fn helper() -> u8 { 2 }";
    let b = base("buf.push(helper() + order);", "", local);
    let err = extract::rewrite(&manifest(&[], false), b.clone()).unwrap_err();
    assert!(err.contains("hoist it first"), "{err}");
    let prep = manifest(&["fn helper"], false);
    let hoisted = generated(&prep, &b);
    assert_eq!(verify(&prep, &b, &hoisted), Vec::<String>::new());
    let m = manifest(&[], false);
    let a = generated(&m, &hoisted);
    assert_eq!(verify(&m, &hoisted, &a), Vec::<String>::new());
}

#[test]
fn hoisted_name_shadowing_a_glob_import_fails() {
    let extra = "mod other {\n    pub fn helper() -> u8 { 1 }\n}\nuse other::*;\npub fn user() -> u8 { helper() }\n";
    let b = base("buf.push(helper());", extra, "fn helper() -> u8 { 2 }");
    let prep = manifest(&["fn helper"], false);
    let hoisted = generated(&prep, &b);
    has(
        &verify(&prep, &b, &hoisted),
        "fn user: name `helper` resolves differently",
    );
}
