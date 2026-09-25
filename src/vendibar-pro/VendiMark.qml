// The vendiOS mark: a flat diamond in the theme accent (80% wide, 92% tall,
// the same shape as the website, the icon, fastfetch and the boot splash).
import QtQuick

Canvas {
    id: mark
    property color accent: "#cba6f7"
    implicitWidth: 18
    implicitHeight: 18
    onAccentChanged: requestPaint()
    onWidthChanged: requestPaint()
    onHeightChanged: requestPaint()
    onPaint: {
        const ctx = getContext("2d");
        const w = width, h = height;
        ctx.reset();
        ctx.fillStyle = accent;
        ctx.beginPath();
        ctx.moveTo(w * 0.50, h * 0.04);
        ctx.lineTo(w * 0.90, h * 0.50);
        ctx.lineTo(w * 0.50, h * 0.96);
        ctx.lineTo(w * 0.10, h * 0.50);
        ctx.closePath();
        ctx.fill();
    }
}
