use std::{io, rc::Rc};

use drm::buffer::DrmFourcc;

use super::{SpritePool, SpriteStage};
use crate::{
    drm::Device,
    kms::cursor_plane::ops::{
        CursorBufferIo,
        recording::{
            CursorBufferLedger, CursorBufferOperation, CursorBufferRecord, RecordingCursorBufferIo,
        },
    },
};

const CAPACITY_WIDTH: u32 = 70;
const CAPACITY_HEIGHT: u32 = 64;

fn fixture() -> (
    Rc<Device>,
    Rc<RecordingCursorBufferIo>,
    Rc<CursorBufferLedger>,
    SpritePool,
) {
    let device = Rc::new(Device::for_tests().expect("synthetic DRM device"));
    let (io, ledger) = RecordingCursorBufferIo::new(256);
    (
        device,
        Rc::new(io),
        ledger,
        SpritePool::new(CAPACITY_WIDTH, CAPACITY_HEIGHT),
    )
}

fn prepare(
    pool: &mut SpritePool,
    device: &Rc<Device>,
    io: &Rc<RecordingCursorBufferIo>,
    version: u64,
    hot_x: u16,
    hot_y: u16,
    width: u32,
    height: u32,
    bytes: &[u8],
) -> io::Result<SpriteStage> {
    pool.prepare(
        Rc::clone(device),
        Rc::clone(io) as Rc<dyn CursorBufferIo>,
        version,
        hot_x,
        hot_y,
        width,
        height,
        bytes,
    )
}

fn image(width: usize, height: usize, color: [u8; 4]) -> Vec<u8> {
    color.repeat(width * height)
}

fn create_handles(ledger: &CursorBufferLedger) -> Vec<u32> {
    ledger
        .records()
        .into_iter()
        .filter_map(|record| match record {
            CursorBufferRecord::Create { handle, .. } => Some(handle),
            _ => None,
        })
        .collect()
}

fn assert_released_in_order(records: &[CursorBufferRecord], handle: u32, fb: u32) {
    let release = records
        .iter()
        .filter(|record| match record {
            CursorBufferRecord::RemoveFb { fb: got, .. } => *got == fb,
            CursorBufferRecord::Unmap { handle: got, .. }
            | CursorBufferRecord::Destroy { handle: got, .. } => *got == handle,
            _ => false,
        })
        .cloned()
        .collect::<Vec<_>>();
    assert!(matches!(
        release.as_slice(),
        [
            CursorBufferRecord::RemoveFb { fb: got_fb, .. },
            CursorBufferRecord::Unmap { handle: got_unmap, .. },
            CursorBufferRecord::Destroy { handle: got_destroy, .. }
        ] if *got_fb == fb && *got_unmap == handle && *got_destroy == handle
    ));
}

#[test]
fn sprite_pool_allocates_two_full_capacity_argb_buffers() {
    // Mutation: allocate image-sized buffers or skip AddFB; the initial pool must publish two full-capacity ARGB buffers.
    let (device, io, ledger, mut pool) = fixture();
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            1,
            0,
            0,
            4,
            4,
            &image(4, 4, [1, 2, 3, 255])
        )
        .expect("stage first image"),
        SpriteStage::Ready
    );
    let records = ledger.records();
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, CursorBufferRecord::Create { .. }))
            .count(),
        2
    );
    assert_eq!(
        records
            .iter()
            .filter(|record| matches!(record, CursorBufferRecord::Map { .. }))
            .count(),
        2
    );
    let add_fbs = records
        .iter()
        .filter_map(|record| match record {
            CursorBufferRecord::AddFb {
                width,
                height,
                format,
                ..
            } => Some((*width, *height, *format)),
            _ => None,
        })
        .collect::<Vec<_>>();
    assert_eq!(add_fbs.len(), 2);
    assert!(add_fbs.iter().all(|(width, height, format)| {
        *width == CAPACITY_WIDTH
            && *height == CAPACITY_HEIGHT
            && *format == DrmFourcc::Argb8888 as u32
    }));
    assert_eq!(ledger.end_state(), (2, 2, 2));

    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
