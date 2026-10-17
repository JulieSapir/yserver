/* expose-order-probe — the order and extent of Expose, MapNotify and
 *                      DamageNotify where yserver diverged from Xorg.
 *
 *   ./expose-order-probe comp | nocomp
 *
 * comp: `obs` redirects root's subwindows Manual (picom-like); nocomp: never.
 * `obs` selects Exposure/SubstructureNotify on every window and holds a
 * NonEmpty (N, subtracted after each step) and a RawRectangles (R) Damage per
 * watched drawable, so one connection sees the total order. `app` issues the
 * requests. Each step: round trips, 200 ms quiet, one line.
 *
 *   1a MapSubwindows    MapNotify top-most first
 *   1b ReparentWindow   damage of a child and its new parent
 *   2  ParentRelative   a Manual RedirectSubwindows parent inhibits the paint
 *   3  overlay release  restore damage after UnredirectSubwindows
 *   4  ClearArea        exposures clipped to the clip list
 *
 * Root's background is set black first, so bg None windows read the same.
 * No ids or timings in the output: diff an Xorg run against a yserver run.
 *
 *   cc -O1 -o expose-order-probe expose-order-probe.c \
 *       -lxcb -lxcb-damage -lxcb-composite -lxcb-xfixes
 */
#include <poll.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <xcb/composite.h>
#include <xcb/damage.h>
#include <xcb/xcb.h>
#include <xcb/xfixes.h>

static xcb_connection_t *obs, *app;
static xcb_screen_t *scr;
static xcb_window_t root;
static uint8_t damage_ev;
static int comp;

#define MAXN 1024
static struct { uint32_t id; const char *name; } names[MAXN];
static int nnames;
static struct { xcb_damage_damage_t d; int raw; } dmgs[MAXN];
static int ndmgs;

static void reg(uint32_t id, const char *n) { names[nnames].id = id; names[nnames++].name = n; }
static const char *nm(uint32_t id)
{
    static char buf[4][16];
    static int k;
    for (int i = nnames - 1; i >= 0; i--)
        if (names[i].id == id)
            return names[i].name;
    if (id == 0)
        return "None";
    k = (k + 1) & 3;
    snprintf(buf[k], 16, "?");
    return buf[k];
}

static void rt(xcb_connection_t *c) { free(xcb_get_input_focus_reply(c, xcb_get_input_focus(c), NULL)); }

static void watch_damage(xcb_drawable_t w)
{
    for (int raw = 0; raw < 2; raw++) {
        xcb_damage_damage_t d = xcb_generate_id(obs);
        xcb_damage_create(obs, d, w,
                          raw ? XCB_DAMAGE_REPORT_LEVEL_RAW_RECTANGLES : XCB_DAMAGE_REPORT_LEVEL_NON_EMPTY);
        dmgs[ndmgs].d = d;
        dmgs[ndmgs++].raw = raw;
    }
}
static void drop_damage(void)
{
    for (int i = 0; i < ndmgs; i++)
        xcb_damage_destroy(obs, dmgs[i].d);
    ndmgs = 0;
    rt(obs);
}
static int is_raw(xcb_damage_damage_t d)
{
    for (int i = 0; i < ndmgs; i++)
        if (dmgs[i].d == d)
            return dmgs[i].raw;
    return -1;
}

static void sel(xcb_window_t w)
{
    uint32_t m = XCB_EVENT_MASK_EXPOSURE | XCB_EVENT_MASK_SUBSTRUCTURE_NOTIFY;
    xcb_change_window_attributes(obs, w, XCB_CW_EVENT_MASK, &m);
}

