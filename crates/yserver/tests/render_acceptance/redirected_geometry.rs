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

/// Xorg paints a window's newly visible border (`borderExposed`,
/// `PW_BORDER`) before its exposures (`miHandleValidateExposures`,
/// mi/miwindow.c:211-240). C: a child (80,80) 50x50 bw 4 crossing W's
/// edge, W grown 100 -> 200, both map orders: ring red as on Xorg.
/// Mutation: skip `paint_window_border_region`.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn a_childs_border_revealed_by_a_grow_is_painted() {
    for (n, child_first) in [(0u32, true), (1, false)] {
        let (w, c) = (0x1D10 + 2 * n, 0x1D11 + 2 * n);
        let Some(mut f) = rg_fixture() else {
            eprintln!("skipping: no Vk");
            return;
        };
        let root = yserver_core::resources::ROOT_WINDOW.0;
        rg_window(&mut f, w, root, 300, 300, 100, 100, 0, WHITE, 0, 1);
        rg_window(&mut f, c, w, 80, 80, 50, 50, 4, BLUE, RED, 1);
        if child_first {
            wz_map(&mut f, c);
            wz_map(&mut f, w);
        } else {
            wz_map(&mut f, w);
            wz_map(&mut f, c);
        }
        wz_configure(&mut f, w, 0x0C, &[200, 200]);
        rg_assert(
            &mut f,
            w,
            if child_first {
                "C child-first"
            } else {
                "C parent-first"
            },
            &[
                (81, 105, RED),
                (105, 81, RED),
                (135, 105, RED),
                (105, 135, RED),
                (90, 90, BLUE),
                (120, 120, BLUE),
            ],
        );
    }
}

/// CF: the same C under an UNREDIRECTED W inside a redirected frame F
/// (a reparenting WM); the ring lands in F's backing.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn a_childs_border_revealed_inside_a_redirected_frame_is_painted() {
    const F: u32 = 0x1D20;
    const W: u32 = 0x1D21;
    const C: u32 = 0x1D22;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    let root = yserver_core::resources::ROOT_WINDOW.0;
    rg_window(&mut f, F, root, 300, 500, 220, 220, 0, WHITE, 0, 1);
    rg_window(&mut f, W, F, 10, 10, 100, 100, 0, WHITE, 0, 1);
    rg_window(&mut f, C, W, 80, 80, 50, 50, 4, BLUE, RED, 1);
    wz_map(&mut f, C);
    wz_map(&mut f, W);
    wz_map(&mut f, F);
    wz_configure(&mut f, W, 0x0C, &[200, 200]);
    rg_assert(
        &mut f,
        F,
        "CF",
        &[
            (91, 115, RED),
            (115, 91, RED),
            (145, 115, RED),
            (115, 145, RED),
            (130, 130, BLUE),
        ],
    );
}

/// U: a sibling over C's top-left ring unmaps; Xorg paints the ring it
/// uncovers.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn a_border_uncovered_by_an_unmap_is_painted() {
    const W: u32 = 0x1D30;
    const C: u32 = 0x1D31;
    const S: u32 = 0x1D32;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    let root = yserver_core::resources::ROOT_WINDOW.0;
    rg_window(&mut f, W, root, 400, 300, 120, 120, 0, WHITE, 0, 1);
    rg_window(&mut f, C, W, 10, 10, 50, 50, 4, BLUE, RED, 1);
    rg_window(&mut f, S, W, 0, 0, 30, 30, 0, GREEN, 0, 1);
    wz_map(&mut f, C);
    wz_map(&mut f, S);
    wz_map(&mut f, W);
    rg_assert(&mut f, W, "U before", &[(11, 20, GREEN), (20, 11, GREEN)]);
    f.req(10, 0, &S.to_le_bytes()); // UnmapWindow
    rg_assert(
        &mut f,
        W,
        "U",
        &[
            (11, 20, RED),
            (20, 11, RED),
            (11, 11, RED),
            (25, 25, BLUE),
            (5, 5, WHITE),
        ],
    );
}

const CYAN: u32 = 0x0000_FFFF;

/// M: C mapped under an already-mapped higher sibling S in a shared
/// backing. Its ring is `borderClip`-clipped (`miComputeClips`): under S it
/// stays S's green, elsewhere it is red.
/// Mutations: drop the occluder subtract; clip to `target.content_bounds()`.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn a_ring_painted_under_a_higher_sibling_leaves_the_sibling() {
    const W: u32 = 0x1E10;
    const C: u32 = 0x1E11;
    const S: u32 = 0x1E12;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    let root = yserver_core::resources::ROOT_WINDOW.0;
    rg_window(&mut f, W, root, 600, 0, 120, 120, 0, WHITE, 0, 1);
    rg_window(&mut f, C, W, 10, 10, 50, 50, 4, BLUE, RED, 1);
    rg_window(&mut f, S, W, 0, 0, 30, 30, 0, GREEN, 0, 1);
    wz_map(&mut f, W);
    wz_map(&mut f, S);
    wz_map(&mut f, C);
    rg_assert(
        &mut f,
        W,
        "M",
        &[
            (11, 20, GREEN),
            (20, 11, GREEN),
            (25, 25, GREEN),
            (40, 40, BLUE),
            (11, 40, RED),
            (40, 11, RED),
        ],
    );
}

