/* GH #277: where is the ACTIVATING event of a passive grab reported?
 *
 * Client A owns top-level T (100,100 400x300) with child B (the "bar",
 * 0,250 400x50 inside T), plus top-level W2 (600,100 100x100). A grabs
 * Mod4+"2" on the root. Driver D sends Super, "2", "3", release "3",
 * release "2", release Super via XTEST. Each event A (and C, owner of B
 * in one variant) receives is logged with event/child/coords.
 *
 *   cc -O1 -o probe key-grab-activation-probe.c -lxcb -lxcb-xtest -lxcb-xinput
 */
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <unistd.h>
#include <xcb/xcb.h>
#include <xcb/xinput.h>
#include <xcb/xtest.h>

static xcb_connection_t *a, *c, *d;
static xcb_screen_t *s;
static xcb_window_t T, B, W2;
static uint8_t kc_super, kc_2, kc_3, xi_op;

static xcb_connection_t *connect_retry(void)
{
    xcb_connection_t *x;
    for (int t = 0; (x = xcb_connect(NULL, NULL)) && xcb_connection_has_error(x) && t < 50; t++) {
        xcb_disconnect(x);
        usleep(100000);
    }
    return x;
}

static void sync_server(xcb_connection_t *x)
{
    free(xcb_get_input_focus_reply(x, xcb_get_input_focus(x), NULL));
}

static const char *wname(xcb_window_t w)
{
    static char buf[4][16];
    static int i;
    if (w == XCB_NONE) return "None";
    if (w == s->root) return "root";
    if (w == T) return "T";
    if (w == B) return "B";
    if (w == W2) return "W2";
    i = (i + 1) & 3;
    snprintf(buf[i], sizeof buf[i], "0x%x", w);
    return buf[i];
}

static const char *kname(uint8_t k)
{
    if (k == kc_super) return "Super";
    if (k == kc_2) return "2";
    if (k == kc_3) return "3";
    return "?";
}

static void drain(xcb_connection_t *x, const char *who)
{
    sync_server(x);
    xcb_generic_event_t *e;
    while ((e = xcb_poll_for_event(x))) {
        uint8_t t = e->response_type & 0x7f;
        if (t == XCB_KEY_PRESS || t == XCB_KEY_RELEASE || t == XCB_BUTTON_PRESS ||
            t == XCB_BUTTON_RELEASE) {
            xcb_key_press_event_t *k = (xcb_key_press_event_t *)e;
            const char *n = t == XCB_KEY_PRESS ? "KeyPress" : t == XCB_KEY_RELEASE ? "KeyRelease"
                          : t == XCB_BUTTON_PRESS ? "ButtonPress" : "ButtonRelease";
            printf("  %s %s(%s) event=%s child=%s ev=%d,%d root=%d,%d state=0x%x same=%u\n", who,
                   n, t <= XCB_KEY_RELEASE ? kname(k->detail) : "btn", wname(k->event),
                   wname(k->child), k->event_x, k->event_y, k->root_x, k->root_y, k->state,
                   k->same_screen);
        } else if (t == XCB_GE_GENERIC) {
            xcb_ge_generic_event_t *g = (xcb_ge_generic_event_t *)e;
            if (g->extension == xi_op &&
                (g->event_type == XCB_INPUT_KEY_PRESS || g->event_type == XCB_INPUT_KEY_RELEASE)) {
                xcb_input_key_press_event_t *k = (xcb_input_key_press_event_t *)e;
                printf("  %s XI2 %s(%s) dev=%u src=%u event=%s child=%s ev=%d,%d\n", who,
                       g->event_type == XCB_INPUT_KEY_PRESS ? "KeyPress" : "KeyRelease",
                       kname((uint8_t)k->detail), k->deviceid, k->sourceid, wname(k->event),
                       wname(k->child), k->event_x >> 16, k->event_y >> 16);
            }
        } else if (t == 0) {
            xcb_generic_error_t *er = (xcb_generic_error_t *)e;
            printf("  %s error %u (major %u minor %u)\n", who, er->error_code, er->major_code,
                   er->minor_code);
        }
        /* Focus, crossing and MappingNotify are not what this probe is about. */
        free(e);
    }
}

static void fake(uint8_t type, uint8_t detail)
{
    xcb_test_fake_input(d, type, detail, XCB_CURRENT_TIME, XCB_NONE, 0, 0, 0);
    sync_server(d);
    usleep(50000);
}

static void warp(int x, int y)
{
    xcb_test_fake_input(d, XCB_MOTION_NOTIFY, 0, XCB_CURRENT_TIME, s->root, x, y, 0);
    sync_server(d);
    usleep(100000);
}

static xcb_window_t mkwin(xcb_connection_t *x, xcb_window_t parent, int px, int py, int w, int h,
                          uint32_t mask)
{
    xcb_window_t id = xcb_generate_id(x);
    uint32_t vals[2] = {s->white_pixel, mask};
    xcb_create_window(x, XCB_COPY_FROM_PARENT, id, parent, px, py, w, h, 0,
                      XCB_WINDOW_CLASS_INPUT_OUTPUT, s->root_visual,
                      XCB_CW_BACK_PIXEL | XCB_CW_EVENT_MASK, vals);
    xcb_map_window(x, id);
    return id;
}

