//! Full-state cursor atomic request construction.

use crate::{
    drm::atomic::AtomicCommitRequest,
    kms::{
        cursor_plane::topology::CursorPlaneProperties,
        render::platform::cursor_state::{
            CursorBinding, CursorPropertyUpdate, PreparedCursorCommit,
        },
    },
};

pub(crate) fn prepare_cursor_commit(
    properties: CursorPlaneProperties,
    crtc: ::drm::control::crtc::Handle,
    applied: Option<CursorBinding>,
    valid: bool,
    next: Option<CursorBinding>,
    width: u32,
    height: u32,
) -> PreparedCursorCommit {
    let mut updates = Vec::new();
    let mut add = |property, value| {
        updates.push(CursorPropertyUpdate {
            object: properties.plane.into(),
            property,
            value,
        });
    };

    match next {
        Some(binding) => {
            let attach = !valid || applied.is_none();
            let new_framebuffer = !valid
                || applied.is_none_or(|previous| {
                    previous.lease != binding.lease || previous.framebuffer != binding.framebuffer
                });
            if attach || new_framebuffer {
                add(properties.fb_id, u64::from(binding.framebuffer));
            }
            if attach {
                add(properties.crtc_id, u64::from(u32::from(crtc)));
            }
            if !valid || applied.is_none_or(|previous| previous.x != binding.x) {
                add(properties.crtc_x, i64::from(binding.x) as u64);
            }
            if !valid || applied.is_none_or(|previous| previous.y != binding.y) {
                add(properties.crtc_y, i64::from(binding.y) as u64);
            }
            if attach {
                add(properties.src_x, 0);
                add(properties.src_y, 0);
                add(properties.src_w, u64::from(width) << 16);
                add(properties.src_h, u64::from(height) << 16);
                add(properties.crtc_w, u64::from(width));
                add(properties.crtc_h, u64::from(height));
            }
            if (!valid || applied.is_none_or(|previous| previous.hot_x != binding.hot_x))
                && let Some(property) = properties.hotspot_x
            {
                add(property, u64::from(binding.hot_x));
            }
            if (!valid || applied.is_none_or(|previous| previous.hot_y != binding.hot_y))
                && let Some(property) = properties.hotspot_y
            {
                add(property, u64::from(binding.hot_y));
            }
        }
        None => {
            if !valid || applied.is_some() {
                add(properties.fb_id, 0);
                add(properties.crtc_id, 0);
            }
        }
    }

    PreparedCursorCommit {
        prior: applied,
        next,
        updates,
    }
}

pub(crate) fn request_for_cursor_commit(prepared: &PreparedCursorCommit) -> AtomicCommitRequest {
    let mut request = AtomicCommitRequest::new();
    for update in &prepared.updates {
        let update = update.atomic_update();
        request.add_cursor_property(update.object, update.property, update.value);
    }
    request
}

#[cfg(test)]
mod tests {
    use super::prepare_cursor_commit;
    use crate::{
        drm::Device,
        kms::{
            cursor_plane::{
                ops::{CursorBufferIo, recording::RecordingCursorBufferIo},
                sprites::{SpritePool, SpriteStage},
                topology::CursorPlaneProperties,
            },
            render::platform::cursor_state::CursorBinding,
        },
    };
    use drm::control::{crtc, from_u32, plane, property};
    use std::{collections::BTreeSet, rc::Rc};

    fn property(value: u32) -> property::Handle {
        from_u32(value).expect("nonzero property handle")
    }

