//! CPU frames on lavapipe. Oracle for bytes: `get_image` of the backing at
//! the frame's seq; damage and extents from spec §4.0, §4.5, §4.5.2.

use yserver_core::{resources::ROOT_WINDOW, rootless::Rect};

use super::{Call, Deliver, Fixture, Ready, Release, lock};
use crate::{
    kms::render::gpu::core_thread_fence_waits,
    rootless::{
        Path,
        host::{FrameEvent, FrameSeq, HandoffKind},
    },
};

const W: u32 = 0x0010_0001;
const C: u32 = 0x0010_0002;

fn rect(x: i32, y: i32, width: u16, height: u16) -> Rect {
    Rect {
        x,
        y,
        width,
        height,
    }
}

/// W at (10, 10), 40x30, border 2, mapped.
fn mapped(f: &mut Fixture) {
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 2);
    f.map(W);
}

fn covers(outer: &[Rect], inner: Rect) -> bool {
    outer.iter().any(|r| {
        r.x <= inner.x
            && r.y <= inner.y
            && r.x + i32::from(r.width) >= inner.x + i32::from(inner.width)
            && r.y + i32::from(r.height) >= inner.y + i32::from(inner.height)
    })
}

/// §4.0: native content = outer extent; §4.5: the first frame is full.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn first_frame_is_full_extent_and_equals_the_backing() {
    let Some(mut f) = Fixture::rootless_or_skip(|_| {}) else {
        return;
    };
    mapped(&mut f);
    f.fill(W, 0x00ff_0000, (5, 5, 10, 10));
    let frames = f.frames_until(1);
    let frame = &frames[0];
    assert_eq!(frame.id, f.id(W));
    assert_eq!((frame.width, frame.height, frame.stride), (44, 34, 176));
    assert_eq!(frame.depth, 24, "CopyFromParent on the root");
    assert_eq!(frame.damage, [rect(0, 0, 44, 34)]);
    assert_eq!(frame.backing_gen, f.backing(W).as_u64());
    assert_eq!(frame.pixels, f.full_readback(W));
    f.tail();
    assert!(f.calls().contains(&Call::FirstFrame(f.id(W))));
}

/// §4.0: backing (0,0) is the outer corner, content at (bw, bw); X child
/// coordinates are relative to the parent's content origin.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn child_damage_arrives_in_backing_pixels() {
    let Some(mut f) = Fixture::rootless_or_skip(|_| {}) else {
        return;
    };
    f.create_window(W, ROOT_WINDOW.0, (10, 10, 40, 30), 3);
    f.create_window(C, W, (5, 6, 10, 8), 0);
    f.map(C);
    f.map(W);
    f.frames_until(1);
    f.tail();
    f.fill(C, 0x0000_ff00, (0, 0, 10, 8));
    let frames = f.frames_until(2);
    assert_eq!(frames[1].damage, [rect(8, 9, 10, 8)]);
    assert_eq!(frames[1].pixels, f.full_readback(W));
}

/// §4.5: the snapshot is acked on `Delivered` only; a dropped frame's
/// damage is still in the store and in the next frame.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn snapshot_acked_only_on_delivered() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.deliver = Deliver::DropNext) else {
        return;
    };
    mapped(&mut f);
    let a = rect(4, 4, 6, 6);
    f.fill(W, 0x00ff_0000, (2, 2, 6, 6));
    f.frames_until(1);
    f.tail();
    let id = f.id(W);
    assert!(f.calls().contains(&Call::Cpu(id, FrameSeq(1))));
    let store: Vec<Rect> = f
        .store_damage(W)
        .into_iter()
        .map(|r| {
            rect(
                r.offset.x,
                r.offset.y,
                r.extent.width as u16,
                r.extent.height as u16,
            )
        })
        .collect();
    assert!(covers(&store, a), "dropped frame acked: {store:?}");
    let frames = f.frames_until(2);
    assert!(covers(&frames[1].damage, a));
    f.tail();
    assert!(f.store_damage(W).is_empty(), "delivered frame 2 acked");
}

/// §4.5 resize rule: a readback whose generation went stale is discarded,
/// and the next frame is full at the new extent.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn resize_during_readback_discards_and_repaints_full() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.ready = Ready::Manual) else {
        return;
    };
    mapped(&mut f);
    f.tail();
    f.tail();
    let id = f.id(W);
    let old = f.backing(W).as_u64();
    let in_flight = matches!(&f.presenter().windows[&id].path, Path::Cpu(cpu) if cpu.in_flight());
    assert!(in_flight, "the first readback started");
    f.resize(W, 60, 40);
    let frames = f.frames_until(1);
    let new = f.backing(W).as_u64();
    assert_ne!(old, new);
    assert!(frames.iter().all(|fr| fr.backing_gen == new));
    assert_eq!((frames[0].width, frames[0].height), (64, 44));
    assert_eq!(frames[0].damage, [rect(0, 0, 64, 44)]);
    assert_eq!(frames[0].pixels, f.full_readback(W));
}

/// §4.5.1: a late `Delivered` of a first frame from before a resize does
/// not show the window; the new generation's first frame does.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn late_delivered_after_a_resize_does_not_show_the_window() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| {
        log.deliver = Deliver::Manual;
        log.ready = Ready::Manual;
    }) else {
        return;
    };
    mapped(&mut f);
    f.frames_until(1);
    let id = f.id(W);
    f.resize(W, 60, 40);
    f.tail();
    f.link.post_frame(FrameEvent::Delivered {
        id,
        seq: FrameSeq(1),
    });
    f.link.post_frame(FrameEvent::Ready { id });
    let frames = f.frames_until(2);
    assert!(
        !f.calls().contains(&Call::FirstFrame(id)),
        "old frame shown"
    );
    assert_eq!(frames[1].backing_gen, f.backing(W).as_u64());
    f.link.post_frame(FrameEvent::Delivered {
        id,
        seq: FrameSeq(2),
    });
    f.tail();
    assert!(f.calls().contains(&Call::FirstFrame(id)));
}