static uint8_t keycode_for(xcb_keysym_t sym)
{
    const xcb_setup_t *st = xcb_get_setup(d);
    int n = st->max_keycode - st->min_keycode + 1;
    xcb_get_keyboard_mapping_reply_t *r = xcb_get_keyboard_mapping_reply(
        d, xcb_get_keyboard_mapping(d, st->min_keycode, n), NULL);
    xcb_keysym_t *syms = xcb_get_keyboard_mapping_keysyms(r);
    uint8_t found = 0;
    for (int k = 0; k < n && !found; k++)
        for (int j = 0; j < r->keysyms_per_keycode; j++)
            if (syms[k * r->keysyms_per_keycode + j] == sym) {
                found = st->min_keycode + k;
                break;
            }
    free(r);
    return found;
}

#define KEYS (XCB_EVENT_MASK_KEY_PRESS | XCB_EVENT_MASK_KEY_RELEASE)

enum grabkind { CORE_KEY, XI2_KEY };

static void variant(const char *name, enum grabkind gk, int owner_events, int focus_mode,
                    int px, int py, int c_owns_b)
{
    printf("== %s\n", name);
    /* Root key selection for A (core variants only). */
    uint32_t rootmask = gk == CORE_KEY ? KEYS : 0;
    xcb_change_window_attributes(a, s->root, XCB_CW_EVENT_MASK, &rootmask);
    T = mkwin(a, s->root, 100, 100, 400, 300, gk == CORE_KEY ? KEYS : 0);
    sync_server(a);
    B = mkwin(c_owns_b ? c : a, T, 0, 250, 400, 50, gk == CORE_KEY ? KEYS : 0);
    W2 = mkwin(a, s->root, 600, 100, 100, 100, gk == CORE_KEY ? KEYS : 0);
    if (c_owns_b) sync_server(c);
    if (gk == XI2_KEY) {
        struct { xcb_input_event_mask_t h; uint32_t m; } em = {
            {XCB_INPUT_DEVICE_ALL_MASTER, 1},
            XCB_INPUT_XI_EVENT_MASK_KEY_PRESS | XCB_INPUT_XI_EVENT_MASK_KEY_RELEASE};
        xcb_window_t ws[4] = {s->root, T, W2, B};
        for (int i = 0; i < (c_owns_b ? 3 : 4); i++)
            xcb_input_xi_select_events(a, ws[i], 1, &em.h);
    }
    xcb_window_t fw = focus_mode == 0 ? XCB_INPUT_FOCUS_POINTER_ROOT : focus_mode == 1 ? T : W2;
    xcb_set_input_focus(a, XCB_INPUT_FOCUS_POINTER_ROOT, fw, XCB_CURRENT_TIME);
    if (gk == CORE_KEY) {
        xcb_grab_key(a, owner_events, s->root, XCB_MOD_MASK_4, kc_2, XCB_GRAB_MODE_ASYNC,
                     XCB_GRAB_MODE_ASYNC);
    } else {
        uint32_t mask = XCB_INPUT_XI_EVENT_MASK_KEY_PRESS | XCB_INPUT_XI_EVENT_MASK_KEY_RELEASE;
        uint32_t mods = XCB_MOD_MASK_4;
        free(xcb_input_xi_passive_grab_device_reply(
            a,
            xcb_input_xi_passive_grab_device(a, XCB_CURRENT_TIME, s->root, XCB_CURSOR_NONE, kc_2,
                                             XCB_INPUT_DEVICE_ALL_MASTER, 1, 1,
                                             XCB_INPUT_GRAB_TYPE_KEYCODE, XCB_INPUT_GRAB_MODE_22_ASYNC,
                                             XCB_INPUT_GRAB_MODE_22_ASYNC, owner_events, &mask, &mods),
            NULL));
    }
    sync_server(a);
    warp(px, py);
    drain(a, "A");
    if (c_owns_b) drain(c, "C");

    const struct { uint8_t type, *key; const char *label; } steps[] = {
        {XCB_KEY_PRESS, &kc_super, "press Super"},
        {XCB_KEY_PRESS, &kc_2, "press 2 (activates)"},
        {XCB_KEY_PRESS, &kc_3, "press 3 (grab active)"},
        {XCB_KEY_RELEASE, &kc_3, "release 3"},
        {XCB_KEY_RELEASE, &kc_2, "release 2 (deactivates)"},
        {XCB_KEY_RELEASE, &kc_super, "release Super"},
    };
    for (size_t i = 0; i < sizeof steps / sizeof steps[0]; i++) {
        fake(steps[i].type, *steps[i].key);
        printf(" %s:\n", steps[i].label);
        drain(a, "A");
        if (c_owns_b) drain(c, "C");
    }

    if (gk == CORE_KEY)
        xcb_ungrab_key(a, kc_2, s->root, XCB_MOD_MASK_4);
    else {
        uint32_t mods = XCB_MOD_MASK_4;
        xcb_input_xi_passive_ungrab_device(a, s->root, kc_2, XCB_INPUT_DEVICE_ALL_MASTER, 1,
                                           XCB_INPUT_GRAB_TYPE_KEYCODE, &mods);
    }
    xcb_destroy_window(a, W2);
    if (c_owns_b) { xcb_destroy_window(c, B); sync_server(c); drain(c, "C"); }
    xcb_destroy_window(a, T);
    uint32_t none = 0;
    xcb_change_window_attributes(a, s->root, XCB_CW_EVENT_MASK, &none);
    sync_server(a);
    usleep(100000);
    drain(a, "A");
    if (c_owns_b) drain(c, "C");
}

