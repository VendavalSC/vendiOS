// LoginPage — the onboarding overlay shown until the daemon has a session. Sign
// in to an existing vendiMessage account or create a new one (open sign-up).
// Premium + minimal, matching the app: accent button, soft fields, a quiet toggle.

import QtQuick
import QtQuick.Controls.Basic

Item {
    id: lp
    property var theme
    property bool busy: false
    property string errorText: ""
    property bool register: false   // false = sign in, true = create account
    signal submit(string user, string password, bool isRegister)

    function go() {
        var u = userField.text.replace(/\s+/g, "");
        var p = passField.text;
        if (!u.length || !p.length) { lp.errorText = "Enter a username and password."; return; }
        lp.errorText = "";
        lp.busy = true;
        lp.submit(u, p, lp.register);
    }

    Rectangle { anchors.fill: parent; color: theme.windowBg }

    Column {
        anchors.centerIn: parent
        width: 320
        spacing: 18

        // brand
        Column {
            width: parent.width; spacing: 6
            Rectangle {
                width: 64; height: 64; radius: 20
                anchors.horizontalCenter: parent.horizontalCenter
                gradient: Gradient {
                    GradientStop { position: 0.0; color: theme.accent }
                    GradientStop { position: 1.0; color: theme.accent2 }
                }
                Text { anchors.centerIn: parent; text: "✦"; color: "white"; font.pixelSize: 30 }
            }
            Text {
                text: "vendiMessage"; color: theme.textPrimary
                font.pixelSize: 24; font.weight: Font.Bold; font.family: theme.ui
                anchors.horizontalCenter: parent.horizontalCenter
            }
            Text {
                text: lp.register ? "Create your account" : "Sign in to continue"
                color: theme.textSecondary; font.pixelSize: 14; font.family: theme.ui
                anchors.horizontalCenter: parent.horizontalCenter
            }
        }

        // fields
        Column {
            width: parent.width; spacing: 10
            component Field: Rectangle {
                property alias text: input.text
                property string placeholder: ""
                property bool secret: false
                width: parent.width; height: 46; radius: 12
                color: theme.inputBg
                border.width: 1
                border.color: input.activeFocus ? Qt.rgba(theme.accent.r, theme.accent.g, theme.accent.b, 0.6)
                                                : "transparent"
                Behavior on border.color { ColorAnimation { duration: 150 } }
                TextField {
                    id: input
                    anchors.fill: parent
                    anchors.leftMargin: 14; anchors.rightMargin: 14
                    verticalAlignment: TextInput.AlignVCenter
                    placeholderText: parent.placeholder
                    placeholderTextColor: theme.textSecondary
                    color: theme.textPrimary
                    font.pixelSize: 15; font.family: theme.ui
                    echoMode: parent.secret ? TextInput.Password : TextInput.Normal
                    background: null
                    Keys.onReturnPressed: lp.go()
                }
            }
            Field { id: userField; placeholder: "Username" }
            Field { id: passField; placeholder: "Password"; secret: true }
        }

        // error
        Text {
            width: parent.width
            visible: lp.errorText.length > 0
            text: lp.errorText; color: "#e5534b"
            font.pixelSize: 13; font.family: theme.ui; wrapMode: Text.Wrap
            horizontalAlignment: Text.AlignHCenter
        }

        // primary button
        Rectangle {
            width: parent.width; height: 46; radius: 12
            gradient: Gradient {
                GradientStop { position: 0.0; color: theme.accent }
                GradientStop { position: 1.0; color: theme.accent2 }
            }
            opacity: lp.busy ? 0.6 : 1.0
            scale: btnTap.pressed ? 0.98 : 1.0
            Behavior on scale { NumberAnimation { duration: 110; easing.type: Easing.OutQuad } }
            Text {
                anchors.centerIn: parent
                text: lp.busy ? "Please wait…" : (lp.register ? "Create account" : "Sign in")
                color: "white"; font.pixelSize: 16; font.weight: Font.DemiBold; font.family: theme.ui
            }
            TapHandler { id: btnTap; enabled: !lp.busy; onTapped: lp.go() }
        }

        // toggle
        Row {
            anchors.horizontalCenter: parent.horizontalCenter
            spacing: 6
            Text {
                text: lp.register ? "Already have an account?" : "New to vendiMessage?"
                color: theme.textSecondary; font.pixelSize: 13; font.family: theme.ui
            }
            Text {
                text: lp.register ? "Sign in" : "Create one"
                color: theme.accent; font.pixelSize: 13; font.weight: Font.DemiBold; font.family: theme.ui
                TapHandler { onTapped: { lp.register = !lp.register; lp.errorText = ""; } }
            }
        }
    }

    // clear the busy state when an error comes back
    onErrorTextChanged: if (errorText.length) busy = false
}