fn sprite_pool_writes_top_left_image_and_clears_pitch_padding() {
    // Mutation: skip the clear or use image_width*4 as stride; old pixels and pitch padding must be zero.
    let (device, io, ledger, mut pool) = fixture();
    let first = image(4, 4, [0x11, 0x12, 0x13, 0xff]);
    let second = image(2, 2, [0x21, 0x22, 0x23, 0xff]);
    let third = image(3, 3, [0x31, 0x32, 0x33, 0xff]);
    prepare(&mut pool, &device, &io, 1, 0, 0, 4, 4, &first).expect("stage 4x4");
    let first_lease = pool.candidate_lease().expect("first candidate");
    prepare(&mut pool, &device, &io, 2, 0, 0, 2, 2, &second).expect("stage 2x2");
    let second_lease = pool.candidate_lease().expect("second candidate");
    assert_ne!(first_lease, second_lease);
    assert_eq!(
        pool.for_tests_sprite_bytes(first_lease).unwrap()[..16],
        first[..16]
    );
    assert_eq!(
        pool.for_tests_sprite_bytes(second_lease).unwrap()[..8],
        second[..8]
    );
    assert!(
        pool.for_tests_sprite_bytes(second_lease).unwrap()[8..512]
            .iter()
            .all(|byte| *byte == 0)
    );

    assert!(pool.cancel_candidate(second_lease));
    prepare(&mut pool, &device, &io, 3, 0, 0, 3, 3, &third).expect("restage into first slot");
    let third_lease = pool.candidate_lease().expect("third candidate");
    assert_eq!(third_lease.slot, first_lease.slot);
    let staged = pool
        .for_tests_sprite_bytes(third_lease)
        .expect("staged bytes");
    assert_eq!(&staged[..12], &third[..12]);
    assert_eq!(&staged[512..524], &third[12..24]);
    assert_eq!(&staged[1024..1036], &third[24..36]);
    assert!(
        staged
            .iter()
            .enumerate()
            .all(|(index, byte)| (index / 512 < 3 && index % 512 < 12) || *byte == 0)
    );
    assert!(pool.all_users_zero());
    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
fn sprite_pool_allocation_failure_rolls_back_in_reverse_order() {
    // Mutation: publish before a successful write or leak the first buffer when initial buffer two fails.
    for operation in [
        CursorBufferOperation::Create,
        CursorBufferOperation::Map,
        CursorBufferOperation::AddFb,
    ] {
        for successful_calls in [0, 1] {
            let (device, io, ledger, mut pool) = fixture();
            ledger.fail_after(operation, successful_calls, 28);
            let error = prepare(
                &mut pool,
                &device,
                &io,
                7,
                1,
                2,
                4,
                4,
                &image(4, 4, [7, 8, 9, 255]),
            )
            .expect_err("scripted allocation failure");
            assert_eq!(
                error.raw_os_error(),
                Some(28),
                "{operation:?}/{successful_calls}"
            );
            assert!(error.to_string().contains("28"), "{error}");
            assert_eq!(pool.wanted_version(), Some(7));
            assert_eq!(ledger.end_state(), (0, 0, 0));
            assert_eq!(
                ledger.failures_taken(),
                vec![operation],
                "{operation:?}/{successful_calls}"
            );
            let records = ledger.records();
            match (operation, successful_calls) {
                (CursorBufferOperation::Create, 0) => assert!(records.is_empty()),
                (CursorBufferOperation::Map, 0) => assert!(matches!(
                    records.as_slice(),
                    [
                        CursorBufferRecord::Create { handle: 1, .. },
                        CursorBufferRecord::Destroy { handle: 1, .. }
                    ]
                )),
                (CursorBufferOperation::AddFb, 0) => assert!(matches!(
                    records.as_slice(),
                    [
                        CursorBufferRecord::Create { handle: 1, .. },
                        CursorBufferRecord::Map { handle: 1, .. },
                        CursorBufferRecord::Unmap { handle: 1, .. },
                        CursorBufferRecord::Destroy { handle: 1, .. }
                    ]
                )),
                (CursorBufferOperation::Create, 1) => {
                    assert_eq!(create_handles(&ledger), vec![1]);
                    assert!(matches!(
                        records.as_slice(),
                        [
                            CursorBufferRecord::Create { handle: 1, .. },
                            CursorBufferRecord::Map { handle: 1, .. },
                            CursorBufferRecord::AddFb { fb: 100, .. },
                            CursorBufferRecord::RemoveFb { fb: 100, .. },
                            CursorBufferRecord::Unmap { handle: 1, .. },
                            CursorBufferRecord::Destroy { handle: 1, .. }
                        ]
                    ));
                }
                (CursorBufferOperation::Map, 1) => {
                    assert_eq!(create_handles(&ledger), vec![1, 2]);
                    assert!(matches!(
                        records.as_slice(),
                        [
                            CursorBufferRecord::Create { handle: 1, .. },
                            CursorBufferRecord::Map { handle: 1, .. },
                            CursorBufferRecord::AddFb { fb: 100, .. },
                            CursorBufferRecord::Create { handle: 2, .. },
                            CursorBufferRecord::Destroy { handle: 2, .. },
                            CursorBufferRecord::RemoveFb { fb: 100, .. },
                            CursorBufferRecord::Unmap { handle: 1, .. },
                            CursorBufferRecord::Destroy { handle: 1, .. }
                        ]
                    ));
                }
                (CursorBufferOperation::AddFb, 1) => {
                    assert_eq!(create_handles(&ledger), vec![1, 2]);
                    assert!(matches!(
                        records.as_slice(),
                        [
                            CursorBufferRecord::Create { handle: 1, .. },
                            CursorBufferRecord::Map { handle: 1, .. },
                            CursorBufferRecord::AddFb { fb: 100, .. },
                            CursorBufferRecord::Create { handle: 2, .. },
                            CursorBufferRecord::Map { handle: 2, .. },
                            CursorBufferRecord::Unmap { handle: 2, .. },
                            CursorBufferRecord::Destroy { handle: 2, .. },
                            CursorBufferRecord::RemoveFb { fb: 100, .. },
                            CursorBufferRecord::Unmap { handle: 1, .. },
                            CursorBufferRecord::Destroy { handle: 1, .. }
                        ]
                    ));
                }
                (CursorBufferOperation::Write, _) => unreachable!(),
                _ => unreachable!("allocation failure schedule has zero or one successes"),
            }
            assert!(pool.candidate_lease().is_none());
            assert_eq!(
                prepare(
                    &mut pool,
                    &device,
                    &io,
                    7,
                    1,
                    2,
                    4,
                    4,
                    &image(4, 4, [7, 8, 9, 255])
                )
                .expect("retry succeeds after allocation failure"),
                SpriteStage::Ready
            );
            drop(pool);
            assert_eq!(ledger.end_state(), (0, 0, 0));
        }
    }

    let (device, io, ledger, mut pool) = fixture();
    prepare(
        &mut pool,
        &device,
        &io,
        1,
        0,
        0,
        4,
        4,
        &image(4, 4, [1, 2, 3, 255]),
    )
    .expect("stage initial candidate");
    let previous = pool.candidate_lease().expect("initial candidate");
    ledger.fail_after(CursorBufferOperation::Write, 0, libc::EAGAIN);
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            2,
            1,
            1,
            4,
            4,
            &image(4, 4, [4, 5, 6, 255])
        )
        .expect("WouldBlock write defers"),
        SpriteStage::Deferred
    );
    assert_eq!(pool.candidate_lease(), Some(previous));
    assert_eq!(pool.candidate_version(), Some(1));
    assert_eq!(pool.wanted_version(), Some(2));
    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
