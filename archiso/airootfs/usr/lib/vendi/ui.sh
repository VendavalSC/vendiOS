#!/usr/bin/env bash
# vendiOS TUI library

# ── Catppuccin Mocha palette (Mauve accent) ───────────────────────────────────
# Attributes are universal (work on every terminal incl. the bare Linux VT).
R=$'\e[0m'
BOLD=$'\e[1m'
DIM=$'\e[2m'
ITALIC=$'\e[3m'
UL=$'\e[4m'

# The whole TUI is built from 24-bit colored background *bands* (no box-drawing
# chars). Those escapes only render in a truecolor terminal emulator (foot /
# alacritty under the live compositor). When the GPU can't bring a compositor
# up — e.g. a too-new card the live kernel can't drive — the installer falls
# through to the bare Linux VT console, which supports only the 16 ANSI colors.
# There, 24-bit escapes are dropped and the band layout collapses to an
# unreadable monochrome mess. So we pick a palette tier at startup:
#
#   truecolor  → exact Catppuccin 24-bit escapes (emulator under compositor)
#   console    → the 16 ANSI indices, AND the Linux VT's 16-slot palette is
#                remapped to the real Catppuccin RGB so it still looks right
#
# Layout code never changes — it only references the FG_*/BG_*/_BG_* vars set
# here. Override detection with VENDI_COLOR=truecolor|console (handy to test).
UI_COLORMODE=''

ui_detect_color() {
    if [[ -n "${VENDI_COLOR:-}" ]]; then
        UI_COLORMODE="$VENDI_COLOR"
    elif [[ "${COLORTERM:-}" == truecolor || "${COLORTERM:-}" == 24bit ]]; then
        UI_COLORMODE='truecolor'
    else
        UI_COLORMODE='console'   # bare Linux VT (no truecolor emulator)
    fi
}

ui_palette_init() {
    [[ -z "$UI_COLORMODE" ]] && ui_detect_color

    if [[ "$UI_COLORMODE" == 'truecolor' ]]; then
        # foreground
        FG_ACCENT=$'\e[38;2;203;166;247m'    # Mauve  #CBA6F7
        FG_ACCENT2=$'\e[38;2;180;190;254m'   # Lavender #B4BEFE
        FG_BLUE=$'\e[38;2;137;180;250m'      # Blue   #89B4FA
        FG_WHITE=$'\e[38;2;205;214;244m'     # Text   #CDD6F4
        FG_SUBTEXT=$'\e[38;2;166;173;200m'   # Subtext0 #A6ADC8
        FG_DIM=$'\e[38;2;108;112;134m'       # Overlay0 #6C7086
        FG_GREEN=$'\e[38;2;166;227;161m'     # Green  #A6E3A1
        FG_TEAL=$'\e[38;2;148;226;213m'      # Teal   #94E2D5
        FG_RED=$'\e[38;2;243;139;168m'       # Red    #F38BA8
        FG_MAROON=$'\e[38;2;235;160;172m'    # Maroon #EBA0AC
        FG_YELLOW=$'\e[38;2;249;226;175m'    # Yellow #F9E2AF
        FG_ORANGE=$'\e[38;2;250;179;135m'    # Peach  #FAB387
        FG_PINK=$'\e[38;2;245;194;231m'      # Pink   #F5C2E7

        # background
        BG_BASE=$'\e[48;2;30;30;46m'         # Base   #1E1E2E
        BG_MANTLE=$'\e[48;2;24;24;37m'       # Mantle #181825
        BG_CRUST=$'\e[48;2;17;17;27m'        # Crust  #11111B
        BG_SURFACE0=$'\e[48;2;49;50;68m'     # Surface0 #313244
        BG_SURFACE1=$'\e[48;2;69;71;90m'     # Surface1 #45475A
        BG_SEL=$'\e[48;2;49;35;73m'          # Mauve-tinted surface

        # inline background escapes (for colored-space fills — no Unicode needed)
        _BG_MAUVE=$'\e[48;2;203;166;247m'
        _BG_GREEN=$'\e[48;2;166;227;161m'
        _BG_RED=$'\e[48;2;243;139;168m'
        _BG_YELLOW=$'\e[48;2;249;226;175m'
        _BG_ORANGE=$'\e[48;2;250;179;135m'
    else
        # ── 16-color console fallback ─────────────────────────────────────────
        # Express every color as a standard ANSI index. The dark Catppuccin
        # backgrounds (base/mantle/crust) all collapse to black; the surfaces to
        # bright-black; selection to blue — enough distinct bands to keep the
        # layout legible anywhere. Indices are remapped to exact Catppuccin RGB
        # by ui_palette_remap below on the real Linux VT.
        FG_ACCENT=$'\e[95m'      # bright magenta → mauve
        FG_ACCENT2=$'\e[94m'     # bright blue → lavender
        FG_BLUE=$'\e[94m'
        FG_WHITE=$'\e[97m'       # bright white → text
        FG_SUBTEXT=$'\e[37m'     # white → subtext
        FG_DIM=$'\e[90m'         # bright black → overlay
        FG_GREEN=$'\e[92m'
        FG_TEAL=$'\e[96m'        # bright cyan → teal
        FG_RED=$'\e[91m'
        FG_MAROON=$'\e[91m'
        FG_YELLOW=$'\e[93m'
        FG_ORANGE=$'\e[93m'      # no orange slot → yellow
        FG_PINK=$'\e[95m'

        BG_BASE=$'\e[40m'        # black
        BG_MANTLE=$'\e[40m'
        BG_CRUST=$'\e[40m'
        BG_SURFACE0=$'\e[100m'   # bright-black bg
        BG_SURFACE1=$'\e[100m'
        BG_SEL=$'\e[44m'         # blue bg → selection highlight

        _BG_MAUVE=$'\e[105m'     # bright magenta bg
        _BG_GREEN=$'\e[102m'
        _BG_RED=$'\e[101m'
        _BG_YELLOW=$'\e[103m'
        _BG_ORANGE=$'\e[103m'
    fi

    # aliases
    BG_PANEL="$BG_MANTLE"
    BG_HEADER="$BG_CRUST"
    ACCENT_ON="${BG_SEL}${BOLD}${FG_ACCENT}"
}

