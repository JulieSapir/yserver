//! Scene retirement by accepted commit identity: shared and copied routes.
//!
//! Every scenario advances lifecycle state only through `Backend` trait
//! entries and injects explicit-cookie completions; none calls a submit or
//! retirement internal.

use super::{
    atomic_events::{
        assert_pending_bo_phases, complete_recorded_attempt, pending_ids, submit_damaged_frame,
    },
    recording_scene::{P1SceneRoute, RecordingSceneFixture},
    support::accepted_event_id,
};
use crate::{
    drm::atomic::tests_support::{AtomicCommitRecord, ScriptedResult},
    kms::vk::scanout::BoPhase,
};
use std::{
    os::fd::RawFd,
    time::{Duration, Instant},
};
use yserver_core::backend::Backend;

const COMPOSE_DEADLINE: Duration = Duration::from_secs(5);

fn damage_root(fixture: &mut RecordingSceneFixture) {
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
}

fn records_since(
    fixture: &RecordingSceneFixture,
    fd: RawFd,
    offset: usize,
) -> Vec<AtomicCommitRecord> {
    fixture.events.recorders[&fd]
        .records()
        .into_iter()
        .skip(offset)
        .filter(|record| record.device_fd == fd)
        .collect()
}

fn record_count(fixture: &RecordingSceneFixture, fd: RawFd) -> usize {
    fixture.events.recorders[&fd].records().len()
}

fn sink_pending_count(fixture: &RecordingSceneFixture, fd: RawFd) -> usize {
    fixture
        .events
        .backend
        .platform
        .devices
        .iter()
        .find(|device| {
            std::os::fd::AsRawFd::as_raw_fd(&std::os::fd::AsFd::as_fd(&device.device)) == fd
        })
        .expect("device fd belongs to the fixture")
        .atomic_commit_sink
        .for_tests_pending_page_flip_entry_count()
}

/// Compose through Backend until the copied route registers a source render,
/// returning its job id. The scene may back off for a moment after a rejected
/// submit, so poll within a bounded deadline instead of ordering by sleeps.
fn compose_copied_frame(fixture: &mut RecordingSceneFixture) -> u64 {
    let deadline = Instant::now() + COMPOSE_DEADLINE;
    loop {
        Backend::before_block(&mut fixture.events.backend);
        damage_root(fixture);
        Backend::maybe_composite(&mut fixture.events.backend).expect("compose through Backend");
        if let Some(&job_id) = fixture.pending_render_job_ids().last() {
            return job_id;
        }
        assert!(
            Instant::now() < deadline,
            "a damaged Backend tick must register a copied source render"
        );
    }
}

/// Release one source render's completion notification and run the Backend
/// completion hook until the job is consumed. Releasing the observer does not
/// signal the real completion fd, so poll through normal Backend entries with
/// a bounded deadline instead of assuming immediate GPU readiness.
fn deliver_render_completion(fixture: &mut RecordingSceneFixture, job_id: u64) {
    fixture
        .readiness
        .as_ref()
        .expect("copied fixture has a readiness observer")
        .release_ready_job(job_id);
    let deadline = Instant::now() + COMPOSE_DEADLINE;
    loop {
        Backend::on_scanout_render_completion(
            &mut fixture.events.backend,
            &mut fixture.events.state,
        );
        if !fixture.pending_render_job_ids().contains(&job_id) {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "copied render job {job_id} was never delivered"
        );
        Backend::before_block(&mut fixture.events.backend);
    }
}

/// Compose, deliver and return the sink's accepted record for one copied frame.
fn accept_copied_frame(fixture: &mut RecordingSceneFixture) -> (u64, AtomicCommitRecord) {
    let sink_fd = fixture.kms_fd(0);
    let offset = record_count(fixture, sink_fd);
    let job_id = compose_copied_frame(fixture);
    deliver_render_completion(fixture, job_id);
    let accepted = records_since(fixture, sink_fd, offset)
        .into_iter()
        .find(|record| accepted_event_id(record).is_some())
        .expect("the sink device accepted the copied frame");
    let id = accepted_event_id(&accepted).expect("accepted copied frame has an identity");
    (id, accepted)
}

/// Tearing entry that retires its initial frame with the accepted explicit
/// ID through the device-qualified injector (no implicit-cookie completion).
fn enter_tearing_with_explicit_ids(
    fixture: &mut RecordingSceneFixture,
    window: u32,
    first_present: u64,
) {
    let fd = fixture.kms_fd(0);
    let offset = record_count(fixture, fd);
    super::super::tearing::submit_initial_tearing_frame(&mut fixture.events.backend);
    let initial = records_since(fixture, fd, offset)
        .into_iter()
        .find(|record| accepted_event_id(record).is_some())
        .expect("the initial tearing frame was accepted");
    let id = accepted_event_id(&initial).expect("accepted initial frame has an identity");
    complete_recorded_attempt(fixture, fd, &initial, id, 1);
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    super::super::tearing::finish_entering_tearing(
        &mut fixture.events.backend,
        window,
        window,
        window,
        first_present,
    );
}

