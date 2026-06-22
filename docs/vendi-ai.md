# vendi AI — MVP spec

> Opt-in, **fully local**, system-aware assistant for vendiOS. Config-by-
> conversation + light assistant tasks, driven from a dedicated `super+a`
> surface that expands the dynamic island into a **Siri-grade conversation
> panel** — intended to be the single most beautiful thing in the quickshell
> setup.

Status: design only (2026-06-22). Nothing built. Box offline — no HW test yet.
Locked constraints: no cloud, no API keys, opt-in (not in base ISO), 8GB+ RAM to
install, model chosen by GPU/VRAM, actions permission-gated, **not for coding**.

---

## 0. Principles

1. **Local or nothing.** Model runs on-device (ollama). The only network calls
   are explicit *tools* (weather/time/search) — never the model.
2. **The base ISO stays lean.** Shipped via `vendi ai install`. Uninstall fully
   removes it. No idle daemon unless installed.
3. **Bounded power.** The model never gets a raw shell by default. It calls
   **structured tools**; mutating actions pass through a **permission card**.
4. **It knows your *system*, not your *life*.** Context = live system/config
   state injected into the prompt. No always-on logger, no personal-data graph.
5. **Gorgeous is a feature.** The panel is a hero surface and is allowed the lush
   Siri treatment that the always-on bar chrome is not (see §1.0).

---

## 1. UX / UI — the hero

### 1.0 Aesthetic reconciliation (important)

The bar rule is *minimal/macOS, no pills/glow/shimmer* (`feedback_bar_aesthetic`).
The AI panel is the **sanctioned exception**: a transient, summoned hero moment
that gets a full iridescent Siri glow — then **collapses back into the clean,
silent notch** so the always-on chrome stays minimal. Apple does exactly this:
restrained menubar, lavish Siri. Glow lives only while the panel is open.

NVIDIA note: the glow is *animated geometry inside a quickshell client surface*
(blurred blobs that orbit/scale/rotate + a rotating ring). Because quickshell
submits a fresh buffer per frame, it **presents fine on NVIDIA** — this is the
same path the notch spring already animates on. We are NOT animating a static
compositor element's shader uniform (the thing that froze in the wallpaper saga).

### 1.1 The surface

`super+a` morphs the **dynamic island → an "Ask vendi" panel**: the notch springs
open (reusing the island expand the user already loves) into a centered, top-
anchored glass sheet (~620×variable, grows with content, max ~70% screen height,
then scrolls). Dismiss: `Esc`, `super+a` again, or click-away → springs back to
the notch.

Anchored to the notch so it reads as "the island is talking," not a new window.

### 1.2 Visual language

**Glass.** Dark translucent sheet (palette `base` @ ~0.62 alpha), 28px corner
radius, 1px hairline inner stroke (`text` @ 0.08), soft drop shadow. Optional
backdrop blur (MultiEffect) if we wire layer-surface blur; not required for v1.

**The aurora (the Siri soul).** Behind the glass, 3–4 large soft color "blobs"
(palette accent + 2–3 harmonics pulled from `vendi theme dynamic`), each
heavily blurred (`MultiEffect { blurEnabled: true; blur: 1.0; blurMax: 64 }`),
slowly **orbiting / scaling / counter-rotating** on looping `NumberAnimation`s.
This is pure geometry motion → NVIDIA-safe. The blobs are clipped to the panel
(idle) or bled slightly past the rounded border to form the glow halo.

**The ring (the Siri "listening" signature).** A conic-gradient ring hugging the
panel border (`QtQuick.Shapes` ConicalGradient, or a pre-baked conic PNG),
**rotated** by an animation. Idle: faint, slow. Thinking: bright, fast. This
single rotating ring is the strongest "this is alive" cue and is 100% geometry.

**Typography.** Inter/SF-like, light weights, generous leading. Prompt text 15px;
answer 16px/1.5; everything `text`/`subtext` palette. Answers **stream token-by-
token**, each new line rising in (`y: 6→0`, `opacity: 0→1`, ~180ms gentle ease).

**Color = wallpaper palette.** The aurora + ring + accents read from the live
material-you palette, so the panel always harmonizes with the desktop and looks
custom on every wallpaper. Recolors live on wallpaper change.

