use drm::control::{crtc, from_u32};
use std::{io::Write as _, time::Duration};

use super::{
    DRM_EVENT_CRTC_SEQUENCE, DRM_EVENT_FLIP_COMPLETE, dispatch_events, drm_event_crtc_sequence,
    drm_event_header, drm_event_vblank,
};

fn flip(user_data: u64, sec: u32, usec: u32, sequence: u32, crtc_id: u32) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
    bytes.extend_from_slice(&32u32.to_ne_bytes());
    bytes.extend_from_slice(&user_data.to_ne_bytes());
    bytes.extend_from_slice(&sec.to_ne_bytes());
    bytes.extend_from_slice(&usec.to_ne_bytes());
    bytes.extend_from_slice(&sequence.to_ne_bytes());
    bytes.extend_from_slice(&crtc_id.to_ne_bytes());
    bytes
}

fn sequence_event(user_data: u64, time_ns: i64, sequence: u64) -> Vec<u8> {
    let mut bytes = Vec::with_capacity(32);
    bytes.extend_from_slice(&DRM_EVENT_CRTC_SEQUENCE.to_ne_bytes());
    bytes.extend_from_slice(&32u32.to_ne_bytes());
    bytes.extend_from_slice(&user_data.to_ne_bytes());
    bytes.extend_from_slice(&time_ns.to_ne_bytes());
    bytes.extend_from_slice(&sequence.to_ne_bytes());
    bytes
}

type FlipCall = (crtc::Handle, u32, Duration, u64);

fn dispatch(bytes: &[u8]) -> (Vec<FlipCall>, Vec<(u64, i64, u64)>) {
    let mut flips = Vec::new();
    let mut sequences = Vec::new();
    dispatch_events(
        bytes,
        &mut |crtc, frame, duration, cookie| flips.push((crtc, frame, duration, cookie)),
        &mut |cookie, time_ns, msc| sequences.push((cookie, time_ns, msc)),
    );
    (flips, sequences)
}

#[test]
// Mutation: discard/truncate the cookie or derive the routed CRTC from cookie bits.
fn dispatch_passes_crtc_handle_and_commit_identity_for_page_flip() {
    let handle: crtc::Handle = from_u32(42).expect("nonzero CRTC");
    let cookie = 0xFEDC_BA98_0000_0042;
    let (flips, sequences) = dispatch(&flip(cookie, 3, 500, 77, 42));
    assert_eq!(flips, [(handle, 77, Duration::new(3, 500_000), cookie)]);
    assert!(sequences.is_empty());
}

#[test]
// Mutation: restore the user_data-as-CRTC fallback when crtc_id is zero.
fn dispatch_does_not_reinterpret_user_data_when_crtc_id_is_zero() {
    let mut bytes = flip(0, 0, 0, 1, 0);
    bytes.extend(flip(0xCAFE_BABE_0000_0009, 0, 0, 2, 0));
    bytes.extend(flip(55, 0, 0, 3, 9));
    let mut flips = Vec::new();
    let mut sequences = Vec::new();
    let unroutable_flips = dispatch_events(
        &bytes,
        &mut |crtc, frame, duration, cookie| flips.push((crtc, frame, duration, cookie)),
        &mut |cookie, time_ns, sequence| sequences.push((cookie, time_ns, sequence)),
    );
    let handle: crtc::Handle = from_u32(9).expect("known CRTC");
    assert_eq!(flips, [(handle, 3, Duration::ZERO, 55)]);
    assert!(sequences.is_empty());
    assert_eq!(unroutable_flips, 2, "each zero-CRTC record is diagnosed");
}

#[test]
// Mutation: fabricate a default flip for an empty read.
fn dispatch_ignores_empty_read() {
    let (flips, sequences) = dispatch(&[]);
    assert!(flips.is_empty());
    assert!(sequences.is_empty());
}

