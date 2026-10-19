/* redirected-backing-probe — what a Manually redirected window's backing pixmap
 * holds after client draws, resizes and tree changes. Diff the same binary under
 * Xorg and yserver (tools/vng-scenarios/redirected-backing.sh, golden=live).
 *
 * Every top-level is override-redirect, RedirectSubwindows(root, Manual) is set
 * and no WM runs. Pixels are read through NameWindowPixmap + GetImage on the
 * pixmap. Colours: red = border, blue = content/draw, white = background,
 * green = sibling/child. One `key=value` fact per line, no XIDs.
 *
 * Cases (plan docs/superpowers/plans/2026-10-18-redirected-backing-geometry.md):
 *   D   client draws/reads on a bordered redirected window, one sub-case per op
 *   C   child crossing the grown edge (both map orders), CF the same under an
 *       unredirected window in a redirected frame
 *   G   SouthEast-gravity child moved into the grown area, GS on shrink
 *   U   an occluding sibling unmaps over a child's border
 *   M   a child mapped under a higher sibling, R then raised
 *   MB  a child's border crossing its bordered redirected parent's edge
 *   N   nested redirect of a child already drawn into its parent's backing
 *   RP  a background-None cover reparented out of a redirected frame after
 *       the frame shrank (fvwm3's resize): what it covered is repainted
 *   UG  an UnmapGravity child on a parent shrink and grow: unmapped, with
 *       UnmapNotify, and what it covered repainted
 *
 * Damage coverage (union, banded) goes to stdout; raw rectangles go to
 * damage-raw.log only: Xorg reports extents ∩ clip per op, yserver a region.
 *
 * Build: cc -O1 -o probe redirected-backing-probe.c -lX11 -lXext -lXcomposite -lXdamage
 */
#include <X11/Xlib.h>
#include <X11/Xutil.h>
#include <X11/extensions/XShm.h>
#include <X11/extensions/Xcomposite.h>
#include <X11/extensions/Xdamage.h>
#include <stdio.h>
#include <stdlib.h>
#include <string.h>
#include <sys/ipc.h>
#include <sys/shm.h>
#include <unistd.h>

#define RED 0xFF0000UL
#define BLUE 0x0000FFUL
#define WHITE 0xFFFFFFUL
#define GREEN 0x00FF00UL
#define CYAN 0x00FFFFUL

static Display *d;
static Window root;
static int dmg_ev;
static FILE *raw;

/* Damage coverage grid, in the damaged drawable's coordinates. */
#define GW 260
#define GH 260
static unsigned char grid[GH][GW];

static int on_error(Display *dpy, XErrorEvent *e) {
    (void)dpy;
    printf("x-error code=%d major=%d minor=%d\n", e->error_code, e->request_code, e->minor_code);
    return 0;
}

static Window mk(Window parent, int x, int y, int w, int h, int bw, unsigned long bg, unsigned long border,
                 int grav) {
    XSetWindowAttributes a;
    a.background_pixel = bg;
    a.border_pixel = border;
    a.override_redirect = True;
    a.win_gravity = grav;
    return XCreateWindow(d, parent, x, y, w, h, bw, 24, InputOutput, CopyFromParent,
                         CWBackPixel | CWBorderPixel | CWOverrideRedirect | CWWinGravity, &a);
}

static void settle(void) {
    XSync(d, False);
    usleep(300000);
    XSync(d, False);
}

static const char *child_name(Window w, Window const *wins, const char *const *names, int n) {
    for (int i = 0; i < n; i++)
        if (wins[i] == w) return names[i];
    return "?";
}

/* Drain events: Expose per named window; damage rasterised into `grid`. */
static void drain(const char *tag, Window const *wins, const char *const *names, int n) {
    while (XPending(d)) {
        XEvent e;
        XNextEvent(d, &e);
        if (e.type == dmg_ev + XDamageNotify) {
            XDamageNotifyEvent *de = (XDamageNotifyEvent *)&e;
            fprintf(raw, "%s damage %d,%d %dx%d\n", tag, de->area.x, de->area.y, de->area.width,
                    de->area.height);
            for (int y = de->area.y; y < de->area.y + de->area.height; y++)
                for (int x = de->area.x; x < de->area.x + de->area.width; x++)
                    if (x >= 0 && y >= 0 && x < GW && y < GH) grid[y][x] = 1;
            continue;
        }
        if (e.type == Expose)
            printf("%s.expose.%s=%d,%d %dx%d\n", tag, child_name(e.xexpose.window, wins, names, n), e.xexpose.x,
                   e.xexpose.y, e.xexpose.width, e.xexpose.height);
    }
}