static void drain(void)
{
    struct pollfd p = {xcb_get_file_descriptor(obs), POLLIN, 0};
    for (;;) {
        xcb_generic_event_t *ev = xcb_poll_for_event(obs);
        if (!ev) {
            xcb_flush(obs);
            if (poll(&p, 1, 200) <= 0)
                return;
            ev = xcb_poll_for_event(obs);
            if (!ev)
                return;
        }
        uint8_t t = ev->response_type & 0x7f;
        if (t == 0) {
            xcb_generic_error_t *e = (void *)ev;
            printf(" Error(%u op%u.%u)", e->error_code, e->major_code, e->minor_code);
        } else if (t == XCB_EXPOSE) {
            xcb_expose_event_t *e = (void *)ev;
            printf(" Expose(%s %u,%u %ux%u c%u)", nm(e->window), e->x, e->y, e->width, e->height, e->count);
        } else if (t == XCB_MAP_NOTIFY) {
            xcb_map_notify_event_t *e = (void *)ev;
            printf(" Map(%s@%s)", nm(e->window), nm(e->event));
        } else if (t == XCB_UNMAP_NOTIFY) {
            xcb_unmap_notify_event_t *e = (void *)ev;
            printf(" Unmap(%s@%s)", nm(e->window), nm(e->event));
        } else if (t == XCB_REPARENT_NOTIFY) {
            xcb_reparent_notify_event_t *e = (void *)ev;
            printf(" Reparent(%s@%s ->%s %d,%d)", nm(e->window), nm(e->event), nm(e->parent), e->x, e->y);
        } else if (t == XCB_CONFIGURE_NOTIFY) {
            xcb_configure_notify_event_t *e = (void *)ev;
            printf(" Configure(%s@%s %d,%d %ux%u)", nm(e->window), nm(e->event), e->x, e->y, e->width,
                   e->height);
        } else if (t == XCB_CREATE_NOTIFY || t == XCB_DESTROY_NOTIFY) {
        } else if (t == damage_ev + XCB_DAMAGE_NOTIFY) {
            xcb_damage_notify_event_t *e = (void *)ev;
            int r = is_raw(e->damage);
            printf(" Dmg%s(%s %d,%d %ux%u)", r < 0 ? "?" : r ? "R" : "N", nm(e->drawable), e->area.x,
                   e->area.y, e->area.width, e->area.height);
        } else {
            printf(" event%u", t);
        }
        free(ev);
    }
}

static void step(const char *label)
{
    rt(app);
    rt(obs);
    printf("  %-16s", label);
    drain();
    printf("\n");
    for (int i = 0; i < ndmgs; i++)
        if (!dmgs[i].raw)
            xcb_damage_subtract(obs, dmgs[i].d, XCB_NONE, XCB_NONE);
    rt(obs);
    fflush(stdout);
}

static xcb_window_t mk(const char *name, xcb_window_t parent, int x, int y, int w, int h, int bg)
{
    /* bg: -1 None, -2 ParentRelative, else pixel */
    xcb_window_t id = xcb_generate_id(app);
    uint32_t mask = XCB_CW_OVERRIDE_REDIRECT, v[2];
    int n = 0;
    if (bg >= 0) {
        mask |= XCB_CW_BACK_PIXEL;
        v[n++] = (uint32_t)bg;
    } else {
        mask |= XCB_CW_BACK_PIXMAP;
        v[n++] = bg == -1 ? XCB_BACK_PIXMAP_NONE : XCB_BACK_PIXMAP_PARENT_RELATIVE;
    }
    v[n++] = 1;
    xcb_create_window(app, XCB_COPY_FROM_PARENT, id, parent, x, y, w, h, 0, XCB_WINDOW_CLASS_INPUT_OUTPUT,
                      XCB_COPY_FROM_PARENT, mask, v);
    rt(app);
    reg(id, name);
    sel(id);
    return id;
}

static const char *bgname(int bg) { return bg == -1 ? "none" : bg == -2 ? "prel" : "pixel"; }

static void redirect_root(int on)
{
    xcb_generic_error_t *e = xcb_request_check(
        obs, on ? xcb_composite_redirect_subwindows_checked(obs, root, XCB_COMPOSITE_REDIRECT_MANUAL)
                : xcb_composite_unredirect_subwindows_checked(obs, root, XCB_COMPOSITE_REDIRECT_MANUAL));
    if (e) {
        printf("%sredirect root: error %u\n", on ? "" : "un", e->error_code);
        exit(1);
    }
}

static void destroy(xcb_window_t w)
{
    drop_damage();
    xcb_destroy_window(app, w);
    rt(app);
    rt(obs);
    printf("  %-16s", "destroy");
    drain();
    printf("\n");
}

/* 1a: top-level T with three unmapped children, then MapSubwindows(T). */
static void case1a(int tbg, int cbg)
{
    printf("1a T=%s children=%s\n", bgname(tbg), bgname(cbg));
    xcb_window_t T = mk("T", root, 50, 50, 300, 200, tbg);
    mk("A", T, 10, 10, 80, 80, cbg);
    mk("B", T, 60, 60, 80, 80, cbg); /* overlaps A */
    mk("C", T, 190, 10, 80, 80, cbg);
    watch_damage(T);
    step("create");
    xcb_map_window(app, T);
    step("map T");
    xcb_map_subwindows(app, T);
    step("MapSubwindows");
    destroy(T);
}

