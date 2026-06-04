#!/usr/bin/env bash
# vendiOS system configuration library

chroot_run() { arch-chroot /mnt "$@"; }

BASE_PKGS=(
    base base-devel linux linux-headers linux-firmware
    amd-ucode intel-ucode
    networkmanager wpa_supplicant iwd
    sudo vim nano git curl wget
    fastfetch htop
    man-db bash-completion zsh
    terminus-font ttf-jetbrains-mono-nerd ttf-nerd-fonts-symbols-common
    noto-fonts noto-fonts-emoji
    limine efibootmgr
    btrfs-progs e2fsprogs dosfstools
    openssh reflector
    pipewire pipewire-audio pipewire-alsa pipewire-pulse wireplumber
    hyprland xdg-desktop-portal-hyprland xdg-desktop-portal-gtk
    waybar wofi mako foot
    brightnessctl playerctl grim slurp wl-clipboard
    polkit-kde-agent qt5-wayland qt6-wayland
    gtk3 gtk4
)

sys_pacstrap() {
    # -K initializes a fresh writable pacman keyring inside the target so
    # pacman in the chroot can import new keys. Without it, the live ISO's
    # keyring is copied as-is and pacman fails to add missing keys.
    pacstrap -K /mnt "${BASE_PKGS[@]}" "$@"
}

sys_genfstab() {
    genfstab -U /mnt >> /mnt/etc/fstab
}

sys_locale() {
    local locale=$1
    echo "LANG=${locale}" > /mnt/etc/locale.conf
    echo "${locale} UTF-8" >> /mnt/etc/locale.gen
    chroot_run locale-gen
}

sys_timezone() {
    local tz=$1
    chroot_run ln -sf "/usr/share/zoneinfo/${tz}" /etc/localtime
    chroot_run hwclock --systohc
}

sys_hostname() {
    local name=$1
    echo "$name" > /mnt/etc/hostname
    cat > /mnt/etc/hosts << EOF
127.0.0.1   localhost
::1         localhost
127.0.1.1   ${name}.localdomain ${name}
EOF
}

sys_keymap() {
    local km=$1
    printf 'KEYMAP=%s\nFONT=ter-v18n\n' "$km" > /mnt/etc/vconsole.conf
}

sys_initramfs() {
    local fs=$1
    local hooks='base udev autodetect microcode modconf kms keyboard keymap block'
    [[ "$fs" == 'btrfs' ]] && hooks+=' btrfs' || hooks+=' filesystems'
    hooks+=' fsck'
    sed -i "s|^HOOKS=.*|HOOKS=(${hooks})|" /mnt/etc/mkinitcpio.conf
    chroot_run mkinitcpio -P
}

sys_sudo() {
    sed -i 's/^# %wheel ALL=(ALL:ALL) ALL/%wheel ALL=(ALL:ALL) ALL/' /mnt/etc/sudoers
}

sys_root_password() {
    printf 'root:%s' "$1" | chroot_run chpasswd
}

sys_user_create() {
    local user=$1 pass=$2
    chroot_run useradd -m -G wheel,audio,video,storage,optical,input -s /bin/zsh "$user"
    printf '%s:%s' "$user" "$pass" | chroot_run chpasswd
}

sys_services_enable() {
    chroot_run systemctl enable NetworkManager
    chroot_run systemctl enable fstrim.timer   2>/dev/null || true
    chroot_run systemctl enable systemd-timesyncd
}

sys_install_os_release() {
    cat > /mnt/etc/os-release << 'EOF'
NAME="vendiOS"
PRETTY_NAME="vendiOS"
ID=vendios
ID_LIKE=arch
BUILD_ID=rolling
VERSION_CODENAME="zero"
ANSI_COLOR="38;2;203;166;247"
HOME_URL="https://github.com/vendi/vendiOS"
SUPPORT_URL="https://github.com/vendi/vendiOS/issues"
BUG_REPORT_URL="https://github.com/vendi/vendiOS/issues"
LOGO=vendios
EOF
}

