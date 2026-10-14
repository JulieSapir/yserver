# Probes: xlat (XTest motion + round-trip probe), xdrag (title-bar drag + lag), xhide (XFIXES HideCursor).
all: xlat xdrag xhide
xlat: src/xlat.c ; $(CC) -O2 -o $@ $< -lxcb -lxcb-xtest -lm
xdrag: src/xdrag.c ; $(CC) -O2 -o $@ $< -lxcb -lxcb-xtest -lm
xhide: src/xhide.c ; $(CC) -O2 -o $@ $< -lX11 -lXfixes
