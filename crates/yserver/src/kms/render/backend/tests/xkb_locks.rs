//! `testdata/xorg-xkb-locks.txt` (Xvfb 21.1.25, issue #253): the keyboard
//! state across keymap reloads, replayed through the core loop.

use super::*;

/// One `> STEP` of the golden and what Xorg answered.
struct LocksStep {
    step: String,
    /// The `= state` line of a `state` step.
    state: Option<String>,
}

struct LocksCase {
    name: String,
    steps: Vec<LocksStep>,
}

fn parse_locks_golden(text: &str) -> Vec<LocksCase> {
    let mut cases: Vec<LocksCase> = Vec::new();
    for line in text.lines() {
        if let Some(c) = line.strip_prefix("## case ") {
            let name = c.split(':').next().unwrap().to_owned();
            cases.push(LocksCase {
                name,
                steps: Vec::new(),
            });
        } else if let Some(s) = line.strip_prefix("> ") {
            cases.last_mut().unwrap().steps.push(LocksStep {
                step: s.to_owned(),
                state: None,
            });
        } else if let Some(s) = line.strip_prefix("= state ") {
            let step = cases.last_mut().unwrap().steps.last_mut().unwrap();
            step.state = Some(s.to_owned());
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
/// details) and an XKB-initialised actor.
struct LocksServer {
    backend: KmsBackend,
    state: yserver_core::server::ServerState,
    listener: std::os::unix::net::UnixStream,
    actor: std::os::unix::net::UnixStream,
}

const LISTENER: u32 = 5;
const ACTOR: u32 = 7;

impl LocksServer {
    fn new() -> Self {
        let mut backend = kbd_map_backend("gb", None);
        let mut state = yserver_core::server::ServerState::new();
        let listener = kbd_map_client_id(&mut state, LISTENER);
        let actor = kbd_map_client_id(&mut state, ACTOR);
        for client in [LISTENER, ACTOR] {
            xkb_client_request(&mut state, &mut backend, client, 136, 0, &[1, 0, 0, 0]);
        }
        let mut select = Vec::new();
        for v in [0x100u16, 0x0fff, 0, 0x0fff, 0xff, 0xff] {
            select.extend_from_slice(&v.to_le_bytes());
        }
        xkb_client_request(&mut state, &mut backend, LISTENER, 136, 1, &select);
        let mut server = Self {
            backend,
            state,
            listener,
            actor,
        };
        server.drain();
        server
    }

    fn drain(&mut self) -> Vec<u8> {
        let _ = kbd_map_drain(&mut self.actor);
        kbd_map_drain(&mut self.listener)
    }

    /// One probe step from the actor (`state` is a query, answered here).
    fn step(&mut self, step: &str) -> Vec<u8> {
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
        let mut server = LocksServer::new();
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