fn sprite_pool_hotspot_only_change_stages_a_distinct_buffer() {
    // Mutation: deduplicate by version alone; hotspot changes need a distinct framebuffer identity.
    let (device, io, ledger, mut pool) = fixture();
    let pixels = image(4, 4, [3, 4, 5, 255]);
    prepare(&mut pool, &device, &io, 12, 1, 1, 4, 4, &pixels).expect("stage hotspot 1,1");
    let first = pool.candidate_lease().expect("first lease");
    let first_fb = pool
        .framebuffer_for_lease(first)
        .expect("first framebuffer");
    prepare(&mut pool, &device, &io, 12, 2, 2, 4, 4, &pixels).expect("stage hotspot 2,2");
    let second = pool.candidate_lease().expect("second lease");
    let second_fb = pool
        .framebuffer_for_lease(second)
        .expect("second framebuffer");
    assert_ne!(first_fb, second_fb);
    assert!(first.matches(12, 1, 1));
    assert!(!second.matches(12, 1, 1));
    assert!(second.matches(12, 2, 2));
    let before = ledger.records();
    assert_eq!(
        prepare(&mut pool, &device, &io, 12, 2, 2, 4, 4, &pixels)
            .expect("repeat existing hotspot identity"),
        SpriteStage::Ready
    );
    assert_eq!(pool.candidate_lease(), Some(second));
    assert_eq!(ledger.records(), before);
    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
fn sprite_pool_restaging_a_held_identity_reuses_its_slot() {
    // Mutation: search only the current candidate; a late identity already in any slot must be reused without writing.
    for hold_first in [false, true] {
        let (device, io, ledger, mut pool) = fixture();
        prepare(
            &mut pool,
            &device,
            &io,
            5,
            0,
            0,
            4,
            4,
            &image(4, 4, [5, 5, 5, 255]),
        )
        .expect("stage A");
        let first = pool.candidate_lease().expect("A lease");
        if hold_first {
            assert!(pool.reserve(first));
            assert!(pool.accept_reserved(first));
        }
        prepare(
            &mut pool,
            &device,
            &io,
            6,
            0,
            0,
            4,
            4,
            &image(4, 4, [6, 6, 6, 255]),
        )
        .expect("stage B");
        let before = ledger.records();
        assert_eq!(pool.candidate_version(), Some(6));
        assert_eq!(
            prepare(
                &mut pool,
                &device,
                &io,
                5,
                0,
                0,
                4,
                4,
                &image(4, 4, [9, 9, 9, 255])
            )
            .expect("late stage A identity"),
            SpriteStage::Ready
        );
        assert_eq!(pool.candidate_lease(), Some(first));
        assert_eq!(ledger.records(), before);
        if hold_first {
            assert_eq!(pool.user_count(first), Some(1));
            assert!(pool.release_user(first));
        }
        assert!(pool.all_users_zero());
        drop(pool);
        assert_eq!(ledger.end_state(), (0, 0, 0));
    }
}

#[test]
fn sprite_pool_pressure_defers_and_keeps_only_the_newest_wanted_image() {
    // Mutation: overwrite live slots, retain an older wanted version, exceed count+1, or shrink away held buffers.
    let (device, io, ledger, mut pool) = fixture();
    pool.set_active_crtc_count(1);
    let pixels_a = image(4, 4, [1, 1, 1, 255]);
    let pixels_b = image(4, 4, [2, 2, 2, 255]);
    prepare(&mut pool, &device, &io, 1, 0, 0, 4, 4, &pixels_a).expect("stage A");
    let a = pool.candidate_lease().unwrap();
    assert!(pool.reserve(a));
    assert!(pool.accept_reserved(a));
    prepare(&mut pool, &device, &io, 2, 0, 0, 4, 4, &pixels_b).expect("stage B");
    let b = pool.candidate_lease().unwrap();
    assert!(pool.reserve(b));
    assert!(pool.accept_reserved(b));
    let bytes_a = pool.for_tests_sprite_bytes(a).unwrap();
    let bytes_b = pool.for_tests_sprite_bytes(b).unwrap();
    let before = ledger.records();
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            3,
            0,
            0,
            4,
            4,
            &image(4, 4, [3, 3, 3, 255])
        )
        .unwrap(),
        SpriteStage::Deferred
    );
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            4,
            0,
            0,
            4,
            4,
            &image(4, 4, [4, 4, 4, 255])
        )
        .unwrap(),
        SpriteStage::Deferred
    );
    assert_eq!(pool.wanted_version(), Some(4));
    assert_eq!(ledger.records(), before);
    assert_eq!(pool.for_tests_sprite_bytes(a), Some(bytes_a));
    assert_eq!(pool.for_tests_sprite_bytes(b), Some(bytes_b));
    assert!(pool.release_user(a));
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            4,
            0,
            0,
            4,
            4,
            &image(4, 4, [4, 4, 4, 255])
        )
        .unwrap(),
        SpriteStage::Ready
    );
    assert_eq!(pool.candidate_lease().unwrap().slot, a.slot);
    assert!(pool.release_user(b));
    assert!(pool.all_users_zero());
    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));

    let (device, io, ledger, mut pool) = fixture();
    pool.set_active_crtc_count(3);
    let mut leases = Vec::new();
    for version in 10..14 {
        assert_eq!(
            prepare(
                &mut pool,
                &device,
                &io,
                version,
                0,
                0,
                4,
                4,
                &image(4, 4, [version as u8, 0, 0, 255]),
            )
            .unwrap(),
            SpriteStage::Ready
        );
        let lease = pool.candidate_lease().unwrap();
        assert!(pool.reserve(lease));
        assert!(pool.accept_reserved(lease));
        leases.push(lease);
    }
    assert_eq!(create_handles(&ledger).len(), 4);
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            14,
            0,
            0,
            4,
            4,
            &image(4, 4, [14, 0, 0, 255])
        )
        .unwrap(),
        SpriteStage::Deferred
    );
    let records_before_shrink = ledger.records();
    pool.set_active_crtc_count(1);
    assert_eq!(ledger.records(), records_before_shrink);
    for lease in leases {
        assert!(pool.release_user(lease));
    }
    assert!(pool.all_users_zero());
    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
