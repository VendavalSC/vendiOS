#!/usr/bin/env bash
if [[ "$(tty)" == "/dev/tty1" ]]; then
    export XDG_RUNTIME_DIR=/run/user/0
    export XDG_SESSION_TYPE=wayland
    export WLR_NO_HARDWARE_CURSORS=1
    export LANG=en_US.UTF-8
    mkdir -p "$XDG_RUNTIME_DIR"
    chmod 700 "$XDG_RUNTIME_DIR"
    fc-cache -f 2>/dev/null
    cage -- foot /usr/bin/vendi-boot 2>/tmp/foot.log || exec /usr/bin/vendi-boot
fi