sys_install_vendios_files() {
    mkdir -p /mnt/etc/vendios
    cat > /mnt/etc/vendios/release << EOF
VENDIOS_VERSION=0.1.0
VENDIOS_CODENAME=zero
VENDIOS_BUILD_DATE=$(date +%Y-%m-%d)
VENDIOS_ARCH=x86_64
VENDIOS_BASE=arch
EOF
    cat > /mnt/etc/vendios/config << 'EOF'
VENDIOS_AUR_HELPER=yay
VENDIOS_DEFAULT_SHELL=zsh
VENDIOS_ACCENT_COLOR="203;166;247"
VENDIOS_SNAPSHOT_MAX=10
EOF
    # copy the fastfetch logo
    mkdir -p /mnt/usr/share/vendios
    [[ -f /usr/share/vendios/logo.txt ]] && \
        cp /usr/share/vendios/logo.txt /mnt/usr/share/vendios/
}

sys_install_vendi_cli() {
    # Mirror the future AUR `vendi` package layout (/usr/bin + /usr/lib/vendi)
    # so `yay -Syu` can manage updates to these paths after install.
    mkdir -p /mnt/usr/bin /mnt/usr/lib/vendi
    for bin in vendi vendi-install vendi-boot vendi-welcome; do
        [[ -f /usr/bin/$bin ]] && install -m 755 /usr/bin/$bin /mnt/usr/bin/$bin
    done
    for lib in ui.sh disk.sh system.sh; do
        [[ -f /usr/lib/vendi/$lib ]] && \
            install -m 644 /usr/lib/vendi/$lib /mnt/usr/lib/vendi/$lib
    done
}

sys_install_aur_helper() {
    # Build yay from AUR as the target user (makepkg refuses to run as root).
    # Then install vendi-git through yay with --overwrite so pacman takes
    # ownership of the files sys_install_vendi_cli pre-staged at /usr/bin
    # and /usr/lib/vendi (otherwise `yay -S vendi-git` later fails with
    # "file exists in filesystem").
    local user=$1

    # temporary NOPASSWD entry — removed when the function returns
    local sudoers_drop=/mnt/etc/sudoers.d/00-vendios-aur
    echo "${user} ALL=(ALL) NOPASSWD: ALL" > "$sudoers_drop"
    chmod 0440 "$sudoers_drop"

    arch-chroot /mnt sudo -u "$user" -H bash -c '
        set -e
        cd /tmp
        rm -rf yay-build
        git clone --depth=1 https://aur.archlinux.org/yay.git yay-build
        cd yay-build
        makepkg -si --noconfirm

        # Claim the pre-staged vendi files so future updates work via yay.
        # --overwrite tells pacman it is allowed to replace existing files.
        yay -S vendi-git --noconfirm \
            --mflags "--noconfirm" \
            --answerclean=N --answerdiff=N \
            --overwrite="/usr/bin/vendi*,/usr/lib/vendi/*,/usr/share/vendios/*"
    '
    local rc=$?

    rm -f "$sudoers_drop"
    return $rc
}

sys_fastfetch_config() {
    local user=$1
    mkdir -p "/mnt/home/${user}"
    local home="/mnt/home/${user}/.config/fastfetch"
    mkdir -p "$home"
    # Catppuccin Mocha theme with Nerd Font icons
    cat > "${home}/config.jsonc" << 'EOF'
{
  "$schema": "https://github.com/fastfetch-cli/fastfetch/raw/dev/doc/json_schema.json",
  "logo": {
    "type": "file",
    "source": "/usr/share/vendios/logo.txt",
    "padding": { "top": 1, "right": 4 }
  },
  "display": {
    "separator": "  ",
    "color": {
      "keys":   "38;2;203;166;247",
      "title":  "38;2;203;166;247",
      "output": "38;2;205;214;244"
    },
    "key": { "width": 12 }
  },
  "modules": [
    {
      "type": "title",
      "keyColor": "38;2;203;166;247"
    },
    { "type": "separator", "string": "─" },
    {
      "type": "os",
      "key": "  os",
      "keyColor": "38;2;203;166;247"
    },
    {
      "type": "kernel",
      "key": "  kernel",
      "keyColor": "38;2;180;190;254"
    },
    {
      "type": "uptime",
      "key": "󰔟  uptime",
      "keyColor": "38;2;148;226;213"
    },
    {
      "type": "packages",
      "key": "  packages",
      "keyColor": "38;2;166;227;161"
    },
    {
      "type": "shell",
      "key": "  shell",
      "keyColor": "38;2;249;226;175"
    },
    {
      "type": "terminal",
      "key": "  terminal",
      "keyColor": "38;2;250;179;135"
    },
    {
      "type": "cpu",
      "key": "  cpu",
      "keyColor": "38;2;243;139;168"
    },
    {
      "type": "gpu",
      "key": "󰍛  gpu",
      "keyColor": "38;2;235;160;172"
    },
    {
      "type": "memory",
      "key": "  memory",
      "keyColor": "38;2;137;180;250"
    },
    {
      "type": "disk",
      "key": "󰋊  disk",
      "keyColor": "38;2;108;112;134"
    },
    { "type": "separator", "string": "─" },
    { "type": "colors", "paddingLeft": 2, "symbol": "circle" }
  ]
}
EOF
    chroot_run chown -R "${user}:${user}" "/home/${user}/.config" 2>/dev/null || true
}

