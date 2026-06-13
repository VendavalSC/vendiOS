// The vendi mark — same obsidian crystal shard the classic bar draws:
// a tall slanted gem, bright left face, deep right face, hairline ridge.
// Faces derive from the theme accent so it keeps the obsidian look anywhere.
import QtQuick

Canvas {
    id: mark
    property color accent: "#cba6f7"
    implicitWidth: 18
    implicitHeight: 18
    onAccentChanged: requestPaint()
    onWidthChanged: requestPaint()
    onPaint: {
        const ctx = getContext("2d");
        const w = width, h = height, a = accent;
        const light = Qt.rgba(a.r + (1 - a.r) * 0.18, a.g + (1 - a.g) * 0.18,
                              a.b + (1 - a.b) * 0.18, 1);
        const dark = Qt.rgba(a.r * 0.58, a.g * 0.58, a.b * 0.58, 1);
        // T top peak · R right shoulder · B bottom tip · L left hip;
        // the T→B ridge splits the shard into two faces.
        const t = [w * 0.42, h * 0.04], r = [w * 0.92, h * 0.30];
        const b = [w * 0.60, h * 0.97], l = [w * 0.10, h * 0.46];
        ctx.reset();
        const face = (pts, col) => {
            ctx.fillStyle = col;
            ctx.beginPath();
            ctx.moveTo(pts[0][0], pts[0][1]);
            for (let i = 1; i < pts.length; i++) ctx.lineTo(pts[i][0], pts[i][1]);
            ctx.closePath();
            ctx.fill();
        };
        face([t, b, l], light);
        face([t, r, b], dark);
        ctx.strokeStyle = "rgba(237, 224, 255, 0.85)";
        ctx.lineWidth = 0.9;
        ctx.beginPath();
        ctx.moveTo(t[0], t[1]);
        ctx.lineTo(b[0], b[1]);
        ctx.stroke();
    }
}
