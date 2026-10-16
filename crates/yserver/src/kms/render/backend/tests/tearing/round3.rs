use super::*;

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutations: classify by XFIXES only, or preserve the deferred cursor wake on tearing exit.
fn transparent_cursor_is_hidden_for_direct_scanout_and_define_cursor_handoff() {
    use yserver_core::{backend::Backend, server::ServerState};

    const WINDOW: u32 = 0x78a1;
    let Some(mut backend) = live_scene_backend() else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let mut state = ServerState::new();
    let (show_executor, _) = install_cursor_plane_with_successful_show(&mut backend);
    let root = backend.core.window_id;
    assert!(
        backend.effective_cursor_hidden(),
        "no effective cursor starts hidden"
    );

    let transparent = create_bitmap_cursor_via_backend(&mut backend, false, false);
    assert!(backend.cursor_records[&transparent.as_raw()].is_fully_transparent);
    Backend::define_cursor(&mut backend, None, root, transparent.as_raw())
        .expect("define transparent core cursor outside tearing");
    assert!(!backend.cursor_hidden, "the cursor is not XFIXES-hidden");
    assert!(backend.effective_cursor_hidden());
    damage_root_and_composite(&mut backend);
    assert!(
        backend.scene.has_pending_page_flips(),
        "expected a pending scene page flip: {}",
        tearing_composite_diagnostics(&backend, None),
    );
    retire_composed_frame(&mut backend, &mut state, 51);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(show_executor.calls().is_empty());
    assert!(!backend.platform.any_cursor_plane_visible());

    // The visible one-pixel cursor remains a real cursor through CreateCursor.
    let opaque = create_bitmap_cursor_via_backend(&mut backend, true, true);
    assert!(!backend.cursor_records[&opaque.as_raw()].is_fully_transparent);

    // Classification also covers core glyphs, recoloring, and the animation-wide
    // rule: one opaque frame makes the animated cursor visible.
    let missing_font = yserver_core::backend::FontHandle::from_raw(0x78a2).unwrap();
    let glyph = Backend::create_glyph_cursor(
        &mut backend,
        None,
        missing_font,
        None,
        0,
        0,
        (u16::MAX, u16::MAX, u16::MAX),
        (0, 0, 0),
    )
    .expect("create transparent fallback glyph cursor");
    assert!(backend.cursor_records[&glyph.as_raw()].is_fully_transparent);
    Backend::recolor_cursor(
        &mut backend,
        None,
        glyph.as_raw(),
        (0, 0, 0),
        (u16::MAX, 0, 0),
    )
    .expect("recolor transparent glyph cursor");
    assert!(backend.cursor_records[&glyph.as_raw()].is_fully_transparent);
    let all_transparent =
        Backend::create_anim_cursor(&mut backend, None, &[(transparent, 40), (glyph, 40)])
            .expect("create all-transparent animation")
            .expect("animation handle");
    assert!(backend.anim_cursor_records[&all_transparent.as_raw()].is_fully_transparent);
    let mixed = Backend::create_anim_cursor(&mut backend, None, &[(transparent, 40), (opaque, 40)])
        .expect("create mixed animation")
        .expect("animation handle");
    assert!(!backend.anim_cursor_records[&mixed.as_raw()].is_fully_transparent);
    Backend::define_cursor(&mut backend, None, root, all_transparent.as_raw())
        .expect("define all-transparent animation");
    assert!(backend.effective_cursor_hidden());
    Backend::define_cursor(&mut backend, None, root, mixed.as_raw())
        .expect("define animation with an opaque frame");
    assert!(!backend.effective_cursor_hidden());
    Backend::define_cursor(&mut backend, None, root, transparent.as_raw())
        .expect("restore transparent core cursor");
    assert!(backend.effective_cursor_hidden());
    damage_root_and_composite(&mut backend);
    retire_composed_frame(&mut backend, &mut state, 55);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(show_executor.calls().is_empty());
    assert!(!backend.platform.any_cursor_plane_visible());

    // The test cursor plane above checks the non-tearing path. Remove that test
    // device now so every direct async commit below is cursor-plane-free.
    backend.platform.devices[0].cursor.plane = None;
    recorder.clear_records();
    present_until_direct_async(&mut backend, &recorder, &mut state, WINDOW, 15_100);
    assert!(backend.pstate.tearing.is_on());
    assert!(backend.pstate.scanout_m2.active());
    assert!(!backend.platform.any_cursor_plane_visible());

    // DefineCursor from a fully transparent image to one opaque pixel must
    // unwind direct scanout and compose the cursor while tearing remains On.
    Backend::define_cursor(&mut backend, None, root, opaque.as_raw())
        .expect("define one-pixel cursor");
    assert!(!backend.effective_cursor_hidden());
    assert!(backend.tearing_cursor_wants_visible());
    assert!(backend.pstate.scanout_m2.unflip_requested);
    assert_eq!(
        backend.pstate.scanout_m2.unflip_reason,
        Some("tearing_cursor_handoff")
    );
    Backend::maybe_composite(&mut backend).expect("submit cursor handoff");
    assert!(!recorder.records().is_empty());
    retire_composed_frame(&mut backend, &mut state, 52);
    assert!(!backend.pstate.scanout_m2.active());
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    damage_root(&mut backend);
    Backend::maybe_composite(&mut backend).expect("compose visible one-pixel cursor");
    assert!(
        backend.scene.has_pending_page_flips(),
        "expected a pending scene page flip: {}",
        tearing_composite_diagnostics(&backend, None),
    );
    retire_composed_frame(&mut backend, &mut state, 53);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Sw
    );
    assert!(backend.scene.for_tests_cursor_footprint_was_presented(0));
    assert!(
        try_live_direct_for_ids(
            &mut backend,
            15_150,
            WINDOW,
            WINDOW,
            yserver_core::present_scheduler::PRESENT_OPTION_ASYNC,
        )
        .is_none(),
        "a one-pixel visible cursor blocks direct scanout after its handoff"
    );

    // A transparent RENDER cursor must clear the scene cursor and permit direct
    // async scanout again after its hidden frame retires.
    let render_cursor = create_render_cursor_via_backend(&mut backend);
    assert!(backend.cursor_records[&render_cursor.as_raw()].is_fully_transparent);
    Backend::define_cursor(&mut backend, None, root, render_cursor.as_raw())
        .expect("define transparent RENDER cursor");
    assert!(backend.effective_cursor_hidden());
    Backend::maybe_composite(&mut backend).expect("compose hidden RENDER cursor");
    assert!(
        backend.scene.has_pending_page_flips(),
        "expected a pending scene page flip: {}",
        tearing_composite_diagnostics(&backend, None),
    );
    retire_composed_frame(&mut backend, &mut state, 54);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(!backend.platform.any_cursor_plane_visible());
    present_until_direct_async(&mut backend, &recorder, &mut state, WINDOW, 15_200);

    let sequence: Vec<_> = recorder
        .records()
        .iter()
        .map(|record| record.kind)
        .collect();
    assert_eq!(
        sequence,
        [
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
        ],
        "transparent → visible → transparent cursor transitions must hand off composed ↔ direct"
    );
    assert!(backend.pstate.tearing.is_on());
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.pending.is_none());
    assert_direct_queue_drained(&backend);
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert!(backend.pstate.tearing.deferred_cursor_wake.is_none());
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.scene.cursor_transition_pending());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );
    assert!(!backend.platform.any_cursor_plane_visible());

    seed_pending_tearing_cursor_wake(&mut backend);
    let exit_present =
        try_direct_present_with_live_storage(&mut backend, 15_300, WINDOW, 800, 600, 0);
    assert!(
        exit_present.is_some(),
        "a non-async Backend Present exits tearing"
    );
    assert_tearing_off(&backend);
    assert!(backend.pstate.scanout_m2.pending.is_some());
    retire_direct_present(&mut backend, &mut state, 55);

    assert_tearing_off(&backend);
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.hold_direct);
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert_direct_queue_drained(&backend);
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden,
        "tearing exit preserves transparent-as-hidden cursor policy"
    );
    assert!(!backend.platform.any_cursor_plane_visible());
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.scene.cursor_transition_pending());
    assert_commit_sequence(
        &recorder,
        &[
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
            AtomicCommitKind::Async,
            AtomicCommitKind::Scene,
        ],
    );
}

