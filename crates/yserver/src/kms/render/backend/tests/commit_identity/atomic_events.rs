use super::support::{AtomicEventFixture, accepted_event_id};
use crate::{
    drm::atomic::{
        AtomicCommitEvent, AtomicCommitKind,
        tests_support::{AtomicCommitRecord, ScriptedResult},
    },
    kms::render::platform::CrtcKey,
};
use std::{os::fd::AsFd as _, time::Duration};
use yserver_core::backend::Backend;

fn raw<H: ::drm::control::ResourceHandle>(value: u32) -> H {
    ::drm::control::from_u32(value).expect("nonzero test handle")
}

fn record(
    user_data: u64,
    flags: ::drm::control::AtomicCommitFlags,
    events: Vec<AtomicCommitEvent>,
    result: ScriptedResult,
) -> AtomicCommitRecord {
    AtomicCommitRecord {
        flags,
        kind: AtomicCommitKind::Scene,
        properties: Vec::new(),
        events,
        result,
        device_fd: 7,
        user_data,
    }
}

#[test]
// Mutation: drain every device's event source on one DRM-fd readiness notification.
fn p1_fixture_routes_events_only_from_the_ready_fd() {
    use std::os::fd::AsRawFd as _;

    let mut fixture = AtomicEventFixture::new(2);
    let crtc = raw(1);
    let fd_a = fixture.backend.platform.devices[0]
        .device
        .as_fd()
        .as_raw_fd();
    let fd_b = fixture.backend.platform.devices[1]
        .device
        .as_fd()
        .as_raw_fd();
    let key_a = fixture.backend.platform.devices[0].key;
    let key_b = fixture.backend.platform.devices[1].key;
    assert_ne!(fd_a, fd_b);
    assert_eq!(
        u32::from(crtc),
        u32::from(fixture.backend.platform.outputs[0].output.crtc)
    );
    assert_eq!(
        u32::from(crtc),
        u32::from(fixture.backend.platform.outputs[1].output.crtc)
    );

    fixture.queue_commit_completion(fd_a, crtc, 0x1_0000_0001, 31, Duration::from_millis(900));
    fixture.queue_commit_completion(fd_b, crtc, 0x2_0000_0002, 42, Duration::from_millis(1200));
    assert_eq!(fixture.event_sources[&fd_a].queued_flip_count(), 1);
    assert_eq!(fixture.event_sources[&fd_b].queued_flip_count(), 1);

    Backend::on_page_flip_ready(&mut fixture.backend, &mut fixture.state, fd_a);

    assert_eq!(
        fixture
            .backend
            .platform
            .present_get_ust_msc(CrtcKey::new(key_a, crtc)),
        (31, 900_000)
    );
    assert_eq!(
        fixture
            .backend
            .platform
            .present_get_ust_msc(CrtcKey::new(key_b, crtc)),
        (0, 0)
    );
    assert_eq!(fixture.event_sources[&fd_a].queued_flip_count(), 0);
    assert_eq!(fixture.event_sources[&fd_b].queued_flip_count(), 1);

    Backend::on_page_flip_ready(&mut fixture.backend, &mut fixture.state, fd_b);

    assert_eq!(
        fixture
            .backend
            .platform
            .present_get_ust_msc(CrtcKey::new(key_b, crtc)),
        (42, 1_200_000)
    );
    assert_eq!(fixture.event_sources[&fd_a].queued_flip_count(), 0);
    assert_eq!(fixture.event_sources[&fd_b].queued_flip_count(), 0);
    fixture.assert_quiescent();
}