# Remap the Linux VT's 16 palette slots to the real Catppuccin RGB (OSC "P"
# sequences). On the bare console this makes the 16-color fallback actually
# look like Catppuccin; emulators ignore it harmlessly. No-op outside console
# mode. Reset with ui_palette_reset (\e]R).
ui_palette_remap() {
    [[ "$UI_COLORMODE" == 'console' ]] || return 0
    printf '\e]P0%s' '1e1e2e'   # black        → base
    printf '\e]P8%s' '313244'   # bright black  → surface0
    printf '\e]P1%s' 'f38ba8'   # red
    printf '\e]P9%s' 'f38ba8'
    printf '\e]P2%s' 'a6e3a1'   # green
    printf '\e]PA%s' 'a6e3a1'
    printf '\e]P3%s' 'f9e2af'   # yellow
    printf '\e]PB%s' 'f9e2af'
    printf '\e]P4%s' '312349'   # blue slot     → mauve-tinted selection
    printf '\e]PC%s' 'b4befe'   # bright blue   → lavender
    printf '\e]P5%s' 'cba6f7'   # magenta       → mauve
    printf '\e]PD%s' 'cba6f7'
    printf '\e]P6%s' '94e2d5'   # cyan          → teal
    printf '\e]PE%s' '94e2d5'
    printf '\e]P7%s' 'a6adc8'   # white         → subtext
    printf '\e]PF%s' 'cdd6f4'   # bright white  → text
}

ui_palette_reset() { [[ "$UI_COLORMODE" == 'console' ]] && printf '\e]R'; }

# Populate the color vars at source time (idempotent; ui_init re-runs after
# setting TERM) so they're never empty if referenced before ui_init.
ui_palette_init

# ── terminal state ────────────────────────────────────────────────────────────
UI_COLS=0
UI_ROWS=0
UI_W=78
UI_X=0
UI_CONTENT_TOP=6
UI_CONTENT_BOT=0

ui_resize() {
    UI_COLS=$(tput cols 2>/dev/null || echo 80)
    UI_ROWS=$(tput lines 2>/dev/null || echo 24)
    UI_W=$(( UI_COLS < 82 ? UI_COLS - 2 : 80 ))
    UI_X=$(( (UI_COLS - UI_W) / 2 ))
    [[ $UI_X -lt 0 ]] && UI_X=0
    UI_CONTENT_BOT=$(( UI_ROWS - 3 ))
}

ui_init() {
    # Pick the palette tier from the *real* terminal (COLORTERM) before we
    # touch TERM — a too-new GPU drops us onto the bare VT, where only the
    # 16-color fallback renders.
    ui_detect_color
    ui_palette_init
    export TERM=xterm-256color
    ui_resize
    printf '\e[?25l\e[?7l'
    ui_palette_remap
    trap ui_cleanup EXIT INT TERM
    trap 'ui_resize; ui_redraw' WINCH
}

ui_cleanup() {
    printf '\e[?25h\e[?7h\e[0m'
    ui_palette_reset
    tput cnorm 2>/dev/null || true
    clear
}

# ── primitives ────────────────────────────────────────────────────────────────
_at()  { printf '\e[%d;%dH' "$1" "$2"; }
_fill() {
    local row=$1 col=$2 w=$3 bg=${4:-${BG_BASE}}
    _at "$row" "$col"
    printf "${bg}%*s${R}" "$w" ''
}