/* Print the grid as canonical y-x bands, then clear it. */
static void coverage(const char *tag) {
    int printed = 0, y = 0;
    while (y < GH) {
        int y1 = y + 1;
        while (y1 < GH && memcmp(grid[y1], grid[y], GW) == 0) y1++;
        for (int x = 0; x < GW;) {
            if (!grid[y][x]) { x++; continue; }
            int x1 = x;
            while (x1 < GW && grid[y][x1]) x1++;
            printf("%s.coverage=%d,%d %dx%d\n", tag, x, y, x1 - x, y1 - y);
            printed++;
            x = x1;
        }
        y = y1;
    }
    if (!printed) printf("%s.coverage=none\n", tag);
    memset(grid, 0, sizeof grid);
}

static unsigned long pix_at(Drawable p, int x, int y) {
    XImage *im = XGetImage(d, p, x, y, 1, 1, AllPlanes, ZPixmap);
    unsigned long v = im ? XGetPixel(im, 0, 0) & 0xFFFFFF : 0xdeadUL;
    if (im) XDestroyImage(im);
    return v;
}

static void sample(const char *tag, Window w, const int (*pts)[2], int n) {
    Pixmap p = XCompositeNameWindowPixmap(d, w);
    XSync(d, False);
    for (int i = 0; i < n; i++)
        printf("%s.pix(%d,%d)=%06lx\n", tag, pts[i][0], pts[i][1], pix_at(p, pts[i][0], pts[i][1]));
    XFreePixmap(d, p);
}

/* D fixture: W 100x100 bw 4, red border, white bg, green child (50,50) 20x20. */
static Window d_window(int slot, Window *child) {
    Window w = mk(root, (slot % 8) * 120, (slot / 8) * 120, 100, 100, 4, WHITE, RED, NorthWestGravity);
    *child = mk(w, 50, 50, 20, 20, 0, GREEN, 0, NorthWestGravity);
    XMapWindow(d, *child);
    XMapWindow(d, w);
    settle();
    return w;
}

/* Compare W's backing, region (0,0) 24x24, with a reference pixmap the same
 * op was drawn into at the content offset (4,4): font differences cancel. */
static void d_compare(const char *tag, Window w, Pixmap ref) {
    Pixmap p = XCompositeNameWindowPixmap(d, w);
    XSync(d, False);
    XImage *a = XGetImage(d, p, 0, 0, 24, 24, AllPlanes, ZPixmap);
    XImage *b = XGetImage(d, ref, 0, 0, 24, 24, AllPlanes, ZPixmap);
    int diff = 0, ink = 0;
    if (a && b)
        for (int y = 0; y < 24; y++)
            for (int x = 0; x < 24; x++) {
                unsigned long pa = XGetPixel(a, x, y) & 0xFFFFFF, pb = XGetPixel(b, x, y) & 0xFFFFFF;
                diff += pa != pb;
                ink += pb == BLUE;
            }
    printf("%s.ink=%s\n", tag, ink ? "yes" : "no");
    printf("%s.mismatch_vs_reference=%d\n", tag, a && b ? diff : -1);
    if (a) XDestroyImage(a);
    if (b) XDestroyImage(b);
    XFreePixmap(d, p);
}

/* The backing as Xorg holds it before the op: red ring, white content. */
static Pixmap d_reference(void) {
    Pixmap ref = XCreatePixmap(d, root, 108, 108, 24);
    GC g = XCreateGC(d, ref, 0, NULL);
    XSetForeground(d, g, RED);
    XFillRectangle(d, ref, g, 0, 0, 108, 108);
    XSetForeground(d, g, WHITE);
    XFillRectangle(d, ref, g, 4, 4, 100, 100);
    XSetForeground(d, g, GREEN);
    XFillRectangle(d, ref, g, 54, 54, 20, 20);
    XFreeGC(d, g);
    return ref;
}

static const int d_points[][2] = {{1, 1}, {3, 3}, {4, 4}, {13, 13}, {14, 14}};

