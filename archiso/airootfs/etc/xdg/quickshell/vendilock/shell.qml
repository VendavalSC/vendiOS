// vendilock — the vendiOS lock screen (quickshell + ext-session-lock).
//
// swaylock's idea, but alive: one centered ring over the blurred wallpaper.
// Typing sweeps an accent arc around the ring (and pulses it), auth spins
// it, failure flashes red and shakes, success blooms the ring open and
// fades everything out before releasing the lock.
//
// Launched by `vendi-ctl lock`. Accent follows ~/.config/vendi/theme-state.

import Quickshell
import Quickshell.Io
import Quickshell.Wayland
import Quickshell.Services.Pam
import QtQuick
import QtQuick.Shapes
import QtQuick.Effects

ShellRoot {
    id: root

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
                root.unlocking = true;        // bloom plays, then unlockTimer fires
                unlockTimer.start();
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

    // Let the bloom-out finish before releasing the lock…
    Timer {
        id: unlockTimer
        interval: 450
        onTriggered: lock.locked = false
    }
    // …and give the unlock request a beat to flush before exiting.
    Timer {
        id: quitTimer
        interval: 150
        onTriggered: Qt.quit()
    }

    WlSessionLock {
        id: lock
        locked: true
        onLockedChanged: if (!locked) quitTimer.start()

        WlSessionLockSurface {
            id: surf
            color: "#11111b"

            // ── key handling: a focused sink, no text field ────────────────
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
                        ring.pulse();
                    }
                    event.accepted = true;
                }
            }

            // ── backdrop: blurred wallpaper, fades in on lock ──────────────
            Image {
                id: wall
                anchors.fill: parent
                source: root.wallpaper ? "file://" + root.wallpaper : ""
                fillMode: Image.PreserveAspectCrop
                visible: false
            }
            MultiEffect {
                anchors.fill: parent
                source: wall
                visible: root.wallpaper !== ""
                blurEnabled: true
                blur: 0.85
                blurMax: 48
                // Lock-in: ease from sharp-ish to settled; unlock fades away.
                opacity: root.unlocking ? 0 : 1
                scale: root.unlocking ? 1.05 : 1.0
                Behavior on opacity { NumberAnimation { duration: 420; easing.type: Easing.InCubic } }
                Behavior on scale   { NumberAnimation { duration: 500; easing.type: Easing.InCubic } }
            }
            Rectangle {
                id: scrim
                anchors.fill: parent
                color: "#11111b"
                opacity: 0
                Component.onCompleted: opacity = root.wallpaper ? 0.52 : 0.96
                Behavior on opacity { NumberAnimation { duration: 600; easing.type: Easing.OutCubic } }
            }

            // ── the ring ───────────────────────────────────────────────────
            Item {
                id: ring
                anchors.centerIn: parent
                width: 200; height: 200

                function pulse() { pop.restart(); }

                // Entry: scale in with overshoot. Unlock: bloom open + fade.
                scale: 0.82
                opacity: 0
                Component.onCompleted: { scale = 1.0; opacity = 1.0; }
                Behavior on scale {
                    enabled: !root.unlocking
                    NumberAnimation { duration: 600; easing.type: Easing.OutBack; easing.overshoot: 1.4 }
                }
                Behavior on opacity { NumberAnimation { duration: 450; easing.type: Easing.OutCubic } }

                // Unlock bloom: expand + vanish.
                states: State {
                    name: "unlocked"; when: root.unlocking
                    PropertyChanges { target: ring; scale: 1.35; opacity: 0 }
                }
                transitions: Transition {
                    to: "unlocked"
                    NumberAnimation { properties: "scale,opacity"; duration: 450; easing.type: Easing.InCubic }
                }

                // Keystroke pop.
                SequentialAnimation {
                    id: pop
                    NumberAnimation { target: ring; property: "scale"; to: 1.045; duration: 70; easing.type: Easing.OutQuad }
                    NumberAnimation { target: ring; property: "scale"; to: 1.0;   duration: 160; easing.type: Easing.OutQuad }
                }

                // Fail shake.
                transform: Translate { id: shake; x: 0 }
                SequentialAnimation {
                    running: root.failed
                    NumberAnimation { target: shake; property: "x"; to: -14; duration: 45 }
                    NumberAnimation { target: shake; property: "x"; to: 11;  duration: 45 }
                    NumberAnimation { target: shake; property: "x"; to: -7;  duration: 45 }
                    NumberAnimation { target: shake; property: "x"; to: 4;   duration: 45 }
                    NumberAnimation { target: shake; property: "x"; to: 0;   duration: 45 }
                }

                // Base ring: faint, breathing while idle.
                Shape {
                    anchors.fill: parent
                    preferredRendererType: Shape.CurveRenderer
                    ShapePath {
                        strokeWidth: 3
                        strokeColor: root.failed ? "#f38ba8" : Qt.rgba(1, 1, 1, 0.16)
                        fillColor: "transparent"
                        capStyle: ShapePath.RoundCap
                        PathAngleArc {
                            centerX: 100; centerY: 100
                            radiusX: 96;  radiusY: 96
                            startAngle: 0; sweepAngle: 360
                        }
                        Behavior on strokeColor { ColorAnimation { duration: 200 } }
                    }
                    SequentialAnimation on opacity {
                        running: !root.authenticating && !root.unlocking
                        loops: Animation.Infinite
                        NumberAnimation { to: 0.55; duration: 2200; easing.type: Easing.InOutSine }
                        NumberAnimation { to: 1.0;  duration: 2200; easing.type: Easing.InOutSine }
                    }
                }

                // Typing arc: sweeps with the password length, springs back on clear.
                Shape {
                    anchors.fill: parent
                    preferredRendererType: Shape.CurveRenderer
                    ShapePath {
                        strokeWidth: 3
                        strokeColor: root.failed ? "#f38ba8" : root.accent
                        fillColor: "transparent"
                        capStyle: ShapePath.RoundCap
                        PathAngleArc {
                            centerX: 100; centerY: 100
                            radiusX: 96;  radiusY: 96
                            startAngle: -90
                            sweepAngle: root.unlocking ? 360 : Math.min(root.password.length * 16, 352)
                            Behavior on sweepAngle {
                                NumberAnimation { duration: 260; easing.type: Easing.OutCubic }
                            }
                        }
                        Behavior on strokeColor { ColorAnimation { duration: 200 } }
                    }
                }

                // Auth spinner: a short accent arc orbiting the ring.
                Shape {
                    anchors.fill: parent
                    preferredRendererType: Shape.CurveRenderer
                    visible: root.authenticating
                    ShapePath {
                        strokeWidth: 3
                        strokeColor: root.accent
                        fillColor: "transparent"
                        capStyle: ShapePath.RoundCap
                        PathAngleArc {
                            centerX: 100; centerY: 100
                            radiusX: 96;  radiusY: 96
                            startAngle: 0; sweepAngle: 80
                        }
                    }
                    RotationAnimation on rotation {
                        running: root.authenticating
                        loops: Animation.Infinite
                        from: 0; to: 360
                        duration: 900
                    }
                }

                // Clock inside the ring.
                Column {
                    anchors.centerIn: parent
                    spacing: 2
                    Text {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: Qt.formatDateTime(sysClock.date, "HH:mm")
                        color: "#f2f2f7"
                        font.family: "JetBrainsMonoNL Nerd Font"
                        font.pixelSize: 44
                        font.weight: Font.Light
                        font.letterSpacing: 2
                    }
                    Text {
                        anchors.horizontalCenter: parent.horizontalCenter
                        text: Qt.formatDateTime(sysClock.date, "ddd d MMM")
                        color: Qt.rgba(1, 1, 1, 0.55)
                        font.family: "JetBrainsMonoNL Nerd Font"
                        font.pixelSize: 13
                        font.letterSpacing: 1
                    }
                }
            }

            // ── status line under the ring ─────────────────────────────────
            Text {
                anchors.horizontalCenter: parent.horizontalCenter
                anchors.top: ring.bottom
                anchors.topMargin: 28
                text: root.failed ? "wrong password"
                    : root.authenticating ? ""
                    : root.password.length > 0 ? "•".repeat(Math.min(root.password.length, 24))
                    : "enter password"
                color: root.failed ? "#f38ba8" : Qt.rgba(1, 1, 1, 0.45)
                font.family: "JetBrainsMonoNL Nerd Font"
                font.pixelSize: 13
                font.letterSpacing: 3
                opacity: root.unlocking ? 0 : 1
                Behavior on opacity { NumberAnimation { duration: 300 } }
            }
        }
    }
}