# ── background ────────────────────────────────────────────────────────────────
ui_draw_bg() {
    local r
    for (( r=1; r<=UI_ROWS; r++ )); do
        _at "$r" 1
        printf "${BG_BASE}%*s${R}" "$UI_COLS" ''
    done
}

ui_clear() {
    printf '\e[2J\e[H'
    ui_draw_bg
}

# ── panel ─────────────────────────────────────────────────────────────────────
# Flat colored-band design — zero Unicode characters, 100% font-independent.
# Uses ANSI 24-bit background fills for a clean modern look.
#
#  Row 1  [CRUST ] vendiOS  [====MAUVE FILL====][SURFACE0 EMPTY] step/total
#  Row 2  [MANTLE]   Title text
#  Row 3  [MAUVE ] ──────────────────────── accent stripe ────────────────────
#  Row 4  [BASE  ] (padding)
#  Row 5  [BASE  ] (padding)
#  Row 6+ [BASE  ] content area
#  Bot+1  [SURF0 ] (divider strip)
#  Bot+2  [MANTLE] key hints

_UI_TITLE=''
_UI_STEP=0
_UI_TOTAL=0

ui_panel_draw() {
    local step=$1 total=$2 title=$3
    _UI_STEP=$step; _UI_TOTAL=$total; _UI_TITLE=$title
    local x=$UI_X w=$UI_W

    # header progress bar math — 9 chars " vendiOS " + 7 chars " X/YY  " = 16
    local pct; (( total > 1 )) && pct=$(( (step-1)*100/(total-1) )) || pct=100
    local bar_w=$(( w - 16 ))
    [[ $bar_w -lt 0 ]] && bar_w=0
    local filled=$(( pct * bar_w / 100 ))
    local empty=$(( bar_w - filled ))

    # Row 1: crust header — brand + Mauve progress fill + step counter
    _at 1 $x
    printf "${BG_CRUST}${FG_ACCENT}${BOLD} vendiOS ${R}"
    printf "${_BG_MAUVE}%*s${R}" "$filled" ''
    printf "${BG_SURFACE0}%*s${R}" "$empty" ''
    printf "${BG_CRUST}${FG_DIM} %2d/%-2d ${R}" "$step" "$total"

    # Row 2: mantle title band
    _at 2 $x
    printf "${BG_MANTLE}%*s${R}" "$w" ''
    _at 2 $(( x+2 ))
    printf "${BG_MANTLE}${BOLD}${FG_WHITE}${title}${R}"

    # Row 3: Mauve accent stripe (full width, no characters needed)
    _at 3 $x
    printf "${_BG_MAUVE}%*s${R}" "$w" ''

    # Rows 4-5: base padding; row 4 carries the step-dots indicator
    _at 4 $x; printf "${BG_BASE}%*s${R}" "$w" ''
    _at 5 $x; printf "${BG_BASE}%*s${R}" "$w" ''
    ui_step_dots "$step" "$total"

    # Content rows 6 … UI_CONTENT_BOT
    local r
    for (( r=6; r<=UI_CONTENT_BOT; r++ )); do
        _at $r $x; printf "${BG_BASE}%*s${R}" "$w" ''
    done

    # CONTENT_BOT+1: Surface0 divider strip
    _at $(( UI_CONTENT_BOT+1 )) $x
    printf "${BG_SURFACE0}%*s${R}" "$w" ''

    # CONTENT_BOT+2: Mantle footer band
    _at $(( UI_CONTENT_BOT+2 )) $x
    printf "${BG_MANTLE}%*s${R}" "$w" ''
}

ui_redraw() {
    ui_resize
    ui_clear
    ui_panel_draw "$_UI_STEP" "$_UI_TOTAL" "$_UI_TITLE"
}