sys_zsh_config() {
    local user=$1
    local home="/mnt/home/${user}"
    mkdir -p "$home"

    cat > "${home}/.zshrc" << 'ZSHRC'
# vendiOS zsh — Catppuccin Mocha + Nerd Fonts
autoload -Uz compinit && compinit -d ~/.cache/zcompdump
autoload -Uz colors && colors

# ── Catppuccin Mocha colors ───────────────────────────────────
_C_MAUVE=$'\e[38;2;203;166;247m'
_C_LAVENDER=$'\e[38;2;180;190;254m'
_C_TEXT=$'\e[38;2;205;214;244m'
_C_OVERLAY0=$'\e[38;2;108;112;134m'
_C_RED=$'\e[38;2;243;139;168m'
_C_GREEN=$'\e[38;2;166;227;161m'
_C_BASE=$'\e[48;2;30;30;46m'
_C_MANTLE=$'\e[48;2;24;24;37m'
_C_SEL=$'\e[48;2;49;35;73m'
_C_R=$'\e[0m'
_C_B=$'\e[1m'

# ── prompt ────────────────────────────────────────────────────
_vendi_git_branch() {
    local branch
    branch=$(git symbolic-ref --short HEAD 2>/dev/null) || return
    local dirty=''
    git diff --quiet 2>/dev/null || dirty=' ●'
    printf '  %s%s' "$branch" "$dirty"   # nf-pl-branch
}

_vendi_prompt() {
    local code=$?
    local arrow_fg; arrow_fg=$( [[ $code -eq 0 ]] && echo '203;166;247' || echo '243;139;168' )
    # Pre-expand: PROMPT_SUBST expands $() in PS1 itself, but does NOT
    # re-expand command substitutions emitted from inside a function.
    local branch; branch=$(_vendi_git_branch)

    # segment 1: user
    printf '%b' "%{\e[38;2;203;166;247m%}%B%n%b%f"
    # separator
    printf '%b' "%{\e[38;2;108;112;134m%}  %f"    # nf-pl-right_soft_divider
    # segment 2: dir (shortened)
    printf '%b' "%{\e[38;2;180;190;254m%}%3~%f"
    # git
    printf '%b' "%{\e[38;2;108;112;134m%}${branch}%f"
    # arrow
    printf '\n'
    printf '%b' "%{\e[38;2;${arrow_fg}m%} %f "    # nf-pl-right_hard_divider
}

setopt PROMPT_SUBST
PS1='$(_vendi_prompt)'
RPROMPT=''

# ── history ───────────────────────────────────────────────────
HISTSIZE=50000
SAVEHIST=50000
HISTFILE="${HOME}/.cache/zsh_history"
setopt HIST_IGNORE_ALL_DUPS HIST_IGNORE_SPACE SHARE_HISTORY INC_APPEND_HISTORY

# ── completions ───────────────────────────────────────────────
zstyle ':completion:*' menu select
zstyle ':completion:*' list-colors "${(s.:.)LS_COLORS}"
zstyle ':completion:*' matcher-list 'm:{a-z}={A-Z}' 'r:|=*' 'l:|=* r:|=*'
zstyle ':completion:*:descriptions' format '%F{#CBA6F7}── %d ──%f'

# ── ls colors (Catppuccin) ────────────────────────────────────
export LS_COLORS='di=38;2;137;180;250:ln=38;2;148;226;213:ex=38;2;166;227;161:fi=38;2;205;214;244:*.zip=38;2;250;179;135:*.tar=38;2;250;179;135:*.gz=38;2;250;179;135'

# ── aliases ───────────────────────────────────────────────────
alias ls='ls --color=auto --group-directories-first'
alias ll='ls -la --color=auto --group-directories-first'
alias la='ls -A --color=auto'
alias lt='ls -laht --color=auto'
alias grep='grep --color=auto'
alias diff='diff --color=auto'
alias fetch='fastfetch'
alias update='vendi update'
alias cls='clear'
alias ..='cd ..'
alias ...='cd ../..'
alias mkdir='mkdir -p'

# ── environment ───────────────────────────────────────────────
export EDITOR=vim
export VISUAL=vim
export PAGER='less -R'
export MANPAGER='less -R'
export TERM=xterm-256color

# ── welcome on first shell ────────────────────────────────────
[[ -z "$VENDI_GREETED" ]] && {
    export VENDI_GREETED=1
    vendi fetch 2>/dev/null || true
}
ZSHRC

    cat > "${home}/.zprofile" << 'ZPRO'
[[ -f ~/.zshrc ]] && source ~/.zshrc
# autostart Hyprland on tty1 login
[[ -z "$WAYLAND_DISPLAY" && "$(tty)" == "/dev/tty1" ]] && exec Hyprland
ZPRO

    chroot_run chown "${user}:${user}" \
        "/home/${user}/.zshrc" \
        "/home/${user}/.zprofile" 2>/dev/null || true
}