fn copied_fixture(route: P1SceneRoute) -> Option<RecordingSceneFixture> {
    let fixture = RecordingSceneFixture::with_route(1, route)
        .expect("copied recording scene fixture construction")?;
    fixture
        .readiness
        .as_ref()
        .expect("copied fixture has a readiness observer")
        .hold_ready_jobs();
    Some(fixture)
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: advance the sink BO pool in backend dispatch before the identity gate; the
// BO-phase / generation snapshots then change on the stale and unknown events.
fn p1_shared_scene_checks_identity_before_advancing_bo_pool() {
    let Some(mut fixture) = RecordingSceneFixture::new(1).expect("recording scene fixture") else {
        return;
    };
    let fd = fixture.kms_fd(0);
    let first = submit_damaged_frame(&mut fixture);
    let first_id = accepted_event_id(&first[0].1).expect("first scene request accepted");
    let crtc = first[0].1.events[0].crtc;
    complete_recorded_attempt(&mut fixture, fd, &first[0].1, first_id, 81);
    assert!(!fixture.events.backend.scene.has_pending_page_flips());

    let second = submit_damaged_frame(&mut fixture);
    let second_id = accepted_event_id(&second[0].1).expect("second scene request accepted");
    assert_ne!(first_id, second_id);
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(second_id)
    );
    let phases = assert_pending_bo_phases(&fixture);
    let observation = fixture
        .events
        .backend
        .scene
        .retirement_observation_for_tests(0);
    assert!(
        observation.contains("KmsFlipPending { event_id: Some("),
        "the second frame's acknowledgment is pending: {observation}"
    );
    let store_damage = fixture.events.store_damage_observation();
    let generations = format!("{:?}", fixture.events.backend.platform.bo_generations);
    let pool_releases = fixture
        .events
        .backend
        .scene
        .pending_pool_release_count_for_tests();

    fixture
        .events
        .queue_commit_completion(fd, crtc, first_id, 82, Duration::from_millis(820));
    fixture
        .events
        .queue_commit_completion(fd, crtc, u64::MAX, 83, Duration::from_millis(830));
    Backend::on_page_flip_ready(&mut fixture.events.backend, &mut fixture.events.state, fd);
    assert_eq!(pending_ids(&fixture, fd, crtc), [second_id]);
    assert!(fixture.events.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(second_id)
    );
    assert_eq!(assert_pending_bo_phases(&fixture), phases);
    // Drawable damage snapshots, output damage and pool slots are untouched,
    // and so is the live DrawableStore damage.
    assert_eq!(
        fixture.events.store_damage_observation(),
        store_damage,
        "live drawable damage changed"
    );
    assert_eq!(
        fixture
            .events
            .backend
            .scene
            .retirement_observation_for_tests(0),
        observation
    );
    assert_eq!(
        format!("{:?}", fixture.events.backend.platform.bo_generations),
        generations
    );
    assert_eq!(
        fixture
            .events
            .backend
            .scene
            .pending_pool_release_count_for_tests(),
        pool_releases
    );

    complete_recorded_attempt(&mut fixture, fd, &second[0].1, second_id, 84);
    assert!(pending_ids(&fixture, fd, crtc).is_empty());
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    assert!(
        fixture
            .events
            .backend
            .scene
            .retirement_observation_for_tests(0)
            .starts_with("acks=[]"),
        "the accepted completion left a scene acknowledgment"
    );
    assert!(
        assert_pending_bo_phases(&fixture)
            .iter()
            .flatten()
            .all(|phase| *phase != BoPhase::Pending),
        "the accepted completion left a Pending BO"
    );
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: install the render `job_id` as the KMS ID before the sink submits (the held-frame
// `pending_page_flip_event_id` assertion fails), or publish the attempted cookie when sink KMS
// rejects (the post-rejection stage / sink-entry assertions fail).
fn p1_copied_frame_gets_identity_only_after_sink_acceptance() {
    let Some(mut fixture) = copied_fixture(P1SceneRoute::CopiedSameDevice) else {
        return;
    };
    let sink_fd = fixture.kms_fd(0);
    let recorder = fixture.events.recorders[&sink_fd].clone();
    let start = record_count(&fixture, sink_fd);

    // Source composition ran, but its real completion is withheld.
    let job_id = compose_copied_frame(&mut fixture);
    Backend::on_scanout_render_completion(&mut fixture.events.backend, &mut fixture.events.state);
    assert_eq!(
        fixture.pending_render_job_ids(),
        [job_id],
        "completion was delivered"
    );
    assert_eq!(
        record_count(&fixture, sink_fd),
        start,
        "sink KMS was submitted early"
    );
    assert!(fixture.events.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        None,
        "render readiness cannot install a KMS identity"
    );
    assert_eq!(sink_pending_count(&fixture, sink_fd), 0);

    // Actual readiness, but the sink's KMS submission is rejected.
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    deliver_render_completion(&mut fixture, job_id);
    let rejected = records_since(&fixture, sink_fd, start);
    assert_eq!(rejected.len(), 1, "exactly one sink KMS attempt");
    assert_ne!(rejected[0].result, ScriptedResult::Ok);
    assert_ne!(
        rejected[0].user_data, 0,
        "the attempt still consumed a cookie"
    );
    assert!(fixture.pending_render_job_ids().is_empty());
    assert_eq!(
        sink_pending_count(&fixture, sink_fd),
        0,
        "rejected cookie was enqueued"
    );
    assert!(
        !fixture.events.backend.scene.has_pending_page_flips(),
        "a rejected sink submit must not leave a scene obligation"
    );
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        None
    );

    // A later normal accepted copied frame stores the sink-device identity.
    let (id, accepted) = accept_copied_frame(&mut fixture);
    assert_ne!(id, rejected[0].user_data);
    let crtc = accepted.events[0].crtc;
    assert_eq!(pending_ids(&fixture, sink_fd, crtc), [id]);
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(id),
        "the stored scene identity must equal the sink-device record"
    );
    complete_recorded_attempt(&mut fixture, sink_fd, &accepted, id, 91);
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    assert_eq!(sink_pending_count(&fixture, sink_fd), 0);
    fixture.finish_after_normal_drain();
}

