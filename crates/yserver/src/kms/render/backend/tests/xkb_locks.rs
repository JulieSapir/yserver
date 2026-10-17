//! `testdata/xorg-xkb-locks.txt` (Xvfb 21.1.25, issue #253): the keyboard
//! state across keymap reloads, replayed through the core loop.

use super::*;

/// XKB event types the replay compares.
const STATE_NOTIFY: u8 = 2;

/// One `> STEP` of the golden and what Xorg answered.
struct LocksStep {
    step: String,
    /// The `= state` line of a `state` step.
    state: Option<String>,
    /// XKB events on the listener selecting everything, and on the one
    /// selecting only StateNotify with the case's details.
    all: Vec<Vec<u8>>,
    selected: Vec<Vec<u8>>,
}

struct LocksCase {
    name: String,
    details: u16,
    steps: Vec<LocksStep>,
}

fn locks_raw(line: &str) -> Vec<u8> {
    let h = line.rsplit("raw=").next().unwrap();
    (0..32)
        .map(|i| u8::from_str_radix(&h[2 * i..2 * i + 2], 16).unwrap())
        .collect()
}

fn parse_locks_golden(text: &str) -> Vec<LocksCase> {
    let mut cases: Vec<LocksCase> = Vec::new();
    for line in text.lines() {
        if let Some(c) = line.strip_prefix("## case ") {
            let mut f = c.split(": ");
            let name = f.next().unwrap().to_owned();
            let details = f.next().unwrap().strip_prefix("-s ").unwrap();
            cases.push(LocksCase {
                name,
                details: u16::from_str_radix(details, 16).unwrap(),
                steps: Vec::new(),
            });
        } else if let Some(s) = line.strip_prefix("> ") {
            cases.last_mut().unwrap().steps.push(LocksStep {
                step: s.to_owned(),
                state: None,
                all: Vec::new(),
                selected: Vec::new(),
            });
        } else if let Some(step) = cases.last_mut().and_then(|c| c.steps.last_mut()) {
            if let Some(s) = line.strip_prefix("= state ") {
                step.state = Some(s.to_owned());
            } else if line.starts_with("e xkb ") {
                step.all.push(locks_raw(line));
            } else if line.starts_with("e xkbs ") {
                step.selected.push(locks_raw(line));
            }
        }
    }
    cases
}

/// The `KEY=0xNN` / `KEY=N` field of a golden line.
fn locks_field(line: &str, key: &str) -> u32 {
    let v = line
        .split(' ')
        .find_map(|f| f.strip_prefix(key)?.strip_prefix('='))
        .unwrap_or_else(|| panic!("{key} in {line}"));
    v.strip_prefix("0x").map_or_else(
        || v.parse().unwrap(),
        |h| u32::from_str_radix(h, 16).unwrap(),
    )
}

/// A gb server as the probe sees it: an XKB listener (all events, all
/// details), one selecting only StateNotify with `details`, and an
/// XKB-initialised actor.
struct LocksServer {
    backend: KmsBackend,
    state: yserver_core::server::ServerState,
    listener: std::os::unix::net::UnixStream,
    selective: std::os::unix::net::UnixStream,
    actor: std::os::unix::net::UnixStream,
}

const LISTENER: u32 = 5;
const SELECTIVE: u32 = 6;
const ACTOR: u32 = 7;

impl LocksServer {
    fn new(details: u16) -> Self {
        let mut backend = kbd_map_backend("gb", None);
        let mut state = yserver_core::server::ServerState::new();
        let listener = kbd_map_client_id(&mut state, LISTENER);
        let selective = kbd_map_client_id(&mut state, SELECTIVE);
        let actor = kbd_map_client_id(&mut state, ACTOR);
        for client in [LISTENER, SELECTIVE, ACTOR] {
            xkb_client_request(&mut state, &mut backend, client, 136, 0, &[1, 0, 0, 0]);
        }
        let select =
            |fields: &[u16]| -> Vec<u8> { fields.iter().flat_map(|v| v.to_le_bytes()).collect() };
        let all = select(&[0x100, 0x0fff, 0, 0x0fff, 0xff, 0xff]);
        xkb_client_request(&mut state, &mut backend, LISTENER, 136, 1, &all);
        // affectWhich StateNotify, its details: affectState, stateDetails.
        let some = select(&[0x100, 0x0004, 0, 0, 0, 0, 0x3fff, details]);
        xkb_client_request(&mut state, &mut backend, SELECTIVE, 136, 1, &some);
        let mut server = Self {
            backend,
            state,
            listener,
            selective,
            actor,
        };
        server.drain();
        server
    }