sys_install_wm() {
    local user=$1
    local home="/mnt/home/${user}"

    # ── hyprland ──────────────────────────────────────────────
    local hcfg="${home}/.config/hypr"
    mkdir -p "$hcfg"
    cat > "${hcfg}/hyprland.conf" << 'EOF'
# vendiOS — Hyprland config (temp until vendiWM)
# Catppuccin Mocha

monitor = ,preferred,auto,1

exec-once = waybar
exec-once = mako
exec-once = /usr/lib/polkit-kde-authentication-agent-1

$mod    = SUPER
$term   = foot
$menu   = wofi --show drun --allow-images --prompt ""

input {
    kb_layout    = us
    follow_mouse = 1
    sensitivity  = 0
    touchpad {
        natural_scroll    = true
        disable_while_typing = true
    }
}

general {
    gaps_in          = 5
    gaps_out         = 12
    border_size      = 2
    col.active_border   = rgb(CBA6F7) rgb(B4BEFE) 45deg
    col.inactive_border = rgb(313244)
    layout           = dwindle
    resize_on_border = true
}

decoration {
    rounding = 10
    blur {
        enabled  = true
        size     = 8
        passes   = 3
        noise    = 0.02
        contrast = 1.1
    }
    drop_shadow      = true
    shadow_range     = 18
    shadow_render_power = 3
    col.shadow       = rgba(1e1e2ecc)
    col.shadow_inactive = rgba(1e1e2e66)
    dim_inactive     = true
    dim_strength     = 0.08
}

animations {
    enabled = true
    bezier  = snap,   0.25, 1.0,  0.5,  1.0
    bezier  = slide,  0.4,  0.0,  0.2,  1.0
    bezier  = expo,   0.16, 1.0,  0.3,  1.0

    animation = windows,       1, 4,  snap,  slide
    animation = windowsOut,    1, 3,  expo,  popin 80%
    animation = border,        1, 8,  slide
    animation = borderangle,   1, 80, slide, loop
    animation = fade,          1, 4,  expo
    animation = workspaces,    1, 4,  expo,  slide
}

dwindle {
    pseudotile      = false
    preserve_split  = true
    smart_split     = true
}

gestures {
    workspace_swipe = true
    workspace_swipe_fingers = 3
}

misc {
    force_default_wallpaper = 0
    disable_hyprland_logo   = true
    disable_splash_rendering = true
    background_color        = rgb(1e1e2e)
}

# ── keybinds ──────────────────────────────────────────────────
bind = $mod,       Return,     exec,        $term
bind = $mod,       Space,      exec,        $menu
bind = $mod,       Q,          killactive
bind = $mod SHIFT, E,          exit
bind = $mod,       F,          fullscreen
bind = $mod,       V,          togglefloating
bind = $mod,       P,          pseudo
bind = $mod,       J,          togglesplit
bind = $mod,       L,          exec,        swaylock

# focus
bind = $mod, left,  movefocus, l
bind = $mod, right, movefocus, r
bind = $mod, up,    movefocus, u
bind = $mod, down,  movefocus, d
bind = $mod, H,     movefocus, l
bind = $mod, L,     movefocus, r
bind = $mod, K,     movefocus, u
bind = $mod, J,     movefocus, d

# workspaces
bind = $mod,       1, workspace, 1
bind = $mod,       2, workspace, 2
bind = $mod,       3, workspace, 3
bind = $mod,       4, workspace, 4
bind = $mod,       5, workspace, 5
bind = $mod SHIFT, 1, movetoworkspace, 1
bind = $mod SHIFT, 2, movetoworkspace, 2
bind = $mod SHIFT, 3, movetoworkspace, 3
bind = $mod SHIFT, 4, movetoworkspace, 4
bind = $mod SHIFT, 5, movetoworkspace, 5

# screenshot
bind  = , Print,          exec, grim ~/Screenshots/$(date +%s).png
bind  = SHIFT, Print,     exec, grim -g "$(slurp)" ~/Screenshots/$(date +%s).png

# brightness / volume
bindel = ,XF86MonBrightnessUp,   exec, brightnessctl set +5%
bindel = ,XF86MonBrightnessDown, exec, brightnessctl set 5%-
bindel = ,XF86AudioRaiseVolume,  exec, wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%+
bindel = ,XF86AudioLowerVolume,  exec, wpctl set-volume @DEFAULT_AUDIO_SINK@ 5%-
bindl  = ,XF86AudioMute,         exec, wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle

# mouse
bindm = $mod, mouse:272, movewindow
bindm = $mod, mouse:273, resizewindow

# window rules
windowrulev2 = float,        class:(wofi)
windowrulev2 = center,       class:(wofi)
windowrulev2 = float,        class:(mako)
windowrulev2 = animation popin, class:(wofi)
EOF

    # ── waybar ────────────────────────────────────────────────
    local wcfg="${home}/.config/waybar"
    mkdir -p "$wcfg"
    cat > "${wcfg}/config.jsonc" << 'EOF'
{
  "layer": "top",
  "position": "top",
  "height": 36,
  "spacing": 4,
  "modules-left":   ["hyprland/workspaces", "hyprland/window"],
  "modules-center": ["clock"],
  "modules-right":  ["pulseaudio", "network", "memory", "cpu", "battery", "tray"],

  "hyprland/workspaces": {
    "format": "{icon}",
    "format-icons": {
      "1": "󰲡", "2": "󰲣", "3": "󰲥", "4": "󰲧", "5": "󰲩",
      "active": "󰮯", "default": "󰊠"
    },
    "persistent-workspaces": { "*": 5 }
  },
  "hyprland/window": { "max-length": 50, "separate-outputs": true },
  "clock": {
    "format": "  {:%H:%M}",
    "format-alt": "󰸗  {:%a %d %b}",
    "tooltip-format": "<tt>{calendar}</tt>"
  },
  "cpu":    { "format": "  {usage}%", "interval": 2 },
  "memory": { "format": "  {used:.1f}G", "interval": 4 },
  "network": {
    "format-wifi":         "  {essid}",
    "format-ethernet":     "󰈀  {ipaddr}",
    "format-disconnected": "󰤭  offline",
    "tooltip-format-wifi": "{signalStrength}%"
  },
  "pulseaudio": {
    "format":        "{icon}  {volume}%",
    "format-muted":  "󰝟  muted",
    "format-icons":  { "default": ["󰕿", "󰖀", "󰕾"] },
    "on-click":      "wpctl set-mute @DEFAULT_AUDIO_SINK@ toggle"
  },
  "battery": {
    "format":          "{icon}  {capacity}%",
    "format-charging": "󰂄  {capacity}%",
    "format-icons":    ["󰂎","󰁺","󰁻","󰁼","󰁽","󰁾","󰁿","󰂀","󰂁","󰂂","󰁹"]
  },
  "tray": { "spacing": 8 }
}
EOF

    cat > "${wcfg}/style.css" << 'EOF'
/* vendiOS waybar — Catppuccin Mocha */
* { font-family: "JetBrainsMono Nerd Font", monospace; font-size: 13px; border: none; }

window#waybar {
    background: rgba(17,17,27,0.92);
    color: #CDD6F4;
    border-bottom: 1px solid rgba(203,166,247,0.2);
}