#[test]
// Mutation: return after the first event or fail to advance over unknown records.
fn dispatch_walks_every_event_in_one_read() {
    let mut bytes = flip(5, 0, 0, 1, 42);
    bytes.extend_from_slice(&99u32.to_ne_bytes());
    bytes.extend_from_slice(&8u32.to_ne_bytes());
    bytes.extend(sequence_event(0x8000_0000_0000_0042, -7, 0x1_0000_0002));
    bytes.extend(flip(6, 0, 0, 2, 42));
    let order = std::cell::RefCell::new(Vec::new());
    dispatch_events(
        &bytes,
        &mut |_, frame, _, cookie| order.borrow_mut().push(("flip", u64::from(frame), cookie)),
        &mut |cookie, time_ns, msc| {
            order
                .borrow_mut()
                .push(("sequence", cookie, (time_ns as u64) ^ msc))
        },
    );
    assert_eq!(
        *order.borrow(),
        [
            ("flip", 1, 5),
            (
                "sequence",
                0x8000_0000_0000_0042,
                (-7i64 as u64) ^ 0x1_0000_0002
            ),
            ("flip", 2, 6),
        ]
    );
}

#[test]
// Mutation: decode the truncated payload or continue from a guessed boundary after overrun.
fn dispatch_stops_at_an_event_that_overruns_the_read() {
    let mut bytes = flip(5, 0, 0, 1, 42);
    bytes.extend_from_slice(&DRM_EVENT_FLIP_COMPLETE.to_ne_bytes());
    bytes.extend_from_slice(&64u32.to_ne_bytes());
    bytes.extend_from_slice(&[0xAA; 8]);
    bytes.extend(flip(6, 0, 0, 2, 42));
    let (flips, _) = dispatch(&bytes);
    assert_eq!(flips.len(), 1);
    assert_eq!(flips[0].1, 1);
}

#[test]
// Mutation: remove malformed-length guards or dispatch incomplete payloads as complete events.
fn dispatch_rejects_short_and_zero_length_event_records() {
    let mut short_flip = 2u32.to_ne_bytes().to_vec();
    short_flip.extend_from_slice(&8u32.to_ne_bytes());
    short_flip.extend(flip(6, 0, 0, 2, 42));
    let mut wrong_sequence = DRM_EVENT_CRTC_SEQUENCE.to_ne_bytes().to_vec();
    wrong_sequence.extend_from_slice(&16u32.to_ne_bytes());
    wrong_sequence.extend_from_slice(&[0; 8]);
    wrong_sequence.extend(flip(7, 0, 0, 3, 42));

    let cases = [
        (vec![0; 7], 0, 0),
        (vec![2, 0, 0, 0, 0, 0, 0, 0], 0, 0),
        (
            {
                let mut b = 2u32.to_ne_bytes().to_vec();
                b.extend_from_slice(&4u32.to_ne_bytes());
                b
            },
            0,
            0,
        ),
        (short_flip, 1, 0),
        (wrong_sequence, 1, 0),
    ];
    for (bytes, expected_flips, expected_sequences) in cases {
        let (flips, sequences) = dispatch(&bytes);
        assert_eq!(flips.len(), expected_flips, "bytes={bytes:?}");
        assert_eq!(sequences.len(), expected_sequences, "bytes={bytes:?}");
    }
}

#[test]
// Mutation: collapse the independent CRTC field into user_data or remove it from the ABI struct.
fn drm_event_vblank_struct_is_32_bytes() {
    assert_eq!(std::mem::size_of::<drm_event_vblank>(), 32);
    assert_eq!(std::mem::offset_of!(drm_event_vblank, user_data), 8);
    assert_eq!(std::mem::offset_of!(drm_event_vblank, sequence), 24);
    assert_eq!(std::mem::offset_of!(drm_event_vblank, crtc_id), 28);
}

#[test]
// Mutation: truncate sequence cookies, make time unsigned, or narrow sequence to 32 bits.
fn dispatch_decodes_crtc_sequence() {
    let cookie = 0xCAFE_BABE_0000_0042;
    let sequence = 0xFEDC_BA98_7654_3210;
    let (flips, sequences) = dispatch(&sequence_event(cookie, -1_234_567, sequence));
    assert!(flips.is_empty());
    assert_eq!(sequences, [(cookie, -1_234_567, sequence)]);
}