static void case_d(void) {
    int slot = 0;
    Window c;

    /* PolyFillRectangle, and the ClipByChildren fill over the child. */
    {
        Window w = d_window(slot++, &c);
        GC gc = XCreateGC(d, w, 0, NULL);
        XSetForeground(d, gc, BLUE);
        XFillRectangle(d, w, gc, 0, 0, 10, 10);
        XFillRectangle(d, w, gc, 40, 40, 40, 40);
        settle();
        const int p[][2] = {{1, 1}, {3, 3}, {4, 4}, {13, 13}, {14, 14}, {60, 60}, {48, 48}};
        sample("D.fill", w, p, 7);
        printf("D.getimage-win(0,0)=%06lx\n", pix_at(w, 0, 0));
        printf("D.getimage-win(9,9)=%06lx\n", pix_at(w, 9, 9));
        printf("D.getimage-win(10,10)=%06lx\n", pix_at(w, 10, 10));
        printf("D.getimage-win(-1,-1)=%06lx\n", pix_at(w, -1, -1));
        Pixmap dst = XCreatePixmap(d, root, 12, 12, 24);
        GC g2 = XCreateGC(d, dst, 0, NULL);
        XCopyArea(d, w, dst, g2, 0, 0, 12, 12, 0, 0);
        printf("D.copyarea-from-win(0,0)=%06lx\n", pix_at(dst, 0, 0));
        printf("D.copyarea-from-win(9,9)=%06lx\n", pix_at(dst, 9, 9));
        printf("D.copyarea-from-win(10,10)=%06lx\n", pix_at(dst, 10, 10));
        XFreeGC(d, g2);
        XFreePixmap(d, dst);

        /* ShmGetImage of the window. */
        XShmSegmentInfo si;
        XImage *im = XShmCreateImage(d, DefaultVisual(d, DefaultScreen(d)), 24, ZPixmap, NULL, &si, 12, 12);
        si.shmid = shmget(IPC_PRIVATE, im->bytes_per_line * im->height, IPC_CREAT | 0600);
        si.shmaddr = im->data = shmat(si.shmid, NULL, 0);
        si.readOnly = False;
        XShmAttach(d, &si);
        XShmGetImage(d, w, im, 0, 0, AllPlanes);
        printf("D.shmgetimage-win(0,0)=%06lx\n", XGetPixel(im, 0, 0) & 0xFFFFFF);
        printf("D.shmgetimage-win(9,9)=%06lx\n", XGetPixel(im, 9, 9) & 0xFFFFFF);
        printf("D.shmgetimage-win(10,10)=%06lx\n", XGetPixel(im, 10, 10) & 0xFFFFFF);
        XShmDetach(d, &si);
        XDestroyImage(im);
        shmdt(si.shmaddr);
        shmctl(si.shmid, IPC_RMID, NULL);
        XFreeGC(d, gc);
        XUnmapWindow(d, w);
    }

    /* PutImage: a blue 10x10 at (0,0). */
    {
        Window w = d_window(slot++, &c);
        GC gc = XCreateGC(d, w, 0, NULL);
        XImage *im = XCreateImage(d, DefaultVisual(d, DefaultScreen(d)), 24, ZPixmap, 0,
                                  calloc(10 * 10, 4), 10, 10, 32, 0);
        for (int y = 0; y < 10; y++)
            for (int x = 0; x < 10; x++) XPutPixel(im, x, y, BLUE);
        XPutImage(d, w, gc, im, 0, 0, 0, 0, 10, 10);
        settle();
        sample("D.putimage", w, d_points, 5);
        XDestroyImage(im);
        XFreeGC(d, gc);
        XUnmapWindow(d, w);
    }

    /* ShmPutImage. */
    {
        Window w = d_window(slot++, &c);
        GC gc = XCreateGC(d, w, 0, NULL);
        XShmSegmentInfo si;
        XImage *im = XShmCreateImage(d, DefaultVisual(d, DefaultScreen(d)), 24, ZPixmap, NULL, &si, 10, 10);
        si.shmid = shmget(IPC_PRIVATE, im->bytes_per_line * im->height, IPC_CREAT | 0600);
        si.shmaddr = im->data = shmat(si.shmid, NULL, 0);
        si.readOnly = False;
        XShmAttach(d, &si);
        for (int y = 0; y < 10; y++)
            for (int x = 0; x < 10; x++) XPutPixel(im, x, y, BLUE);
        XShmPutImage(d, w, gc, im, 0, 0, 0, 0, 10, 10, False);
        settle();
        sample("D.shmputimage", w, d_points, 5);
        XShmDetach(d, &si);
        XDestroyImage(im);
        shmdt(si.shmaddr);
        shmctl(si.shmid, IPC_RMID, NULL);
        XFreeGC(d, gc);
        XUnmapWindow(d, w);
    }

    /* CopyPlane: an all-ones 10x10 bitmap, foreground blue. */
    {
        Window w = d_window(slot++, &c);
        Pixmap bm = XCreatePixmap(d, root, 10, 10, 1);
        GC g1 = XCreateGC(d, bm, 0, NULL);
        XSetForeground(d, g1, 1);
        XFillRectangle(d, bm, g1, 0, 0, 10, 10);
        GC gc = XCreateGC(d, w, 0, NULL);
        XSetForeground(d, gc, BLUE);
        XSetBackground(d, gc, GREEN);
        XCopyPlane(d, bm, w, gc, 0, 0, 10, 10, 0, 0, 1);
        settle();
        sample("D.copyplane", w, d_points, 5);
        XFreeGC(d, g1);
        XFreeGC(d, gc);
        XFreePixmap(d, bm);
        XUnmapWindow(d, w);
    }

    /* ImageText8 and PolyText8 with "fixed", compared with a reference pixmap. */
    XFontStruct *fs = XLoadQueryFont(d, "fixed");
    if (!fs) {
        printf("D.text=no-font\n");
        return;
    }
    for (int image = 1; image >= 0; image--) {
        const char *tag = image ? "D.imagetext8" : "D.polytext8";
        Window w = d_window(slot++, &c);
        Pixmap ref = d_reference();
        GC gc = XCreateGC(d, w, 0, NULL);
        XSetFont(d, gc, fs->fid);
        XSetForeground(d, gc, BLUE);
        XSetBackground(d, gc, BLUE);
        int y = fs->ascent;
        if (image) {
            XDrawImageString(d, w, gc, 0, y, "MW", 2);
            XDrawImageString(d, ref, gc, 4, 4 + y, "MW", 2);
        } else {
            XDrawString(d, w, gc, 0, y, "MW", 2);
            XDrawString(d, ref, gc, 4, 4 + y, "MW", 2);
        }
        settle();
        d_compare(tag, w, ref);
        XFreeGC(d, gc);
        XFreePixmap(d, ref);
        XUnmapWindow(d, w);
    }
    XFreeFont(d, fs);
}