/// MB: a child's ring stops at its bordered parent's content
/// (`mi/mivaltree.c:386`). W 100x100 bw 3 red, C (80,20) 40x40 bw 4 cyan,
/// mapped after W.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn a_childs_ring_stops_at_its_bordered_parents_content() {
    const W: u32 = 0x1E20;
    const C: u32 = 0x1E21;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    let root = yserver_core::resources::ROOT_WINDOW.0;
    rg_window(&mut f, W, root, 700, 120, 100, 100, 3, WHITE, RED, 1);
    rg_window(&mut f, C, W, 80, 20, 40, 40, 4, BLUE, CYAN, 1);
    wz_map(&mut f, W);
    wz_map(&mut f, C);
    rg_assert(
        &mut f,
        W,
        "MB",
        &[
            (84, 40, CYAN),
            (90, 24, CYAN),
            (95, 40, BLUE),
            (104, 40, RED),
            (104, 24, RED),
            (104, 70, RED),
            (1, 1, RED),
        ],
    );
}

/// N: C, drawn into its redirected parent's backing, is redirected itself
/// (a tray icon). `compNewPixmap` seeds C's pixmap from the parent's
/// (composite/compalloc.c:532-560), which holds C's pixels; C's own leaf
/// is stale. Mutation: always overlay the leaves.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn nested_redirect_seeds_from_the_ancestor_backing() {
    const W: u32 = 0x1F10;
    const C: u32 = 0x1F11;
    const GC: u32 = 0x1F12;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    let root = yserver_core::resources::ROOT_WINDOW.0;
    rg_window(&mut f, W, root, 700, 300, 120, 120, 0, WHITE, 0, 1);
    rg_window(&mut f, C, W, 10, 10, 50, 50, 3, BLUE, RED, 1);
    wz_map(&mut f, C);
    wz_map(&mut f, W);
    or_create_gc(&mut f, GC, C, GREEN);
    or_fill(&mut f, C, GC, 0, 0, 20, 20);
    let mut body = Vec::new();
    body.extend_from_slice(&C.to_le_bytes());
    body.extend_from_slice(&[1u8, 0, 0, 0]); // CompositeRedirectManual
    f.req(144, 1, &body);
    rg_assert(
        &mut f,
        C,
        "N",
        &[
            (1, 1, RED),
            (3, 3, GREEN),
            (10, 10, GREEN),
            (30, 30, BLUE),
            (52, 52, BLUE),
        ],
    );
}

/// RP: fvwm3's resize. Two background-None covers are raised over frame F
/// (background None), F shrinks 400 -> 250 wide (the NorthEast button and
/// right border move left 150 under the covers), then the covers are
/// reparented out while mapped. Xorg's ReparentWindow unmaps them first
/// (`dix/window.c:2518-2519`), so the button and border are repainted
/// where the covers were. Mutation: skip the backend unmap on reparent.
#[test]
#[ignore = "needs live Vulkan ICD"]
fn a_cover_reparented_out_of_a_redirected_frame_uncovers_what_it_covered() {
    const PARK: u32 = 0x2010;
    const F: u32 = 0x2011;
    const T: u32 = 0x2012;
    const B: u32 = 0x2013;
    const R: u32 = 0x2014;
    const C: u32 = 0x2015;
    const HT: u32 = 0x2016;
    const HR: u32 = 0x2017;
    let Some(mut f) = rg_fixture() else {
        eprintln!("skipping: no Vk");
        return;
    };
    let root = yserver_core::resources::ROOT_WINDOW.0;
    let visual = yserver_core::resources::ROOT_VISUAL.0;
    // CWBackPixmap None | CWWinGravity NorthWest.
    let none = |f: &mut ProtoFixture, wid: u32, parent: u32, x: i16, y: i16, w: u16, h: u16| {
        or_create_window(f, wid, parent, 24, x, y, w, h, 0, visual, 0x21, &[0, 1]);
    };
    let reparent = |f: &mut ProtoFixture, wid: u32, parent: u32| {
        let mut body = Vec::new();
        body.extend_from_slice(&wid.to_le_bytes());
        body.extend_from_slice(&parent.to_le_bytes());
        body.extend_from_slice(&(-1i16).to_le_bytes());
        body.extend_from_slice(&(-1i16).to_le_bytes());
        f.req(7, 0, &body);
    };
    rg_window(&mut f, PARK, root, -10, -10, 10, 10, 0, WHITE, 0, 1);
    none(&mut f, F, root, 0, 720, 400, 100);
    rg_window(&mut f, T, F, 0, 0, 340, 20, 0, BLUE, 0, 1);
    rg_window(&mut f, B, F, 340, 0, 20, 20, 0, RED, 0, 3);
    rg_window(&mut f, R, F, 395, 20, 5, 80, 0, GREEN, 0, 3);
    rg_window(&mut f, C, F, 0, 20, 395, 80, 0, WHITE, 0, 1);
    f.req(9, 0, &F.to_le_bytes()); // MapSubwindows
    wz_map(&mut f, F);
    none(&mut f, HT, PARK, -1, -1, 1, 1);
    none(&mut f, HR, PARK, -1, -1, 1, 1);
    reparent(&mut f, HT, F);
    reparent(&mut f, HR, F);
    wz_configure(&mut f, HT, 0x40, &[0]); // Raise
    wz_configure(&mut f, HR, 0x40, &[0]);
    wz_configure(&mut f, HT, 0x0F, &[0, 0, 400, 20]);
    wz_map(&mut f, HT);
    wz_configure(&mut f, HR, 0x0F, &[245, 0, 155, 100]);
    wz_map(&mut f, HR);
    wz_configure(&mut f, T, 0x0C, &[190, 20]);
    wz_configure(&mut f, C, 0x0C, &[245, 80]);
    wz_configure(&mut f, F, 0x0C, &[250, 100]);
    reparent(&mut f, HT, PARK);
    reparent(&mut f, HR, PARK);
    rg_assert(
        &mut f,
        F,
        "RP",
        &[
            (100, 10, BLUE),
            (200, 10, RED),
            (247, 50, GREEN),
            (100, 50, WHITE),
        ],
    );
}