fn copied_scene_retains_bo_for_old_or_wrong_device_cookie(route: P1SceneRoute) {
    let Some(mut fixture) = copied_fixture(route) else {
        return;
    };
    let sink_fd = fixture.kms_fd(0);
    let other_fd = fixture.other_fd(0);

    let (first_id, first) = accept_copied_frame(&mut fixture);
    let crtc = first.events[0].crtc;
    complete_recorded_attempt(&mut fixture, sink_fd, &first, first_id, 101);
    assert!(!fixture.events.backend.scene.has_pending_page_flips());

    let (second_id, second) = accept_copied_frame(&mut fixture);
    assert_ne!(first_id, second_id);
    let phases = assert_pending_bo_phases(&fixture);
    let observation = fixture
        .events
        .backend
        .scene
        .retirement_observation_for_tests(0);
    let store_damage = fixture.events.store_damage_observation();
    let copy_ownership = fixture.copy_ownership_observation();
    assert!(!copy_ownership.is_empty(), "copied route has copy slots");
    assert!(fixture.pending_render_job_ids().is_empty());
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(second_id)
    );

    // An earlier retired explicit ID on the sink device ...
    fixture.events.queue_commit_completion(
        sink_fd,
        crtc,
        first_id,
        102,
        Duration::from_millis(1020),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        sink_fd,
    );
    // ... and the matching-number event on the device that does not own the output.
    fixture.events.queue_commit_completion(
        other_fd,
        crtc,
        second_id,
        103,
        Duration::from_millis(1030),
    );
    Backend::on_page_flip_ready(
        &mut fixture.events.backend,
        &mut fixture.events.state,
        other_fd,
    );

    assert_eq!(pending_ids(&fixture, sink_fd, crtc), [second_id]);
    assert!(fixture.events.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(second_id)
    );
    assert_eq!(
        assert_pending_bo_phases(&fixture),
        phases,
        "sink BO advanced"
    );
    assert_eq!(
        fixture
            .events
            .backend
            .scene
            .retirement_observation_for_tests(0),
        observation,
        "scene acknowledgment, damage or pool slots changed"
    );
    assert_eq!(
        fixture.events.store_damage_observation(),
        store_damage,
        "live drawable damage changed"
    );
    assert_eq!(
        fixture.copy_ownership_observation(),
        copy_ownership,
        "copy ownership changed on a stale/wrong-device event"
    );
    assert!(
        fixture.pending_render_job_ids().is_empty(),
        "no copy render job may be outstanding while KMS is pending"
    );

    // Only the sink-device accepted event drains the frame.
    complete_recorded_attempt(&mut fixture, sink_fd, &second, second_id, 104);
    assert!(pending_ids(&fixture, sink_fd, crtc).is_empty());
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: attribute using the render fd instead of the sink fd, or use zero-cookie FIFO after
// an explicit mismatch; the retention assertions (or the final drain) fail.
fn p1_copied_scene_retains_bo_for_old_or_wrong_device_cookie_same_device() {
    copied_scene_retains_bo_for_old_or_wrong_device_cookie(P1SceneRoute::CopiedSameDevice);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: as above, with the sink on a separate KMS device.
fn p1_copied_scene_retains_bo_for_old_or_wrong_device_cookie_separate_device() {
    copied_scene_retains_bo_for_old_or_wrong_device_cookie(P1SceneRoute::CopiedSeparateDevice);
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: return or store the first (failed) attempted ID after Vsync succeeds.
fn p1_shared_scene_async_fallback_awaits_successful_vsync_id() {
    let Some(mut fixture) = RecordingSceneFixture::new(1).expect("recording scene fixture") else {
        return;
    };
    let fd = fixture.kms_fd(0);
    let recorder = fixture.events.recorders[&fd].clone();
    enter_tearing_with_explicit_ids(&mut fixture, 0x78a1, 15_100);
    let offset = record_count(&fixture, fd);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Ok(()));
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.events.backend).expect("compose async through Backend");

    let attempts = records_since(&fixture, fd, offset);
    assert_eq!(attempts.len(), 2, "async attempt then Vsync fallback");
    assert_eq!(
        attempts[0].kind,
        crate::drm::atomic::AtomicCommitKind::Async
    );
    assert_ne!(attempts[0].result, ScriptedResult::Ok);
    assert_eq!(attempts[1].result, ScriptedResult::Ok);
    let failed_id = attempts[0].user_data;
    let vsync_id = accepted_event_id(&attempts[1]).expect("Vsync fallback accepted");
    assert_ne!(failed_id, 0);
    assert_ne!(failed_id, vsync_id, "attempts carry distinct IDs");
    let crtc = attempts[1].events[0].crtc;
    assert_eq!(pending_ids(&fixture, fd, crtc), [vsync_id]);
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(vsync_id),
        "the scene awaits the successful request's ID"
    );

    fixture
        .events
        .queue_commit_completion(fd, crtc, failed_id, 111, Duration::from_millis(1110));
    Backend::on_page_flip_ready(&mut fixture.events.backend, &mut fixture.events.state, fd);
    assert!(fixture.events.backend.scene.has_pending_page_flips());
    assert_eq!(pending_ids(&fixture, fd, crtc), [vsync_id]);

    complete_recorded_attempt(&mut fixture, fd, &attempts[1], vsync_id, 112);
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    fixture.finish_after_normal_drain();
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: enqueue or store the attempted ID before checking either request result.
fn p1_shared_scene_failed_fallback_creates_no_flip_obligation() {
    let Some(mut fixture) = RecordingSceneFixture::new(1).expect("recording scene fixture") else {
        return;
    };
    let fd = fixture.kms_fd(0);
    let recorder = fixture.events.recorders[&fd].clone();
    enter_tearing_with_explicit_ids(&mut fixture, 0x78a2, 15_200);
    let offset = record_count(&fixture, fd);
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EINVAL)));
    recorder.script_result(Err(std::io::Error::from_raw_os_error(libc::EIO)));
    damage_root(&mut fixture);
    Backend::maybe_composite(&mut fixture.events.backend).expect("compose failing async");

    let attempts = records_since(&fixture, fd, offset);
    assert_eq!(attempts.len(), 2, "async attempt then Vsync fallback");
    assert!(attempts.iter().all(|a| a.result != ScriptedResult::Ok));
    assert_ne!(attempts[0].user_data, 0);
    assert_ne!(attempts[1].user_data, 0);
    assert_ne!(attempts[0].user_data, attempts[1].user_data);
    assert_eq!(
        sink_pending_count(&fixture, fd),
        0,
        "a failed cookie was enqueued"
    );
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        None
    );

    // Normal failed-submit recycling, then an accepted frame to quiescence.
    let deadline = Instant::now() + COMPOSE_DEADLINE;
    let accepted = loop {
        let before = record_count(&fixture, fd);
        Backend::before_block(&mut fixture.events.backend);
        damage_root(&mut fixture);
        Backend::maybe_composite(&mut fixture.events.backend).expect("compose after failed submit");
        if let Some(record) = records_since(&fixture, fd, before)
            .into_iter()
            .find(|record| accepted_event_id(record).is_some())
        {
            break record;
        }
        assert!(
            Instant::now() < deadline,
            "a frame must be accepted after the failed submit"
        );
    };
    let id = accepted_event_id(&accepted).expect("accepted frame has an identity");
    assert!(attempts.iter().all(|a| a.user_data != id));
    assert_eq!(
        fixture.events.backend.scene.pending_page_flip_event_id(0),
        Some(id)
    );
    complete_recorded_attempt(&mut fixture, fd, &accepted, id, 121);
    assert!(!fixture.events.backend.scene.has_pending_page_flips());
    fixture.finish_after_normal_drain();
}