# ── shard logo ────────────────────────────────────────────────────────────────
# The vendi shard (block-element art with embedded 24-bit colors). Each line
# carries only FG escapes until a trailing reset, so painting BG_BASE first
# keeps the panel background behind the glyphs. Prints at <start_row>,
# centered; echoes nothing if the art is missing (safe on weird media).
ui_shard() {
    local start_row=$1 art=/usr/share/vendios/shard.txt
    [[ -r "$art" ]] || return 0
    local row=$start_row line clean w
    while IFS= read -r line; do
        clean=$(printf '%b' "$line" | sed 's/\x1b\[[0-9;]*m//g')
        w=${#clean}
        _at "$row" $(( UI_X + (UI_W - w) / 2 ))
        printf '%s%b%s' "$BG_BASE" "$line" "$R"
        (( row++ ))
    done < "$art"
}

# ── step dots ─────────────────────────────────────────────────────────────────
# One mark per step, centered on panel row 4: done = accent, current = bright
# block, future = dim dot. cp437-safe glyphs only (■ and ·).
ui_step_dots() {
    local step=$1 total=$2
    local dots='' i
    for (( i=1; i<=total; i++ )); do
        if (( i < step ));  then dots+="${FG_ACCENT}■${R}${BG_BASE} "
        elif (( i == step )); then dots+="${BOLD}${FG_WHITE}■${R}${BG_BASE} "
        else dots+="${FG_DIM}·${R}${BG_BASE} "
        fi
    done
    local w=$(( total * 2 - 1 ))
    _at 4 $(( UI_X + (UI_W - w) / 2 ))
    printf "${BG_BASE}%b${R}" "$dots"
}

# ── box ───────────────────────────────────────────────────────────────────────
# Single-line box (cp437-safe) inside the panel: ui_box <row> <height> [title]
ui_box() {
    local row=$1 h=$2 title=${3:-}
    local sx=$(( UI_X+3 )) bw=$(( UI_W-6 ))
    # (tr is byte-based and mangles multibyte '─' — build the run by hand)
    local horiz='' i
    for (( i=0; i<bw-2; i++ )); do horiz+='─'; done
    _at "$row" $sx; printf "${BG_BASE}${FG_DIM}┌%s┐${R}" "$horiz"
    if [[ -n "$title" ]]; then
        _at "$row" $(( sx+3 ))
        printf "${BG_BASE}${FG_DIM} ${BOLD}${FG_ACCENT}%s${R}${BG_BASE}${FG_DIM} ${R}" "$title"
    fi
    local r
    for (( r=row+1; r<row+h-1; r++ )); do
        _at "$r" $sx;               printf "${BG_BASE}${FG_DIM}│${R}"
        _at "$r" $(( sx+bw-1 ));    printf "${BG_BASE}${FG_DIM}│${R}"
    done
    _at $(( row+h-1 )) $sx; printf "${BG_BASE}${FG_DIM}└%s┘${R}" "$horiz"
}

# ── content helpers ───────────────────────────────────────────────────────────
# Text drawn over the panel must keep BG_BASE alive across the resets inside
# the caller's string, or every printed cell drops to the terminal-default
# background (dark halo boxes around text).
_bgkeep() {
    local text=$1
    text="${text//$'\e[0m'/$'\e[0m'${BG_BASE}}"
    printf '%s%b%s' "$BG_BASE" "$text" "$R"
}

# ui_pline <row_offset_from_6> <text_with_colors>
ui_pline() {
    local offset=$1; shift
    local row=$(( 6 + offset ))
    _at "$row" $(( UI_X + 3 ))
    _bgkeep "$*"
}

# ui_center_text <row_offset> <text> (centers in panel)
ui_center_text() {
    local offset=$1 text=$2
    local row=$(( 6 + offset ))
    local clean; clean=$(printf '%b' "$text" | sed 's/\x1b\[[0-9;]*m//g')
    local pad=$(( (UI_W - ${#clean}) / 2 ))
    _at "$row" $(( UI_X + pad ))
    _bgkeep "$text"
}

# ── key hints bar ─────────────────────────────────────────────────────────────
ui_hints() {
    local row=$(( UI_CONTENT_BOT + 2 ))
    _at "$row" $(( UI_X + 3 ))
    local sep=''
    for h in "$@"; do
        printf '%s' "$sep"
        if [[ "$h" == *:* ]]; then
            local key="${h%%:*}" desc="${h#*:}"
            printf "${BG_MANTLE}${BOLD}${FG_ACCENT}${key}${R}${BG_MANTLE}${FG_DIM} ${desc}${R}"
        else
            printf "${BG_MANTLE}${FG_DIM}${h}${R}"
        fi
        sep="${BG_MANTLE}${FG_DIM}  ·  ${R}"
    done
}

# ── scrollable list menu ──────────────────────────────────────────────────────
# ui_menu <title> <step> <total> <result_var> <item...>
# returns 0=selected, 1=back
ui_menu() {
    local title=$1 step=$2 total=$3 result_var=$4
    shift 4
    local items=("$@") count=${#items[@]} cursor=0 offset=0
    local visible=$(( UI_CONTENT_BOT - 8 ))
    [[ $visible -lt 4 ]] && visible=4

    # draw a single list row (idx = absolute item index)
    _mrow() {
        local idx=$1 vis=$(( $1 - offset ))
        [[ $vis -lt 0 || $vis -ge $visible ]] && return
        local row=$(( 7 + vis ))
        if [[ $idx -eq $cursor ]]; then
            _at "$row" $UI_X; printf "${BG_SEL}%*s${R}" $UI_W ''
            _at "$row" $(( UI_X+3 )); printf "${BG_SEL}${FG_ACCENT}${BOLD}> ${FG_WHITE}${items[$idx]}${R}"
        else
            _fill "$row" $UI_X $UI_W
            _at "$row" $(( UI_X+3 )); printf "${BG_BASE}${FG_DIM}  ${FG_WHITE}${items[$idx]}${R}${BG_BASE}"
        fi
    }

    # redraw entire list area (used on scroll or initial draw)
    _mall() {
        local i
        for (( i=0; i<visible; i++ )); do
            local idx=$(( offset+i )) row=$(( 7+i ))
            if (( idx < count )); then
                if [[ $idx -eq $cursor ]]; then
                    _at "$row" $UI_X; printf "${BG_SEL}%*s${R}" $UI_W ''
                    _at "$row" $(( UI_X+3 )); printf "${BG_SEL}${FG_ACCENT}${BOLD}> ${FG_WHITE}${items[$idx]}${R}"
                else
                    _fill "$row" $UI_X $UI_W
                    _at "$row" $(( UI_X+3 )); printf "${BG_BASE}${FG_DIM}  ${FG_WHITE}${items[$idx]}${R}${BG_BASE}"
                fi
            else
                _fill "$row" $UI_X $UI_W
            fi
        done
        if [[ $count -gt $visible ]]; then
            _at $(( 7+visible+1 )) $(( UI_X+3 ))
            printf "${BG_BASE}${FG_DIM}$(( cursor+1 )) / ${count}${R}${BG_BASE}     "
        fi
    }

    # full initial draw — only once
    ui_clear
    ui_panel_draw "$step" "$total" "$title"
    ui_hints "Up/Down:navigate" "Enter:select" "Esc:back"
    _mall

    local prev=$cursor
    while true; do
        local key; IFS= read -rsn1 key
        prev=$cursor
        case "$key" in
            $'\x1b')
                read -rsn2 -t 0.05 k2
                case "$k2" in
                    '[A')
                        if (( cursor > 0 )); then
                            (( cursor-- ))
                            if (( cursor < offset )); then
                                (( offset-- )); _mall
                            else
                                _mrow $prev; _mrow $cursor
                            fi
                        fi ;;
                    '[B')
                        if (( cursor < count-1 )); then
                            (( cursor++ ))
                            if (( cursor >= offset+visible )); then
                                (( offset++ )); _mall
                            else
                                _mrow $prev; _mrow $cursor
                            fi
                        fi ;;
                    '') return 1 ;;
                esac ;;
            '') printf -v "$result_var" '%s' "${items[$cursor]}"; return 0 ;;
        esac
    done
}

# ── searchable menu ───────────────────────────────────────────────────────────
ui_search_menu() {
    local title=$1 step=$2 total=$3 result_var=$4
    shift 4
    local all=("$@") query='' cursor=0 offset=0
    local visible=$(( UI_CONTENT_BOT - 12 ))
    [[ $visible -lt 3 ]] && visible=3

    # search input field (Surface0 band with Surface1 underline)
    _sbox() {
        local sx=$(( UI_X+3 )) sw=$(( UI_W-8 ))
        _at 7 $sx; printf "${BG_BASE}${FG_DIM}Search${R}${BG_BASE}"
        _at 8 $sx; printf "${BG_SURFACE0}%*s${R}" "$sw" ''
        _at 8 $sx; printf "${BG_SURFACE0} ${FG_WHITE}${query}${FG_ACCENT}_ ${R}"
        _at 9 $sx; printf "${BG_SURFACE1}%*s${R}" "$sw" ''
    }

    _srow() {
        local items_r=("${!1}") idx=$2 vis=$(( $2 - offset ))
        [[ $vis -lt 0 || $vis -ge $visible ]] && return
        local row=$(( 11+vis ))
        if [[ $idx -eq $cursor ]]; then
            _at "$row" $UI_X; printf "${BG_SEL}%*s${R}" $UI_W ''
            _at "$row" $(( UI_X+3 )); printf "${BG_SEL}${FG_ACCENT}${BOLD}> ${FG_WHITE}${items_r[$idx]}${R}"
        else
            _fill "$row" $UI_X $UI_W
            _at "$row" $(( UI_X+3 )); printf "${BG_BASE}${FG_DIM}  ${FG_WHITE}${items_r[$idx]}${R}${BG_BASE}"
        fi
    }

    _slist() {
        local items_r=("${!1}") cnt=$2
        local i
        for (( i=0; i<visible; i++ )); do
            local idx=$(( offset+i )) row=$(( 11+i ))
            if (( idx < cnt )); then
                if [[ $idx -eq $cursor ]]; then
                    _at "$row" $UI_X; printf "${BG_SEL}%*s${R}" $UI_W ''
                    _at "$row" $(( UI_X+3 )); printf "${BG_SEL}${FG_ACCENT}${BOLD}> ${FG_WHITE}${items_r[$idx]}${R}"
                else
                    _fill "$row" $UI_X $UI_W
                    _at "$row" $(( UI_X+3 )); printf "${BG_BASE}${FG_DIM}  ${FG_WHITE}${items_r[$idx]}${R}${BG_BASE}"
                fi
            else
                _fill "$row" $UI_X $UI_W
            fi
        done
        [[ $cnt -eq 0 ]] && { _at 12 $(( UI_X+3 )); printf "${BG_BASE}${FG_DIM}no matches      ${R}${BG_BASE}"; }
    }

    # build initial filtered list
    local items=()
    for item in "${all[@]}"; do
        [[ -z "$query" || "${item,,}" == *"${query,,}"* ]] && items+=("$item")
    done
    local count=${#items[@]}

    # full draw once
    ui_clear
    ui_panel_draw "$step" "$total" "$title"
    ui_hints "Up/Down:navigate" "type:filter" "Enter:select" "Esc:back"
    _sbox
    _slist items[@] $count

    local prev=$cursor prev_offset=$offset
    while true; do
        local key; IFS= read -rsn1 key
        prev=$cursor; prev_offset=$offset
        local query_changed=0

        case "$key" in
            $'\x1b')
                read -rsn2 -t 0.05 k2
                case "$k2" in
                    '[A') (( cursor>0 )) && (( cursor-- )); (( cursor<offset )) && (( offset-- )) ;;
                    '[B') (( cursor<count-1 )) && (( cursor++ )); (( cursor>=offset+visible )) && (( offset++ )) ;;
                    '') return 1 ;;
                esac ;;
            '')
                [[ $count -gt 0 ]] && { printf -v "$result_var" '%s' "${items[$cursor]}"; return 0; } ;;
            $'\x7f'|$'\b') query="${query%?}"; cursor=0; offset=0; query_changed=1 ;;
            *) [[ ${#key} -eq 1 && "$key" =~ [[:print:]] ]] && { query+="$key"; cursor=0; offset=0; query_changed=1; } ;;
        esac

        if [[ $query_changed -eq 1 ]]; then
            items=()
            for item in "${all[@]}"; do
                [[ -z "$query" || "${item,,}" == *"${query,,}"* ]] && items+=("$item")
            done
            count=${#items[@]}
            (( cursor >= count && count > 0 )) && cursor=$(( count-1 ))
            _sbox; _slist items[@] $count
        elif [[ $offset -ne $prev_offset ]]; then
            _slist items[@] $count
        elif [[ $cursor -ne $prev ]]; then
            _srow items[@] $prev; _srow items[@] $cursor
        fi
    done
}

# ── text input ────────────────────────────────────────────────────────────────
ui_input() {
    local title=$1 step=$2 total=$3 prompt=$4 result_var=$5 default=${6:-}
    local value="$default"
    printf '\e[?25h'

    while true; do
        ui_clear
        ui_panel_draw "$step" "$total" "$title"
        ui_hints "Enter:confirm" "Esc:back"

        local sx=$(( UI_X+3 )) sw=$(( UI_W-6 ))
        _at 8 $sx; printf "${BG_BASE}${FG_SUBTEXT}${prompt}${R}${BG_BASE}"
        _at 10 $sx; printf "${BG_SURFACE1}%*s${R}" "$sw" ''
        _at 11 $sx; printf "${BG_SURFACE0}%*s${R}" "$sw" ''
        _at 11 $sx; printf "${BG_SURFACE0} ${FG_WHITE}${value}${FG_ACCENT}_ ${R}"
        _at 12 $sx; printf "${BG_SURFACE1}%*s${R}" "$sw" ''

        local key; IFS= read -rsn1 key
        case "$key" in
            $'\x1b') read -rsn2 -t 0.05 k2; [[ -z "$k2" ]] && { printf '\e[?25l'; return 1; } ;;
            '') printf '\e[?25l'; printf -v "$result_var" '%s' "$value"; return 0 ;;
            $'\x7f'|$'\b') value="${value%?}" ;;
            *) [[ ${#key} -eq 1 && "$key" =~ [[:print:]] ]] && value+="$key" ;;
        esac
    done
}

# ── password input ────────────────────────────────────────────────────────────
ui_password() {
    local title=$1 step=$2 total=$3 prompt=$4 result_var=$5
    local value=''
    printf '\e[?25h'

    _draw_pass() {
        local v=$1 label=$2
        local sx=$(( UI_X+3 )) sw=$(( UI_W-6 ))
        local stars=''; local n=${#v}; while (( n-- > 0 )); do stars+='*'; done
        _at 8 $sx; printf "${BG_BASE}${FG_SUBTEXT}${prompt}${R}${BG_BASE}"
        _at 10 $sx; printf "${BG_BASE}${FG_DIM}${label}${R}${BG_BASE}"
        _at 11 $sx; printf "${BG_SURFACE1}%*s${R}" "$sw" ''
        _at 12 $sx; printf "${BG_SURFACE0}%*s${R}" "$sw" ''
        _at 12 $sx; printf "${BG_SURFACE0} ${FG_ACCENT}${stars}_ ${R}"
        _at 13 $sx; printf "${BG_SURFACE1}%*s${R}" "$sw" ''
    }

    while true; do
        ui_clear
        ui_panel_draw "$step" "$total" "$title"
        ui_hints "Enter:confirm" "Esc:back"
        _draw_pass "$value" "Password:"

        local key; IFS= read -rsn1 key
        case "$key" in
            $'\x1b') read -rsn2 -t 0.05 k2; [[ -z "$k2" ]] && { printf '\e[?25l'; return 1; } ;;
            '')
                [[ -z "$value" ]] && continue
                # confirm
                local cv=''
                while true; do
                    ui_clear
                    ui_panel_draw "$step" "$total" "$title"
                    ui_hints "Enter:confirm" "Esc:retype"
                    _draw_pass "$cv" "Confirm password:"
                    IFS= read -rsn1 ckey
                    case "$ckey" in
                        $'\x1b') read -rsn2 -t 0.05 ck2; [[ -z "$ck2" ]] && { cv=''; break; } ;;
                        '')
                            if [[ "$value" == "$cv" ]]; then
                                printf '\e[?25l'
                                printf -v "$result_var" '%s' "$value"
                                return 0
                            else
                                local sx=$(( UI_X+3 ))
                                _at 15 $sx; printf "${BG_BASE}${FG_RED}Passwords do not match — try again${R}${BG_BASE}"
                                sleep 1.2; cv=''
                            fi ;;
                        $'\x7f'|$'\b') cv="${cv%?}" ;;
                        *) [[ ${#ckey} -eq 1 ]] && cv+="$ckey" ;;
                    esac
                done ;;
            $'\x7f'|$'\b') value="${value%?}" ;;
            *) [[ ${#key} -eq 1 ]] && value+="$key" ;;
        esac
    done
}

# ── yes/no confirm ────────────────────────────────────────────────────────────
ui_confirm() {
    local title=$1 step=$2 total=$3 msg=$4
    local sel=0

    while true; do
        ui_clear
        ui_panel_draw "$step" "$total" "$title"
        ui_hints "Left/Right:choose" "Enter:confirm" "Esc:back"

        local sx=$(( UI_X+3 ))
        _at 8 $sx; printf "${BG_BASE}${FG_WHITE}${msg}${R}${BG_BASE}"

        _at 11 $sx
        if [[ $sel -eq 0 ]]; then
            printf "${BG_SEL}${BOLD}${FG_WHITE}  Yes  ${R}   ${FG_DIM}  No  ${R}"
        else
            printf "${BG_BASE}${FG_DIM}  Yes  ${R}${BG_BASE}   ${BG_SEL}${BOLD}${FG_WHITE}  No  ${R}${BG_BASE}"
        fi

        local key; IFS= read -rsn1 key
        case "$key" in
            $'\x1b')
                read -rsn2 -t 0.05 k2
                case "$k2" in
                    '[C'|'[D') sel=$(( 1-sel )) ;;
                    '') return 1 ;;
                esac ;;
            '') return $sel ;;
            'y'|'Y') return 0 ;;
            'n'|'N') return 1 ;;
        esac
    done
}