#[test]
// Mutation: keep using drm::Device::receive_events and synthesize a zero cookie.
fn drain_events_delivers_the_commit_identity_of_a_flip_complete() {
    let (device, mut feed) = device_with_event_feed();
    let cookie = 0xF123_4567_89AB_CDEF;
    feed.write_all(&flip(cookie, 3, 500, 77, 42))
        .expect("inject event bytes");
    let handle: crtc::Handle = from_u32(42).expect("nonzero CRTC");
    let mut seen = Vec::new();
    super::drain_events(
        &device,
        |crtc, frame, duration, user_data| seen.push((crtc, frame, duration, user_data)),
        |_, _, _| {},
    )
    .expect("poll and drain event bytes");
    assert_eq!(seen, [(handle, 77, Duration::new(3, 500_000), cookie)]);
}

#[test]
// Mutation: remove the poll-before-read guard and block on an empty inherited fd.
fn drain_events_returns_when_no_event_is_pending() {
    let (device, feed) = device_with_event_feed();
    let (tx, rx) = std::sync::mpsc::channel();
    let reader = std::thread::spawn(move || {
        let mut flips = 0;
        let result = super::drain_events(&device, |_, _, _, _| flips += 1, |_, _, _| {});
        tx.send((result.is_ok(), flips)).expect("send drain result");
    });
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2))
            .expect("empty drain returns"),
        (true, 0)
    );
    drop(feed);
    reader.join().expect("drain thread exits");
}

fn device_with_event_feed() -> (crate::drm::Device, std::os::unix::net::UnixStream) {
    let (ours, feed) = std::os::unix::net::UnixStream::pair().expect("socketpair");
    ours.set_nonblocking(true)
        .expect("event-feed socket supports nonblocking reads");
    let device = crate::drm::Device::from_inherited_kms_fd(
        std::os::fd::OwnedFd::from(ours),
        "<socket-backed-test-device>",
    );
    (device, feed)
}

#[allow(dead_code)]
fn _abi_types(_: drm_event_header, _: drm_event_crtc_sequence) {}

#[test]
fn drm_crtc_queue_sequence_struct_is_24_bytes() {
    // drm.h: __u32 crtc_id; __u32 flags; __u64 sequence; __u64 user_data;
    // → 4+4+8+8 = 24 bytes.
    assert_eq!(std::mem::size_of::<super::drm_crtc_queue_sequence>(), 24);
    assert_eq!(std::mem::align_of::<super::drm_crtc_queue_sequence>(), 8);
}

#[test]
fn drm_event_crtc_sequence_struct_is_32_bytes() {
    // drm.h: struct drm_event base (8B) + __u64 user_data + __s64 time_ns
    // + __u64 sequence = 8 + 8 + 8 + 8 = 32.
    assert_eq!(std::mem::size_of::<super::drm_event_crtc_sequence>(), 32);
    assert_eq!(std::mem::align_of::<super::drm_event_crtc_sequence>(), 8);
}

#[test]
fn drm_crtc_queue_sequence_ioctl_request_code() {
    // _IOWR('d' /*0x64*/, 0x3C, drm_crtc_queue_sequence):
    //   (3 << 30) | (24 << 16) | (0x64 << 8) | 0x3C = 0xC018643C
    assert_eq!(
        super::DRM_IOCTL_CRTC_QUEUE_SEQUENCE,
        0xC018_643C_u32 as super::IoctlReq
    );
}

#[test]
fn queue_sequence_flags_absolute_with_next_on_miss() {
    use super::{DRM_CRTC_SEQUENCE_NEXT_ON_MISS, drm_crtc_queue_sequence};
    let req = drm_crtc_queue_sequence {
        crtc_id: 0x42,
        flags: DRM_CRTC_SEQUENCE_NEXT_ON_MISS,
        sequence: 0x1234_5678_9ABC_DEF0,
        user_data: 0x42,
    };
    assert_eq!(req.flags & 1, 0, "RELATIVE bit clear for absolute target");
    assert_eq!(req.flags & 2, 2, "NEXT_ON_MISS set");
}
