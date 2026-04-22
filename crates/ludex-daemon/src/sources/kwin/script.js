// ludex KWin foreground-window forwarder
//
// Loaded into KWin on daemon startup via org.kde.kwin.Scripting.loadScript.
// Connects to workspace.windowActivated and calls the ludex daemon's
// D-Bus service every time the active window changes. The daemon owns
// all state; this script only forwards observations.
//
// Notes on the Plasma 6 KWin scripting sandbox:
//
//  1. callDBus destinations must start with org.kde.* or
//     org.freedesktop.*. `net.*` is silently dropped. Our daemon
//     registers the receiver under org.kde.ludex.Tracker1 to satisfy
//     this.
//  2. callDBus does not reliably marshal JS Number to D-Bus `u`
//     (uint32) or JS Boolean to `b`. Strings always round-trip
//     cleanly, so every argument below is stringified and parsed on
//     the daemon side.

function notify(win) {
    if (!win) {
        return;
    }
    var pid = (typeof win.pid === "number" && win.pid > 0) ? win.pid : 0;
    if (pid === 0) {
        return;
    }
    callDBus(
        "org.kde.ludex.Tracker1",
        "/org/kde/ludex/ForegroundEvents",
        "org.kde.ludex.ForegroundEvents1",
        "ReportWindowActivated",
        String(pid),
        win.fullScreen ? "true" : "false",
        (win.resourceClass || "").toString(),
        (win.caption || "").toString()
    );
}

workspace.windowActivated.connect(notify);

// Fire once so the daemon picks up whatever window is already active
// when the script is loaded (matters for daemon restarts mid-session).
if (workspace.activeWindow) {
    notify(workspace.activeWindow);
}