#[test]
#[ignore = "needs live Vulkan ICD (lavapipe)"]
// Mutation: omit the direct cursor rebind from DefineCursor; the held Vsync frame must keep a visible sprite.
fn define_visible_cursor_rebinds_held_vsync_direct_scanout() {
    use yserver_core::backend::Backend;

    const WINDOW: u32 = 0x78b1;
    let Some(mut backend) = direct_backend(1) else {
        return;
    };
    let recorder = recording_sink(&mut backend, 0);
    let (show_executor, hide_executor) = install_cursor_plane_with_successful_show(&mut backend);
    let root = backend.core.window_id;

    let transparent = create_bitmap_cursor_via_backend(&mut backend, false, false);
    Backend::define_cursor(&mut backend, None, root, transparent.as_raw())
        .expect("select transparent root cursor");
    assert!(backend.effective_cursor_hidden());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hidden
    );

    assert!(
        try_direct_present_with_live_storage(&mut backend, 15_300, WINDOW, 800, 600, 0).is_some()
    );
    assert!(backend.pstate.scanout_m2.pending.is_some());
    assert_eq!(recorder.records().len(), 1);
    assert_eq!(recorder.records()[0].kind, AtomicCommitKind::Scene);
    let mut state = yserver_core::server::ServerState::new();
    retire_direct_present(&mut backend, &mut state, 70);
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.hold_direct);
    assert!(!backend.platform.any_cursor_plane_visible());

    let visible = create_bitmap_cursor_via_backend(&mut backend, true, true);
    Backend::define_cursor(&mut backend, None, root, visible.as_raw())
        .expect("select opaque root cursor while direct scanout is held");
    assert!(!backend.effective_cursor_hidden());
    assert_eq!(show_executor.calls().len(), 1);
    assert_eq!(hide_executor.calls().len(), 0);
    assert!(backend.platform.any_cursor_plane_visible());
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.hold_direct);
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    Backend::maybe_composite(&mut backend).expect("keep held direct scanout with rebound cursor");

    assert!(matches!(
        backend.pstate.tearing.state,
        crate::kms::render::backend::TearingState::Off
    ));
    assert_eq!(backend.pstate.tearing.deferred_cursor_wake, None);
    assert!(backend.platform.async_ledger.for_tests_is_empty());
    assert_eq!(
        backend.scene.cursor_mode(),
        crate::kms::render::scene::CursorPlaneMode::Hw,
        "the scene records the hardware plane rebound during direct scanout"
    );
    assert!(!backend.scene.has_pending_page_flips());
    assert!(!backend.scene.cursor_transition_pending());
    assert!(backend.platform.any_cursor_plane_visible());
    assert!(backend.pstate.scanout_m2.active());
    assert!(backend.pstate.scanout_m2.hold_direct);
    assert!(!backend.pstate.scanout_m2.unflip_requested);
    assert!(backend.pstate.scanout_m2.unflip_awaiting_outputs.is_empty());
    assert_direct_queue_drained(&backend);
    assert_commit_sequence(&recorder, &[AtomicCommitKind::Scene]);
    backend.platform.wait_idle_bounded();
}
