use super::{
    AtomicCommitEvent, AtomicCommitKind, AtomicCommitRequest, AtomicCommitSink, CommitWindow,
    PendingPageFlip, RawAtomicModeReq, event_identity_required, pending_page_flip_index,
};
use drm::control::{AtomicCommitFlags, RawResourceHandle, from_u32, property};
use std::{collections::VecDeque, time::Instant};

fn raw<H: drm::control::ResourceHandle>(value: u32) -> H {
    from_u32(value).expect("nonzero raw DRM handle")
}

#[test]
// Mutation: retain duplicate entries, preserve reverse object insertion order, or move a value to another object's slice.
fn raw_atomic_request_groups_properties_per_object_in_kernel_order() {
    let object_10: RawResourceHandle = raw::<drm::control::plane::Handle>(10).into();
    let object_20: RawResourceHandle = raw::<drm::control::plane::Handle>(20).into();
    let property_3: property::Handle = raw(3);
    let property_7: property::Handle = raw(7);
    let property_9: property::Handle = raw(9);
    let mut request = RawAtomicModeReq::default();
    request.add_raw_property(object_20, property_7, 70);
    request.add_raw_property(object_10, property_9, 91);
    request.add_raw_property(object_10, property_3, 30);
    request.add_raw_property(object_10, property_9, 99);

    assert_eq!(
        request
            .objects
            .iter()
            .map(|h| u32::from(*h))
            .collect::<Vec<_>>(),
        [10, 20]
    );
    assert_eq!(request.count_props_per_object, [2, 1]);
    assert_eq!(request.props, [3, 9, 7]);
    assert_eq!(request.values, [30, 99, 70]);
}

#[test]
// Mutation: reorder/omit an ABI field or encode an ioctl with a different payload size.
fn drm_mode_atomic_struct_and_ioctl_request_code() {
    assert_eq!(std::mem::size_of::<super::drm_mode_atomic>(), 56);
    assert_eq!(std::mem::offset_of!(super::drm_mode_atomic, flags), 0);
    assert_eq!(std::mem::offset_of!(super::drm_mode_atomic, count_objs), 4);
    assert_eq!(std::mem::offset_of!(super::drm_mode_atomic, objs_ptr), 8);
    assert_eq!(
        std::mem::offset_of!(super::drm_mode_atomic, count_props_ptr),
        16
    );
    assert_eq!(std::mem::offset_of!(super::drm_mode_atomic, props_ptr), 24);
    assert_eq!(
        std::mem::offset_of!(super::drm_mode_atomic, prop_values_ptr),
        32
    );
    assert_eq!(std::mem::offset_of!(super::drm_mode_atomic, reserved), 40);
    assert_eq!(std::mem::offset_of!(super::drm_mode_atomic, user_data), 48);
    assert_eq!(
        super::DRM_IOCTL_MODE_ATOMIC,
        0xC038_64BC_u32 as crate::drm::page_flip::IoctlReq
    );
    let request = RawAtomicModeReq::default()
        .kernel_request(AtomicCommitFlags::PAGE_FLIP_EVENT, 0x1234)
        .expect("ABI request");
    assert_eq!(request.reserved, 0);
}

#[test]
// Mutation: set kernel_request.user_data to zero while the recorder still sees the assigned ID.
fn raw_atomic_kernel_request_preserves_full_width_cookie() {
    let object: RawResourceHandle = raw::<drm::control::plane::Handle>(22).into();
    let property: property::Handle = raw(8);
    let mut raw_request = RawAtomicModeReq::default();
    raw_request.add_raw_property(object, property, 0xFEDC_BA98_7654_3210);
    let cookie = 0xFEDC_BA98_7654_3210;
    let flags = AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK;
    let request = raw_request
        .kernel_request(flags, cookie)
        .expect("ABI request");

    assert_eq!(request.user_data, cookie);
    assert_eq!(request.flags, flags.bits());
    assert_eq!(request.count_objs, 1);
    // SAFETY: request arrays remain alive and unchanged until these slices are inspected.
    unsafe {
        assert_eq!(
            std::slice::from_raw_parts(request.objs_ptr as *const RawResourceHandle, 1),
            [object]
        );
        assert_eq!(
            std::slice::from_raw_parts(request.count_props_ptr as *const u32, 1),
            [1]
        );
        assert_eq!(
            std::slice::from_raw_parts(request.props_ptr as *const u32, 1),
            [8]
        );
        assert_eq!(
            std::slice::from_raw_parts(request.prop_values_ptr as *const u64, 1),
            [0xFEDC_BA98_7654_3210]
        );
    }
}