    /// The XKB events since the last drain: `(listener, selective)`.
    fn drain(&mut self) -> (Vec<u8>, Vec<u8>) {
        let _ = kbd_map_drain(&mut self.actor);
        (
            kbd_map_drain(&mut self.listener),
            kbd_map_drain(&mut self.selective),
        )
    }

    /// One probe step from the actor (`state` is a query, answered here).
    fn step(&mut self, step: &str) -> (Vec<u8>, Vec<u8>) {
        let (s, b) = (&mut self.state, &mut self.backend);
        if let Some(layout) = step
            .strip_prefix("run:setxkbmap ")
            .and_then(|r| r.rsplit_once("-layout "))
            .map(|(_, l)| l)
        {
            xkb_client_request(s, b, ACTOR, 136, 23, &get_kbd_by_name_body(layout));
        } else if let Some(m) = step.strip_prefix("lock:") {
            let (affect, mods) = m.split_once(':').unwrap();
            let mut body = [0u8; 12];
            body[..2].copy_from_slice(&0x100u16.to_le_bytes());
            body[2] = u8::from_str_radix(affect, 16).unwrap();
            body[3] = u8::from_str_radix(mods, 16).unwrap();
            xkb_client_request(s, b, ACTOR, 136, 5, &body);
        } else if let Some(kc) = step
            .strip_prefix("down:")
            .map(|k| (2, k))
            .or_else(|| step.strip_prefix("up:").map(|k| (3, k)))
        {
            let mut body = [0u8; 32];
            body[0] = kc.0;
            body[1] = kc.1.parse().unwrap();
            xkb_client_request(s, b, ACTOR, 146, 2, &body);
        } else {
            panic!("step {step}");
        }
        self.drain()
    }

    /// Our `= state` fields, through GetState on the listener.
    fn state_fields(&mut self) -> Vec<(&'static str, u32)> {
        let _ = self.drain();
        xkb_client_request(
            &mut self.state,
            &mut self.backend,
            LISTENER,
            136,
            4,
            &[0x00, 0x01, 0, 0],
        );
        let r = kbd_map_drain(&mut self.listener);
        assert_eq!(r.len(), 32, "GetState reply {r:02x?}");
        assert_eq!(r[0], 1, "GetState reply {r:02x?}");
        vec![
            ("mods", u32::from(r[8])),
            ("base", u32::from(r[9])),
            ("latched", u32::from(r[10])),
            ("locked", u32::from(r[11])),
            ("group", u32::from(r[12])),
            ("lockedGroup", u32::from(r[13])),
        ]
    }
}

/// XkbGetKbdByName as setxkbmap sends it for `layout` (evdev, pc105).
fn get_kbd_by_name_body(layout: &str) -> Vec<u8> {
    let mut symbols = String::from("pc");
    for (i, l) in layout.split(',').enumerate() {
        symbols.push('+');
        symbols.push_str(l);
        if i > 0 {
            symbols.push_str(&format!(":{}", i + 1));
        }
    }
    symbols.push_str("+inet(evdev)");
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&0x0100u16.to_le_bytes()); // deviceSpec
    body.extend_from_slice(&0x00bfu16.to_le_bytes()); // need
    body.extend_from_slice(&0x00ffu16.to_le_bytes()); // want
    body.extend_from_slice(&[1, 0]); // load, pad
    for s in [
        "",
        "evdev+aliases(qwerty)",
        "complete",
        "complete",
        &symbols,
        "pc(pc105)",
    ] {
        body.push(u8::try_from(s.len()).unwrap());
        body.extend_from_slice(s.as_bytes());
    }
    while !body.len().is_multiple_of(4) {
        body.push(0);
    }
    body
}