/// §4.5.1 liveness (b): a frame dropped after a resize is retried full at
/// the new generation.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn host_drops_a_resized_frame_then_retries_full() {
    let Some(mut f) = Fixture::rootless_or_skip(|_| {}) else {
        return;
    };
    mapped(&mut f);
    f.frames_until(1);
    f.tail();
    lock(&f.log).deliver = Deliver::DropNext;
    f.resize(W, 60, 40);
    let frames = f.frames_until(3);
    let new = f.backing(W).as_u64();
    assert_eq!(frames[1].backing_gen, new);
    assert_eq!(frames[2].backing_gen, new);
    assert_eq!(frames[2].damage, [rect(0, 0, 64, 44)]);
    assert_eq!(frames[2].pixels, f.full_readback(W));
}

/// §4.5: frames are owned and `Send`; a host thread keeps them.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn frames_outlive_the_call() {
    let (tx, rx) = std::sync::mpsc::channel();
    let holder = std::thread::spawn(move || rx.iter().collect::<Vec<_>>());
    let Some(mut f) = Fixture::rootless_or_skip(|log| {
        log.release = Release::Never;
        log.sink = Some(tx);
    }) else {
        return;
    };
    mapped(&mut f);
    let mut oracle = Vec::new();
    for (k, pixel) in [0x00ff_0000u32, 0x0000_ff00, 0x0000_00ff]
        .into_iter()
        .enumerate()
    {
        f.fill(W, pixel, (0, 0, 20 + 5 * k as u16, 20));
        f.frames_until(k + 1);
        oracle.push(f.full_readback(W));
    }
    lock(&f.log).sink = None;
    drop(f);
    let held = holder.join().expect("holder thread");
    assert_eq!(held.len(), 3);
    for (frame, want) in held.iter().zip(&oracle) {
        assert_eq!(&frame.pixels, want, "frame {:?}", frame.seq);
    }
}

/// §4.5.2: buffers released out of order are refilled from their own seq;
/// one older than the history gets a full copy.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn pool_buffers_released_out_of_order_stay_canonical() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.release = Release::Manual) else {
        return;
    };
    mapped(&mut f);
    let fills: [(u32, (i16, i16, u16, u16)); 5] = [
        (0x00ff_0000, (0, 0, 8, 8)),
        (0x0000_ff00, (10, 0, 8, 8)),
        (0x0000_00ff, (20, 0, 8, 8)),
        (0x00ff_ff00, (0, 10, 8, 8)),
        (0x00ff_00ff, (10, 10, 8, 8)),
    ];
    let release = |f: &mut Fixture, seq: u64| {
        let id = f.id(W);
        let frame = {
            let mut log = lock(&f.log);
            let at = log
                .held
                .iter()
                .position(|fr| fr.seq == FrameSeq(seq))
                .expect("held");
            log.held.remove(at)
        };
        f.link.post_frame(FrameEvent::Released {
            id,
            seq: frame.seq,
            pixels: frame.pixels,
        });
    };
    let mut oracle = Vec::new();
    for (k, (pixel, r)) in fills.into_iter().enumerate() {
        match k {
            3 => release(&mut f, 2),
            4 => release(&mut f, 1),
            _ => {}
        }
        f.fill(W, pixel, r);
        f.frames_until(k + 1);
        oracle.push(f.full_readback(W));
    }
    release(&mut f, 3);
    let frames = lock(&f.log).frames.clone();
    assert_eq!(frames.len(), 5);
    for (frame, want) in frames.iter().zip(&oracle) {
        assert_eq!(&frame.pixels, want, "frame {:?}", frame.seq);
    }
}

/// §4.5.2 "Order".
#[test]
#[ignore = "needs live Vulkan ICD"]
fn submissions_are_in_seq_order() {
    let Some(mut f) = Fixture::rootless_or_skip(|_| {}) else {
        return;
    };
    mapped(&mut f);
    for k in 0..6u16 {
        f.fill(W, 0x0010_0000 * u32::from(k + 1), (k as i16 * 4, 0, 4, 4));
        f.frames_until(usize::from(k) + 1);
    }
    let seqs: Vec<u64> = f
        .calls()
        .iter()
        .filter_map(|c| match c {
            Call::Cpu(_, seq) => Some(seq.0),
            _ => None,
        })
        .collect();
    assert_eq!(seqs, (1..=6).collect::<Vec<_>>());
}

/// Spec §10: the CPU path never waits on a fence on the core thread.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn cpu_path_never_waits_on_the_core_thread() {
    let Some(mut f) = Fixture::rootless_or_skip(|_| {}) else {
        return;
    };
    mapped(&mut f);
    let before = core_thread_fence_waits();
    for k in 1..=5u16 {
        f.fill(W, 0x0001_0000 * u32::from(k), (k as i16 * 4, 0, 4, 4));
        f.frames_until(usize::from(k));
    }
    assert_eq!(core_thread_fence_waits(), before);
}

/// D4: a host asking for dmabufs gets CPU frames in RL1.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn dmabuf_handoff_falls_back_to_cpu() {
    let Some(mut f) = Fixture::rootless_or_skip(|log| log.handoff = HandoffKind::Dmabuf) else {
        return;
    };
    mapped(&mut f);
    let frames = f.frames_until(1);
    assert_eq!(frames[0].pixels, f.full_readback(W));
}
