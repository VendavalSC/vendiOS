#!/usr/bin/env bash

# Pick the GPU the live compositor should render on. The installer's truecolor
# UI only exists under a Wayland compositor (Hyprland → foot), so the
# compositor MUST come up. On multi-GPU machines wlroots/aquamarine may grab a
# card the live kernel can't drive (e.g. a brand-new NVIDIA the bundled kernel
# predates) and crash to a bare console. So we hand the compositor an explicit,
# preference-ordered device list:
#
#   1. only cards with a *connected* display (so we scan out where the user
#      can actually see it), and
#   2. open drivers the live kernel reliably drives (amdgpu/i915/xe/radeon)
#      before nouveau/nvidia/unknown.
#
# Result: on a box with the monitor on both an AMD and a too-new NVIDIA card,
# the compositor lands on AMD and the user sees the real UI. Single-GPU and
# hybrid-laptop setups fall out of the same logic. Echoes a colon-list, or
# nothing (let the compositor auto-pick) when no display is found.
vendi_pick_gpus() {
    local pref=() rest=() d card c drv node connected
    for d in /sys/class/drm/card[0-9]*; do
        [[ -e "$d/device" ]] || continue
        card=${d##*/}
        connected=0
        for c in "$d/$card"-*/status; do
            [[ -r "$c" ]] || continue
            [[ "$(cat "$c" 2>/dev/null)" == connected ]] && { connected=1; break; }
        done
        [[ $connected -eq 1 ]] || continue
        drv=$(basename "$(readlink -f "$d/device/driver" 2>/dev/null)" 2>/dev/null)
        node="/dev/dri/$card"
        case "$drv" in
            amdgpu|i915|xe|radeon) pref+=("$node") ;;
            *)                     rest+=("$node") ;;
        esac
    done
    # Prefer the well-supported cards alone; only fall back to the rest if
    # that's all that has a display.
    local list=("${pref[@]}")
    [[ ${#list[@]} -eq 0 ]] && list=("${rest[@]}")
    (IFS=:; printf '%s' "${list[*]}")
}

if [[ "$(tty)" == "/dev/tty1" ]]; then
    export XDG_RUNTIME_DIR=/run/user/0
    export XDG_SESSION_TYPE=wayland
    export XDG_CURRENT_DESKTOP=vendiOS
    export WLR_NO_HARDWARE_CURSORS=1
    export WLR_RENDERER_ALLOW_SOFTWARE=1
    export LANG=en_US.UTF-8
    mkdir -p "$XDG_RUNTIME_DIR"
    chmod 700 "$XDG_RUNTIME_DIR"
    fc-cache -f 2>/dev/null

    # Steer the compositor onto a usable GPU (WLR_* for wlroots/cage, AQ_* for
    # Hyprland's aquamarine backend). Harmless when there's a single GPU.
    _gpus="$(vendi_pick_gpus)"
    if [[ -n "$_gpus" ]]; then
        export WLR_DRM_DEVICES="$_gpus"
        export AQ_DRM_DEVICES="$_gpus"
    fi

    # Try Hyprland → cage(+foot) → bare console. Each fallback still gets a
    # usable installer: foot/Hyprland give the truecolor UI; the bare console
    # path renders via ui.sh's 16-color fallback (still legible, just plainer).
    if command -v Hyprland >/dev/null; then
        Hyprland >/tmp/hyprland.log 2>&1 || cage -- foot /usr/bin/vendi-boot 2>/tmp/foot.log || exec /usr/bin/vendi-boot
    else
        cage -- foot /usr/bin/vendi-boot 2>/tmp/foot.log || exec /usr/bin/vendi-boot
    fi
fi
