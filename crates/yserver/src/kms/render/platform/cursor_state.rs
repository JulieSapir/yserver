//! Accepted cursor-plane state and ownership carried by cursor commits.

use crate::{
    drm::atomic::AtomicPropertyUpdate,
    kms::cursor_plane::{sprites::SpriteBufferLease, topology::CursorPlaneProperties},
};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CursorBinding {
    pub(crate) properties: CursorPlaneProperties,
    pub(crate) lease: SpriteBufferLease,
    pub(crate) framebuffer: u32,
    pub(crate) x: i32,
    pub(crate) y: i32,
    pub(crate) hot_x: u16,
    pub(crate) hot_y: u16,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CursorPropertyUpdate {
    pub(crate) object: ::drm::control::RawResourceHandle,
    pub(crate) property: ::drm::control::property::Handle,
    pub(crate) value: u64,
}

impl CursorPropertyUpdate {
    pub(crate) fn atomic_update(self) -> AtomicPropertyUpdate {
        AtomicPropertyUpdate {
            object: self.object,
            property: self.property,
            value: self.value,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct PreparedCursorCommit {
    pub(crate) prior: Option<CursorBinding>,
    pub(crate) next: Option<CursorBinding>,
    pub(crate) updates: Vec<CursorPropertyUpdate>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct InFlightCursorCommit {
    pub(crate) commit_id: Option<u64>,
    pub(crate) prepared: Option<PreparedCursorCommit>,
}

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub(crate) struct CursorCrtcState {
    pub(crate) applied: Option<CursorBinding>,
    pub(crate) valid: bool,
    pub(crate) resume_visible: bool,
}

#[cfg(test)]
mod tests {
    use super::{CursorBinding, CursorCrtcState, InFlightCursorCommit};
    use crate::{
        drm::{CursorDeviceCaps, Device},
        kms::{
            cursor_plane::{
                CursorPlane,
                ops::{CursorBufferIo, recording::RecordingCursorBufferIo},
                sprites::SpritePool,
                topology::{CursorPlaneTestPlane, CursorPlaneTestSnapshot, CursorStatePath},
            },
            render::platform::{KmsCursorState, PlatformBackend},
        },
    };
    use drm::control::from_u32;
    use std::rc::Rc;

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

    fn lease(
        version: u64,
        hot_x: u16,
        hot_y: u16,
    ) -> crate::kms::cursor_plane::sprites::SpriteBufferLease {
        let device = Rc::new(Device::for_tests().expect("synthetic DRM device"));
        let (io, _) = RecordingCursorBufferIo::new(1);
        let io: Rc<dyn CursorBufferIo> = Rc::new(io);
        let mut pool = SpritePool::new(64, 32);
        assert_eq!(
            pool.prepare(device, io, version, hot_x, hot_y, 3, 2, &[0; 24])
                .expect("sprite setup"),
            crate::kms::cursor_plane::sprites::SpriteStage::Ready
        );
        pool.candidate_lease().expect("staged lease")
    }

    fn cursor_plane(crtc: ::drm::control::crtc::Handle, missing: Option<&str>) -> CursorPlane {
        let device = Rc::new(Device::for_tests().expect("synthetic DRM device"));
        let (io, _) = RecordingCursorBufferIo::new(1);
        let buffer_io: Rc<dyn CursorBufferIo> = Rc::new(io);
        let snapshot = CursorPlaneTestSnapshot {
            crtcs: vec![u32::from(crtc)],
            planes: vec![CursorPlaneTestPlane {
                handle: 21,
                possible_crtcs: vec![u32::from(crtc)],
                property_names: CURSOR_PROPERTIES
                    .iter()
                    .copied()
                    .filter(|name| Some(*name) != missing)
                    .map(str::to_owned)
                    .collect(),
            }],
            cursor_caps: Some(CursorDeviceCaps::new(false, Some("amdgpu".to_owned()))),
            has_buffer: true,
        };
        CursorPlane::for_tests_from_snapshot(device, &[crtc], buffer_io, snapshot, 64, 32)
    }

    fn binding(crtc: ::drm::control::crtc::Handle) -> CursorBinding {
        let plane = cursor_plane(crtc, None);
        CursorBinding {
            properties: plane
                .properties_for(crtc)
                .expect("complete cursor properties"),
            lease: lease(1, 2, 1),
            framebuffer: 91,
            x: 10,
            y: 20,
            hot_x: 2,
            hot_y: 1,
        }
    }

    fn test_state(crtc: ::drm::control::crtc::Handle, missing: Option<&str>) -> KmsCursorState {
        let mut state = KmsCursorState::new();
        state.plane = Some(cursor_plane(crtc, missing));
        state
    }

    #[test]
    // Mutation: make both paths read the legacy visibility map or both read applied state.
    fn hw_bound_on_reads_the_path_specific_state() {
        let crtc = from_u32(11).expect("nonzero CRTC handle");

        let mut legacy = test_state(crtc, Some("FB_ID"));
        assert_eq!(legacy.state_path(), CursorStatePath::Legacy);
        legacy
            .plane
            .as_mut()
            .unwrap()
            .for_tests_set_visible(crtc, true);
        assert!(legacy.hw_bound_on(crtc));
        legacy
            .plane
            .as_mut()
            .unwrap()
            .for_tests_set_visible(crtc, false);
        assert!(!legacy.hw_bound_on(crtc));

        let mut atomic = test_state(crtc, None);
        assert_eq!(atomic.state_path(), CursorStatePath::Atomic);
        atomic
            .plane
            .as_mut()
            .unwrap()
            .for_tests_set_visible(crtc, true);
        assert!(
            !atomic.hw_bound_on(crtc),
            "legacy visibility is ignored on Atomic"
        );
        atomic.crtc_states.insert(
            crtc,
            CursorCrtcState {
                applied: Some(binding(crtc)),
                ..CursorCrtcState::default()
            },
        );
        atomic
            .plane
            .as_mut()
            .unwrap()
            .for_tests_set_visible(crtc, false);
        assert!(
            atomic.hw_bound_on(crtc),
            "applied state is authoritative on Atomic"
        );
        atomic.crtc_states.get_mut(&crtc).unwrap().applied = None;
        assert!(!atomic.hw_bound_on(crtc));
    }

    #[test]
    // Mutation: retire an ID-tagged cursor commit on any CRTC event or keep a legacy None record for an ID event.
    fn retire_keeps_an_id_tagged_record_on_a_different_event_id() {
        let mut platform = PlatformBackend::for_tests();
        let crtc = platform.outputs[0].output.crtc;
        let cursor = &mut platform.devices[0].cursor;
        cursor.atomic_cursor_commits_in_flight.insert(
            crtc,
            InFlightCursorCommit {
                commit_id: Some(7),
                prepared: None,
            },
        );

        assert!(!platform.retire_atomic_cursor_commit_for_output(0, Some(8)));
        assert!(
            platform.devices[0]
                .cursor
                .atomic_cursor_commits_in_flight
                .contains_key(&crtc)
        );
        assert!(!platform.retire_atomic_cursor_commit_for_output(0, None));
        assert!(
            platform.devices[0]
                .cursor
                .atomic_cursor_commits_in_flight
                .contains_key(&crtc)
        );
        assert!(platform.retire_atomic_cursor_commit_for_output(0, Some(7)));
        assert!(
            !platform.devices[0]
                .cursor
                .atomic_cursor_commits_in_flight
                .contains_key(&crtc)
        );

        platform.devices[0]
            .cursor
            .atomic_cursor_commits_in_flight
            .insert(
                crtc,
                InFlightCursorCommit {
                    commit_id: None,
                    prepared: None,
                },
            );
        assert!(platform.retire_atomic_cursor_commit_for_output(0, Some(7)));
        assert!(
            !platform.devices[0]
                .cursor
                .atomic_cursor_commits_in_flight
                .contains_key(&crtc)
        );
    }
}