#[test]
// Mutation: accept rejected/TEST_ONLY cookies or truncate accepted cookies to u32.
fn p1_fixture_acceptance_reader_excludes_nonaccepted_records() {
    use ::drm::control::AtomicCommitFlags as Flags;

    let event = vec![AtomicCommitEvent {
        crtc: raw(1),
        motion_timestamp: None,
    }];
    let cookie = 0x1_0000_0005;
    let success = record(
        cookie,
        Flags::PAGE_FLIP_EVENT,
        event.clone(),
        ScriptedResult::Ok,
    );
    let rejected = record(
        cookie,
        Flags::PAGE_FLIP_EVENT,
        event.clone(),
        ScriptedResult::Error {
            kind: std::io::ErrorKind::InvalidInput,
            raw_os_error: Some(libc::EINVAL),
        },
    );
    let test_only = record(
        cookie,
        Flags::PAGE_FLIP_EVENT | Flags::TEST_ONLY,
        event.clone(),
        ScriptedResult::Ok,
    );
    let no_event = record(cookie, Flags::NONBLOCK, Vec::new(), ScriptedResult::Ok);
    let zero_cookie = record(0, Flags::PAGE_FLIP_EVENT, event, ScriptedResult::Ok);

    assert_eq!(accepted_event_id(&success), Some(cookie));
    assert_eq!(accepted_event_id(&rejected), None);
    assert_eq!(accepted_event_id(&test_only), None);
    assert_eq!(accepted_event_id(&no_event), None);
    assert_eq!(accepted_event_id(&zero_cookie), None);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: replace the live scene with a fake flip override, omit backend
// event delivery, or suppress re-damage during deferred-release draining.
fn p1_recording_scene_smoke_completes_through_backend() {
    use std::os::fd::{AsFd as _, AsRawFd as _};

    let Some(mut fixture) = super::recording_scene::RecordingSceneFixture::new(1)
        .expect("recording scene fixture construction")
    else {
        return;
    };
    let root = fixture.events.backend.core.window_id;
    let width = fixture.events.backend.platform.fb_w;
    let height = fixture.events.backend.platform.fb_h;
    Backend::clear_area(
        &mut fixture.events.backend,
        None,
        root,
        0,
        None,
        0,
        0,
        width,
        height,
        (0, 0),
    )
    .expect("damage root through Backend");
    Backend::maybe_composite(&mut fixture.events.backend).expect("compose through Backend");

    let fd = fixture.events.backend.platform.devices[0]
        .device
        .as_fd()
        .as_raw_fd();
    let records = fixture.events.recorders[&fd].records();
    assert!(
        !records.is_empty(),
        "the real atomic recorder must see submit"
    );
    assert!(records.iter().all(|record| record.device_fd == fd));
    assert!(fixture.events.backend.scene.has_pending_page_flips());
    // Task 1's pre-Task-2 smoke explicitly exercises the zero-cookie path.
    for record in records.iter().filter(|record| {
        record.result == ScriptedResult::Ok
            && record
                .flags
                .contains(::drm::control::AtomicCommitFlags::PAGE_FLIP_EVENT)
            && !record
                .flags
                .contains(::drm::control::AtomicCommitFlags::TEST_ONLY)
    }) {
        for event in &record.events {
            fixture.events.queue_commit_completion(
                fd,
                event.crtc,
                0,
                51,
                Duration::from_millis(1500),
            );
        }
    }
    assert_eq!(fixture.events.event_sources[&fd].queued_flip_count(), 1);
    Backend::on_page_flip_ready(&mut fixture.events.backend, &mut fixture.events.state, fd);
    Backend::maybe_composite(&mut fixture.events.backend)
        .expect("drain normal scene completion work");
    // The helper drives a new damaged frame so SceneCompositor::tick reaches
    // its production deferred-release drain point.
    fixture.drain_deferred_pool_releases(Duration::from_secs(5));

    fixture.assert_quiescent();
    assert!(
        fixture
            .events
            .backend
            .platform
            .scanout_pools
            .iter()
            .filter_map(Option::as_ref)
            .flat_map(|scanout| scanout.display_pool().bos.iter())
            .all(|bo| bo.state.phase != crate::kms::vk::scanout::BoPhase::Pending),
        "normal Backend retirement must leave no Pending scanout BO"
    );
    let root = fixture.events.backend.core.window_id;
    for xid in [0x6a00, 0x6a01, root] {
        Backend::free_pixmap(&mut fixture.events.backend, None, xid)
            .expect("free recording scene drawable through Backend");
    }
    for _ in 0..32 {
        Backend::before_block(&mut fixture.events.backend);
    }
    assert_eq!(fixture.events.backend.store.pending_retire_count(), 0);
    assert!(fixture.events.backend.store.lookup(root).is_none());
    fixture.finish();
}