# ── progress bar ──────────────────────────────────────────────────────────────
# Colored-fill bar: Mauve for filled, Surface0 for empty — no characters needed
ui_progress() {
    local offset=$1 pct=$2 label=$3
    local row=$(( 6+offset )) sx=$(( UI_X+3 ))
    local bw=$(( UI_W-10 ))
    local filled=$(( pct*bw/100 ))
    local empty=$(( bw - filled ))

    _at "$row" $sx
    printf "${BG_BASE}${FG_SUBTEXT}%s%*s${R}" "$label" $(( UI_W-6-${#label} )) ''
    _at $(( row+1 )) $sx
    printf "${_BG_MAUVE}%*s${R}" "$filled" ''
    printf "${BG_SURFACE0}%*s${R}" "$empty" ''
    printf "${BG_BASE}  ${BOLD}${FG_WHITE}${pct}%%${R}"
}

# ── spinner ───────────────────────────────────────────────────────────────────
_SP=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
_SI=0
ui_spin() {
    local offset=$1 label=$2
    _at $(( 6+offset )) $(( UI_X+3 ))
    printf "${BG_BASE}${FG_ACCENT}${_SP[$_SI]}${R}${BG_BASE}  ${FG_DIM}${label}${R}${BG_BASE}"
    _SI=$(( (_SI+1)%10 ))
}

# ── notification flash ────────────────────────────────────────────────────────
ui_flash() {
    local msg=$1 color=${2:-$FG_GREEN}
    _at $(( UI_CONTENT_BOT-1 )) $(( UI_X+3 ))
    printf "${color}${msg}${R}"
}

# ── disk visualization ────────────────────────────────────────────────────────
# A proportional, accurate partition map driven by disk_map (real byte sizes,
# physical order, free-space gaps included) plus a readable table beneath it.
# Sets UI_DISK_BAR_ROWS to the number of content rows it consumed so callers
# can lay out controls below it.

# pick the band bg + fg + short type name for a segment
_ui_seg_color() {
    # args: kind fs  → sets _SEG_BG _SEG_FG _SEG_KIND
    local kind=$1 fs=$2
    if [[ "$kind" == free ]]; then
        _SEG_BG="${_BG_GREEN}"; _SEG_FG="${FG_GREEN}"; _SEG_KIND="Free"; return
    fi
    case "$fs" in
        vfat|fat32|fat16) _SEG_BG="${_BG_YELLOW}"; _SEG_FG="${FG_YELLOW}"; _SEG_KIND="${fs:-EFI}" ;;
        btrfs|ext4|xfs)   _SEG_BG="${_BG_MAUVE}";  _SEG_FG="${FG_ACCENT}"; _SEG_KIND="$fs" ;;
        swap)             _SEG_BG="${_BG_ORANGE}"; _SEG_FG="${FG_ORANGE}"; _SEG_KIND="swap" ;;
        ntfs|fuseblk)     _SEG_BG="${_BG_RED}";    _SEG_FG="${FG_RED}";    _SEG_KIND="ntfs" ;;
        *)                _SEG_BG="${BG_SURFACE1}";_SEG_FG="${FG_SUBTEXT}";_SEG_KIND="${fs:-raw}" ;;
    esac
}

