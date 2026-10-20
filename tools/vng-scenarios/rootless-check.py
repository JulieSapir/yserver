#!/usr/bin/env python3
"""Check rootless-headless.sh's recording (`--rootless-record DIR`) against
the X server itself: every viewable top-level has a `mapped` event (an
override-redirect one as a popup, a SHAPEd one with a `reshaped` event),
and its newest recorded frame, cropped by the border width xwininfo
would report, equals GetImage of the window (what `xwd -id` reads).
Retries for a while, since a frame may trail the window's last paint.

  rootless-check.py <record-dir>     exit 0 = pass
"""
import sys
import time

from Xlib import X, display
from Xlib.ext import shape

WM_CHECK_WINDOW = 0x107  # resources::ROOTLESS_WM_CHECK_WINDOW, a server window
TRIES = 20


def parse(line):
    head, *rest = line.split()
    return head, dict(kv.split("=", 1) for kv in rest if "=" in kv)


def toplevels(d):
    out = []
    for w in d.screen().root.query_tree().children:
        attrs = w.get_attributes()
        if w.id == WM_CHECK_WINDOW or attrs.map_state != X.IsViewable:
            continue
        if attrs.win_class != X.InputOutput:
            continue
        out.append((w, attrs))
    return out


def check(d, rec):
    events = [parse(l) for l in open(f"{rec}/events.log") if l.strip()]
    frames = [parse("frame " + l)[1] for l in open(f"{rec}/frames.log") if l.strip()]
    mapped = {kv["window"]: kv for head, kv in events if head == "mapped"}
    reshaped = {kv["id"] for head, kv in events if head == "reshaped"}
    problems = []
    tops = toplevels(d)
    if len(tops) < 4:
        problems.append(f"only {len(tops)} top-levels viewable")
    for w, attrs in tops:
        wid = f"0x{w.id:x}"
        geo = w.get_geometry()
        ev = mapped.get(wid)
        if ev is None:
            problems.append(f"{wid}: no mapped event")
            continue
        if attrs.override_redirect and not ev["kind"].startswith("popup"):
            problems.append(f"{wid}: override-redirect but kind={ev['kind']}")
        if w.shape_query_extents().bounding_shaped and ev["id"] not in reshaped:
            problems.append(f"{wid}: shaped but no reshaped event")
        mine = [f for f in frames if f["window"] == wid]
        if not mine:
            problems.append(f"{wid}: no frame")
            continue
        f = mine[-1]
        bw, w_, h_ = geo.border_width, geo.width, geo.height
        if (int(f["width"]), int(f["height"])) != (w_ + 2 * bw, h_ + 2 * bw):
            problems.append(f"{wid}: frame {f['width']}x{f['height']}, window {w_}x{h_} bw {bw}")
            continue
        data = open(f"{rec}/{f['file']}", "rb").read()
        stride = int(f["stride"])
        img = w.get_image(0, 0, w_, h_, X.ZPixmap, 0xFFFFFFFF).data
        bad = 0
        for y in range(h_):
            row = data[(y + bw) * stride + bw * 4:(y + bw) * stride + (bw + w_) * 4]
            want = img[y * w_ * 4:(y + 1) * w_ * 4]
            for x in range(w_):
                if row[x * 4:x * 4 + 3] != want[x * 4:x * 4 + 3]:
                    bad += 1
        if bad:
            problems.append(f"{wid}: {bad} pixels differ from GetImage (frame {f['file']})")
        else:
            print(f"ok {wid} kind={ev['kind']} {w_}x{h_} bw={bw} frame={f['file']}")
    return problems


def main():
    rec = sys.argv[1]
    d = display.Display()
    for i in range(TRIES):
        problems = check(d, rec)
        if not problems:
            print("pass")
            return 0
        if i + 1 < TRIES:
            time.sleep(0.5)
    for p in problems:
        print("fail:", p)
    return 1


if __name__ == "__main__":
    sys.exit(main())