/* 1b: R reparented into mapped P, then out to root. */
static void case1b(int bg)
{
    printf("1b bg=%s\n", bgname(bg));
    xcb_window_t P = mk("P", root, 50, 50, 300, 200, bg);
    xcb_window_t R = mk("R", root, 400, 60, 120, 100, bg);
    xcb_window_t K = mk("Rk", R, 10, 10, 50, 40, bg);
    xcb_map_window(app, K);
    watch_damage(P);
    watch_damage(R);
    step("create");
    xcb_map_window(app, P);
    xcb_map_window(app, R);
    step("map P,R");
    xcb_reparent_window(app, R, P, 20, 30);
    step("reparent into P");
    xcb_reparent_window(app, R, root, 400, 60);
    step("reparent to root");
    xcb_destroy_window(app, R);
    destroy(P);
}

static void summarize(const char *what, xcb_drawable_t d, int w, int h)
{
    xcb_get_image_cookie_t ck = xcb_get_image(app, XCB_IMAGE_FORMAT_Z_PIXMAP, d, 0, 0, w, h, ~0u);
    xcb_generic_error_t *e = NULL;
    xcb_get_image_reply_t *r = xcb_get_image_reply(app, ck, &e);
    if (!r) {
        printf("  %-16s error %u\n", what, e ? e->error_code : 0);
        free(e);
        return;
    }
    uint32_t *px = (uint32_t *)xcb_get_image_data(r);
    int len = xcb_get_image_data_length(r) / 4;
    uint32_t col[8];
    int cnt[8], nc = 0, other = 0;
    for (int i = 0; i < len; i++) {
        uint32_t c = px[i] & 0xffffff;
        int j;
        for (j = 0; j < nc; j++)
            if (col[j] == c)
                break;
        if (j < nc)
            cnt[j]++;
        else if (nc < 8) {
            col[nc] = c;
            cnt[nc++] = 1;
        } else
            other++;
    }
    printf("  %-16s", what);
    for (int j = 0; j < nc; j++)
        printf(" %06x x%d", col[j], cnt[j]);
    if (other)
        printf(" other x%d", other);
    printf("\n");
    free(r);
}

/* 2: ParentRelative K (and grandchild G) under P whose subwindows are redirected Manual. */
static void case2(int pbg, int map_parent_last, int kbg)
{
    printf("2 P=%s K=%s %s\n", bgname(pbg), bgname(kbg), map_parent_last ? "map-P-last" : "map-K-last");
    xcb_window_t P = mk("P", root, 50, 50, 300, 200, pbg);
    xcb_window_t K = mk("K", P, 20, 20, 120, 100, kbg);
    xcb_window_t G = mk("G", K, 10, 10, 50, 40, -2);
    xcb_map_window(app, G);
    if (comp)
        xcb_composite_redirect_subwindows(obs, P, XCB_COMPOSITE_REDIRECT_MANUAL);
    rt(obs);
    watch_damage(P);
    watch_damage(K);
    step("create");
    if (map_parent_last) {
        xcb_map_window(app, K);
        step("map K (P unmapped)");
        xcb_map_window(app, P);
        step("map P");
    } else {
        xcb_map_window(app, P);
        step("map P");
        xcb_map_window(app, K);
        step("map K");
    }
    summarize("GetImage P", P, 300, 200);
    summarize("GetImage K", K, 120, 100);
    if (comp) {
        xcb_pixmap_t pm = xcb_generate_id(obs);
        xcb_composite_name_window_pixmap(obs, K, pm);
        rt(obs);
        summarize("GetImage K-pixmap", pm, 120, 100);
        xcb_free_pixmap(obs, pm);
    }
    destroy(P);
}

/* 3: windows mapped under the COW, then ReleaseOverlayWindow. variant: 0 keep
 * redirect, 1 unredirect then release, 2 release then unredirect. */