static void case_c(void) {
    for (int order = 0; order < 2; order++) {
        const char *tag = order ? "C-parent-first" : "C-child-first";
        Window w = mk(root, 300, 300, 100, 100, 0, WHITE, 0, NorthWestGravity);
        Window c = mk(w, 80, 80, 50, 50, 4, BLUE, RED, NorthWestGravity);
        Window wins[] = {w, c};
        const char *names[] = {"W", "C"};
        XSelectInput(d, c, ExposureMask);
        if (order) {
            XMapWindow(d, w);
            XMapWindow(d, c);
        } else {
            XMapWindow(d, c);
            XMapWindow(d, w);
        }
        settle();
        drain(tag, wins, names, 2);
        Damage dm = XDamageCreate(d, w, XDamageReportRawRectangles);
        settle();
        drain(tag, wins, names, 2);
        memset(grid, 0, sizeof grid);
        printf("%s.phase=resize\n", tag);
        XResizeWindow(d, w, 200, 200);
        settle();
        drain(tag, wins, names, 2);
        coverage(tag);
        XDamageDestroy(d, dm);
        const int p[][2] = {{81, 105}, {105, 81}, {135, 105}, {105, 135}, {90, 90}, {120, 120}, {132, 132}, {99, 99}};
        sample(tag, w, p, 8);
        XUnmapWindow(d, w);
        settle();
        drain(tag, wins, names, 2);
    }
}