/// Golden (`xorg-xkb-locks.txt`, Xvfb 21.1.25): a reload by name keeps the
/// locked modifiers, and each held lock key still locks or unlocks on its
/// release as before the reload: Xorg's state after every `state` step.
#[test]
fn reload_keeps_locks_and_held_keys_as_xorg() {
    let cases = parse_locks_golden(include_str!("../../../testdata/xorg-xkb-locks.txt"));
    let mut failures = Vec::new();
    let mut checked = 0;
    // The locked group across a reload is not ported (yserver resets it).
    for case in cases
        .iter()
        .filter(|c| c.name.starts_with("reload-") && c.name != "reload-group-lock")
    {
        let mut server = LocksServer::new(case.details);
        for (i, step) in case.steps.iter().enumerate() {
            if step.step != "state" {
                let _ = server.step(&step.step);
                continue;
            }
            let xorg = step.state.as_deref().expect("state line");
            for (key, ours) in server.state_fields() {
                let want = locks_field(xorg, key);
                if ours != want {
                    failures.push(format!(
                        "{} step {i}: {key} ours {ours:#x} xorg {want:#x}",
                        case.name
                    ));
                }
            }
            checked += 1;
        }
    }
    assert_eq!(checked, 26, "state steps checked");
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}

/// The events of `types` in `wire` (32-byte XKB events), against Xorg's:
/// all but sequence, time and device (Xorg's 3, our 1); Xvfb's XKB major
/// 135 is our 136.
fn locks_compare_events(
    what: &str,
    wire: &[u8],
    xorg: &[Vec<u8>],
    types: &[u8],
    failures: &mut Vec<String>,
) {
    let ours: Vec<&[u8]> = wire
        .chunks(32)
        .filter(|e| e[0] & 0x7f == 85 && types.contains(&e[1]))
        .collect();
    let xorg: Vec<Vec<u8>> = xorg
        .iter()
        .filter(|e| types.contains(&e[1]))
        .map(|e| {
            let mut e = e.clone();
            if e[1] == STATE_NOTIFY && e[30] == 135 {
                e[30] = 136;
            }
            e
        })
        .collect();
    let same = ours.len() == xorg.len()
        && ours
            .iter()
            .zip(&xorg)
            .all(|(o, x)| o[..2] == x[..2] && o[8] == 1 && o[9..] == x[9..]);
    if !same {
        failures.push(format!("{what}:\n  ours {ours:02x?}\n  xorg {xorg:02x?}"));
    }
}

/// Golden (`xorg-xkb-locks.txt`, Xvfb 21.1.25): XkbStateNotify from key
/// presses (XTEST), XkbLockModifiers and reloads, with Xorg's `changed`
/// (`XkbStateChangedFlags`), on a listener selecting every detail and on
/// one selecting only some (`XkbSendStateNotify`: `stateNotifyMask &
/// changed`).
#[test]
fn state_notifies_match_xorg() {
    let cases = parse_locks_golden(include_str!("../../../testdata/xorg-xkb-locks.txt"));
    assert_eq!(cases.len(), 16, "golden parsed");
    let mut failures = Vec::new();
    // The locked group across a reload is not ported (yserver resets it).
    for case in cases.iter().filter(|c| c.name != "reload-group-lock") {
        let mut server = LocksServer::new(case.details);
        for (i, step) in case.steps.iter().enumerate() {
            if step.step == "state" {
                continue;
            }
            let (all, selected) = server.step(&step.step);
            let what = format!("{} step {i} {}", case.name, step.step);
            let types = [STATE_NOTIFY];
            locks_compare_events(&what, &all, &step.all, &types, &mut failures);
            let what = format!("{what} (selective)");
            locks_compare_events(&what, &selected, &step.selected, &types, &mut failures);
        }
    }
    assert!(failures.is_empty(), "{}", failures.join("\n"));
}
