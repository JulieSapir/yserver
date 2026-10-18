//! What a redirected window's backing holds after client draws, resizes and
//! tree changes. Every expected pixel is Xorg's, measured by
//! `tools/vng-scenarios/redirected-backing-probe.c` (plan
//! `2026-10-18-redirected-backing-geometry`).
use super::*;

const RED: u32 = 0x00FF_0000;
const BLUE: u32 = 0x0000_00FF;
const WHITE: u32 = 0x00FF_FFFF;
const GREEN: u32 = 0x0000_FF00;

/// CWBackPixel | CWBorderPixel | CWWinGravity.
const RG_MASK: u32 = 0x0000_002A;

/// A fixture with `RedirectSubwindows(root, Manual)` set, as a compositor does.
fn rg_fixture() -> Option<ProtoFixture> {
    let mut f = ProtoFixture::new()?;
    let root = yserver_core::resources::ROOT_WINDOW.0;
    let mut body = Vec::new();
    body.extend_from_slice(&root.to_le_bytes());
    body.extend_from_slice(&[1u8, 0, 0, 0]); // CompositeRedirectManual
    f.req(144, 2, &body);
    Some(f)
}

#[allow(clippy::too_many_arguments)]
fn rg_window(
    f: &mut ProtoFixture,
    wid: u32,
    parent: u32,
    x: i16,
    y: i16,
    w: u16,
    h: u16,
    bw: u16,
    bg: u32,
    border: u32,
    gravity: u32,
) {
    let visual = yserver_core::resources::ROOT_VISUAL.0;
    or_create_window(
        f,
        wid,
        parent,
        24,
        x,
        y,
        w,
        h,
        bw,
        visual,
        RG_MASK,
        &[bg, border, gravity],
    );
}

/// One pixel of the backing `res` paints into, in backing coordinates.
fn rg_pix(f: &mut ProtoFixture, res: u32, x: usize, y: usize) -> [u8; 3] {
    let (sw, sh, px) = f.backing(res);
    assert!(
        x < sw as usize && y < sh as usize,
        "({x},{y}) outside {sw}x{sh}"
    );
    let o = (y * sw as usize + x) * 4;
    [px[o], px[o + 1], px[o + 2]]
}

fn rg_assert(f: &mut ProtoFixture, res: u32, what: &str, pts: &[(usize, usize, u32)]) {
    for &(x, y, want) in pts {
        assert_eq!(
            rg_pix(f, res, x, y),
            or_bgr(want),
            "{what}: backing ({x},{y}) must be {want:06x} as on Xorg",
        );
    }
}

/// D fixture: W 100x100 bw 4, red border, white bg, green child (50,50)
/// 20x20, both mapped under the redirect.
fn rg_d_window(f: &mut ProtoFixture, w: u32, c: u32) {
    let root = yserver_core::resources::ROOT_WINDOW.0;
    rg_window(f, w, root, 0, 0, 100, 100, 4, WHITE, RED, 1);
    rg_window(f, c, w, 50, 50, 20, 20, 0, GREEN, 0, 1);
    wz_map(f, c);
    wz_map(f, w);
}

/// Xorg: (1,1)/(3,3) ring, (4,4)/(13,13) the blue 10x10, (14,14) background.
const D_POINTS: [(usize, usize, u32); 5] = [
    (1, 1, RED),
    (3, 3, RED),
    (4, 4, BLUE),
    (13, 13, BLUE),
    (14, 14, WHITE),
];