static void case_g(void) {
    Window w = mk(root, 0, 300, 100, 100, 0, WHITE, 0, NorthWestGravity);
    XMapWindow(d, w);
    settle();
    Window c = mk(w, 60, 60, 30, 30, 3, BLUE, RED, SouthEastGravity);
    Window wins[] = {w, c};
    const char *names[] = {"W", "C"};
    XSelectInput(d, c, ExposureMask);
    XMapWindow(d, c);
    settle();
    drain("G", wins, names, 2);
    const int pb[][2] = {{61, 75}, {75, 75}};
    sample("G-before", w, pb, 2);
    printf("G.phase=resize\n");
    XResizeWindow(d, w, 200, 200);
    settle();
    drain("G", wins, names, 2);
    const int p[][2] = {{161, 178}, {194, 178}, {178, 161}, {178, 194}, {178, 178}, {75, 75}, {61, 75}};
    sample("G", w, p, 7);
    XUnmapWindow(d, w);
}

static void case_gs(void) {
    Window w = mk(root, 0, 500, 200, 200, 0, WHITE, 0, NorthWestGravity);
    Window c = mk(w, 160, 160, 30, 30, 3, BLUE, RED, SouthEastGravity);
    Window wins[] = {w, c};
    const char *names[] = {"W", "C"};
    XSelectInput(d, c, ExposureMask);
    XMapWindow(d, c);
    XMapWindow(d, w);
    settle();
    drain("GS", wins, names, 2);
    printf("GS.phase=resize\n");
    XResizeWindow(d, w, 100, 100);
    settle();
    drain("GS", wins, names, 2);
    const int p[][2] = {{61, 78}, {94, 78}, {78, 61}, {78, 94}, {78, 78}};
    sample("GS", w, p, 5);
    XUnmapWindow(d, w);
}

static void case_u(void) {
    Window w = mk(root, 400, 300, 120, 120, 0, WHITE, 0, NorthWestGravity);
    Window c = mk(w, 10, 10, 50, 50, 4, BLUE, RED, NorthWestGravity);
    Window s = mk(w, 0, 0, 30, 30, 0, GREEN, 0, NorthWestGravity);
    Window wins[] = {w, c, s};
    const char *names[] = {"W", "C", "S"};
    XSelectInput(d, c, ExposureMask);
    XMapWindow(d, c);
    XMapWindow(d, s);
    XMapWindow(d, w);
    settle();
    drain("U", wins, names, 3);
    const int pb[][2] = {{11, 20}, {20, 11}, {25, 25}};
    sample("U-before", w, pb, 3);
    Damage dm = XDamageCreate(d, w, XDamageReportRawRectangles);
    settle();
    drain("U", wins, names, 3);
    memset(grid, 0, sizeof grid);
    printf("U.phase=unmap\n");
    XUnmapWindow(d, s);
    settle();
    drain("U", wins, names, 3);
    coverage("U");
    XDamageDestroy(d, dm);
    const int p[][2] = {{11, 20}, {20, 11}, {11, 11}, {25, 25}, {5, 5}};
    sample("U", w, p, 5);
    XUnmapWindow(d, w);
}

static void case_m(void) {
    Window w = mk(root, 600, 0, 120, 120, 0, WHITE, 0, NorthWestGravity);
    Window c = mk(w, 10, 10, 50, 50, 4, BLUE, RED, NorthWestGravity);
    Window s = mk(w, 0, 0, 30, 30, 0, GREEN, 0, NorthWestGravity);
    XMapWindow(d, w);
    XMapWindow(d, s);
    settle();
    XMapWindow(d, c);
    settle();
    const int p[][2] = {{11, 20}, {20, 11}, {25, 25}, {40, 40}, {11, 40}, {40, 11}};
    sample("M", w, p, 6);
    XRaiseWindow(d, c);
    settle();
    const int r[][2] = {{11, 20}, {20, 11}, {25, 25}, {5, 5}};
    sample("R", w, r, 4);
    XUnmapWindow(d, w);
}

/* MB: W 100x100 bw 3 (red), child (80,20) 40x40 bw 4 (cyan) crossing W's
 * right edge. In W's pixmap W's right ring is x 103..105. */
static void case_mb(void) {
    for (int order = 0; order < 2; order++) {
        const char *tag = order ? "MB-parent-first" : "MB-child-first";
        Window w = mk(root, 700, 120, 100, 100, 3, WHITE, RED, NorthWestGravity);
        Window c = mk(w, 80, 20, 40, 40, 4, BLUE, CYAN, NorthWestGravity);
        if (order) {
            XMapWindow(d, w);
            settle();
            XMapWindow(d, c);
        } else {
            XMapWindow(d, c);
            XMapWindow(d, w);
        }
        settle();
        const int p[][2] = {{84, 40}, {90, 24}, {95, 40}, {104, 40}, {104, 24}, {104, 70}, {1, 1}};
        sample(tag, w, p, 7);
        XUnmapWindow(d, w);
    }
}

