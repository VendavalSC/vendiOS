// Shared state for the dashboard's Tools tab — one instance on the shell root,
// so every screen's dashboard shows the same focus timer, note and calculator
// (and a finished focus session notifies once, not once per monitor).
// It lives outside the dashboard, so nothing stops when the island closes.
import Quickshell
import Quickshell.Io
import QtQuick

Scope {
    id: ts

    // ── tools (the Tools tab) ────────────────────────────────────────────────
    // Every tool's state lives out here on the dashboard, not inside its page,
    // so switching tools — or closing the island — never stops the focus timer
    // or drops a half-typed note or calculation.

    // Sticky notes: rounded cards you drop anywhere on the Notes board.
    // A ListModel (not a JS array) so editing one note updates it in place —
    // the board never rebuilds under your cursor. Saved to notes.json a beat
    // after the last change. The old single notepad (notes.md) becomes the
    // first sticky on the first run.
    readonly property var noteColors: ["#f9e2af", "#f5c2e7", "#a6e3a1", "#89dceb", "#cba6f7", "#fab387"]
    property bool notesLoaded: false
    property int noteSeq: 0
    ListModel { id: notesModel }
    readonly property alias notes: notesModel
    FileView {
        id: notesFile
        path: Quickshell.env("HOME") + "/.config/vendi/notes.json"
        printErrors: false
        onLoaded: {
            try {
                for (const n of JSON.parse(text()) || []) ts.noteAppend(n);
            } catch (e) {}
            ts.notesLoaded = true;
        }
        onLoadFailed: ts.wantLegacy = true
    }
    // only looked at when there's no notes.json yet
    property bool wantLegacy: false
    FileView {
        id: legacyNotes
        path: ts.wantLegacy ? Quickshell.env("HOME") + "/.config/vendi/notes.md" : ""
        printErrors: false
        onLoaded: {
            const txt = text().trim();
            if (txt !== "") ts.noteAppend({ text: txt, x: 24, y: 24 });
            ts.notesLoaded = true;
            ts.noteSaveSoon();
        }
        onLoadFailed: ts.notesLoaded = true
    }
    Timer {
        id: noteSave
        interval: 400
        onTriggered: {
            const out = [];
            for (let i = 0; i < notesModel.count; i++) {
                const n = notesModel.get(i);
                out.push({ id: n.nid, text: n.text, color: n.color, x: Math.round(n.x),
                           y: Math.round(n.y), rot: Math.round(n.rot * 100) / 100, z: n.z });
            }
            notesFile.setText(JSON.stringify(out, null, 1));
        }
    }
    function noteSaveSoon() { if (notesLoaded) noteSave.restart(); }
    function noteAppend(n) {
        noteSeq = Math.max(noteSeq, (n.id ?? 0) + 1, notesModel.count + 1);
        notesModel.append({
            nid: n.id ?? noteSeq,
            text: n.text ?? "",
            color: n.color ?? noteColors[notesModel.count % noteColors.length],
            x: n.x ?? 24, y: n.y ?? 24,
            // a slight tilt so the board reads like paper, not a grid
            rot: n.rot ?? (Math.random() * 4 - 2),
            z: n.z ?? notesModel.count,
            fresh: n.fresh ?? false,
        });
    }
    function noteIndex(nid) {
        for (let i = 0; i < notesModel.count; i++) if (notesModel.get(i).nid === nid) return i;
        return -1;
    }
    // New note at (x, y) in board coords; returns its id (the board focuses it).
    function noteAdd(x, y) {
        const nid = noteSeq + 1;
        let top = 0;
        for (let i = 0; i < notesModel.count; i++) top = Math.max(top, notesModel.get(i).z);
        noteAppend({ id: nid, x: x, y: y, z: top + 1, fresh: true,
                     color: noteColors[(nid - 1) % noteColors.length] });
        noteSaveSoon();
        return nid;
    }
    function noteSet(nid, key, value) {
        const i = noteIndex(nid);
        if (i < 0 || notesModel.get(i)[key] === value) return;
        notesModel.setProperty(i, key, value);
        noteSaveSoon();
    }
    function noteRaise(nid) {
        let top = 0;
        for (let i = 0; i < notesModel.count; i++) top = Math.max(top, notesModel.get(i).z);
        const i = noteIndex(nid);
        if (i >= 0 && notesModel.get(i).z < top) noteSet(nid, "z", top + 1);
    }
    function noteRemove(nid) {
        const i = noteIndex(nid);
        if (i >= 0) { notesModel.remove(i); noteSaveSoon(); }
    }

    // Calculator: a tiny recursive-descent parser (no eval of what you typed).
    // + - × ÷ % ^, parentheses, implicit ×, sqrt/sin/cos/tan/ln/log/abs, pi, e,
    // and `ans` for the last result.
    property string calcExpr: ""
    property var calcHistory: []           // [{e, r}] newest first
    property real calcAns: 0
    function calcEval(src) {
        const s = src.replace(/×/g, "*").replace(/÷/g, "/").replace(/−/g, "-").replace(/\s+/g, "");
        let i = 0;
        const peek = () => s[i];
        const fns = { sqrt: Math.sqrt, sin: Math.sin, cos: Math.cos, tan: Math.tan,
                      ln: Math.log, log: Math.log10, abs: Math.abs };
        function num() {
            const m = /^(\d+\.?\d*|\.\d+)(e[+-]?\d+)?/i.exec(s.slice(i));
            if (!m) throw "syntax";
            i += m[0].length;
            return parseFloat(m[0]);
        }
        function atom() {
            if (peek() === "(") {
                i++;
                const v = expr();
                if (peek() === ")") i++;         // forgive a missing close paren
                return v;
            }
            const w = /^[a-z]+/i.exec(s.slice(i));
            if (w) {
                const n = w[0].toLowerCase();
                i += w[0].length;
                if (n === "pi") return Math.PI;
                if (n === "e") return Math.E;
                if (n === "ans") return ts.calcAns;
                if (fns[n]) return fns[n](unary());
                throw "syntax";
            }
            return num();
        }
        function postfix() {
            let v = atom();
            while (peek() === "%") { i++; v /= 100; }
            return v;
        }
        function power() {
            const b = postfix();
            if (peek() === "^") { i++; return Math.pow(b, unary()); }
            return b;
        }
        function unary() {
            if (peek() === "-") { i++; return -unary(); }
            if (peek() === "+") { i++; return unary(); }
            return power();
        }
        function term() {
            let v = unary();
            for (;;) {
                const c = peek();
                if (c === "*") { i++; v *= unary(); }
                else if (c === "/") { i++; v /= unary(); }
                else if (c === "(" || (c && /[a-z\d.]/i.test(c))) v *= unary();   // 2pi, 3(4)
                else return v;
            }
        }
        function expr() {
            let v = term();
            for (;;) {
                if (peek() === "+") { i++; v += term(); }
                else if (peek() === "-") { i++; v -= term(); }
                else return v;
            }
        }
        if (s === "") return NaN;
        const v = expr();
        if (i < s.length) throw "syntax";
        return v;
    }
    function calcFmt(v) {
        if (!isFinite(v)) return isNaN(v) ? "—" : (v > 0 ? "∞" : "−∞");
        if (Math.abs(v) >= 1e15 || (v !== 0 && Math.abs(v) < 1e-9)) return v.toExponential(6);
        return String(parseFloat(v.toPrecision(12)));
    }
    readonly property string calcPreview: {
        try { const v = calcEval(calcExpr); return isNaN(v) ? "" : calcFmt(v); }
        catch (e) { return ""; }
    }
    function calcPress(k) {
        if (k === "C") { calcExpr = ""; return; }
        if (k === "⌫") { calcExpr = calcExpr.slice(0, -1); return; }
        if (k === "=") {
            let v;
            try { v = calcEval(calcExpr); } catch (e) { return; }
            if (isNaN(v)) return;
            calcHistory = [{ e: calcExpr, r: calcFmt(v) }].concat(calcHistory).slice(0, 30);
            calcAns = v;
            calcExpr = calcFmt(v);
            return;
        }
        calcExpr += k;
    }

    // Focus (pomodoro). Wall-clock based: the countdown is `endAt - now`, so it
    // stays exact no matter what the island is doing.
    // Durations and today's tally persist in ~/.config/vendi/focus.json.
    FileView {
        id: focusFile
        path: Quickshell.env("HOME") + "/.config/vendi/focus.json"
        printErrors: false
        onLoaded: {
            ts.focusLeft = ts.focusPhaseLen;
            ts.focusRollDay();
        }
        onLoadFailed: error => { if (error === FileViewError.FileNotFound) writeAdapter(); }
        JsonAdapter {
            id: fz
            property int focusMin: 25
            property int shortMin: 5
            property int longMin: 15
            property string day: ""
            property int sessions: 0
            property int minutes: 0
        }
    }
    readonly property int focusMin: fz.focusMin
    readonly property int shortMin: fz.shortMin
    readonly property int longMin: fz.longMin
    readonly property int focusDoneToday: fz.sessions
    readonly property int focusMinToday: fz.minutes
    function focusRollDay() {
        const today = Qt.formatDate(new Date(), "yyyy-MM-dd");
        if (fz.day !== today) {
            fz.day = today;
            fz.sessions = 0;
            fz.minutes = 0;
            focusFile.writeAdapter();
        }
    }
    property string focusPhase: "focus"    // focus · short · long
    property int focusRound: 0             // focus sessions finished this cycle (0-3)
    property bool focusRunning: false
    property real focusEndAt: 0
    property int focusLeft: focusMin * 60  // seconds left (live while running)
    readonly property int focusPhaseLen: (focusPhase === "focus" ? focusMin
                                        : focusPhase === "short" ? shortMin : longMin) * 60
    readonly property string focusClock: {
        const s = Math.max(0, focusLeft);
        return String(Math.floor(s / 60)).padStart(2, "0") + ":" + String(s % 60).padStart(2, "0");
    }
    readonly property string focusLabel: focusPhase === "focus" ? "Focus"
                                       : focusPhase === "short" ? "Short break" : "Long break"
    readonly property bool focusActive: focusRunning || focusLeft !== focusPhaseLen
    Timer {
        interval: 250; repeat: true
        running: ts.focusRunning
        onTriggered: {
            ts.focusLeft = Math.ceil((ts.focusEndAt - Date.now()) / 1000);
            if (ts.focusLeft <= 0) ts.focusAdvance(true);
        }
    }
    function focusToggle() {
        if (focusRunning) {
            focusRunning = false;
        } else {
            focusEndAt = Date.now() + focusLeft * 1000;
            focusRunning = true;
        }
    }
    function focusReset() {
        focusRunning = false;
        focusLeft = focusPhaseLen;
    }
    function focusSetPhase(p) {
        focusPhase = p;
        focusLeft = focusPhaseLen;
        if (focusRunning) focusEndAt = Date.now() + focusLeft * 1000;
    }
    // Next phase. A finished phase announces itself and the next one starts on
    // its own; skipping moves on silently and keeps the current run state.
    function focusAdvance(finished) {
        let next;
        if (focusPhase === "focus") {
            if (finished) {
                focusRollDay();
                fz.sessions += 1;
                fz.minutes += focusMin;
                focusFile.writeAdapter();
            }
            focusRound = (focusRound + 1) % 4;
            next = focusRound === 0 ? "long" : "short";
        } else next = "focus";
        if (finished) {
            const msg = next === "focus" ? ["Break's over", "Back to it — " + focusMin + " minutes of focus."]
                      : next === "long"  ? ["Four down!", "Take a long break — " + longMin + " minutes."]
                                         : ["Focus session done", "Take " + shortMin + " — you earned it."];
            Quickshell.execDetached(["notify-send", "-a", "Focus", "-i", "alarm-symbolic", msg[0], msg[1]]);
            Quickshell.execDetached(["sh", "-c",
                "f=/usr/share/sounds/freedesktop/stereo/complete.oga; [ -f \"$f\" ] && pw-play \"$f\" 2>/dev/null"]);
            focusRunning = true;
        }
        focusSetPhase(next);
    }
    function focusAdjust(which, d) {
        const clamp = v => Math.max(1, Math.min(120, v));
        if (which === "focus") fz.focusMin = clamp(fz.focusMin + d);
        else if (which === "short") fz.shortMin = clamp(fz.shortMin + d);
        else fz.longMin = clamp(fz.longMin + d);
        focusFile.writeAdapter();
        if (!focusRunning && which === focusPhase) focusLeft = focusPhaseLen;
    }
}