.modules-left, .modules-center, .modules-right { padding: 0 8px; }

#workspaces button {
    color: #6C7086;
    padding: 0 6px;
    border-radius: 6px;
    transition: all 0.2s;
}
#workspaces button.active {
    color: #CBA6F7;
    background: rgba(203,166,247,0.15);
}
#workspaces button:hover {
    color: #CDD6F4;
    background: rgba(255,255,255,0.08);
}

#window      { color: #A6ADC8; }
#clock       { color: #B4BEFE; font-weight: bold; }
#cpu         { color: #F38BA8; }
#memory      { color: #89B4FA; }
#network     { color: #94E2D5; }
#pulseaudio  { color: #A6E3A1; }
#battery     { color: #F9E2AF; }
#battery.charging { color: #A6E3A1; }
#tray        { color: #CDD6F4; }

tooltip {
    background: #1E1E2E;
    color: #CDD6F4;
    border: 1px solid rgba(203,166,247,0.4);
    border-radius: 8px;
}
EOF

    # ── mako (notifications) ──────────────────────────────────
    local mcfg="${home}/.config/mako"
    mkdir -p "$mcfg"
    cat > "${mcfg}/config" << 'EOF'
background-color=#1E1E2E
text-color=#CDD6F4
border-color=#CBA6F7
border-size=1
border-radius=8
width=320
height=120
margin=12
padding=12,14
font=JetBrainsMono Nerd Font 11
default-timeout=5000