static void case_n(void) {
    Window w = mk(root, 700, 300, 120, 120, 0, WHITE, 0, NorthWestGravity);
    Window c = mk(w, 10, 10, 50, 50, 3, BLUE, RED, NorthWestGravity);
    XMapWindow(d, c);
    XMapWindow(d, w);
    settle();
    GC g = XCreateGC(d, c, 0, NULL);
    XSetForeground(d, g, GREEN);
    XFillRectangle(d, c, g, 0, 0, 20, 20);
    settle();
    XCompositeRedirectWindow(d, c, CompositeRedirectManual);
    settle();
    const int p[][2] = {{1, 1}, {3, 3}, {10, 10}, {30, 30}, {52, 52}};
    sample("N", c, p, 5);
    XFreeGC(d, g);
    XUnmapWindow(d, w);
}

static void case_cf(void) {
    Window f = mk(root, 300, 500, 220, 220, 0, WHITE, 0, NorthWestGravity);
    Window w = mk(f, 10, 10, 100, 100, 0, WHITE, 0, NorthWestGravity);
    Window c = mk(w, 80, 80, 50, 50, 4, BLUE, RED, NorthWestGravity);
    XMapWindow(d, c);
    XMapWindow(d, w);
    XMapWindow(d, f);
    settle();
    XResizeWindow(d, w, 200, 200);
    settle();
    const int p[][2] = {{91, 115}, {115, 91}, {145, 115}, {115, 145}, {130, 130}};
    sample("CF", f, p, 5);
    XUnmapWindow(d, f);
}

static Window mk_none(Window parent, int x, int y, int w, int h, int grav) {
    XSetWindowAttributes a;
    a.background_pixmap = None;
    a.override_redirect = True;
    a.win_gravity = grav;
    return XCreateWindow(d, parent, x, y, w, h, 0, 24, InputOutput, CopyFromParent,
                         CWBackPixmap | CWOverrideRedirect | CWWinGravity, &a);
}

/* RP: frame F (bg None) with title T, button B (NE), right border R (NE)
 * and client area C; two bg-None covers raised over it, F shrunk 400 -> 250
 * wide (B and R move left 150 under the covers), then the covers are
 * reparented out while still mapped. */
static void case_rp(void) {
    Window park = mk(root, -10, -10, 10, 10, 0, WHITE, 0, NorthWestGravity);
    Window f = mk_none(root, 0, 720, 400, 100, NorthWestGravity);
    Window t = mk(f, 0, 0, 340, 20, 0, BLUE, 0, NorthWestGravity);
    Window b = mk(f, 340, 0, 20, 20, 0, RED, 0, NorthEastGravity);
    mk(f, 395, 20, 5, 80, 0, GREEN, 0, NorthEastGravity);
    Window c = mk(f, 0, 20, 395, 80, 0, WHITE, 0, NorthWestGravity);
    XMapSubwindows(d, f);
    XMapWindow(d, f);
    Window ht = mk_none(park, -1, -1, 1, 1, NorthWestGravity);
    Window hr = mk_none(park, -1, -1, 1, 1, NorthWestGravity);
    settle();
    XReparentWindow(d, ht, f, -1, -1);
    XReparentWindow(d, hr, f, -1, -1);
    XRaiseWindow(d, ht);
    XRaiseWindow(d, hr);
    XMoveResizeWindow(d, ht, 0, 0, 400, 20);
    XMapWindow(d, ht);
    XMoveResizeWindow(d, hr, 245, 0, 155, 100);
    XMapWindow(d, hr);
    settle();
    XResizeWindow(d, t, 190, 20);
    XResizeWindow(d, c, 245, 80);
    XResizeWindow(d, f, 250, 100);
    XReparentWindow(d, ht, park, -1, -1);
    XReparentWindow(d, hr, park, -1, -1);
    settle();
    XWindowAttributes ba;
    XGetWindowAttributes(d, b, &ba);
    printf("RP.button-at=%d,%d\n", ba.x, ba.y);
    const int p[][2] = {{100, 10}, {200, 10}, {247, 50}, {100, 50}};
    sample("RP", f, p, 4);
    XDestroyWindow(d, f);
    XDestroyWindow(d, park);
}