#[test]
// Mutation: let TEST_ONLY, empty-target, or non-event submissions require an identity.
fn atomic_event_identity_policy_excludes_test_only_and_no_event_requests() {
    let event = [AtomicCommitEvent {
        crtc: raw(1),
        motion_timestamp: None,
    }];
    let cases = [
        (AtomicCommitFlags::PAGE_FLIP_EVENT, &event[..], true),
        (
            AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::NONBLOCK,
            &event[..],
            true,
        ),
        (AtomicCommitFlags::NONBLOCK, &event[..], false),
        (AtomicCommitFlags::PAGE_FLIP_EVENT, &[][..], false),
        (
            AtomicCommitFlags::PAGE_FLIP_EVENT | AtomicCommitFlags::TEST_ONLY,
            &event[..],
            false,
        ),
        (
            AtomicCommitFlags::PAGE_FLIP_EVENT
                | AtomicCommitFlags::TEST_ONLY
                | AtomicCommitFlags::NONBLOCK,
            &event[..],
            false,
        ),
    ];
    for (flags, events, expected) in cases {
        assert_eq!(
            event_identity_required(flags, events),
            expected,
            "{flags:?}"
        );
    }

    let recorder = std::rc::Rc::new(
        crate::drm::atomic::tests_support::RecordingAtomicCommitExecutor::default(),
    );
    let sink = AtomicCommitSink::new(recorder.clone());
    let device = crate::drm::Device::for_tests().expect("synthetic device");
    assert!(
        sink.submit_with_commit_id(
            &device,
            AtomicCommitFlags::TEST_ONLY | AtomicCommitFlags::PAGE_FLIP_EVENT,
            AtomicCommitKind::Scene,
            AtomicCommitRequest::new(),
            &event,
        )
        .is_err()
    );
    assert_eq!(
        recorder.records().len(),
        0,
        "precondition must precede executor"
    );
    sink.submit(
        &device,
        AtomicCommitFlags::TEST_ONLY,
        AtomicCommitKind::Scene,
        AtomicCommitRequest::new(),
        &[],
    )
    .expect("dedicated TEST_ONLY request remains unit-returning");
    sink.submit(
        &device,
        AtomicCommitFlags::NONBLOCK,
        AtomicCommitKind::Scene,
        AtomicCommitRequest::new(),
        &[],
    )
    .expect("empty non-event request remains unit-returning");
    assert!(
        recorder
            .records()
            .iter()
            .all(|record| record.user_data == 0)
    );
    assert_eq!(sink.for_tests_pending_page_flip_entry_count(), 0);
}

fn pending(id: u64) -> PendingPageFlip {
    PendingPageFlip {
        commit_id: id,
        async_path: None,
        async_accepted_at: None,
        async_present_received_at: None,
        tearing_generation: None,
        direct_present_received_at: None,
    }
}

#[test]
// Mutation: route every explicit cookie to the queue front or fall back to FIFO after a miss.
fn pending_page_flip_index_is_exact_or_zero_cookie_fifo() {
    let queue = VecDeque::from([pending(11), pending(22)]);
    assert_eq!(pending_page_flip_index(&queue, Some(22)), Some(1));
    assert_eq!(pending_page_flip_index(&queue, Some(99)), None);
    assert_eq!(pending_page_flip_index(&queue, Some(11)), Some(0));
    assert_eq!(pending_page_flip_index(&queue, None), Some(0));
    let one = VecDeque::from([pending(33)]);
    assert_eq!(pending_page_flip_index(&one, None), Some(0));
    assert_eq!(pending_page_flip_index(&one, Some(33)), Some(0));
    assert_eq!(pending_page_flip_index(&VecDeque::new(), None), None);
    assert_eq!(pending_page_flip_index(&VecDeque::new(), Some(33)), None);
}

#[test]
// Mutation: use saturating progression or wrap the final ID to zero.
fn commit_id_allocator_does_not_reuse_after_exhaustion() {
    let mut window = CommitWindow::new(Instant::now());
    window.next_commit_id = Some(u64::MAX);
    assert_eq!(window.allocate_commit_id().expect("last ID"), u64::MAX);
    let error = window
        .allocate_commit_id()
        .expect_err("exhaustion must fail");
    assert!(error.to_string().contains("exhausted"));
}