/// Xorg draws a redirected window at its content origin in the pixmap
/// (`__fbPixOffXWin`, fb/fb.h:445; `screen_x = drawable.x - bw`,
/// composite/compwindow.c:240-252), clipped by its children.
/// Mutation: `host_drawable_target` names the backing again.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn client_draws_land_in_a_bordered_redirected_windows_content() {
    const GC: u32 = 0x1A00;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };

    // PolyFillRectangle, then a ClipByChildren fill across the child.
    rg_d_window(&mut f, 0x1A10, 0x1A11);
    or_create_gc(&mut f, GC, 0x1A10, BLUE);
    or_fill(&mut f, 0x1A10, GC, 0, 0, 10, 10);
    or_fill(&mut f, 0x1A10, GC, 40, 40, 40, 40);
    rg_assert(&mut f, 0x1A10, "PolyFillRectangle", &D_POINTS);
    rg_assert(
        &mut f,
        0x1A10,
        "ClipByChildren fill",
        &[(60, 60, GREEN), (48, 48, BLUE)],
    );

    // PutImage, ZPixmap depth 24.
    rg_d_window(&mut f, 0x1A20, 0x1A21);
    let mut body = Vec::new();
    body.extend_from_slice(&0x1A20u32.to_le_bytes());
    body.extend_from_slice(&GC.to_le_bytes());
    body.extend_from_slice(&10u16.to_le_bytes());
    body.extend_from_slice(&10u16.to_le_bytes());
    body.extend_from_slice(&0i16.to_le_bytes());
    body.extend_from_slice(&0i16.to_le_bytes());
    body.extend_from_slice(&[0, 24, 0, 0]);
    for _ in 0..100 {
        body.extend_from_slice(&BLUE.to_le_bytes());
    }
    f.req(72, 2, &body);
    rg_assert(&mut f, 0x1A20, "PutImage", &D_POINTS);

    // CopyPlane from an all-ones bitmap, foreground blue.
    rg_d_window(&mut f, 0x1A30, 0x1A31);
    const BM: u32 = 0x1A32;
    const BM_GC: u32 = 0x1A33;
    let root = yserver_core::resources::ROOT_WINDOW.0;
    let mut body = Vec::new();
    body.extend_from_slice(&BM.to_le_bytes());
    body.extend_from_slice(&root.to_le_bytes());
    body.extend_from_slice(&10u16.to_le_bytes());
    body.extend_from_slice(&10u16.to_le_bytes());
    f.req(53, 1, &body); // CreatePixmap depth 1
    or_create_gc(&mut f, BM_GC, BM, 1);
    or_fill(&mut f, BM, BM_GC, 0, 0, 10, 10);
    let mut body = Vec::new();
    body.extend_from_slice(&BM.to_le_bytes());
    body.extend_from_slice(&0x1A30u32.to_le_bytes());
    body.extend_from_slice(&GC.to_le_bytes());
    body.extend_from_slice(&[0; 8]);
    body.extend_from_slice(&10u16.to_le_bytes());
    body.extend_from_slice(&10u16.to_le_bytes());
    body.extend_from_slice(&1u32.to_le_bytes());
    f.req(63, 0, &body);
    rg_assert(&mut f, 0x1A30, "CopyPlane", &D_POINTS);
}

/// `DoGetImage` and `CopyArea` read a window from its content origin, and
/// GetImage at (-1,-1) reads the border (Xorg: ff0000).
/// Mutation: `host_drawable_target` names the backing again.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn get_image_and_copy_area_read_a_redirected_window_through_its_content() {
    const W: u32 = 0x1B10;
    const C: u32 = 0x1B11;
    const GC: u32 = 0x1B12;
    const PM: u32 = 0x1B13;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    rg_d_window(&mut f, W, C);
    or_create_gc(&mut f, GC, W, BLUE);
    or_fill(&mut f, W, GC, 0, 0, 10, 10);
    let _ = f.take_reply();

    // GetImage(ZPixmap, W, -1, -1, 12, 12): reply pixel (i, j) is window (i-1, j-1).
    let mut body = Vec::new();
    body.extend_from_slice(&W.to_le_bytes());
    body.extend_from_slice(&(-1i16).to_le_bytes());
    body.extend_from_slice(&(-1i16).to_le_bytes());
    body.extend_from_slice(&12u16.to_le_bytes());
    body.extend_from_slice(&12u16.to_le_bytes());
    body.extend_from_slice(&u32::MAX.to_le_bytes());
    f.req(73, 2, &body);
    let reply = f.take_reply();
    assert!(
        reply.len() >= 32 + 12 * 12 * 4,
        "GetImage reply: {} bytes",
        reply.len()
    );
    assert_eq!(reply[0], 1, "a reply, not an error: {:02x?}", &reply[..32]);
    let get = |wx: i32, wy: i32| {
        let o = 32 + (((wy + 1) * 12 + (wx + 1)) * 4) as usize;
        [reply[o], reply[o + 1], reply[o + 2]]
    };
    for (x, y, want) in [(-1, -1, RED), (0, 0, BLUE), (9, 9, BLUE), (10, 10, WHITE)] {
        assert_eq!(get(x, y), or_bgr(want), "GetImage window ({x},{y})");
    }

    // CopyArea(W -> pixmap, 0,0 12x12).
    let root = yserver_core::resources::ROOT_WINDOW.0;
    let mut body = Vec::new();
    body.extend_from_slice(&PM.to_le_bytes());
    body.extend_from_slice(&root.to_le_bytes());
    body.extend_from_slice(&12u16.to_le_bytes());
    body.extend_from_slice(&12u16.to_le_bytes());
    f.req(53, 24, &body);
    let mut body = Vec::new();
    body.extend_from_slice(&W.to_le_bytes());
    body.extend_from_slice(&PM.to_le_bytes());
    body.extend_from_slice(&GC.to_le_bytes());
    body.extend_from_slice(&[0; 8]);
    body.extend_from_slice(&12u16.to_le_bytes());
    body.extend_from_slice(&12u16.to_le_bytes());
    f.req(62, 0, &body);
    let pm_host = f
        .state
        .resources
        .pixmap(yserver_protocol::x11::ResourceId(PM))
        .and_then(|p| p.host_xid)
        .expect("pixmap host")
        .as_raw();
    let px = f
        .backend
        .get_image_pixels_for_tests(pm_host, 2, 0, 0, 12, 12, !0)
        .expect("get_image")
        .expect("bytes");
    for (x, y, want) in [(0usize, 0usize, BLUE), (9, 9, BLUE), (10, 10, WHITE)] {
        let o = (y * 12 + x) * 4;
        assert_eq!(
            [px[o], px[o + 1], px[o + 2]],
            or_bgr(want),
            "CopyArea from window ({x},{y})"
        );
    }
}

