#!/usr/bin/env python3
"""Property byte-order probe: a big-endian ('B') and a little-endian ('l')
raw X11 client write and read the same window properties.

Every value is printed as the READER decodes it in its own byte order, so a
server that stores client order shows up as swapped numbers on the other
client. Prints no ids or timestamps, so two servers' logs diff directly.
"""
import os
import socket
import struct
import sys

PROPERTY_NOTIFY = 28
CARDINAL, INTEGER, STRING = 6, 19, 31
# XI2 device properties and RotateProperties diverge for known reasons
# (see the scenario header); PROBE_FULL=1 prints them anyway.
FULL = os.environ.get("PROBE_FULL") == "1"


class Client:
    def __init__(self, order, name):
        self.e = ">" if order == "B" else "<"
        self.name = name
        disp = os.environ.get("DISPLAY", ":0").split(":")[1].split(".")[0]
        self.s = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.s.connect(f"/tmp/.X11-unix/X{disp}")
        self.s.sendall(struct.pack(self.e + "cxHHHHxx", order.encode(), 11, 0, 0, 0))
        head = self.recv(8)
        if head[0] != 1:
            sys.exit(f"{name}: setup refused")
        extra = struct.unpack(self.e + "H", head[6:8])[0] * 4
        body = self.recv(extra)
        self.base, self.mask = struct.unpack(self.e + "II", body[4:12])
        vlen = struct.unpack(self.e + "H", body[16:18])[0]
        nfmt = body[21]
        off = 32 + ((vlen + 3) & ~3) + 8 * nfmt
        self.root = struct.unpack(self.e + "I", body[off:off + 4])[0]
        self.seq = 0
        self.next_id = 0
        self.events = []
        self.errors = []

    def recv(self, n):
        out = b""
        while len(out) < n:
            chunk = self.s.recv(n - len(out))
            if not chunk:
                sys.exit(f"{self.name}: connection closed")
            out += chunk
        return out

    def xid(self):
        self.next_id += 1
        return self.base | self.next_id

    def send(self, opcode, data, body):
        body += b"\0" * (-len(body) % 4)
        self.s.sendall(struct.pack(self.e + "BBH", opcode, data, 1 + len(body) // 4) + body)
        self.seq = (self.seq + 1) & 0xFFFF
        return self.seq

    def wait(self, seq):
        """Read until the reply (or error) for `seq`; queue events."""
        while True:
            pkt = self.recv(32)
            kind = pkt[0] & 0x7F
            pseq = struct.unpack(self.e + "H", pkt[2:4])[0]
            if kind == 1:
                extra = struct.unpack(self.e + "I", pkt[4:8])[0] * 4
                pkt += self.recv(extra)
                if pseq == seq:
                    return pkt
            elif kind == 0:
                self.errors.append((pkt[1], pkt[10], pseq))
                if pseq == seq:
                    return None
            else:
                self.events.append(pkt)

    def sync(self):
        # GetInputFocus as a round trip.
        self.wait(self.send(43, 0, b""))

    def intern(self, name):
        n = name.encode()
        rep = self.wait(self.send(16, 0, struct.pack(self.e + "HH", len(n), 0) + n))
        return struct.unpack(self.e + "I", rep[8:12])[0]

    def atom_name(self, atom):
        rep = self.wait(self.send(17, 0, struct.pack(self.e + "I", atom)))
        n = struct.unpack(self.e + "H", rep[8:10])[0]
        return rep[32:32 + n].decode()

    def change(self, mode, win, prop, typ, fmt, values):
        if fmt == 8:
            data = bytes(values)
        else:
            data = b"".join(struct.pack(self.e + ("H" if fmt == 16 else "I"), v) for v in values)
        body = struct.pack(self.e + "IIIB3xI", win, prop, typ, fmt, len(values)) + data
        return self.send(18, mode, body)

    def get(self, win, prop, offset=0, length=1000, delete=False, typ=0):
        rep = self.wait(self.send(20, int(delete),
                                  struct.pack(self.e + "IIIII", win, prop, typ, offset, length)))
        if rep is None:
            return "error"
        fmt = rep[1]
        typ, after, n = struct.unpack(self.e + "III", rep[8:20])
        data = rep[32:]
        vals = decode(self.e, fmt, n, data)
        return f"format={fmt} type={typ} nitems={n} bytes_after={after} [{vals}] raw={data[:n * max(fmt // 8, 1)].hex()}"

    def select_property_change(self, win):
        self.send(2, 0, struct.pack(self.e + "III", win, 0x800, 0x0040_0000))  # CWEventMask

    def drain_events(self, names, win):
        self.sync()
        out = []
        for ev in self.events:
            if ev[0] & 0x7F == PROPERTY_NOTIFY:
                w, atom = struct.unpack(self.e + "II", ev[4:12])
                state = "Deleted" if ev[16] else "NewValue"
                rel = "W" if w == win else "other"
                out.append(f"PropertyNotify win={rel} atom={names.get(atom, atom)} state={state}")
        self.events = []
        return out


def decode(e, fmt, n, data):
    if fmt == 8:
        return data[:n].hex()
    if fmt not in (16, 32):
        return ""
    code, w = ("H", 2) if fmt == 16 else ("I", 4)
    return " ".join(f"0x{struct.unpack(e + code, data[i * w:(i + 1) * w])[0]:0{w * 2}x}"
                    for i in range(n))


def query_ext(c, name):
    n = name.encode()
    rep = c.wait(c.send(98, 0, struct.pack(c.e + "HH", len(n), 0) + n))
    return rep[9] if rep[8] else None


def ext_props(clients, a32, a16):
    """RandR output properties and XI2 device properties, BE writer."""
    be = clients[0]
    for c in clients:
        c.rr = query_ext(c, "RANDR")
        c.xi = query_ext(c, "XInputExtension")
        c.wait(c.send(c.rr, 0, struct.pack(c.e + "II", 1, 6)))
        c.wait(c.send(c.xi, 47, struct.pack(c.e + "HH", 2, 2)))
    rep = be.wait(be.send(be.rr, 25, struct.pack(">I", be.root)))
    ncrtc, nout = struct.unpack(">HH", rep[16:20])
    output = struct.unpack(">I", rep[32 + 4 * ncrtc:36 + 4 * ncrtc])[0]
    for fmt, atom, vals in ((32, a32, [0x01020304, 5]), (16, a16, [0x0102, 0x0304, 7])):
        code = "I" if fmt == 32 else "H"
        data = b"".join(struct.pack(">" + code, v) for v in vals)
        data += b"\0" * (-len(data) % 4)
        be.send(be.rr, 13, struct.pack(">IIIBBxxI", output, atom, INTEGER, fmt, 0, len(vals)) + data)
        if FULL:
            be.send(be.xi, 57, struct.pack(">HBBIII", 2, 0, fmt, atom, INTEGER, len(vals)) + data)
        be.sync()
        for c in clients:
            rep = c.wait(c.send(c.rr, 15, struct.pack(c.e + "IIIIIBBxx", output, atom, 0, 0, 100, 0, 0)))
            if rep is None:
                print(f"RandR-output-{fmt} {c.name}-read: error")
            else:
                typ, after, n = struct.unpack(c.e + "III", rep[8:20])
                print(f"RandR-output-{fmt} {c.name}-read: format={rep[1]} type={typ} nitems={n} "
                      f"bytes_after={after} [{decode(c.e, rep[1], n, rep[32:])}]")
            if not FULL:
                continue
            rep = c.wait(c.send(c.xi, 59, struct.pack(c.e + "HBxIIII", 2, 0, atom, 0, 0, 100)))
            if rep is None:
                print(f"XI2-device-{fmt} {c.name}-read: error")
            else:
                typ, after, n = struct.unpack(c.e + "III", rep[8:20])
                print(f"XI2-device-{fmt} {c.name}-read: format={rep[20]} type={typ} nitems={n} "
                      f"bytes_after={after} [{decode(c.e, rep[20], n, rep[32:])}]")


def main():
    be = Client("B", "BE")
    le = Client("l", "LE")
    clients = (be, le)

    # The window belongs to the LE client; both select PropertyChange on it.
    win = le.xid()
    le.send(1, 0, struct.pack("<IIhhHHHHII", win, le.root, 0, 0, 10, 10, 0, 1, 0, 0))
    le.sync()
    for c in clients:
        c.select_property_change(win)
        c.sync()

    names = {}
    atoms = {}
    for n in ("PROBE_BE32", "PROBE_BE16", "PROBE_BE8", "PROBE_LE32", "PROBE_LE16",
              "PROBE_R1", "PROBE_R2", "PROBE_R3"):
        atoms[n] = le.intern(n)
        names[atoms[n]] = n
    p = atoms

    def reads(label, prop, **kw):
        for c in clients:
            print(f"{label} {c.name}-read: {c.get(win, prop, **kw)}")

    def events(label):
        for c in clients:
            for line in c.drain_events(names, win):
                print(f"{label} {c.name}-event: {line}")

    # 1. BE writer.
    be.change(0, win, p["PROBE_BE32"], CARDINAL, 32, [0x01020304, 5])
    be.change(0, win, p["PROBE_BE16"], INTEGER, 16, [0x0102, 0x0304, 7])
    be.change(0, win, p["PROBE_BE8"], STRING, 8, [0x61, 0x62, 0x01, 0x02, 0x03])
    be.sync()
    events("be-write")
    for n in ("PROBE_BE32", "PROBE_BE16", "PROBE_BE8"):
        reads(f"{n}", p[n])

    # 2. BE append + BE prepend on format 32, LE append on the same.
    be.change(2, win, p["PROBE_BE32"], CARDINAL, 32, [0x0A0B0C0D])
    be.change(1, win, p["PROBE_BE32"], CARDINAL, 32, [0x11223344])
    le.change(2, win, p["PROBE_BE32"], CARDINAL, 32, [0x55667788])
    le.sync()
    be.sync()
    events("append")
    reads("PROBE_BE32-appended", p["PROBE_BE32"])

    # 3. LE writer.
    le.change(0, win, p["PROBE_LE32"], CARDINAL, 32, [0x01020304, 5])
    le.change(0, win, p["PROBE_LE16"], INTEGER, 16, [0x0102, 0x0304, 7])
    le.sync()
    events("le-write")
    for n in ("PROBE_LE32", "PROBE_LE16"):
        reads(n, p[n])

    # 4. Windows: long_offset/long_length (32-bit units).
    reads("BE32-window(1,1)", p["PROBE_BE32"], offset=1, length=1)
    reads("BE32-window(2,5)", p["PROBE_BE32"], offset=2, length=5)
    reads("BE16-window(0,1)", p["PROBE_BE16"], offset=0, length=1)
    reads("BE16-window(1,1)", p["PROBE_BE16"], offset=1, length=1)
    reads("LE16-window(1,1)", p["PROBE_LE16"], offset=1, length=1)
    reads("BE8-window(1,1)", p["PROBE_BE8"], offset=1, length=1)
    reads("BE32-type-mismatch", p["PROBE_BE32"], typ=INTEGER)

    # 5. delete=True from the BE client, then from the LE client.
    print(f"delete BE-read: {be.get(win, p['PROBE_LE32'], delete=True)}")
    events("be-delete")
    print(f"after-delete LE-read: {le.get(win, p['PROBE_LE32'])}")
    print(f"delete LE-read: {le.get(win, p['PROBE_BE16'], delete=True)}")
    events("le-delete")
    print(f"after-delete BE-read: {be.get(win, p['PROBE_BE16'])}")
    # delete with a window that leaves bytes_after: not deleted.
    print(f"partial-delete BE-read: {be.get(win, p['PROBE_BE32'], offset=0, length=1, delete=True)}")
    events("partial-delete")
    reads("after-partial-delete", p["PROBE_BE32"])

    # 6. RotateProperties by the BE client. Only with PROBE_FULL=1: yserver
    #    does not implement RotateProperties (BadRequest).
    for i, n in enumerate(("PROBE_R1", "PROBE_R2", "PROBE_R3")):
        be.change(0, win, p[n], CARDINAL, 32, [0x0100 + i])
    be.sync()
    events("rotate-setup")
    if FULL:
        plist = [p["PROBE_R1"], p["PROBE_R2"], p["PROBE_R3"]]
        be.send(114, 0, struct.pack(">IHh", win, 3, 1)
                + b"".join(struct.pack(">I", a) for a in plist))
        be.sync()
        events("rotate")
        for n in ("PROBE_R1", "PROBE_R2", "PROBE_R3"):
            reads(f"rotated-{n}", p[n])

    # 7. ListProperties, decoded per client.
    for c in clients:
        rep = c.wait(c.send(21, 0, struct.pack(c.e + "I", win)))
        cnt = struct.unpack(c.e + "H", rep[8:10])[0]
        got = [struct.unpack(c.e + "I", rep[32 + 4 * i:36 + 4 * i])[0] for i in range(cnt)]
        print(f"ListProperties {c.name}: {sorted(names.get(a, f'?{a}') for a in got)}")

    ext_props(clients, le.intern("PROBE_EXT32"), le.intern("PROBE_EXT16"))

    for c in clients:
        for code, major, seq in c.errors:
            print(f"{c.name} error code={code} major={major}")
    open("PROBE-DONE", "w").close()


main()
