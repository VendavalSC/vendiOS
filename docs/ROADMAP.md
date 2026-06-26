# vendiOS Roadmap — "better than 99.9% of Hyprland setups"

Grounded in a direct audit of the maintainer's daily-driver **Omarchy 3.8.2** vs
vendiOS. Goal: keep vendiOS's unique strengths and close the tooling/dev/gaming
gaps, with first-class widgets and a curated experience.

## Where vendiOS already wins (don't rebuild)
- **Notch / dynamic-island bar** (vendibar-pro): media/AI/weather/match/info cards,
  battery badge — far slicker than Waybar.
- **Built-in local AI** (`vendi ai`, super+space) — system-aware, no cloud/keys.
- Own compositor (**vendiwm**) with iOS-spring animations, touch gestures.
- Live theme switching, **dynamic day/night wallpaper**, TUI installer, screensaver,
  control center (wifi/bt/notifications), snapshots/rollback, fingerprint.

## Where Omarchy is ahead (gaps to close)
Capture suite (screenshot/record/**OCR**/transcode); **voice typing** (voxtype);
**clipboard history**; **web apps (PWAs)**; **night light**; **idle daemon**; **OSD**
popups; curated **dev env** (LazyVim/mise/lazygit/starship/tmux); full **gaming
stack** (steam/proton/gamescope/gamemode/mangohud/lutris/heroic/moonlight); **hardware
profiles**; **font management**; deeper **theming cascade** (nvim/btop/term/gtk/browser);
**app/browser management** + usable first-run.

---

## Plan

### 1. vendiBar Pro — gadget framework + widgets
Make notch cards pluggable (a gadget registry), then ship:
- **Claude Code gadget** — island expands like now-playing: left = session-limit %
  ring, right = Claude logo; tap → model/tokens/reset-time; pulse while working.
- **Status indicators**: screen-recording, mic/voice-active, idle/DND, VPN,
  update-count, screen-share.
- **New gadgets**: AirPods/BT battery (L/R/case %), now-playing scrubber + art-tint,
  Pomodoro/focus timer, GitHub/CI pill + build/deploy progress island, net-speed +
  GPU-temp mini-graph, clipboard-history popover, color picker, emoji/glyph picker,
  calendar peek + countdown, inline calculator, download/file-transfer island,
  quick-note scratchpad.

### 2. Dev environment — `vendi dev` + vendiVim  ← STARTING HERE
- **vendiVim** = LazyVim base (don't reinvent) + vendiOS theme integration (follows
  `vendi theme`), curated language extras (rust/ts/python/go/lua/qml/c), sane keymaps.
- **`vendi dev`** installer: mise (runtimes), lazygit/lazydocker, starship,
  zellij/tmux, ripgrep/fd/fzf/bat/eza/zoxide; optional zed/vscode/helix. kitty stays
  default terminal.

### 3. Compositor / WM (vendiwm)
Tearing + VRR path for fullscreen games (FPS, esp. NVIDIA); tasteful snappy animation
pass; scratchpad (drop-down term); smart gaps; window groups/tabs; richer window
rules; multi-monitor + per-monitor workspaces; per-corner rounding + blur knobs.

### 4. Gaming — `vendi game`
gamemode, gamescope, mangohud, vkBasalt, Steam + Proton-GE, lutris/heroic, moonlight
(stream from the RTX desktop), lib32/Vulkan, controller udev, auto power→performance
on launch, per-game gamescope profiles, NVIDIA env.

### 5. Productivity / utilities (Omarchy parity)
`vendi shot|record|ocr` (grim/slurp/wf-recorder/tesseract); `vendi voice` (whisper.cpp
local → types into focused field); `vendi clip` (cliphist + notch popover); `vendi
webapp` (theme-aware PWAs); `vendi night` (color-temp + schedule); idle daemon
(dim→lock→dpms→suspend) wired to vendilock; OSD popups (vol/brightness/caps); `vendi
font`; DND/notification-silencing toggle.

### 6. Theming depth
Cascade themes into nvim (vendiVim), btop, kitty, GTK, web apps; more shipped themes;
per-theme wallpapers. (Accent/theme-state plumbing already exists.)

### 7. Hardware / system
Hardware profiles (`vendi hw nvidia|intel|amd|laptop-*`) for GPU/touchpad/power;
deeper update (firmware/orphans/keyring/log-analyze); hybrid-GPU toggle; battery polish.

### 8. Curated app layer
Rehabilitate `vendi install` for real apps (browser/files/dev/gaming bundles) + a
default app set for an instantly-usable first-run.

## Sequencing
1. Gadget framework + Claude gadget + indicators
2. **`vendi dev` + vendiVim**  ← chosen start
3. Capture + voice + clipboard + OSD + night light
4. `vendi game` + compositor tearing/VRR
5. Theming depth + hardware profiles + curated apps
6. Compositor animation pass

## Notes
- vendiMessage was built then dropped (2026-06-26) — messenger network-effects make it
  pointless for a distro. Don't revisit.
- Audit source: Omarchy 3.8.2 on the maintainer's machine (LazyVim, mise, Unity/Godot
  game-dev, AirPods, ~282 explicit pkgs) vs vendiOS (~94 pkgs).