### 1.3 States (each a distinct aurora behavior)

| State | Aurora | Ring | Notes |
|---|---|---|---|
| **Ready** | slow drift (8–12s loops), low sat | faint, ~static | input shows `Ask vendi…` |
| **Thinking** | speeds up (2–3s), blobs scale-pulse, brightens | bright, ~1.5s/rev | the "working" hero moment |
| **Responding** | calms, settles toward accent | gentle | tokens stream in |
| **Permission** | dims behind the card | paused | card slides up (spring), focus on it |
| **Done/idle** | back to slow drift | faint | awaits next prompt |
| **Error** | brief warm-red wash | quick flash + 4px shake | geometry shake, ~250ms |

### 1.4 Motion choreography (`super+a`)

1. **Expand:** notch `lw/cw/ch` spring to panel size — reuse the existing notch
   constants (`spring 7.4–9.6, damping 0.64–0.66, mass 0.70–0.82, epsilon 2.5`).
2. **Content in:** input field + aurora fade/rise (~160ms, after the geometry
   settles ~60%).
3. **Submit:** input collapses to a small chip at the top; aurora **ignites**
   → Thinking.
4. **Stream:** answer lines rise+fade in as tokens arrive.
5. **Permission (if a tool needs it):** a glass card springs up from the panel
   bottom; aurora dims; Allow/Deny.
6. **Collapse:** panel springs back into the notch; aurora fades to nothing.

### 1.5 Layout mockups

```
READY                                  THINKING
╭──────────────────────────────────╮   ╭──────────────────────────────────╮
│  ✦ vendi                          │   │  ✦ vendi      ⟳ (ring spinning)   │
│                                   │   │  ❝ make a theme called hello…❞    │
│   ╭────────────────────────────╮  │   │                                   │
│   │  Ask vendi…              ⏎ │  │   │      · · ·  (aurora pulsing)      │
│   ╰────────────────────────────╯  │   │      thinking…                    │
│        (slow aurora behind)       │   │                                   │
╰──────────────────────────────────╯   ╰──────────────────────────────────╯

RESPONDING                             PERMISSION CARD
╭──────────────────────────────────╮   ╭──────────────────────────────────╮
│  ✦ vendi                          │   │  ✦ vendi                          │
│  ❝ what's 18% of 2340 ❞           │   │  ❝ create theme hello ❞           │
│                                   │   │   ────────────────────────────    │
│  18% of 2,340 is **421.2**.       │   │   vendi will:                     │
│  ▌(streaming…)                    │   │   • write ~/.config/vendi/        │
│                                   │   │     themes/hello.kdl              │
│                                   │   │     bg #1e1e2e  accent #cba6f7    │
│                                   │   │   [ Deny ]        [ Allow ]  ◻ always│
╰──────────────────────────────────╯   ╰──────────────────────────────────╯
```

### 1.6 Voice? — not in MVP

Siri is voice; local STT (whisper.cpp) + TTS is a second heavy model + latency +
mic UX. **MVP is typed input.** Voice is a post-MVP add (note: whisper-small is
~0.5GB and CPU-slow; gate behind the same hardware tiers). The aurora is designed
to look just as alive for typed input.

---

## 2. Architecture

```
 super+a (vendiwm Action::ShowAi)
        │  bar IPC (same path as dashboard/control-center root signal)
        ▼
 quickshell panel (AiPanel.qml)  ◄──stream tokens / permission req──┐
        │  request (prompt) over unix socket                        │
        ▼                                                           │
 vendi-ai daemon (Rust, user service)                               │
        │  HTTP localhost:11434 (ollama, tool-calling, streaming)   │
        ▼                                                           │
   ollama  ──► local model (Qwen-class, size by HW)                 │
        │                                                           │
        ├─ tool call ──► execute (vendi verbs / launch / calc / web)┘
        └─ Tier1/2 tool ──► ask panel for permission, await Allow/Deny
```

- **`vendi-ai`** — new Rust crate in `src/` (matches vendiwm/vendi-ctl stack).
  Owns: prompt assembly, ollama HTTP (reqwest, NDJSON stream), the tool registry +
  executor, the permission protocol. Runs as a **user systemd service**, started
  on first `super+a` (socket-activated) so there's **no idle cost** until used.
