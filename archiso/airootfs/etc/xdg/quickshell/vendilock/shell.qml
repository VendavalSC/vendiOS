// vendilock — the vendiOS lock screen (quickshell + ext-session-lock).
//
// The bar's center notch comes alive. On lock it stretches, detaches and
// swims to the middle of the screen as a wobbling blob (satellite circles
// keep its outline undulating — it's a blob, not a perfect circle) showing
// only the time in white. The wallpaper behind is blurred, never darkened.
// On unlock the blob flashes to the theme accent, dips, then shoots off the
// top of the screen shrinking as it goes — and the notch slides back down
// from above, docking exactly where the bar's real notch lives.
//
// Typing pulses the blob; wrong passwords shake it. `vendi-ctl lock`.

import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Services.Pam
import QtQuick
import QtQuick.Effects

ShellRoot {
    id: root

    // Must match vendibar-pro's center notch.
    readonly property int notchW: 236
    readonly property int notchH: 38
    readonly property int notchR: 15
    readonly property color notchColor: "#0b0b12"
    readonly property int circleD: 210

    property color accent: "#cba6f7"
    property string wallpaper: ""
    property string password: ""
    property bool authenticating: false
    property bool failed: false
    property bool unlocking: false

    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/theme-state"
        onLoaded: {
            const m = /ACCENT_HEX=([0-9a-fA-F]{6})/.exec(text());
            if (m) root.accent = "#" + m[1];
        }
    }
    FileView {
        path: Quickshell.env("HOME") + "/.config/vendi/wallpaper"
        onLoaded: root.wallpaper = text().trim()
    }

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

    Timer { id: quitTimer; interval: 150; onTriggered: Qt.quit() }
    Timer { id: dockTimer; interval: 60;  onTriggered: lock.locked = false }

    WlSessionLock {
        id: lock
        locked: true
        onLockedChanged: if (!locked) quitTimer.start()

        WlSessionLockSurface {
            id: surf
            color: "#11111b"

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

            // ── backdrop: blurred wallpaper, never darkened ────────────────
            Image {
                id: wall
                anchors.fill: parent
                source: root.wallpaper ? "file://" + root.wallpaper : ""
                fillMode: Image.PreserveAspectCrop
                visible: root.wallpaper !== ""
            }
            MultiEffect {
                id: blurFx
                anchors.fill: parent
                source: wall
                visible: root.wallpaper !== ""
                blurEnabled: true
                blurMax: 48
                blur: 0
            }

            // ── the blob ───────────────────────────────────────────────────
            Item {
                id: blob
                x: surf.notchX
                y: -root.notchR
                width: root.notchW
                height: root.notchH + root.notchR
                property real shapeRadius: root.notchR
                // 0 → docked notch shape, 1 → full blob (drives satellites/text).
                property real blobness: 0

                transform: [
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
                    color: root.notchColor
                }
                // Satellites: same-color circles drifting around the center —
                // their union with the body makes the outline undulate.
                Item {
                    id: sats
                    anchors.fill: parent
                    opacity: blob.blobness
                    // Slow orbit keeps the silhouette moving forever.
                    RotationAnimation on rotation {
                        from: 0; to: 360; duration: 11000
                        loops: Animation.Infinite
                        running: blob.blobness > 0.2 && !root.unlocking
                    }
                    // Breathing offset so the bulges swell and sink.
                    property real wob: 0
                    SequentialAnimation on wob {
                        loops: Animation.Infinite
                        running: blob.blobness > 0.2 && !root.unlocking
                        NumberAnimation { to: 1; duration: 1700; easing.type: Easing.InOutSine }
                        NumberAnimation { to: 0; duration: 1700; easing.type: Easing.InOutSine }
                    }
                    Repeater {
                        model: [
                            { ang: 0.0,  rad: 0.42, off: 0.105 },
                            { ang: 2.1,  rad: 0.38, off: 0.140 },
                            { ang: 4.2,  rad: 0.40, off: 0.120 },
                        ]
                        Rectangle {
                            property real bulge: modelData.off + sats.wob * 0.035
                            width: blob.width * modelData.rad * 2
                            height: width
                            radius: width / 2
                            color: body.color   // follows the accent wash on unlock
                            x: blob.width / 2 + Math.cos(modelData.ang) * blob.width * bulge - width / 2
                            y: blob.height / 2 + Math.sin(modelData.ang) * blob.height * bulge - height / 2
                        }
                    }
                }
                // Idle breathing of the whole body — alive even when still.
                SequentialAnimation {
                    loops: Animation.Infinite
                    running: blob.blobness > 0.9 && !root.authenticating && !root.unlocking
                    alwaysRunToEnd: true
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "xScale"; to: 1.015; duration: 1900; easing.type: Easing.InOutSine }
                        NumberAnimation { target: stretch; property: "yScale"; to: 0.985; duration: 1900; easing.type: Easing.InOutSine }
                    }
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "xScale"; to: 0.985; duration: 1900; easing.type: Easing.InOutSine }
                        NumberAnimation { target: stretch; property: "yScale"; to: 1.015; duration: 1900; easing.type: Easing.InOutSine }
                    }
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "xScale"; to: 1.0; duration: 1900; easing.type: Easing.InOutSine }
                        NumberAnimation { target: stretch; property: "yScale"; to: 1.0; duration: 1900; easing.type: Easing.InOutSine }
                    }
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

            // ── unlock, act two: the notch slides back in from above ───────
            Rectangle {
                id: topNotch
                x: surf.notchX
                y: -(root.notchH + root.notchR + 24)   // parked off-screen
                width: root.notchW
                height: root.notchH + root.notchR
                radius: root.notchR
                color: root.notchColor
            }

            // ── choreography ───────────────────────────────────────────────
            // Lock-in: anticipation stretch, then detach + travel with
            // squash & stretch; blur rises underneath.
            SequentialAnimation {
                id: detachAnim
                // Anticipation: the notch swells downward first.
                NumberAnimation { target: blob; property: "height"; to: root.notchH + root.notchR + 12; duration: 130; easing.type: Easing.OutQuad }
                ParallelAnimation {
                    // Travel + morph.
                    NumberAnimation { target: blob; property: "y"; to: surf.circleY; duration: 760; easing.type: Easing.OutBack; easing.overshoot: 0.7 }
                    NumberAnimation { target: blob; property: "x"; to: surf.circleX; duration: 640; easing.type: Easing.InOutCubic }
                    NumberAnimation { target: blob; property: "width";  to: root.circleD; duration: 640; easing.type: Easing.InOutCubic }
                    NumberAnimation { target: blob; property: "height"; to: root.circleD; duration: 640; easing.type: Easing.InOutCubic }
                    NumberAnimation { target: blob; property: "shapeRadius"; to: root.circleD / 2; duration: 640; easing.type: Easing.InOutCubic }
                    NumberAnimation { target: blurFx; property: "blur"; to: 1; duration: 700; easing.type: Easing.InOutCubic }
                    NumberAnimation { target: blob; property: "blobness"; to: 1; duration: 760; easing.type: Easing.InCubic }
                    // Squash & stretch while falling.
                    SequentialAnimation {
                        ParallelAnimation {
                            NumberAnimation { target: stretch; property: "yScale"; to: 1.14; duration: 260; easing.type: Easing.OutQuad }
                            NumberAnimation { target: stretch; property: "xScale"; to: 0.90; duration: 260; easing.type: Easing.OutQuad }
                        }
                        ParallelAnimation {
                            NumberAnimation { target: stretch; property: "yScale"; to: 1.0; duration: 420; easing.type: Easing.OutBack; easing.overshoot: 2.2 }
                            NumberAnimation { target: stretch; property: "xScale"; to: 1.0; duration: 420; easing.type: Easing.OutBack; easing.overshoot: 2.2 }
                        }
                    }
                }
            }

            // Unlock, act one: accent wash, dip, then fly off the top while
            // shrinking; the blur melts at the same time.
            SequentialAnimation {
                id: flyupAnim
                // Color + anticipation dip.
                ParallelAnimation {
                    ColorAnimation { target: body; property: "color"; to: root.accent; duration: 260 }
                    // Satellites collapse fast so the wash is one clean shape.
                    NumberAnimation { target: blob; property: "blobness"; to: 0; duration: 170 }
                    NumberAnimation { target: blob; property: "y"; to: surf.circleY + 26; duration: 200; easing.type: Easing.OutQuad }
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "yScale"; to: 0.88; duration: 200; easing.type: Easing.OutQuad }
                        NumberAnimation { target: stretch; property: "xScale"; to: 1.10; duration: 200; easing.type: Easing.OutQuad }
                    }
                }
                // Launch.
                ParallelAnimation {
                    NumberAnimation { target: blob; property: "y"; to: -root.circleD - 80; duration: 480; easing.type: Easing.InCubic }
                    NumberAnimation { target: blob; property: "scale"; to: 0.72; duration: 480; easing.type: Easing.InCubic }
                    ParallelAnimation {
                        NumberAnimation { target: stretch; property: "yScale"; to: 1.18; duration: 300; easing.type: Easing.InQuad }
                        NumberAnimation { target: stretch; property: "xScale"; to: 0.88; duration: 300; easing.type: Easing.InQuad }
                    }
                    NumberAnimation { target: blurFx; property: "blur"; to: 0; duration: 620; easing.type: Easing.InOutCubic }
                }
                ScriptAction { script: notchInAnim.start() }
            }

            // Unlock, act two: the notch glides down from off-screen and docks.
            SequentialAnimation {
                id: notchInAnim
                NumberAnimation {
                    target: topNotch; property: "y"
                    to: -root.notchR
                    duration: 380
                    easing.type: Easing.OutCubic
                }
                ScriptAction { script: dockTimer.start() }
            }

            Connections {
                target: root
                function onUnlockingChanged() { if (root.unlocking) flyupAnim.start(); }
            }

            Component.onCompleted: detachAnim.start()
        }
    }
}
