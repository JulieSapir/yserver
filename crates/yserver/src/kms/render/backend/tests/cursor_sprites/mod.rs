mod legacy;
mod lifecycle;
mod motion;
mod scene_attach;
mod support;

#[cfg(test)]
mod path_install_tests {
    use super::{
        super::commit_identity::support::AtomicEventFixture,
        support::{assert_no_cursor_resources, finish_fixture, retire_pending_output},
    };
    use crate::{
        drm::{CursorDeviceCaps, atomic::AtomicCommitKind},
        kms::cursor_plane::{
            ops::{
                CursorBufferIo,
                recording::{CursorBufferLedger, RecordingCursorBufferIo},
            },
            topology::{
                CursorPathReason, CursorPlaneTestPlane, CursorPlaneTestSnapshot, CursorStatePath,
            },
        },
    };
    use std::{
        collections::HashSet,
        os::fd::{AsFd as _, AsRawFd as _},
        rc::Rc,
    };
    use yserver_core::backend::Backend;

    const CURSOR_PROPERTIES: [&str; 12] = [
        "FB_ID",
        "CRTC_ID",
        "CRTC_X",
        "CRTC_Y",
        "CRTC_W",
        "CRTC_H",
        "SRC_X",
        "SRC_Y",
        "SRC_W",
        "SRC_H",
        "HOTSPOT_X",
        "HOTSPOT_Y",
    ];

    fn snapshot(
        crtc: u32,
        missing: Option<&str>,
        driver_name: Option<&str>,
    ) -> CursorPlaneTestSnapshot {
        let property_names = CURSOR_PROPERTIES
            .into_iter()
            .filter(|name| Some(*name) != missing)
            .map(str::to_owned)
            .collect();
        CursorPlaneTestSnapshot {
            crtcs: vec![crtc],
            planes: vec![CursorPlaneTestPlane {
                handle: 21,
                possible_crtcs: vec![crtc],
                property_names,
            }],
            cursor_caps: Some(CursorDeviceCaps::new(false, driver_name.map(str::to_owned))),
            has_buffer: true,
        }
    }

    fn install(
        events: &mut AtomicEventFixture,
        snapshot: CursorPlaneTestSnapshot,
    ) -> Rc<CursorBufferLedger> {
        let (io, ledger) = RecordingCursorBufferIo::new(1);
        let buffer_io: Rc<dyn CursorBufferIo> = Rc::new(io);
        events
            .backend
            .platform
            .for_tests_install_cursor_plane_from_snapshot(0, snapshot, buffer_io, 64, 64);
        ledger
    }

    fn verify_position_only_move(
        events: &mut AtomicEventFixture,
        expected_path: CursorStatePath,
        expected_reason: CursorPathReason,
        ledger: &CursorBufferLedger,
    ) {
        let crtc = events.backend.platform.outputs[0].output.crtc;
        let state = &events.backend.platform.devices[0].cursor;
        assert_eq!(state.state_path(), expected_path);
        // Mutation: disable the installed position-only atomic cursor gate.
        assert!(
            state.atomic_cursor,
            "installation must enable position-only atomic cursor support"
        );
        assert_eq!(
            state.plane.as_ref().unwrap().state_path().reason,
            expected_reason
        );
        let (plane_handle, crtc_x, crtc_y) = state
            .plane
            .as_ref()
            .unwrap()
            .atomic_position_properties(crtc)
            .expect("assigned plane exposes position properties");
        assert!(state.plane.as_ref().unwrap().sprites().all_users_zero());
        events.backend.platform.devices[0]
            .cursor
            .plane
            .as_mut()
            .unwrap()
            .for_tests_set_visible(crtc, true);
        events
            .backend
            .platform
            .for_tests_seed_atomic_cursor_position(0, plane_handle, crtc_x, crtc_y, 100, 100, 0, 0);

        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        Backend::maybe_composite(&mut events.backend).expect("submit cursor-only position");

        let fd = events.backend.platform.devices[0]
            .device
            .as_fd()
            .as_raw_fd();
        let accepted = super::support::accepted_live_records(&events.recorders[&fd]);
        assert_eq!(accepted.len(), 1);
        assert_eq!(accepted[0].kind, AtomicCommitKind::CursorOnly);
        let updated: HashSet<_> = accepted[0]
            .properties
            .iter()
            .map(|property| property.property)
            .collect();
        assert_eq!(updated, HashSet::from([crtc_x, crtc_y]));
        assert_eq!(updated.len(), 2);

        let mut delivered = HashSet::new();
        retire_pending_output(events, &mut delivered, 0, 1);
        finish_fixture(events, &mut delivered);
        assert_no_cursor_resources(&events.backend, Some(ledger));
        assert_eq!(ledger.records(), []);
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }

