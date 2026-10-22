use super::support::{
    AtomicPathFixture, PathKind, accept_cursor_detach, accepted_live_records,
    applied_cursor_binding, assert_no_cursor_resources, create_argb_cursor, damage_root,
    define_root_cursor, finish_atomic_fixture, has_unretired_accepted_event, record_property,
    retire_cursor_record, retire_pending_output, sprite_bytes,
};
use crate::drm::atomic::AtomicCommitKind;
use std::{
    collections::HashSet,
    time::{Duration, Instant},
};
use yserver_core::backend::{Backend, CursorHandle};

fn create_animation(
    backend: &mut crate::kms::render::backend::KmsBackend,
    frames: &[([u8; 4], (u16, u16))],
) -> CursorHandle {
    let cursors: Vec<_> = frames
        .iter()
        .map(|(pixel, hotspot)| create_argb_cursor(backend, (4, 4), *pixel, *hotspot))
        .collect();
    let animation_frames: Vec<_> = cursors
        .iter()
        .copied()
        .map(|cursor| (cursor, 60_000))
        .collect();
    Backend::create_anim_cursor(backend, None, &animation_frames)
        .expect("CreateAnimCursor through Backend")
        .expect("KMS animation handle")
}

fn compose_show(fixture: &mut AtomicPathFixture, cursor: CursorHandle) {
    let backend = &mut fixture.scene.events.backend;
    define_root_cursor(backend, cursor);
    Backend::warp_pointer_root(backend, &mut fixture.scene.events.state, 100, 90);
    damage_root(fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("compose ShowOnRetire through Backend");
    assert!(
        has_unretired_accepted_event(&fixture.scene.events, &HashSet::new(), 0),
        "ShowOnRetire is queued on an accepted scene event"
    );
}

fn advance_animation(fixture: &mut AtomicPathFixture, expected_frame: usize) {
    let backend = &mut fixture.scene.events.backend;
    let animation = backend
        .active_cursor_anim
        .as_mut()
        .expect("CreateAnimCursor arms the animation timer");
    animation.next_frame = Instant::now() - Duration::from_millis(1);
    Backend::maybe_composite(backend).expect("advance cursor through the Backend timer path");
    assert_eq!(
        backend.active_cursor_anim.as_ref().unwrap().frame,
        expected_frame,
        "one expired timer advances one animation frame"
    );
}

fn sprite_image_matches(
    fixture: &AtomicPathFixture,
    binding: crate::kms::render::platform::cursor_state::CursorBinding,
    expected: &[u8],
    width: usize,
    height: usize,
) -> bool {
    const RECORDING_CURSOR_PITCH: usize = 256;
    let Some(actual) = sprite_bytes(&fixture.scene.events.backend, 0, binding.lease) else {
        return false;
    };
    let row_bytes = width * 4;
    expected.len() == row_bytes * height
        && (0..height).all(|row| {
            actual[row * RECORDING_CURSOR_PITCH..row * RECORDING_CURSOR_PITCH + row_bytes]
                == expected[row * row_bytes..(row + 1) * row_bytes]
        })
}

fn retire_show_and_attach(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<super::support::CompletionKey>,
    frame: u32,
) -> Option<crate::kms::render::platform::cursor_state::CursorBinding> {
    let scene_id = retire_pending_output(&mut fixture.scene.events, delivered, 0, frame);
    assert_ne!(scene_id, 0, "scene transition retires by its accepted ID");
    if !has_unretired_accepted_event(&fixture.scene.events, delivered, 0) {
        assert!(
            !fixture
                .scene
                .events
                .backend
                .platform
                .cursor_commit_in_flight_for_output(0),
            "a missing cursor attach leaves no pending cursor commit"
        );
        assert!(
            !fixture.scene.events.backend.scene.has_pending_page_flip(0),
            "scene transition leaves no pending flip"
        );
        return None;
    }
    let attach_id = retire_pending_output(&mut fixture.scene.events, delivered, 0, frame + 1);
    let device_fd = fixture.scene.kms_fd(0);
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let attach = accepted_live_records(&fixture.scene.events.recorders[&device_fd])
        .into_iter()
        .find(|record| record.user_data == attach_id)
        .expect("retirement submits a cursor attach");
    assert_eq!(attach.kind, AtomicCommitKind::CursorOnly);
    assert!(record_property(&attach, properties.fb_id).is_some_and(|fb| fb != 0));
    assert!(
        !fixture
            .scene
            .events
            .backend
            .platform
            .cursor_commit_in_flight_for_output(0),
        "cursor attach obligation retires by its accepted ID"
    );
    assert!(
        !fixture.scene.events.backend.scene.has_pending_page_flip(0),
        "scene flip obligation retires by its accepted ID"
    );
    applied_cursor_binding(&fixture.scene.events.backend, 0)
}

fn detach_cursor(
    fixture: &mut AtomicPathFixture,
    delivered: &mut HashSet<super::support::CompletionKey>,
    frame: u32,
) {
    let detach = accept_cursor_detach(fixture, delivered, 0);
    retire_cursor_record(fixture, delivered, 0, &detach, frame);
    assert!(applied_cursor_binding(&fixture.scene.events.backend, 0).is_none());
    assert_no_cursor_resources(&fixture.scene.events.backend, None);
    assert!(
        !fixture.scene.events.backend.scene.has_pending_page_flip(0),
        "cursor detach leaves no scene flip obligation"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore hide-on-version-mismatch; the cursor-attach expectation must fail.
fn retire_show_binds_newer_cursor_version() {
    let Some(mut fixture) = AtomicPathFixture::new(PathKind::Physical)
        .expect("newer-cursor retirement fixture construction")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let old = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x11, 0x22, 0x33, 0xff],
        (1, 2),
    );
    compose_show(&mut fixture, old);
    let newest = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x44, 0x55, 0x66, 0xff],
        (3, 1),
    );
    define_root_cursor(&mut fixture.scene.events.backend, newest);
    let newest_record = fixture.scene.events.backend.cursor_records[&newest.as_raw()].clone();

    let binding = retire_show_and_attach(&mut fixture, &mut delivered, 1);
    let attached = binding.is_some();
    let newest_identity = binding.is_some_and(|binding| {
        (binding.hot_x, binding.hot_y) == (newest_record.hot_x, newest_record.hot_y)
            && sprite_image_matches(&fixture, binding, &newest_record.bgra_bytes, 4, 4)
    });
    if attached {
        detach_cursor(&mut fixture, &mut delivered, 3);
    } else {
        Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
        assert_no_cursor_resources(&fixture.scene.events.backend, None);
    }
    finish_atomic_fixture(fixture, &mut delivered);
    assert!(attached, "ShowOnRetire submits a cursor attach");
    assert!(
        newest_identity,
        "retirement uploads the newest live cursor bytes and hotspot"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: restore hide-on-version-mismatch; the newest-frame attach must be missing.
fn retire_show_with_several_versions_in_one_flip_shows_newest() {
    let Some(mut fixture) = AtomicPathFixture::new(PathKind::Physical)
        .expect("animated-cursor retirement fixture construction")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let animation = create_animation(
        &mut fixture.scene.events.backend,
        &[
            ([0x11, 0x12, 0x13, 0xff], (0, 1)),
            ([0x21, 0x22, 0x23, 0xff], (1, 2)),
            ([0x31, 0x32, 0x33, 0xff], (2, 3)),
        ],
    );
    compose_show(&mut fixture, animation);
    assert_eq!(
        fixture
            .scene
            .events
            .backend
            .active_cursor_anim
            .as_ref()
            .unwrap()
            .frame,
        0
    );
    advance_animation(&mut fixture, 1);
    advance_animation(&mut fixture, 2);
    let newest_record = fixture.scene.events.backend.cursor_records[&animation.as_raw()].clone();
    let frame0_version = fixture.scene.events.backend.anim_cursor_records[&animation.as_raw()]
        .frames[0]
        .record
        .version;
    assert!(newest_record.version > frame0_version);

    let binding = retire_show_and_attach(&mut fixture, &mut delivered, 1);
    let attached = binding.is_some();
    let newest_identity = binding.is_some_and(|binding| {
        (binding.hot_x, binding.hot_y) == (newest_record.hot_x, newest_record.hot_y)
            && sprite_image_matches(&fixture, binding, &newest_record.bgra_bytes, 4, 4)
    });
    if attached {
        detach_cursor(&mut fixture, &mut delivered, 3);
    } else {
        Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
        assert_no_cursor_resources(&fixture.scene.events.backend, None);
    }
    finish_atomic_fixture(fixture, &mut delivered);
    assert!(attached, "the newest live frame receives a cursor attach");
    assert!(
        newest_identity,
        "retirement binds the newest of several live frames"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: check uploaded_image_matches before the hidden live entry; a cached image must not be shown.
fn retire_show_hidden_newest_intent_still_hides() {
    let Some(mut fixture) = AtomicPathFixture::new(PathKind::Physical)
        .expect("hidden-cursor retirement fixture construction")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let cursor = create_argb_cursor(
        &mut fixture.scene.events.backend,
        (4, 4),
        [0x71, 0x72, 0x73, 0xff],
        (1, 2),
    );
    compose_show(&mut fixture, cursor);
    retire_show_and_attach(&mut fixture, &mut delivered, 1)
        .expect("initial current cursor attaches before hide");
    detach_cursor(&mut fixture, &mut delivered, 3);

    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, false);
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("compose a second ShowOnRetire with the cached image");
    let record = fixture.scene.events.backend.cursor_records[&cursor.as_raw()].clone();
    let cache_match = fixture
        .scene
        .events
        .backend
        .platform
        .cursor_plane_uploaded_image_matches_for_output(
            0,
            record.version,
            record.hot_x,
            record.hot_y,
        );
    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
    let offset = fixture.scene.events.recorders[&fixture.scene.kms_fd(0)]
        .records()
        .len();
    let binding = retire_show_and_attach(&mut fixture, &mut delivered, 5);
    let attached = binding.is_some();
    let device_fd = fixture.scene.kms_fd(0);
    let properties = super::support::cursor_properties_for_output(&fixture.scene.events.backend, 0);
    let hidden_attach_submitted =
        accepted_live_records(&fixture.scene.events.recorders[&device_fd])
            .into_iter()
            .skip(offset)
            .any(|commit| {
                commit.kind == AtomicCommitKind::CursorOnly
                    && record_property(&commit, properties.fb_id).is_some_and(|fb| fb != 0)
            });
    if attached {
        let detach = accept_cursor_detach(&mut fixture, &mut delivered, 0);
        retire_cursor_record(&mut fixture, &mut delivered, 0, &detach, 7);
    } else {
        assert_no_cursor_resources(&fixture.scene.events.backend, None);
    }
    finish_atomic_fixture(fixture, &mut delivered);
    assert!(
        cache_match,
        "the captured image is still cached before hidden intent"
    );
    assert!(
        !attached && !hidden_attach_submitted,
        "hidden newest intent submits no cursor attach"
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: use the compose-time cache identity on a cache hit; the newer hotspot assertion must fail.
fn retire_show_keeps_frame_zero_when_current_and_uses_new_hotspot_update() {
    let Some(mut fixture) = AtomicPathFixture::new(PathKind::Physical)
        .expect("frame-zero cursor retirement fixture construction")
    else {
        return;
    };
    let mut delivered = HashSet::new();
    let animation = create_animation(
        &mut fixture.scene.events.backend,
        &[
            ([0x81, 0x82, 0x83, 0xff], (1, 2)),
            ([0x81, 0x82, 0x83, 0xff], (4, 5)),
        ],
    );
    compose_show(&mut fixture, animation);
    assert_eq!(
        fixture
            .scene
            .events
            .backend
            .active_cursor_anim
            .as_ref()
            .unwrap()
            .frame,
        0
    );
    let frame0 = fixture.scene.events.backend.cursor_records[&animation.as_raw()].clone();
    let first = retire_show_and_attach(&mut fixture, &mut delivered, 1)
        .expect("current frame zero attaches on the first show");
    let first_is_frame0 = (first.hot_x, first.hot_y) == (frame0.hot_x, frame0.hot_y)
        && sprite_image_matches(&fixture, first, &frame0.bgra_bytes, 4, 4);
    detach_cursor(&mut fixture, &mut delivered, 3);

    Backend::set_cursor_hidden(&mut fixture.scene.events.backend, false);
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.scene.events.backend)
        .expect("compose the next frame-zero ShowOnRetire");
    advance_animation(&mut fixture, 1);
    let newest = fixture.scene.events.backend.cursor_records[&animation.as_raw()].clone();
    assert_eq!(
        newest.bgra_bytes, frame0.bgra_bytes,
        "only the hotspot changed"
    );
    assert_ne!((newest.hot_x, newest.hot_y), (frame0.hot_x, frame0.hot_y));
    assert!(
        fixture
            .scene
            .events
            .backend
            .platform
            .cursor_plane_uploaded_image_matches_for_output(
                0,
                frame0.version,
                frame0.hot_x,
                frame0.hot_y,
            )
    );

    let second = retire_show_and_attach(&mut fixture, &mut delivered, 5);
    let hotspot_updated = second.is_some_and(|binding| {
        (binding.hot_x, binding.hot_y) == (newest.hot_x, newest.hot_y)
            && sprite_image_matches(&fixture, binding, &newest.bgra_bytes, 4, 4)
    });
    if second.is_some() {
        detach_cursor(&mut fixture, &mut delivered, 7);
    } else {
        Backend::set_cursor_hidden(&mut fixture.scene.events.backend, true);
        assert_no_cursor_resources(&fixture.scene.events.backend, None);
    }
    finish_atomic_fixture(fixture, &mut delivered);
    assert!(
        first_is_frame0,
        "the current frame zero is shown on the first hardware attach"
    );
    assert!(
        hotspot_updated,
        "a hotspot-only update uses the live frame's hotspot"
    );
}