/// G fixture: W `size` square at root, white; a SouthEast-gravity child
/// 30x30 bw 3 (blue, red border) at `(c_at, c_at)`; both mapped.
fn rg_gravity(f: &mut ProtoFixture, w: u32, c: u32, size: u16, c_at: i16) {
    let root = yserver_core::resources::ROOT_WINDOW.0;
    rg_window(f, w, root, 0, 0, size, size, 0, WHITE, 0, 1);
    rg_window(f, c, w, c_at, c_at, 30, 30, 3, BLUE, RED, 9);
    wz_map(f, c);
    wz_map(f, w);
}

/// `compCopyWindow` copies each gravity group from `pOldPixmap`
/// (composite/compwindow.c:489-528) into the reallocated pixmap.
/// Xorg: ring red, content blue at the new place, vacated area white.
/// Mutation: `carry_from = None`.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn gravity_moves_a_child_into_the_grown_area_with_its_pixels() {
    const W: u32 = 0x1C10;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    rg_gravity(&mut f, W, 0x1C11, 100, 60);
    rg_assert(&mut f, W, "before", &[(61, 75, RED), (75, 75, BLUE)]);
    wz_configure(&mut f, W, 0x0C, &[200, 200]);
    rg_assert(
        &mut f,
        W,
        "G",
        &[
            (161, 178, RED),
            (194, 178, RED),
            (178, 161, RED),
            (178, 194, RED),
            (178, 178, BLUE),
            (75, 75, WHITE),
            (61, 75, WHITE),
        ],
    );
}

/// GS: the child moves up-left on a shrink (green before this branch; guard).
#[test]
#[ignore = "needs live Vulkan ICD"]
fn gravity_moves_a_child_up_left_on_shrink() {
    const W: u32 = 0x1C20;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    rg_gravity(&mut f, W, 0x1C21, 200, 160);
    wz_configure(&mut f, W, 0x0C, &[100, 100]);
    rg_assert(
        &mut f,
        W,
        "GS",
        &[
            (61, 78, RED),
            (94, 78, RED),
            (78, 61, RED),
            (78, 94, RED),
            (78, 78, BLUE),
        ],
    );
}

/// While W is redirected its backing is authoritative and the leaves of
/// its subtree are stale; `compReallocPixmap` seeds only from the parent
/// (`compNewPixmap`, composite/compalloc.c:532-560), never from them. C has
/// no background, so the part the grow reveals keeps whatever the seed put
/// there, and must not be C's stale leaf.
/// Mutation: overlay the leaves in `reallocate_redirected_backing`.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn realloc_does_not_round_trip_through_leaves() {
    const W: u32 = 0x1C30;
    const C: u32 = 0x1C31;
    const GC: u32 = 0x1C32;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    let root = yserver_core::resources::ROOT_WINDOW.0;
    let visual = yserver_core::resources::ROOT_VISUAL.0;
    rg_window(&mut f, W, root, 0, 0, 100, 100, 0, WHITE, 0, 1);
    // CWBackPixmap None.
    or_create_window(
        &mut f,
        C,
        W,
        24,
        80,
        10,
        40,
        30,
        0,
        visual,
        0x0000_0001,
        &[0],
    );
    wz_map(&mut f, C);
    wz_map(&mut f, W);
    or_create_gc(&mut f, GC, C, BLUE);
    or_fill(&mut f, C, GC, 0, 0, 40, 30);
    let c_host = f.host_xid(C);
    assert!(
        f.backend
            .fill_leaf_rect_for_tests(c_host, 0, 0, 40, 30, GREEN)
    );
    wz_configure(&mut f, W, 0x0C, &[150, 150]);
    rg_assert(&mut f, W, "C's copied part", &[(85, 15, BLUE)]);
    assert_ne!(
        rg_pix(&mut f, W, 110, 15),
        or_bgr(GREEN),
        "the revealed part of C must not come from its stale leaf",
    );
}
