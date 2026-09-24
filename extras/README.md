# extras — local-only, never shipped

Personal features that stay out of the public ISO:

- `vendi-claude-status` — Claude Code usage/state gadget for the bar. Uses the
  user's Claude OAuth token against an undocumented endpoint and draws
  Anthropic's mark (`src/vendibar-pro/claude.svg`, stripped by build.sh).
  Install locally: `sudo install -Dm755 extras/vendi-claude-status /usr/bin/`
- vendi-buds (`src/vendi-buds`) — AirPods control; not built by build.sh.
