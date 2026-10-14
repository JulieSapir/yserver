/* xhide <seconds>: XFixesHideCursor on the root window for <seconds>, then ShowCursor. */
#include <X11/Xlib.h>
#include <X11/extensions/Xfixes.h>
#include <stdlib.h>
#include <unistd.h>
int main(int argc, char **argv) {
    Display *d = XOpenDisplay(NULL); if (!d) return 1;
    XFixesHideCursor(d, DefaultRootWindow(d)); XSync(d, False);
    sleep(argc > 1 ? atoi(argv[1]) : 10);
    XFixesShowCursor(d, DefaultRootWindow(d)); XSync(d, False);
    XCloseDisplay(d); return 0;
}