fn sprite_pool_never_rewrites_a_buffer_with_users_or_reservations() {
    // Mutation: release users on accept, underflow counts, or accept a stale rewritten lease.
    let (device, io, ledger, mut pool) = fixture();
    prepare(
        &mut pool,
        &device,
        &io,
        1,
        0,
        0,
        4,
        4,
        &image(4, 4, [1, 1, 1, 255]),
    )
    .expect("stage A");
    let a = pool.candidate_lease().unwrap();
    assert!(pool.reserve(a));
    assert!(pool.accept_reserved(a));
    assert!(pool.reserve(a));
    assert!(pool.accept_reserved(a));
    assert_eq!(pool.user_count(a), Some(2));

    prepare(
        &mut pool,
        &device,
        &io,
        2,
        0,
        0,
        4,
        4,
        &image(4, 4, [2, 2, 2, 255]),
    )
    .expect("stage B in other slot");
    let b = pool.candidate_lease().unwrap();
    assert!(pool.reserve(b));
    assert!(pool.accept_reserved(b));
    let bytes_a = pool.for_tests_sprite_bytes(a).unwrap();
    let bytes_b = pool.for_tests_sprite_bytes(b).unwrap();
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            3,
            0,
            0,
            4,
            4,
            &image(4, 4, [3, 3, 3, 255])
        )
        .unwrap(),
        SpriteStage::Deferred
    );
    assert!(pool.release_user(a));
    assert_eq!(pool.user_count(a), Some(1));
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            3,
            0,
            0,
            4,
            4,
            &image(4, 4, [3, 3, 3, 255])
        )
        .unwrap(),
        SpriteStage::Deferred
    );
    assert_eq!(pool.for_tests_sprite_bytes(a), Some(bytes_a));
    assert_eq!(pool.for_tests_sprite_bytes(b), Some(bytes_b));
    assert!(pool.release_user(a));
    assert!(!pool.release_user(a));
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            3,
            0,
            0,
            4,
            4,
            &image(4, 4, [3, 3, 3, 255])
        )
        .unwrap(),
        SpriteStage::Ready
    );
    assert!(
        !pool.reserve(a),
        "stale lease cannot reserve a rewritten slot"
    );
    assert!(pool.release_user(b));
    assert!(pool.all_users_zero());
    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
