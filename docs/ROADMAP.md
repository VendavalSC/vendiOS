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

### 2. Dev environment — `vendi dev` + vendiVim  ← SHIPPED
- **vendiVim** = LazyVim base (don't reinvent) + vendiOS theme integration (follows
  `vendi theme`), curated language extras (rust/ts/python/go/lua/qml/c), sane keymaps.
  ✅ Config at `/usr/share/vendios/vendivim`; a generated `vendi` colorscheme reads
  `~/.config/vendi/nvim.lua` (written by `vendi theme`, incl. the dynamic theme) and an
  fs-watcher recolors open editors live.
- **`vendi dev`** installer: mise (runtimes), lazygit, starship, zellij/tmux,
  ripgrep/fd/fzf/bat/eza/zoxide; optional zed/vscode/helix (`vendi dev editors`). kitty
  stays default terminal. ✅ subcommands: `setup`/`tools`/`vim`/`editors`/`status`.

### 3. Compositor / WM (vendiwm)
Tearing + VRR path for fullscreen games (FPS, esp. NVIDIA); tasteful snappy animation
pass; scratchpad (drop-down term); smart gaps; window groups/tabs; richer window
rules; multi-monitor + per-monitor workspaces; per-corner rounding + blur knobs.

### 4. Gaming — `vendi game`
✅ SHIPPED — `vendi game setup` (auto-enables multilib; installs gamemode/gamescope/
mangohud/lib32/Vulkan/steam/lutris + AUR heroic/protonup-qt/vkbasalt/moonlight via
yay/paru) · `vendi game run [--gpu nvidia|amd] [--gamescope[=WxH]] [--no-mango] <cmd>`
layers gamemode+mangohud+gamescope, injects dual-GPU offload env, bumps power profile to
performance for the run · `vendi game status`. Verified on HW (status, run wrapper, pkg
resolution). TODO: controller udev rules, per-game profile presets.

### 5. Productivity / utilities (Omarchy parity)
✅ `vendi shot|record|ocr` (grim/slurp/wf-recorder/tesseract) + `vendi clip` (cliphist,
fzf/wofi picker, self-starting watcher) + `vendi font` (mono font cascade across
kitty/foot/alacritty, persists across theme switches) — SHIPPED, verified on HW; binds
wired in config.rs (Print / Super+Shift+S / Super+Shift+R / Super+Shift+T / Super+V).
✅ `vendi night` (color temp) — vendiwm applies the CRTC gamma LUT itself (no wlr-gamma
protocol/external tool); on/off/toggle/warmer/cooler/<kelvin>/status, persists +
re-applies on login. ✅ `vendi voice` — local whisper.cpp STT typed into the focused field
via wtype, now that vendiwm advertises virtual-keyboard-unstable-v1. Both verified to
build + deploy; bound to super+shift+n / super+shift+v.
Remaining: `vendi webapp` (theme-aware PWAs); idle daemon (compositor already does
lock+dpms; add dim→suspend stages); OSD popups (notch already bulges for vol/brightness —
extend to caps/mic); DND/notification toggle (talk to vendiwm's island notification
server); night-light auto schedule (sunset/sunrise).

### 6. Theming depth
Cascade `vendi theme` into: nvim (vendiVim ✅), kitty ✅, GTK/libadwaita ✅ (@define-color
in gtk-4.0/gtk-3.0 → file pickers, nautilus, nwg-look follow). TODO: btop (needs btop
shipped), web apps, more shipped themes, per-theme wallpapers. (Accent/theme-state
plumbing already exists.)

### 7. Hardware / system
Hardware profiles (`vendi hw nvidia|intel|amd|laptop-*`) for GPU/touchpad/power;
deeper update (firmware/orphans/keyring/log-analyze); hybrid-GPU toggle; battery polish.

### 8. Curated app layer
Rehabilitate `vendi install` for real apps (browser/files/dev/gaming bundles) + a
default app set for an instantly-usable first-run.

## Sequencing
1. Gadget framework + Claude gadget + indicators
2. ~~**`vendi dev` + vendiVim**~~ ✅ SHIPPED (2026-06-27)
3. capture + clipboard ✅ SHIPPED (`vendi shot|record|ocr|clip|font`, 2026-06-27); voice +
   OSD + night light still TODO
4. gaming ✅ SHIPPED (`vendi game setup|run|status`, 2026-06-27)
3. Capture + voice + clipboard + OSD + night light
4. `vendi game` + compositor tearing/VRR
5. Theming depth + hardware profiles + curated apps
6. Compositor animation pass

## Notes
- vendiMessage was built then dropped (2026-06-26) — messenger network-effects make it
  pointless for a distro. Don't revisit.
- Audit source: Omarchy 3.8.2 on the maintainer's machine (LazyVim, mise, Unity/Godot
  game-dev, AirPods, ~282 explicit pkgs) vs vendiOS (~94 pkgs).
