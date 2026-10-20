//! The presenter shell: installation, event order, live ids (spec §4.2,
//! §4.5.1).

use yserver_core::{
    resources::ROOT_WINDOW,
    rootless::{CoordinatePolicy, RootlessEvent},
};

use std::time::{Duration, Instant};

use yserver_core::backend::Backend as _;

use super::{Call, Deliver, Fixture, Ready, Release, Setup, lock};
use crate::rootless::{
    Note,
    frames::Readiness,
    host::{FrameEvent, FrameSeq},
};

const W: u32 = 0x0010_0001;

/// Rooted surface backends never get a presenter, also with the core's
/// rootless mode on: only `rootless::start` installs it.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn rooted_surface_backend_has_no_presenter() {
    let Some(mut f) = Fixture::rooted_or_skip() else {
        return;
    };
    assert!(f.b.pstate.rootless.is_none());
    yserver_core::rootless::enable(&mut f.state, &mut f.b, CoordinatePolicy::Opaque)
        .expect("core rootless");
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    f.tail();
    assert!(
        f.state
            .rootless
            .as_ref()
            .and_then(|r| r.toplevel_id(yserver_protocol::x11::ResourceId(W)))
            .is_some()
    );
    assert!(
        f.b.pstate.rootless.is_none(),
        "events alone install nothing"
    );
}

/// The host hears of a batch after the presenter applied it (plan Task 3;
/// Task 7 relies on it to drop a swapchain before its surface).
#[test]
#[ignore = "needs live Vulkan ICD"]
fn host_sees_the_batch_after_the_presenter() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.setup = Setup::Withheld) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    f.tail();
    let id = f.id(W);
    let calls = f.calls();
    let added = calls
        .iter()
        .position(|c| *c == Call::Presenter(Note::Added(id)))
        .expect("presenter added W");
    let events = calls
        .iter()
        .position(|c| matches!(c, Call::Events(e) if matches!(e.as_slice(), [RootlessEvent::Mapped { id: m, .. }] if *m == id)))
        .expect("host saw Mapped");
    assert!(added < events, "{calls:?}");
    let host_xid = f.host_xid(W);
    assert_eq!(f.presenter().windows[&id].host_xid, host_xid);

    lock(&f.log).calls.clear();
    f.unmap(W);
    f.tail();
    let calls = f.calls();
    assert_eq!(
        calls,
        [
            Call::Presenter(Note::Removed(id)),
            Call::Events(vec![RootlessEvent::Unmapped { id }])
        ]
    );
    assert!(f.presenter().windows.is_empty());
}

/// An id names one mapped lifetime (spec §4.2): a late event for a retired
/// id does nothing to the window's next lifetime.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn frame_events_for_a_retired_id_are_ignored() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.setup = Setup::Withheld) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    f.tail();
    let first = f.id(W);
    f.unmap(W);
    f.tail();
    f.map(W);
    f.tail();
    let second = f.id(W);
    assert_ne!(first, second);

    f.link.post_frame(FrameEvent::SetupDone { id: first });
    f.tail();
    let presenter = f.presenter();
    assert!(!presenter.windows.contains_key(&first));
    assert_eq!(
        presenter.windows[&second].frames.readiness(),
        Readiness::Setup
    );
}

const BUSY_TAILS: usize = 5;

fn wanted(f: &Fixture) -> usize {
    f.calls()
        .iter()
        .filter(|c| matches!(c, Call::FrameWanted(_)))
        .count()
}

fn cpu_frames(f: &Fixture) -> usize {
    lock(&f.log).frames.len()
}

/// §4.5.1: no frame before `SetupDone`; §4.1: damage while the host is not
/// ready sets no deadline.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn withheld_setup_gets_no_frame_and_no_wakeup() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.setup = Setup::Withheld) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    for _ in 0..BUSY_TAILS {
        f.fill(W, 0x00ff_0000, (0, 0, 8, 8));
        f.tail();
    }
    assert_eq!(cpu_frames(&f), 0);
    assert!(!f.in_flight(W));
    assert_eq!(f.wakeups(Instant::now()).0, None);
}

/// §4.5.1: `SetupDone` alone (no `Ready`) starts the first frame in the
/// same tail; `first_frame` follows the host's `Delivered`.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn setup_done_alone_starts_the_first_frame_in_the_same_tail() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| {
        log.setup = Setup::Withheld;
        log.ready = Ready::Manual;
        log.deliver = Deliver::Manual;
    }) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    f.tail();
    let id = f.id(W);
    assert!(!f.in_flight(W));
    f.link.post_frame(FrameEvent::SetupDone { id });
    f.tail();
    assert!(f.in_flight(W), "SetupDone alone starts the first frame");
    f.frames_until(1);
    f.tail();
    assert!(!f.calls().contains(&Call::FirstFrame(id)));
    f.link.post_frame(FrameEvent::Delivered {
        id,
        seq: FrameSeq(1),
    });
    f.tail();
    assert!(f.calls().contains(&Call::FirstFrame(id)));
}