fn sprite_pool_reservations_alone_prevent_rewrites() {
    // Mutation: ignore prepared_users when choosing a writable slot; reserved bytes must remain untouched.
    let (device, io, ledger, mut pool) = fixture();
    prepare(
        &mut pool,
        &device,
        &io,
        1,
        0,
        0,
        4,
        4,
        &image(4, 4, [1, 0, 0, 255]),
    )
    .expect("stage A");
    let a = pool.candidate_lease().expect("A candidate");
    assert!(pool.reserve(a));

    prepare(
        &mut pool,
        &device,
        &io,
        2,
        0,
        0,
        4,
        4,
        &image(4, 4, [2, 0, 0, 255]),
    )
    .expect("stage B");
    let b = pool.candidate_lease().expect("B candidate");
    assert!(pool.reserve(b));
    assert_eq!(pool.prepared_user_count(), 2);
    assert_eq!(pool.user_count(a), Some(0));
    assert_eq!(pool.user_count(b), Some(0));

    let bytes_a = pool.for_tests_sprite_bytes(a).expect("A bytes");
    let bytes_b = pool.for_tests_sprite_bytes(b).expect("B bytes");
    let records_before = ledger.records();
    assert_eq!(
        prepare(
            &mut pool,
            &device,
            &io,
            3,
            0,
            0,
            4,
            4,
            &image(4, 4, [3, 0, 0, 255]),
        )
        .expect("pressure defers while both slots are reserved"),
        SpriteStage::Deferred
    );
    assert_eq!(pool.wanted_version(), Some(3));
    assert_eq!(pool.prepared_user_count(), 2);
    assert_eq!(ledger.records(), records_before);
    assert_eq!(pool.for_tests_sprite_bytes(a), Some(bytes_a));
    assert_eq!(pool.for_tests_sprite_bytes(b), Some(bytes_b));

    assert!(pool.reject_reserved(a));
    assert!(pool.reject_reserved(b));
    assert_eq!(pool.prepared_user_count(), 0);
    assert!(pool.all_users_zero());
    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
fn sprite_pool_reservation_rejection_restores_the_prior_owner() {
    // Mutation: decrement an accepted user when rejecting a reservation, or accept without a reservation.
    let (device, io, ledger, mut pool) = fixture();
    prepare(
        &mut pool,
        &device,
        &io,
        1,
        0,
        0,
        4,
        4,
        &image(4, 4, [1, 0, 0, 255]),
    )
    .expect("stage A");
    let a = pool.candidate_lease().unwrap();
    assert!(pool.reserve(a));
    assert!(pool.accept_reserved(a));
    prepare(
        &mut pool,
        &device,
        &io,
        2,
        0,
        0,
        4,
        4,
        &image(4, 4, [2, 0, 0, 255]),
    )
    .expect("stage B");
    let b = pool.candidate_lease().unwrap();
    assert!(pool.reserve(b));
    assert_eq!(pool.prepared_user_count(), 1);
    assert!(pool.reject_reserved(b));
    assert_eq!(pool.prepared_user_count(), 0);
    assert_eq!(pool.user_count(a), Some(1));
    assert_eq!(pool.user_count(b), Some(0));
    assert!(!pool.can_accept_reserved(b));
    assert!(!pool.accept_reserved(b));
    assert!(pool.reserve(b));
    assert!(pool.accept_reserved(b));
    assert!(pool.reserve(b));
    assert!(pool.reject_reserved(b));
    assert_eq!(pool.user_count(b), Some(1));
    assert_eq!(pool.prepared_user_count(), 0);
    assert!(pool.release_user(b));
    assert!(pool.release_user(a));
    assert!(pool.all_users_zero());
    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}

#[test]
fn sprite_pool_drop_forgets_buffers_with_live_users() {
    // Mutation: release every buffer on drop; live users and reservations must retain their DRM resources.
    let (device, io, ledger, mut pool) = fixture();
    prepare(
        &mut pool,
        &device,
        &io,
        1,
        0,
        0,
        4,
        4,
        &image(4, 4, [1, 1, 1, 255]),
    )
    .expect("stage A");
    let a = pool.candidate_lease().unwrap();
    assert!(pool.reserve(a));
    assert!(pool.accept_reserved(a));
    prepare(
        &mut pool,
        &device,
        &io,
        2,
        0,
        0,
        4,
        4,
        &image(4, 4, [2, 2, 2, 255]),
    )
    .expect("stage free B");
    let handles = create_handles(&ledger);
    assert_eq!(handles, vec![1, 2]);
    let fb_a = pool.framebuffer_for_lease(a).unwrap();
    let b = pool.candidate_lease().unwrap();
    let fb_b = pool.framebuffer_for_lease(b).unwrap();
    drop(pool);
    let records = ledger.records();
    assert_released_in_order(&records, handles[1], fb_b);
    assert!(records.iter().any(|record| matches!(record,
        CursorBufferRecord::AddFb { fb, .. } if *fb == fb_a
    )));
    assert!(!records.iter().any(|record| matches!(record,
        CursorBufferRecord::RemoveFb { fb, .. } if *fb == fb_a
    )));
    assert_eq!(ledger.end_state(), (1, 1, 1));

    let (device, io, ledger, mut pool) = fixture();
    prepare(
        &mut pool,
        &device,
        &io,
        3,
        0,
        0,
        4,
        4,
        &image(4, 4, [3, 3, 3, 255]),
    )
    .expect("stage reserved-only slot");
    let reserved = pool.candidate_lease().unwrap();
    assert!(pool.reserve(reserved));
    drop(pool);
    assert_eq!(ledger.end_state(), (1, 1, 1));
}

#[test]
fn sprite_pool_rejects_oversize_and_wrong_length_images() {
    // Mutation: remove capacity validation or accept a byte slice longer than width*height*4.
    let (device, io, ledger, mut pool) = fixture();
    let wrong_length = image(2, 2, [4, 5, 6, 255]);
    let invalid = [
        (71, 64, image(71, 64, [1, 1, 1, 255])),
        (70, 65, image(70, 65, [2, 2, 2, 255])),
        (0, 4, Vec::new()),
        (2, 2, wrong_length[..wrong_length.len() - 1].to_vec()),
        (2, 2, [wrong_length.as_slice(), &[7]].concat()),
    ];
    for (version, (width, height, bytes)) in invalid.into_iter().enumerate() {
        let result = prepare(
            &mut pool,
            &device,
            &io,
            version as u64 + 1,
            0,
            0,
            width,
            height,
            &bytes,
        );
        let error = result.expect_err("invalid cursor input");
        assert_eq!(error.kind(), io::ErrorKind::InvalidInput);
        assert_eq!(pool.wanted_version(), None);
        assert!(pool.candidate_lease().is_none());
        assert!(ledger.records().is_empty());
    }
    assert_eq!(ledger.end_state(), (0, 0, 0));
    drop(pool);
    assert_eq!(ledger.end_state(), (0, 0, 0));
}