    #[test]
    // Mutation: read the legacy visible map on Atomic, making the false hw_bound_on assertion fail.
    fn atomic_install_ignores_legacy_visible_map_until_applied_state_exists() {
        let mut events = AtomicEventFixture::new(1);
        let ledger = install(&mut events, snapshot(1, None, Some("amdgpu")));
        let crtc = events.backend.platform.outputs[0].output.crtc;
        let state = &events.backend.platform.devices[0].cursor;
        assert_eq!(state.state_path(), CursorStatePath::Atomic);
        assert_eq!(
            state.plane.as_ref().unwrap().state_path().reason,
            CursorPathReason::Qualified
        );
        events.backend.platform.devices[0]
            .cursor
            .plane
            .as_mut()
            .unwrap()
            .for_tests_set_visible(crtc, true);
        assert!(!events.backend.platform.devices[0].cursor.hw_bound_on(crtc));
        Backend::warp_pointer_root(&mut events.backend, &mut events.state, 120, 110);
        assert!(
            !events.backend.platform.has_eligible_atomic_cursor_move(),
            "a legacy visible-map entry is not an applied Atomic binding"
        );
        Backend::maybe_composite(&mut events.backend).expect("no Atomic cursor move");

        let fd = events.backend.platform.devices[0]
            .device
            .as_fd()
            .as_raw_fd();
        assert!(super::support::accepted_live_records(&events.recorders[&fd]).is_empty());
        assert!(
            !events
                .backend
                .platform
                .for_tests_atomic_cursor_commit_is_in_flight(
                    events.backend.platform.devices[0].key,
                    crtc,
                )
        );
        let mut delivered = HashSet::new();
        finish_fixture(&mut events, &mut delivered);
        assert_no_cursor_resources(&events.backend, Some(&ledger));
        assert_eq!(ledger.records(), []);
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }

    #[test]
    // Mutation: branch position submission on `state_path`, or qualify an
    // incomplete property set as Atomic.
    fn installing_an_incomplete_plane_computes_legacy_and_the_same_behavior() {
        let mut events = AtomicEventFixture::new(1);
        let ledger = install(&mut events, snapshot(1, Some("FB_ID"), Some("amdgpu")));
        verify_position_only_move(
            &mut events,
            CursorStatePath::Legacy,
            CursorPathReason::MissingProperty("FB_ID"),
            &ledger,
        );
    }

    #[test]
    // Mutation: mark zero-plane devices topology-blocked or latch their legacy
    // probe during installation.
    fn apple_dcp_style_device_keeps_the_legacy_probe() {
        let mut events = AtomicEventFixture::new(1);
        let crtc = events.backend.platform.outputs[0].output.crtc;
        let (io, ledger) = RecordingCursorBufferIo::new(1);
        let buffer_io: Rc<dyn CursorBufferIo> = Rc::new(io);
        events
            .backend
            .platform
            .for_tests_install_cursor_plane_from_snapshot(
                0,
                CursorPlaneTestSnapshot {
                    crtcs: vec![1],
                    planes: Vec::new(),
                    cursor_caps: Some(CursorDeviceCaps::new(false, Some("apple".to_owned()))),
                    has_buffer: true,
                },
                buffer_io,
                64,
                64,
            );

        let state = &events.backend.platform.devices[0].cursor;
        assert_eq!(state.state_path(), CursorStatePath::Legacy);
        assert_eq!(
            state.plane.as_ref().unwrap().state_path().reason,
            CursorPathReason::NoCursorPlane
        );
        assert!(
            state
                .plane
                .as_ref()
                .unwrap()
                .for_tests_explicit_plane_assignment()
                .is_none()
        );
        assert!(events.backend.platform.cursor_plane_available());
        let key = events.backend.platform.devices[0].key;
        assert!(
            !events
                .backend
                .platform
                .for_tests_hw_cursor_disabled_for_device(key)
        );
        assert!(
            events
                .backend
                .platform
                .for_tests_note_unbound_cursor_failure(
                    0,
                    crtc,
                    &std::io::Error::from_raw_os_error(libc::ENXIO),
                )
        );
        assert!(
            events
                .backend
                .platform
                .for_tests_hw_cursor_disabled_for_device(key)
        );
        assert!(!events.backend.platform.cursor_plane_available());
        assert_no_cursor_resources(&events.backend, Some(&ledger));
        assert_eq!(ledger.end_state(), (0, 0, 0));
        let mut delivered = HashSet::new();
        finish_fixture(&mut events, &mut delivered);
    }
}