/* Structure events in arrival order, windows named. */
static void ug_events(const char *tag, Window const *wins, const char *const *names, int n) {
    while (XPending(d)) {
        XEvent e;
        XNextEvent(d, &e);
        switch (e.type) {
        case ConfigureNotify:
            printf("%s.event=ConfigureNotify ev=%s win=%s %dx%d\n", tag, child_name(e.xconfigure.event, wins, names, n),
                   child_name(e.xconfigure.window, wins, names, n), e.xconfigure.width, e.xconfigure.height);
            break;
        case UnmapNotify:
            printf("%s.event=UnmapNotify ev=%s win=%s from_configure=%d\n", tag,
                   child_name(e.xunmap.event, wins, names, n), child_name(e.xunmap.window, wins, names, n),
                   e.xunmap.from_configure);
            break;
        case MapNotify:
            printf("%s.event=MapNotify ev=%s win=%s\n", tag, child_name(e.xmap.event, wins, names, n),
                   child_name(e.xmap.window, wins, names, n));
            break;
        case GravityNotify:
            printf("%s.event=GravityNotify ev=%s win=%s %d,%d\n", tag, child_name(e.xgravity.event, wins, names, n),
                   child_name(e.xgravity.window, wins, names, n), e.xgravity.x, e.xgravity.y);
            break;
        default:
            break;
        }
    }
}

/* UG: W (white) holds S (green, all of W), K (cyan, NorthEast) and G (blue,
 * UnmapGravity, on top). W shrinks, G is mapped again, W grows. */
static void case_ug(void) {
    Window w = mk(root, 500, 600, 200, 100, 0, WHITE, 0, NorthWestGravity);
    Window s = mk(w, 0, 0, 200, 100, 0, GREEN, 0, NorthWestGravity);
    Window k = mk(w, 180, 0, 20, 20, 0, CYAN, 0, NorthEastGravity);
    Window g = mk(w, 0, 0, 100, 50, 0, BLUE, 0, UnmapGravity);
    Window wins[] = {w, s, k, g};
    const char *names[] = {"W", "S", "K", "G"};
    XSelectInput(d, w, StructureNotifyMask | SubstructureNotifyMask);
    XSelectInput(d, k, StructureNotifyMask);
    XSelectInput(d, g, StructureNotifyMask);
    XMapSubwindows(d, w);
    XMapWindow(d, w);
    settle();
    ug_events("UG-setup", wins, names, 4);
    const int p[][2] = {{50, 25}, {120, 40}, {135, 10}};
    for (int grow = 0; grow < 2; grow++) {
        const char *tag = grow ? "UG-grow" : "UG-shrink";
        if (grow) {
            XMapWindow(d, g);
            settle();
            ug_events("UG-remap", wins, names, 4);
        }
        printf("%s.phase=resize\n", tag);
        XResizeWindow(d, w, grow ? 250 : 150, grow ? 120 : 100);
        settle();
        ug_events(tag, wins, names, 4);
        XWindowAttributes ga;
        XGetWindowAttributes(d, g, &ga);
        printf("%s.G.map_state=%s\n", tag,
               ga.map_state == IsViewable ? "viewable" : ga.map_state == IsUnviewable ? "unviewable" : "unmapped");
        sample(tag, w, p, 3);
    }
    XDestroyWindow(d, w);
    settle();
    XSync(d, True);
}

int main(void) {
    d = XOpenDisplay(NULL);
    if (!d) return 1;
    raw = fopen("damage-raw.log", "w");
    if (!raw) raw = stderr;
    setvbuf(stdout, NULL, _IOLBF, 0);
    XSetErrorHandler(on_error);
    root = DefaultRootWindow(d);
    int err, maj, min;
    Bool pix;
    if (!XDamageQueryExtension(d, &dmg_ev, &err) || !XShmQueryVersion(d, &maj, &min, &pix)) {
        printf("missing=damage-or-shm\n");
        return 1;
    }
    XCompositeRedirectSubwindows(d, root, CompositeRedirectManual);
    settle();
    case_d();
    case_c();
    case_g();
    case_u();
    case_m();
    case_mb();
    case_n();
    case_gs();
    case_cf();
    case_rp();
    case_ug();
    printf("done\n");
    fclose(raw);
    return 0;
}