int main(void)
{
    a = connect_retry(); c = connect_retry(); d = connect_retry();
    if (xcb_connection_has_error(a) || xcb_connection_has_error(c) || xcb_connection_has_error(d)) {
        printf("cannot connect\n");
        return 1;
    }
    s = xcb_setup_roots_iterator(xcb_get_setup(a)).data;
    free(xcb_test_get_version_reply(d, xcb_test_get_version(d, 2, 2), NULL));
    const xcb_query_extension_reply_t *xi = xcb_get_extension_data(a, &xcb_input_id);
    xi_op = xi ? xi->major_opcode : 0;
    free(xcb_input_xi_query_version_reply(a, xcb_input_xi_query_version(a, 2, 2), NULL));

    kc_super = keycode_for(0xffeb /* Super_L */);
    kc_2 = keycode_for(0x32);
    kc_3 = keycode_for(0x33);
    /* Mod4 must contain Super_L, or the grab never matches. */
    xcb_get_modifier_mapping_reply_t *mm =
        xcb_get_modifier_mapping_reply(d, xcb_get_modifier_mapping(d), NULL);
    xcb_keycode_t *mk = xcb_get_modifier_mapping_keycodes(mm);
    int super_in_mod4 = 0;
    for (int j = 0; j < mm->keycodes_per_modifier; j++)
        if (mk[3 * mm->keycodes_per_modifier + j] == kc_super) super_in_mod4 = 1;
    free(mm);
    printf("keycodes: Super=%u 2=%u 3=%u super_in_mod4=%d\n", kc_super, kc_2, kc_3, super_in_mod4);

    /* Pointer over B = root (300,375); over root background = (50,50). */
    variant("V1 core PointerRoot owner_events=1 pointer-over-B", CORE_KEY, 1, 0, 300, 375, 0);
    variant("V2 core PointerRoot owner_events=0 pointer-over-B", CORE_KEY, 0, 0, 300, 375, 0);
    variant("V3 core focus=T owner_events=1 pointer-over-B", CORE_KEY, 1, 1, 300, 375, 0);
    variant("V4 core focus=W2 owner_events=1 pointer-over-B", CORE_KEY, 1, 2, 300, 375, 0);
    variant("V5 core PointerRoot owner_events=1 pointer-over-root", CORE_KEY, 1, 0, 50, 50, 0);
    variant("V6 core PointerRoot owner_events=1 pointer-over-B B-owned-by-C", CORE_KEY, 1, 0, 300,
            375, 1);

    /* Button variant: GrabButton on T owner_events=1; A selects ButtonPress
     * on root and B but NOT on T. Pointer over B. */
    printf("== V9 core button grab on T owner_events=1, A selects root+B not T\n");
    uint32_t bm = XCB_EVENT_MASK_BUTTON_PRESS | XCB_EVENT_MASK_BUTTON_RELEASE;
    xcb_change_window_attributes(a, s->root, XCB_CW_EVENT_MASK, &bm);
    T = mkwin(a, s->root, 100, 100, 400, 300, 0);
    B = mkwin(a, T, 0, 250, 400, 50, bm);
    W2 = 0;
    xcb_grab_button(a, 1, T, bm, XCB_GRAB_MODE_ASYNC, XCB_GRAB_MODE_ASYNC, XCB_NONE, XCB_NONE, 1,
                    XCB_MOD_MASK_ANY);
    sync_server(a);
    warp(300, 375);
    drain(a, "A");
    fake(XCB_BUTTON_PRESS, 1);
    printf(" press 1 (activates):\n");
    drain(a, "A");
    fake(XCB_BUTTON_RELEASE, 1);
    printf(" release 1:\n");
    drain(a, "A");
    printf(" variant 9b: same, grab on T with owner_events=0:\n");
    xcb_ungrab_button(a, 1, T, XCB_MOD_MASK_ANY);
    xcb_grab_button(a, 0, T, bm, XCB_GRAB_MODE_ASYNC, XCB_GRAB_MODE_ASYNC, XCB_NONE, XCB_NONE, 1,
                    XCB_MOD_MASK_ANY);
    sync_server(a);
    fake(XCB_BUTTON_PRESS, 1);
    printf(" press 1 (activates):\n");
    drain(a, "A");
    fake(XCB_BUTTON_RELEASE, 1);
    printf(" release 1:\n");
    drain(a, "A");

    printf("done\n");
    xcb_disconnect(d);
    xcb_disconnect(c);
    xcb_disconnect(a);
    return 0;
}
