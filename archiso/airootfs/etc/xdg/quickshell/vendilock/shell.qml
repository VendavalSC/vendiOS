// vendilock — the vendiOS lock screen (quickshell + ext-session-lock).
//
// The desktop never disappears: the compositor freezes it and blurs it in
// place (vendiwm lock backdrop), so windows stay visible — just frosted.
// The bar melts its side modules first (over IPC); the center notch stays
// put and the lock blob takes its exact place in a seamless swap, then
// swells, lets go, and settles mid-screen as a living blob — wobbling,
// telling only the time, leaning toward the mouse when you come near. On
// unlock it dips, shoots off the top while shrinking, the blur melts away,
// and the whole bar glides back down from the edge.
//
// Typing pulses the blob; wrong passwords shake it. `vendi-ctl lock`,
// bound to super+escape.

import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Services.Pam
import QtQuick

ShellRoot {
    id: root

    // Matches vendibar-pro's center notch; the real width is fetched from
    // the bar over IPC so the blob swap is pixel-perfect.
    property int notchW: 236
    readonly property int notchH: 38
    readonly property int notchR: 15
    readonly property color blobColor: "#0b0b12"
    readonly property int circleD: 210

    property string password: ""
    property bool authenticating: false
    property bool failed: false
    property bool unlocking: false

    SystemClock { id: sysClock; precision: SystemClock.Seconds }

    PamContext {
        id: pam
        onPamMessage: {
            if (this.responseRequired) this.respond(root.password);
        }
        onCompleted: result => {
            root.authenticating = false;
            if (result === PamResult.Success) {
                root.unlocking = true;   // each surface starts its flyup
            } else {
                root.failed = true;
                root.password = "";
            }
        }
    }

    function tryUnlock() {
        if (root.authenticating || root.unlocking || root.password.length === 0) return;
        root.failed = false;
        root.authenticating = true;
        pam.start();
    }

    function barCall(fn) {
        Quickshell.execDetached(["quickshell", "-c", "vendibar-pro", "ipc", "call", "panel", fn]);
    }

    // Act one: the bar melts its side modules (the center notch stays put),
    // then everything swaps out at once as the lock blob takes the notch's
    // exact place.
    Process {
        id: widthProbe
        command: ["quickshell", "-c", "vendibar-pro", "ipc", "call", "panel", "centerWidth"]
        stdout: StdioCollector {
            onStreamFinished: {
                const w = parseInt(text);
                if (w > 60 && w < 800) root.notchW = w;
            }
        }
    }
    // Lock FIRST (the blob maps docked over the live notch — the snapshot
    // the compositor freezes excludes the bar layer, so no ghost and no
    // gap); the bar's chrome vanish happens invisibly behind the lock.
    Component.onCompleted: { barCall("hide"); widthProbe.running = true; lockTimer.start(); }
    Timer { id: lockTimer; interval: 400; onTriggered: { lock.locked = true; vanishTimer.start(); } }
    Timer { id: vanishTimer; interval: 350; onTriggered: barCall("vanish") }
    // Give the unlock request a beat to flush before exiting.
    Timer { id: quitTimer; interval: 150; onTriggered: Qt.quit() }

    WlSessionLock {
        id: lock
        locked: false
        onLockedChanged: if (!locked) { quitTimer.start(); }

        WlSessionLockSurface {
            id: surf
            color: "transparent"

            readonly property real notchX: (width - root.notchW) / 2
            readonly property real circleX: (width - root.circleD) / 2
            readonly property real circleY: (height - root.circleD) / 2

            // ── keys: invisible input ──────────────────────────────────────
            Item {
                anchors.fill: parent
                focus: true
                Keys.onPressed: event => {
                    if (root.unlocking) return;
                    if (event.key === Qt.Key_Return || event.key === Qt.Key_Enter) {
                        root.tryUnlock();
                    } else if (event.key === Qt.Key_Backspace) {
                        if (event.modifiers & Qt.ControlModifier) root.password = "";
                        else root.password = root.password.slice(0, -1);
                        root.failed = false;
                    } else if (event.key === Qt.Key_Escape) {
                        root.password = "";
                        root.failed = false;
                    } else if (event.text.length === 1 && event.text >= " ") {
                        root.password += event.text;
                        root.failed = false;
                        typePulse.restart();
                    }
                    event.accepted = true;
                }
            }

            // Mouse tracking for the blob's curiosity.
            MouseArea {
                id: mouse
                anchors.fill: parent
                hoverEnabled: true
                acceptedButtons: Qt.NoButton
            }

            // ── the blob ───────────────────────────────────────────────────
            Item {
                id: blob
                x: surf.notchX
                y: -root.notchR                      // docked: it IS the notch
                width: root.notchW
                height: root.notchH + root.notchR
                property real shapeRadius: root.notchR
                // 0 → notch silhouette, 1 → full blob (satellites + clock).
                property real blobness: 0

                // Lean toward a nearby pointer — curious, never clingy.
                readonly property real cx: x + width / 2
                readonly property real cy: y + height / 2
                readonly property real mdx: mouse.mouseX - cx
                readonly property real mdy: mouse.mouseY - cy
                readonly property real mdist: Math.sqrt(mdx * mdx + mdy * mdy)
                readonly property real pull: blobness >= 1 && !root.unlocking
                    ? Math.max(0, 1 - mdist / 320) : 0
                transform: [
                    Translate {
                        x: blob.mdist > 1 ? blob.mdx / blob.mdist * blob.pull * 14 : 0
                        y: blob.mdist > 1 ? blob.mdy / blob.mdist * blob.pull * 14 : 0
                        Behavior on x { NumberAnimation { duration: 320; easing.type: Easing.OutCubic } }
                        Behavior on y { NumberAnimation { duration: 320; easing.type: Easing.OutCubic } }
                    },
                    Scale {
                        id: stretch
                        origin.x: blob.width / 2; origin.y: blob.height / 2
                        xScale: 1; yScale: 1
                    },
                    Translate { id: shake; x: 0 }
                ]

                // Main body.
                Rectangle {
                    id: body
                    anchors.fill: parent
                    radius: blob.shapeRadius
                    color: root.blobColor
                }
                // Satellites: same-color circles drifting around the center —
                // their union with the body keeps the outline undulating.
                Item {
                    id: sats
                    anchors.fill: parent
                    opacity: blob.blobness
                    RotationAnimation on rotation {
                        from: 0; to: 360; duration: 9000
                        loops: Animation.Infinite
                        running: blob.blobness > 0.2 && !root.unlocking
                    }
                    // Two breathing phases, different periods — the shape
                    // never repeats exactly.
                    property real wob: 0
                    property real wob2: 0
                    SequentialAnimation on wob {
                        loops: Animation.Infinite
                        running: blob.blobness > 0.2 && !root.unlocking
                        NumberAnimation { to: 1; duration: 1500; easing.type: Easing.InOutSine }
                        NumberAnimation { to: 0; duration: 1500; easing.type: Easing.InOutSine }
                    }
                    SequentialAnimation on wob2 {
                        loops: Animation.Infinite
                        running: blob.blobness > 0.2 && !root.unlocking
                        NumberAnimation { to: 1; duration: 2300; easing.type: Easing.InOutSine }
                        NumberAnimation { to: 0; duration: 2300; easing.type: Easing.InOutSine }
                    }
                    Repeater {
                        model: [
                            { ang: 0.0, rad: 0.42, off: 0.105, w: 1 },
                            { ang: 2.1, rad: 0.37, off: 0.140, w: -1 },
                            { ang: 4.2, rad: 0.40, off: 0.120, w: 1 },
                            { ang: 5.4, rad: 0.34, off: 0.150, w: -1 },
                        ]
                        Rectangle {
                            property real bulge: modelData.off + (modelData.w > 0 ? sats.wob : sats.wob2) * 0.04
                            property real breathe: modelData.rad + (modelData.w > 0 ? sats.wob2 : sats.wob) * 0.025
                            width: blob.width * breathe * 2
                            height: width
                            radius: width / 2
                            color: body.color
                            x: blob.width / 2 + Math.cos(modelData.ang) * blob.width * bulge - width / 2
                            y: blob.height / 2 + Math.sin(modelData.ang) * blob.height * bulge - height / 2
                        }
                    }
                }
                // Idle drift: the settled blob floats a few px, forever.
                // (No alwaysRunToEnd — flyup must own y the moment it starts.)
                SequentialAnimation {
                    loops: Animation.Infinite
                    running: blob.blobness >= 1 && !root.authenticating && !root.unlocking
                    NumberAnimation { target: blob; property: "y"; to: surf.circleY - 7; duration: 2400; easing.type: Easing.InOutSine }
                    NumberAnimation { target: blob; property: "y"; to: surf.circleY + 5; duration: 2400; easing.type: Easing.InOutSine }
                    NumberAnimation { target: blob; property: "y"; to: surf.circleY;     duration: 2400; easing.type: Easing.InOutSine }
                }

                // The time. White. Nothing else.
                Text {
                    anchors.centerIn: parent
                    text: Qt.formatDateTime(sysClock.date, "HH:mm")
                    color: "#ffffff"
                    font.family: "JetBrainsMonoNL Nerd Font"
                    font.pixelSize: 46
                    font.weight: Font.Light
                    font.letterSpacing: 2
                    opacity: blob.blobness
                }

                // Keystroke pop.
                SequentialAnimation {
                    id: typePulse
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "xScale"; to: 1.05; duration: 60; easing.type: Easing.OutQuad }
                        NumberAnimation { target: stretch; property: "yScale"; to: 0.96; duration: 60; easing.type: Easing.OutQuad }
                    }
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "xScale"; to: 1.0; duration: 240; easing.type: Easing.OutBack }
                        NumberAnimation { target: stretch; property: "yScale"; to: 1.0; duration: 240; easing.type: Easing.OutBack }
                    }
                }
                // Fail shake.
                SequentialAnimation {
                    running: root.failed
                    NumberAnimation { target: shake; property: "x"; to: -16; duration: 45 }
                    NumberAnimation { target: shake; property: "x"; to: 12;  duration: 45 }
                    NumberAnimation { target: shake; property: "x"; to: -7;  duration: 45 }
                    NumberAnimation { target: shake; property: "x"; to: 4;   duration: 45 }
                    NumberAnimation { target: shake; property: "x"; to: 0;   duration: 45 }
                }
                // Authenticating: quicker breath.
                SequentialAnimation {
                    running: root.authenticating
                    loops: Animation.Infinite
                    alwaysRunToEnd: true
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "xScale"; to: 1.03; duration: 380; easing.type: Easing.InOutSine }
                        NumberAnimation { target: stretch; property: "yScale"; to: 1.03; duration: 380; easing.type: Easing.InOutSine }
                    }
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "xScale"; to: 1.0; duration: 380; easing.type: Easing.InOutSine }
                        NumberAnimation { target: stretch; property: "yScale"; to: 1.0; duration: 380; easing.type: Easing.InOutSine }
                    }
                }
            }

            // ── choreography ───────────────────────────────────────────────
            // Lock-in: the notch silhouette drips back down from the top
            // edge, stretching as it falls, and rounds out into the blob.
            SequentialAnimation {
                id: detachAnim
                // A beat of stillness while the compositor's blur dissolves
                // the frozen bar underneath — the notch is just the notch…
                PauseAnimation { duration: 420 }
                // …then it swells…
                NumberAnimation { target: blob; property: "height"; to: root.notchH + root.notchR + 14; duration: 140; easing.type: Easing.OutQuad }
                // …then lets go.
                ParallelAnimation {
                    NumberAnimation { target: blob; property: "y"; to: surf.circleY; duration: 800; easing.type: Easing.OutBack; easing.overshoot: 0.8 }
                    NumberAnimation { target: blob; property: "x"; to: surf.circleX; duration: 680; easing.type: Easing.InOutCubic }
                    NumberAnimation { target: blob; property: "width";  to: root.circleD; duration: 680; easing.type: Easing.InOutCubic }
                    NumberAnimation { target: blob; property: "height"; to: root.circleD; duration: 680; easing.type: Easing.InOutCubic }
                    NumberAnimation { target: blob; property: "shapeRadius"; to: root.circleD / 2; duration: 680; easing.type: Easing.InOutCubic }
                    NumberAnimation { target: blob; property: "blobness"; to: 1; duration: 800; easing.type: Easing.InCubic }
                    SequentialAnimation {
                        ParallelAnimation {
                            NumberAnimation { target: stretch; property: "yScale"; to: 1.16; duration: 280; easing.type: Easing.OutQuad }
                            NumberAnimation { target: stretch; property: "xScale"; to: 0.88; duration: 280; easing.type: Easing.OutQuad }
                        }
                        ParallelAnimation {
                            NumberAnimation { target: stretch; property: "yScale"; to: 1.0; duration: 480; easing.type: Easing.OutBack; easing.overshoot: 2.4 }
                            NumberAnimation { target: stretch; property: "xScale"; to: 1.0; duration: 480; easing.type: Easing.OutBack; easing.overshoot: 2.4 }
                        }
                    }
                }
            }

            // Unlock: dip with a squish, then shoot off the top while
            // shrinking. The compositor melts the blur underneath; the bar
            // slides back in on its own.
            SequentialAnimation {
                id: flyupAnim
                ParallelAnimation {
                    NumberAnimation { target: blob; property: "blobness"; to: 0; duration: 170 }
                    NumberAnimation { target: blob; property: "y"; to: surf.circleY + 30; duration: 210; easing.type: Easing.OutQuad }
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "yScale"; to: 0.85; duration: 210; easing.type: Easing.OutQuad }
                        NumberAnimation { target: stretch; property: "xScale"; to: 1.12; duration: 210; easing.type: Easing.OutQuad }
                    }
                }
                ParallelAnimation {
                    NumberAnimation { target: blob; property: "y"; to: -root.circleD - 100; duration: 470; easing.type: Easing.InCubic }
                    NumberAnimation { target: blob; property: "scale"; to: 0.68; duration: 470; easing.type: Easing.InCubic }
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "yScale"; to: 1.22; duration: 320; easing.type: Easing.InQuad }
                        NumberAnimation { target: stretch; property: "xScale"; to: 0.84; duration: 320; easing.type: Easing.InQuad }
                    }
                }
                ScriptAction { script: { lock.locked = false; root.barCall("restore"); } }
            }

            Connections {
                target: root
                function onUnlockingChanged() { if (root.unlocking) flyupAnim.start(); }
            }

            Component.onCompleted: detachAnim.start()
        }
    }
}