UI_DISK_BAR_ROWS=0
ui_disk_bar() {
    local disk=$1 offset=$2
    local bar_w=$(( UI_W-6 )) sx=$(( UI_X+3 ))
    local top=$(( 6+offset ))
    UI_DISK_BAR_ROWS=0

    local total_b; total_b=$(lsblk -dno SIZE --bytes "$disk" 2>/dev/null)
    # fall back to parted for anything lsblk won't report (e.g. image files)
    [[ -z "$total_b" || "$total_b" == 0 ]] && \
        total_b=$(parted -sm "$disk" unit B print 2>/dev/null | sed -n '2{s/[^:]*:\([0-9]*\)B:.*/\1/p}')
    [[ -z "$total_b" || "$total_b" == 0 ]] && return

    # load the map into parallel arrays (so we can size the bar and the table
    # from the same data in one pass)
    local -a kind=() num=() size=() dev=() fs=() label=() mount=()
    while IFS='|' read -r k n st sz dv f l m; do
        kind+=("$k"); num+=("$n"); size+=("$sz"); dev+=("$dv")
        fs+=("$f"); label+=("$l"); mount+=("$m")
    done < <(disk_map "$disk")
    local segs=${#kind[@]}

    # header: disk path + total size
    _at $top $sx
    printf "${BG_BASE}${FG_WHITE}${BOLD}${disk}${R}${BG_BASE}  ${FG_SUBTEXT}$(disk_human "$total_b")${R}${BG_BASE}"

    # proportional colored bar — last segment absorbs rounding remainder
    local bar_row=$(( top+1 ))
    _at $bar_row $sx
    if [[ $segs -eq 0 ]]; then
        printf "${_BG_GREEN}%*s${R}" "$bar_w" ''
    else
        local used=0 i
        for (( i=0; i<segs; i++ )); do
            local w
            if (( i == segs-1 )); then
                w=$(( bar_w - used ))
            else
                w=$(( size[i] * bar_w / total_b ))
                (( w < 1 )) && w=1
                used=$(( used + w ))
            fi
            (( w < 0 )) && w=0
            _ui_seg_color "${kind[i]}" "${fs[i]}"
            printf "${_SEG_BG}%*s${R}" "$w" ''
        done
    fi

    # readable table: swatch · device · size · type · label/mount
    local trow=$(( bar_row+2 )) i shown=0
    for (( i=0; i<segs; i++ )); do
        _ui_seg_color "${kind[i]}" "${fs[i]}"
        _at $trow $sx
        if [[ "${kind[i]}" == free ]]; then
            printf "${_SEG_BG}  ${R}${BG_BASE}  ${FG_GREEN}%-14s${R}${BG_BASE}${FG_SUBTEXT}%9s${R}${BG_BASE}  ${FG_DIM}free space${R}${BG_BASE}" \
                "free" "$(disk_human "${size[i]}")"
        else
            local extra="${mount[i]:+↳ ${mount[i]}}"
            [[ -z "$extra" && -n "${label[i]}" ]] && extra="[${label[i]}]"
            printf "${_SEG_BG}  ${R}${BG_BASE}  ${FG_WHITE}%-14s${R}${BG_BASE}${FG_SUBTEXT}%9s${R}${BG_BASE}  ${_SEG_FG}%-7s${R}${BG_BASE}${FG_DIM}%s${R}${BG_BASE}" \
                "${dev[i]##*/}" "$(disk_human "${size[i]}")" "${_SEG_KIND}" "$extra"
        fi
        (( trow++ )); (( shown++ ))
        # don't run past the panel
        (( trow > UI_CONTENT_BOT-1 )) && break
    done

    UI_DISK_BAR_ROWS=$(( (trow - top) ))
}