    fn properties() -> CursorPlaneProperties {
        CursorPlaneProperties {
            plane: from_u32::<plane::Handle>(21).expect("nonzero plane handle"),
            fb_id: property(101),
            crtc_id: property(102),
            crtc_x: property(103),
            crtc_y: property(104),
            crtc_w: property(105),
            crtc_h: property(106),
            src_x: property(107),
            src_y: property(108),
            src_w: property(109),
            src_h: property(110),
            hotspot_x: Some(property(111)),
            hotspot_y: Some(property(112)),
        }
    }

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
            SpriteStage::Ready
        );
        pool.candidate_lease().expect("staged lease")
    }

    fn binding(
        version: u64,
        framebuffer: u32,
        x: i32,
        y: i32,
        hot_x: u16,
        hot_y: u16,
    ) -> CursorBinding {
        CursorBinding {
            properties: properties(),
            lease: lease(version, hot_x, hot_y),
            framebuffer,
            x,
            y,
            hot_x,
            hot_y,
        }
    }

    fn property_set(
        prepared: &crate::kms::render::platform::cursor_state::PreparedCursorCommit,
    ) -> BTreeSet<(u32, u64)> {
        let updates: BTreeSet<_> = prepared
            .updates
            .iter()
            .map(|update| (u32::from(update.property), update.value))
            .collect();
        assert_eq!(
            updates.len(),
            prepared.updates.len(),
            "each property is unique"
        );
        updates
    }

    fn expected(updates: &[(property::Handle, u64)]) -> BTreeSet<(u32, u64)> {
        updates
            .iter()
            .map(|(property, value)| (u32::from(*property), *value))
            .collect()
    }

    fn crtc() -> crtc::Handle {
        from_u32(11).expect("nonzero CRTC handle")
    }

    #[test]
    // Mutations: use image dimensions for SRC_W/H, treat invalid state as valid, detach an already-detached valid plane, or zero-extend negative coordinates.
    fn prepare_cursor_commit_truth_table() {
        let properties = properties();
        let crtc = crtc();
        let original = binding(1, 91, 10, 20, 2, 1);
        let attach = prepare_cursor_commit(properties, crtc, None, false, Some(original), 64, 32);
        assert_eq!(
            property_set(&attach),
            expected(&[
                (properties.fb_id, 91),
                (properties.crtc_id, 11),
                (properties.crtc_x, 10),
                (properties.crtc_y, 20),
                (properties.src_x, 0),
                (properties.src_y, 0),
                (properties.src_w, 64_u64 << 16),
                (properties.src_h, 32_u64 << 16),
                (properties.crtc_w, 64),
                (properties.crtc_h, 32),
                (properties.hotspot_x.unwrap(), 2),
                (properties.hotspot_y.unwrap(), 1),
            ]),
            "attach uses full-capacity geometry and both optional hotspots",
        );

        let mut no_hotspots = properties;
        no_hotspots.hotspot_x = None;
        no_hotspots.hotspot_y = None;
        let no_hotspot_attach =
            prepare_cursor_commit(no_hotspots, crtc, None, true, Some(original), 64, 32);
        assert_eq!(no_hotspot_attach.updates.len(), 10);
        assert_eq!(
            property_set(&no_hotspot_attach),
            expected(&[
                (no_hotspots.fb_id, 91),
                (no_hotspots.crtc_id, 11),
                (no_hotspots.crtc_x, 10),
                (no_hotspots.crtc_y, 20),
                (no_hotspots.src_x, 0),
                (no_hotspots.src_y, 0),
                (no_hotspots.src_w, 64_u64 << 16),
                (no_hotspots.src_h, 32_u64 << 16),
                (no_hotspots.crtc_w, 64),
                (no_hotspots.crtc_h, 32),
            ])
        );

        let replacement = binding(2, 92, 10, 20, 2, 1);
        let replace_lease = prepare_cursor_commit(
            properties,
            crtc,
            Some(original),
            true,
            Some(replacement),
            64,
            32,
        );
        assert_eq!(
            property_set(&replace_lease),
            expected(&[(properties.fb_id, 92)])
        );

        let moved_replacement = binding(3, 93, 12, 20, 4, 1);
        let replace_move_hotspot = prepare_cursor_commit(
            properties,
            crtc,
            Some(original),
            true,
            Some(moved_replacement),
            64,
            32,
        );
        assert_eq!(
            property_set(&replace_move_hotspot),
            expected(&[
                (properties.fb_id, 93),
                (properties.crtc_x, 12),
                (properties.hotspot_x.unwrap(), 4),
            ])
        );

        let moved = CursorBinding { x: 12, ..original };
        let move_only =
            prepare_cursor_commit(properties, crtc, Some(original), true, Some(moved), 64, 32);
        assert_eq!(
            property_set(&move_only),
            expected(&[(properties.crtc_x, 12)])
        );

        let identical = prepare_cursor_commit(
            properties,
            crtc,
            Some(original),
            true,
            Some(original),
            64,
            32,
        );
        assert!(property_set(&identical).is_empty());

        let detach = prepare_cursor_commit(properties, crtc, Some(original), true, None, 64, 32);
        assert_eq!(
            property_set(&detach),
            expected(&[(properties.fb_id, 0), (properties.crtc_id, 0)])
        );
        let detached_valid = prepare_cursor_commit(properties, crtc, None, true, None, 64, 32);
        assert!(property_set(&detached_valid).is_empty());
        let detached_invalid = prepare_cursor_commit(properties, crtc, None, false, None, 64, 32);
        assert_eq!(
            property_set(&detached_invalid),
            expected(&[(properties.fb_id, 0), (properties.crtc_id, 0)])
        );

        let invalid_equal = prepare_cursor_commit(
            properties,
            crtc,
            Some(original),
            false,
            Some(original),
            64,
            32,
        );
        assert_eq!(
            property_set(&invalid_equal),
            property_set(&attach),
            "invalid applied state resends the complete attachment",
        );

        let negative = CursorBinding {
            x: -12,
            y: -1,
            ..original
        };
        let negative_move = prepare_cursor_commit(
            properties,
            crtc,
            Some(original),
            true,
            Some(negative),
            64,
            32,
        );
        assert_eq!(
            property_set(&negative_move),
            expected(&[
                (properties.crtc_x, (-12_i64) as u64),
                (properties.crtc_y, (-1_i64) as u64),
            ]),
            "negative CRTC positions preserve signed i64 representation",
        );
    }
}