[urgency=high]
border-color=#F38BA8
EOF

    # ── foot (terminal) ───────────────────────────────────────
    local fcfg="${home}/.config/foot"
    mkdir -p "$fcfg"
    cat > "${fcfg}/foot.ini" << 'EOF'
[main]
font=JetBrainsMono Nerd Font:size=12
pad=12x10

[colors]
background=1e1e2e
foreground=cdd6f4
regular0=45475a
regular1=f38ba8
regular2=a6e3a1
regular3=f9e2af
regular4=89b4fa
regular5=f5c2e7
regular6=94e2d5
regular7=bac2de
bright0=585b70
bright1=f38ba8
bright2=a6e3a1
bright3=f9e2af
bright4=89b4fa
bright5=f5c2e7
bright6=94e2d5
bright7=a6adc8
selection-background=313244
selection-foreground=cdd6f4

[cursor]
color=1e1e2e cba6f7

[mouse]
hide-when-typing=yes
EOF

    # ── wofi ─────────────────────────────────────────────────
    local wfcfg="${home}/.config/wofi"
    mkdir -p "$wfcfg"
    cat > "${wfcfg}/style.css" << 'EOF'
* { font-family: "JetBrainsMono Nerd Font"; font-size: 13px; }

window {
    background: rgba(30,30,46,0.96);
    border: 1px solid rgba(203,166,247,0.4);
    border-radius: 12px;
}
#input {
    background: #181825;
    color: #CDD6F4;
    border: none;
    border-bottom: 1px solid rgba(203,166,247,0.3);
    border-radius: 12px 12px 0 0;
    padding: 12px 16px;
    font-size: 14px;
}
#inner-box    { background: transparent; margin: 6px; }
#outer-box    { background: transparent; }
#scroll       { background: transparent; }
#entry        { padding: 8px 12px; border-radius: 8px; color: #CDD6F4; }
#entry:focus  { background: rgba(203,166,247,0.12); color: #CBA6F7; }
#entry:hover  { background: rgba(255,255,255,0.05); }
#text         { color: #CDD6F4; }
#text:focus   { color: #CBA6F7; }
EOF

    # ── screenshots dir ───────────────────────────────────────
    chroot_run mkdir -p "/home/${user}/Screenshots"
    chroot_run chown -R "${user}:${user}" "/home/${user}" 2>/dev/null || true

    # ── enable pipewire ───────────────────────────────────────
    chroot_run systemctl enable --global pipewire pipewire-pulse wireplumber 2>/dev/null || true
}