/// §4.5.1: later frames wait for `Ready`. Paint while Busy asks the host
/// once (`frame_wanted`, D7) and the loop does not spin meanwhile.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn later_frames_wait_for_ready() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.ready = Ready::Manual) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    f.frames_until(1);
    f.tail();
    let id = f.id(W);
    assert_eq!(wanted(&f), 0, "nothing waits");
    f.fill(W, 0x0000_ff00, (3, 3, 5, 5));
    for _ in 0..BUSY_TAILS {
        f.tail();
        let now = Instant::now();
        let (presenter, backend) = f.wakeups(now);
        assert_eq!(presenter, None, "no deadline while Busy");
        assert!(backend.is_none_or(|t| t > now), "the loop would spin");
    }
    assert_eq!(cpu_frames(&f), 1);
    assert!(!f.in_flight(W));
    assert_eq!(wanted(&f), 1, "{:?}", f.calls());
    f.link.post_frame(FrameEvent::Ready { id });
    let frames = f.frames_until(2);
    assert_eq!(
        frames[1].damage,
        [yserver_core::rootless::Rect {
            x: 3,
            y: 3,
            width: 5,
            height: 5
        }]
    );
}

/// Spec §10 `next_wakeup` cases.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn next_wakeup_rules() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.ready = Ready::Manual) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    f.tail();
    f.tail();
    let id = f.id(W);
    assert!(f.in_flight(W));
    let now = Instant::now();
    let poll = f.wakeups(now).0.expect("a deadline while in flight");
    assert!(poll > now && poll <= now + Duration::from_millis(1));

    f.frames_until(1);
    f.tail();
    assert_eq!(f.wakeups(Instant::now()).0, None, "completed, host Busy");

    // Ready + flushed damage, before the tick: now.
    f.link.post_frame(FrameEvent::Ready { id });
    f.b.on_platform_event(&mut f.state);
    f.fill(W, 0x0000_00ff, (0, 0, 4, 4));
    f.b.flush_before_damage_exposure();
    let now = Instant::now();
    assert!(f.wakeups(now).0.is_some_and(|t| t <= now));

    f.frames_until(2);
    f.tail();
    f.link.post_frame(FrameEvent::Ready { id });
    f.tail();
    assert_eq!(f.wakeups(Instant::now()).0, None, "nothing pending");
}

/// The tick and `next_wakeup` share one predicate: an immediate deadline
/// means the next tick makes progress; an exhausted pool sets none.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn tick_and_wakeup_agree() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.release = Release::Never) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    let deadline = Instant::now() + Duration::from_secs(2);
    while cpu_frames(&f) < 3 {
        assert!(Instant::now() < deadline, "frames stalled");
        f.fill(W, 0x0001_0000 * (cpu_frames(&f) as u32 + 1), (0, 0, 4, 4));
        f.tail();
        let now = Instant::now();
        if f.wakeups(now).0.is_some_and(|t| t <= now) {
            let before = f.presenter().progress;
            f.tail();
            assert!(
                f.presenter().progress > before,
                "an immediate deadline made no progress"
            );
        }
    }
    f.fill(W, 0x00ff_ffff, (8, 8, 4, 4));
    for _ in 0..BUSY_TAILS {
        f.tail();
    }
    let before = f.presenter().progress;
    f.tail();
    assert_eq!(f.presenter().progress, before);
    assert_eq!(f.wakeups(Instant::now()).0, None, "pool exhausted: no spin");
    assert_eq!(cpu_frames(&f), 3);
}

/// Without `start` the surface backend's deadline is what it was: nothing
/// pending on a settled headless backend.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn rooted_next_wakeup_unchanged() {
    let Some(mut f) = Fixture::rooted_or_skip() else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    f.fill(W, 0x00ff_0000, (0, 0, 8, 8));
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        f.tail();
        f.b.before_block();
        if f.b.next_wakeup().is_none() {
            break;
        }
        assert!(Instant::now() < deadline, "never settled");
        std::thread::yield_now();
    }
    assert!(f.b.pstate.rootless.is_none());
}

/// RENDER paint still waiting in a render batch while the window is Busy asks
/// the host for a frame (`frame_wanted`) once and is in the next frame.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn batched_render_paint_while_busy_is_seen() {
    use yserver_core::backend::{AnyHandle, WindowHandle};
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.ready = Ready::Manual) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 0);
    f.map(W);
    f.frames_until(1);
    f.tail();
    let id = f.id(W);
    let window = WindowHandle::from_raw(f.host_xid(W)).expect("window handle");
    let pixmap = f.b.create_pixmap(None, 24, 8, 8).expect("pixmap");
    let src =
        f.b.render_create_picture(None, AnyHandle::Pixmap(pixmap), 0, 0, &[])
            .expect("picture")
            .expect("Some")
            .as_raw();
    let dst =
        f.b.render_create_picture(None, AnyHandle::Window(window), 0, 0, &[])
            .expect("picture")
            .expect("Some")
            .as_raw();
    f.tail();
    f.b.render_composite(None, 1, src, 0, dst, 0, 0, 0, 0, 2, 2, 4, 4)
        .expect("composite");
    assert!(
        f.b.engine.has_pending_batches_for_tests(),
        "the composite waits in a batch"
    );
    for _ in 0..BUSY_TAILS {
        f.tail();
    }
    assert_eq!(wanted(&f), 1, "{:?}", f.calls());
    f.link.post_frame(FrameEvent::Ready { id });
    let frames = f.frames_until(2);
    assert!(frames[1].damage.iter().any(|r| r.x <= 2 && r.y <= 2));
}