- **`AiPanel.qml`** — the hero panel (its own quickshell component, loaded by
  shell.qml). Talks to `vendi-ai` over a unix socket; renders states/aurora/cards.
- **ollama** — model runtime; HTTP at `:11434`; supports tool-calling + streaming.

Why a daemon (not the bar calling ollama directly): keeps tool execution +
permissions in trusted Rust, keeps QML thin (UI only), and lets a `vendi ai chat`
CLI reuse the exact same brain.

---

## 3. Model selection (by hardware)

`vendi ai install` detects VRAM (nvidia-smi / sysfs / vulkaninfo), falls back to
RAM, and pulls one model:

| Tier | Detect | Model (Qwen-class instruct, Q4) | Approx |
|---|---|---|---|
| A | VRAM ≥ 16GB (user's 5060 Ti) | `qwen2.5:14b-instruct` | ~9GB |
| B | VRAM 8–12GB | `qwen2.5:7b-instruct` | ~5GB |
| C | no/weak GPU, RAM ≥ 8GB | `qwen2.5:3b-instruct` | ~2GB, CPU, slow |

Below 8GB RAM → install refuses with a clear message. Qwen chosen for **reliable
tool-calling at small sizes** (the whole game). `vendi ai model <name>` overrides.
Model tags are pinned in a small table in the installer so they can be bumped.

---

## 4. Tools (≤7 fat, grouped — never a flat sprawl)

Small models misfire with many thin tools, so each is one tool with an `action`
enum. JSON schemas handed to ollama:

```jsonc
// 1. system_control — the vendi verb surface (Tier 1, some Tier 2)
{ "name": "system_control",
  "description": "Change vendiOS appearance/system settings via the vendi CLI.",
  "parameters": { "type": "object", "properties": {
    "action": { "enum": ["set_theme","create_theme","set_wallpaper","set_bar_color",
                          "set_volume","set_brightness","night_light","wifi","bluetooth",
                          "power"] },
    "args": { "type": "object", "description":
              "action-specific, e.g. {name, colors:{bg,accent,...}} or {path} or {level}" }
  }, "required": ["action"] } }

// 2. read_config — read current state / config (Tier 0)
{ "name": "read_config",
  "description": "Read current vendiOS state or a config file.",
  "parameters": { "type":"object", "properties": {
    "what": { "enum": ["theme","wallpaper","bar","keybinds","layout","monitors",
                       "installed_apps","file"] },
    "path": { "type":"string", "description":"required only when what=file" }
  }, "required":["what"] } }

// 3. launch_app — open apps, optionally with a URL/args (Tier 1)
{ "name": "launch_app",
  "description": "Launch an application, optionally opening a URL. e.g. firefox + web.whatsapp.com",
  "parameters": { "type":"object", "properties": {
    "app": { "type":"string" },
    "url": { "type":"string" },
    "args":{ "type":"array","items":{"type":"string"} }
  }, "required":["app"] } }

// 4. files — read/search (Tier 0) / write/create (Tier 2)
{ "name":"files",
  "parameters": { "type":"object", "properties": {
    "action": { "enum":["read","search","write","create"] },
    "path": {"type":"string"}, "query":{"type":"string"}, "content":{"type":"string"}
  }, "required":["action"] } }

// 5. calc — precise math/units (NEVER trust model mental math)
{ "name":"calc",
  "parameters": { "type":"object",
    "properties": { "expr": {"type":"string"} }, "required":["expr"] } }
// backend reuses Launcher.qml calc() logic (ported to Rust or shelled).

// 6. web — keyless internet (Tier 0; network egress only here)
{ "name":"web",
  "parameters": { "type":"object", "properties": {
    "action": { "enum":["weather","time","search"] },
    "query":{"type":"string"}, "location":{"type":"string"}
  }, "required":["action"] } }
// weather=open-meteo (reuse vendi-weather), time=system+tz, search=DuckDuckGo lite (no key).

// 7. run_command — Tier-2 escape hatch, always a permission card
{ "name":"run_command",
  "parameters": { "type":"object",
    "properties": { "cmd":{"type":"string"}, "why":{"type":"string"} },
    "required":["cmd","why"] } }
```

Pure-model answers (explain, chat, general knowledge) use **no tool**.

**launch_app alias map** (tool checks first, model knowledge fallback):
`whatsapp→web.whatsapp.com, youtube→youtube.com, gmail→mail.google.com, …`
small curated table so common "open X" requests never resolve to a wrong URL.

---

## 5. Permission model (mirrors Claude Code modes)

| Tier | Examples | Behavior |
|---|---|---|
| **0 read** | read_config, files.read/search, calc, web | auto-run, no prompt |
| **1 reversible** | set_theme/wallpaper/bar, launch_app, volume/brightness | run, show a toast w/ **Undo** |
| **2 mutating** | files.write/create, run_command, set_theme that overwrites, power | **permission card**: exact command/diff in monospace, Allow/Deny, optional "always allow this action kind" |

Permission flow: daemon hits a Tier-1/2 tool → pauses → sends a `permission_request`
to the panel → panel renders the card → user choice returns → daemon executes or
skips and tells the model. "Always allow `<action>`" persists in
`~/.config/vendi/ai.kdl`.

---

## 6. Context assembly (system prompt)

Injected fresh each session (compact, ~true, cheap — no embeddings):
- Identity + the tool list + permission rules + "you control vendiOS; for config
  tasks call system_control; never invent shell."
- **Live state:** current theme name + palette, wallpaper path, bar config
  summary, active layout, monitors, list of installed GUI apps (`.desktop` names),
  date/time/locale/location.
- Anything deeper → the model calls `read_config` on demand.

Keeps the prompt small (matters for 3B/7B context) while staying accurate.

---

## 7. `vendi ai` CLI / install

```
vendi ai install     # RAM>=8 check → install ollama → detect GPU → pull model
                     #   → enable socket-activated vendi-ai.service → done
vendi ai status      # model, VRAM tier, daemon up?, last error
vendi ai model <n>   # override model
vendi ai chat        # terminal fallback to the same brain
vendi ai uninstall   # remove model + ollama + service + configs
```

Install pulls `ollama` (repo or AUR) + the tier model; **for the offline ISO
this is a post-install online step** (models are GBs — they can't ship in the
offline repo). That's fine: the feature is opt-in and online by nature of needing
a download once.

---

## 8. Keybind + compositor wiring

- vendiwm: new `Action::ShowAi` (input.rs/config.rs/run_action) → emits the bar
  IPC that the dashboard/control-center already use (root signal). Bind
  **`super+a`** (free; add to the Super+K cheat-sheet in Dash.qml).
- `AiPanel.qml`: `WlrLayershell.keyboardFocus` true while `aiOpen` (mirror the
  existing `searchOpen` focus toggle) so the input field types.

---

## 9. MVP cut + milestones

**M1 — brain (headless):** `vendi-ai` daemon + ollama + tools 1,2,3,5,6 +
`vendi ai chat`. Prove tool-calling on the 14B (NVIDIA box). No UI.
**M2 — hero panel:** AiPanel.qml — expand-from-notch, aurora + ring, streaming
answers, the 3 core states. Wire `super+a`.
**M3 — permissions:** Tier-1 toasts + Tier-2 cards; add tools 4 (files) + 7
(run_command).
**M4 — polish:** wallpaper-palette aurora, error/empty states, voice deferred.

Ship M1+M2 first — that already does "create a theme called hello", "open firefox
with whatsapp web", "what's 18% of 2340", weather/time — i.e. feels like an
assistant, gorgeously.

---

## 10. Risks / honest caveats

- **Small-model reliability.** 3B/7B will sometimes pick the wrong tool or bad
  args. Mitigate: tight grouped schemas, few-shot examples in the prompt, a
  validation layer in the daemon (reject malformed tool calls, ask the model to
  retry). The 14B (user's box) is the comfortable target.
- **Keyless web search is fragile** (DuckDuckGo-lite scrape can break). Weather/
  time are solid.
- **First-token latency** on CPU tier (3B) is seconds — the aurora "thinking"
  state must feel intentional, not laggy. Stream early.
- **Backdrop blur for a layer surface** may need compositor support; v1 ships the
  aurora-on-dark-glass without it and adds blur later.
- **Voice/STT** intentionally deferred (heavy second model).
```
```

Related memory: `project_vendi_ai.md`. Build/deploy via `project_ssh_deploy.md`
when the box is back.
