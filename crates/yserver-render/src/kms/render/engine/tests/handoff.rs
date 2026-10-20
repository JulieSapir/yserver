//! `begin_readback` and the external-read barriers. Oracle: `get_image`.

use super::*;
use crate::kms::render::{
    gpu::core_thread_fence_waits,
    store::DrawableStore,
    submit_group::FlushReason,
    target::{Dst, Src},
};

const W: u32 = 64;
const H: u32 = 32;
const SPLIT: u32 = 20;
const LEFT: [u8; 4] = [0x10, 0x20, 0x30, 0xff];
const RIGHT: [u8; 4] = [0xa0, 0xb0, 0xc0, 0xff];

fn rect(x: i32, y: i32, w: u32, h: u32) -> vk::Rect2D {
    vk::Rect2D {
        offset: vk::Offset2D { x, y },
        extent: vk::Extent2D {
            width: w,
            height: h,
        },
    }
}

/// A 64×32 depth-32 drawable, `LEFT` for x < 20, `RIGHT` from 20 on.
fn two_colour(
    engine: &mut RenderEngine,
    store: &mut DrawableStore,
    gpu: &mut GpuCore,
) -> DrawableId {
    let id = alloc_drawable_3a(gpu, store, 0x1, W as u16, H as u16);
    let mut bytes = Vec::with_capacity((W * H * 4) as usize);
    for _ in 0..H {
        for x in 0..W {
            bytes.extend_from_slice(if x < SPLIT { &LEFT } else { &RIGHT });
        }
    }
    engine
        .put_image(
            store,
            gpu,
            Dst::server_internal(id),
            vk::Offset2D { x: 0, y: 0 },
            vk::Extent2D {
                width: W,
                height: H,
            },
            &bytes,
            32,
        )
        .expect("put_image");
    id
}

fn poll_until_done(pending: &PendingReadback, vk: &VkContext) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
    while !pending.poll(vk).expect("poll") {
        assert!(
            std::time::Instant::now() < deadline,
            "readback never completed"
        );
        std::thread::yield_now();
    }
}

#[test]
#[ignore = "needs live Vulkan ICD"]
fn begin_readback_matches_get_image_per_rect() {
    let Some(mut gpu) = live_gpu() else {
        eprintln!("no VkContext available — skipping");
        return;
    };
    let mut store = DrawableStore::new();
    let mut engine = RenderEngine::new(&gpu).expect("engine");
    let id = two_colour(&mut engine, &mut store, &mut gpu);
    let rects = [rect(0, 0, 8, 8), rect(30, 10, 12, 6)];

    let pending = engine
        .begin_readback(&mut store, &mut gpu, id, &rects)
        .expect("begin_readback");
    let vk = gpu.vk().expect("vk").clone();
    poll_until_done(&pending, &vk);
    let stride = W as usize * 4;
    let mut canvas = vec![0u8; stride * H as usize];
    pending.write_into(&mut canvas, stride).expect("write_into");
    assert_eq!(pending.regions().collect::<Vec<_>>(), rects);

    let mut covered = vec![false; (W * H) as usize];
    for r in rects {
        let want = engine
            .get_image(&mut store, &mut gpu, Src::server_internal(id), r, 32)
            .expect("get_image");
        let row = r.extent.width as usize * 4;
        for y in 0..r.extent.height as usize {
            let at = (r.offset.y as usize + y) * stride + r.offset.x as usize * 4;
            assert_eq!(
                &canvas[at..at + row],
                &want[y * row..(y + 1) * row],
                "rect {r:?} row {y}"
            );
            for x in 0..r.extent.width as usize {
                covered[(r.offset.y as usize + y) * W as usize + r.offset.x as usize + x] = true;
            }
        }
    }
    for (i, px) in canvas.chunks_exact(4).enumerate() {
        if !covered[i] {
            assert_eq!(px, [0, 0, 0, 0], "pixel {i} outside the rects was written");
        }
    }
    drop(pending);
    engine.drain_all(&mut gpu);
}

#[test]
#[ignore = "needs live Vulkan ICD"]
fn begin_readback_never_waits() {
    let Some(mut gpu) = live_gpu() else {
        eprintln!("no VkContext available — skipping");
        return;
    };
    let mut store = DrawableStore::new();
    let mut engine = RenderEngine::new(&gpu).expect("engine");
    let id = two_colour(&mut engine, &mut store, &mut gpu);
    let vk = gpu.vk().expect("vk").clone();

    let before = core_thread_fence_waits();
    let pending = engine
        .begin_readback(&mut store, &mut gpu, id, &[rect(0, 0, W, H)])
        .expect("begin_readback");
    poll_until_done(&pending, &vk);
    assert_eq!(
        core_thread_fence_waits(),
        before,
        "begin_readback or poll waited"
    );

    engine
        .get_image(
            &mut store,
            &mut gpu,
            Src::server_internal(id),
            rect(0, 0, 4, 4),
            32,
        )
        .expect("get_image");
    assert_eq!(
        core_thread_fence_waits(),
        before + 1,
        "the counter counts get_image's wait"
    );
    drop(pending);
    engine.drain_all(&mut gpu);
}

#[test]
#[ignore = "needs live Vulkan ICD"]
fn external_read_restores_the_sampled_layout() {
    let Some(mut gpu) = live_gpu() else {
        eprintln!("no VkContext available — skipping");
        return;
    };
    let mut store = DrawableStore::new();
    let mut engine = RenderEngine::new(&gpu).expect("engine");
    let id = two_colour(&mut engine, &mut store, &mut gpu);
    let full = rect(0, 0, W, H);
    let before = engine
        .get_image(&mut store, &mut gpu, Src::server_internal(id), full, 32)
        .expect("get_image");
    let vk = gpu.vk().expect("vk").clone();

    let inner = engine.inner.as_mut().expect("inner");
    let (cb, ticket) = begin_op_cb(inner, &mut gpu).expect("cb");
    let (image, extent) = store
        .begin_external_read(id, &vk, cb)
        .expect("known drawable");
    assert_ne!(image, vk::Image::null());
    assert_eq!(extent, full.extent);
    assert_eq!(
        store.get(id).expect("drawable").storage.current_layout,
        vk::ImageLayout::TRANSFER_SRC_OPTIMAL
    );
    store.end_external_read(id, &vk, cb);
    end_and_submit_op(inner, &mut gpu, cb, &ticket).expect("submit");
    store.touch_render_fence(id, ticket.clone());
    engine
        .flush_submit_group(&mut store, &mut gpu, FlushReason::SyncBoundary)
        .expect("flush");
    assert_eq!(
        store.get(id).expect("drawable").storage.current_layout,
        vk::ImageLayout::SHADER_READ_ONLY_OPTIMAL
    );

    let after = engine
        .get_image(&mut store, &mut gpu, Src::server_internal(id), full, 32)
        .expect("get_image after the external read");
    assert_eq!(after, before);
    engine.drain_all(&mut gpu);
}
