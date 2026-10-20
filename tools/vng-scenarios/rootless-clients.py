#!/usr/bin/env python3
"""Clients for rootless-headless.sh that the guest has no ready binary for:
an override-redirect window (a popup) and a SHAPEd one, each painted on
Expose. Runs until killed.

  rootless-clients.py
"""
from Xlib import X, display
from Xlib.ext import shape


def main():
    d = display.Display()
    s = d.screen()
    root = s.root
    popup = root.create_window(
        300, 40, 80, 40, 2, s.root_depth, X.InputOutput, X.CopyFromParent,
        background_pixel=0xFF0000, border_pixel=0x00FF00,
        override_redirect=True, event_mask=X.ExposureMask)
    shaped = root.create_window(
        300, 120, 60, 60, 0, s.root_depth, X.InputOutput, X.CopyFromParent,
        background_pixel=0x0000FF, event_mask=X.ExposureMask)
    shaped.shape_rectangles(shape.SO.Set, shape.SK.Bounding, X.Unsorted, 0, 0,
                            [(0, 0, 60, 20), (20, 20, 20, 40)])
    gc = root.create_gc(foreground=0xFFFF00)
    popup.map()
    shaped.map()
    d.sync()
    print(f"popup 0x{popup.id:x}", flush=True)
    print(f"shaped 0x{shaped.id:x}", flush=True)
    while True:
        ev = d.next_event()
        if ev.type == X.Expose and ev.count == 0:
            ev.window.fill_rectangle(gc, 4, 4, 12, 8)
            d.flush()


if __name__ == "__main__":
    main()