static void case3(int bg, int variant)
{
    static const char *vn[] = {"redirect-kept", "unredirect-then-release", "release-then-unredirect"};
    printf("3 bg=%s %s\n", bgname(bg), comp ? vn[variant] : "no-redirect");
    sel(root);
    xcb_composite_get_overlay_window_reply_t *ow =
        xcb_composite_get_overlay_window_reply(obs, xcb_composite_get_overlay_window(obs, root), NULL);
    xcb_window_t cow = ow ? ow->overlay_win : 0;
    free(ow);
    reg(cow, "COW");
    sel(cow);
    step("GetOverlay");
    xcb_window_t W1 = mk("W1", root, 50, 50, 200, 150, bg);
    xcb_window_t W2 = mk("W2", root, 150, 100, 200, 150, bg);
    xcb_window_t W1c = mk("W1c", W1, 10, 10, 60, 40, bg);
    xcb_map_window(app, W1c);
    watch_damage(W1);
    watch_damage(W2);
    step("create");
    xcb_map_window(app, W1);
    xcb_map_window(app, W2);
    step("map W1,W2");
    if (comp && variant == 1) {
        redirect_root(0);
        step("unredirect");
    }
    xcb_composite_release_overlay_window(obs, root);
    step("ReleaseOverlay");
    if (comp && variant == 2) {
        redirect_root(0);
        step("unredirect");
    }
    xcb_destroy_window(app, W2);
    destroy(W1);
    if (comp && variant != 0) {
        redirect_root(1);
        step("re-redirect");
    }
    uint32_t m = 0;
    xcb_change_window_attributes(obs, root, XCB_CW_EVENT_MASK, &m);
    m = XCB_EVENT_MASK_SUBSTRUCTURE_NOTIFY;
    xcb_change_window_attributes(obs, root, XCB_CW_EVENT_MASK, &m);
    rt(obs);
}

/* 4: ClearArea(exposures) on V whose visible region is L-shaped (S covers its
 * bottom-right), both children of top-level T. */
static void case4(int vbg)
{
    printf("4 V=%s\n", bgname(vbg));
    xcb_window_t T = mk("T", root, 50, 50, 400, 300, 0x404040);
    xcb_window_t V = mk("V", T, 20, 20, 200, 150, vbg);
    mk("S", T, 120, 95, 150, 100, 0x00ff00); /* covers V's (100,75)-(200,150) */
    xcb_map_subwindows(app, T);
    xcb_map_window(app, T);
    watch_damage(T);
    step("create+map");
    static const struct { const char *l; int16_t x, y; uint16_t w, h; } rs[] = {
        {"clear all", 0, 0, 0, 0},
        {"clear 0,0 60x50", 0, 0, 60, 50},
        {"clear 40,30 120x80", 40, 30, 120, 80},
        {"clear 120,90 50x40", 120, 90, 50, 40},
        {"clear 150,0 50x150", 150, 0, 50, 150},
    };
    for (unsigned i = 0; i < sizeof rs / sizeof *rs; i++) {
        xcb_clear_area(app, 1, V, rs[i].x, rs[i].y, rs[i].w, rs[i].h);
        step(rs[i].l);
    }
    destroy(T);
}

int main(int argc, char **argv)
{
    comp = argc > 1 && !strcmp(argv[1], "comp");
    obs = xcb_connect(NULL, NULL);
    app = xcb_connect(NULL, NULL);
    if (xcb_connection_has_error(obs) || xcb_connection_has_error(app))
        return 1;
    scr = xcb_setup_roots_iterator(xcb_get_setup(obs)).data;
    root = scr->root;
    reg(root, "root");
    const xcb_query_extension_reply_t *d = xcb_get_extension_data(obs, &xcb_damage_id);
    if (!d || !d->present)
        return 1;
    damage_ev = d->first_event;
    free(xcb_damage_query_version_reply(obs, xcb_damage_query_version(obs, 1, 1), NULL));
    free(xcb_xfixes_query_version_reply(obs, xcb_xfixes_query_version(obs, 5, 0), NULL));
    free(xcb_composite_query_version_reply(obs, xcb_composite_query_version(obs, 0, 4), NULL));
    uint32_t black = 0;
    xcb_change_window_attributes(app, root, XCB_CW_BACK_PIXEL, &black);
    xcb_clear_area(app, 0, root, 0, 0, 0, 0);
    rt(app);
    if (comp)
        redirect_root(1);
    uint32_t sub = XCB_EVENT_MASK_SUBSTRUCTURE_NOTIFY;
    xcb_change_window_attributes(obs, root, XCB_CW_EVENT_MASK, &sub);
    rt(obs);
    printf("mode %s\n", comp ? "comp" : "nocomp");

    int bgs[] = {-1, 0x808080};
    for (int i = 0; i < 2; i++)
        for (int j = 0; j < 2; j++)
            case1a(bgs[i], bgs[j]);
    for (int i = 0; i < 2; i++)
        case1b(bgs[i]);
    for (int i = 0; i < 2; i++)
        for (int j = 0; j < 2; j++)
            for (int k = 0; k < 2; k++)
                case2(bgs[i], j, k ? 0x2050a0 : -2);
    for (int i = 0; i < 2; i++)
        for (int v = 0; v < (comp ? 3 : 1); v++)
            case3(bgs[i], v);
    for (int i = 0; i < 2; i++)
        case4(bgs[i]);
    printf("done\n");
    xcb_disconnect(app);
    xcb_disconnect(obs);
    return 0;
}
