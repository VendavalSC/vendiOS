#!/usr/bin/env bash
# vendiOS TUI library

# ── Catppuccin Mocha palette (Mauve accent) ───────────────────────────────────
R=$'\e[0m'
BOLD=$'\e[1m'
DIM=$'\e[2m'
ITALIC=$'\e[3m'
UL=$'\e[4m'

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
BG_PANEL=$'\e[48;2;24;24;37m'        # alias Mantle
BG_HEADER=$'\e[48;2;17;17;27m'       # alias Crust

# inline background escapes (for colored-space fills — no Unicode needed)
_BG_MAUVE=$'\e[48;2;203;166;247m'
_BG_GREEN=$'\e[48;2;166;227;161m'
_BG_RED=$'\e[48;2;243;139;168m'
_BG_YELLOW=$'\e[48;2;249;226;175m'
_BG_ORANGE=$'\e[48;2;250;179;135m'

# combined shortcuts
ACCENT_ON="${BG_SEL}${BOLD}${FG_ACCENT}"

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
    export TERM=xterm-256color
    ui_resize
    printf '\e[?25l\e[?7l'
    trap ui_cleanup EXIT INT TERM
    trap 'ui_resize; ui_redraw' WINCH
}

ui_cleanup() {
    printf '\e[?25h\e[?7h\e[0m'
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

    # Rows 4-5: base padding
    _at 4 $x; printf "${BG_BASE}%*s${R}" "$w" ''
    _at 5 $x; printf "${BG_BASE}%*s${R}" "$w" ''

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

# ── content helpers ───────────────────────────────────────────────────────────
# ui_pline <row_offset_from_6> <text_with_colors>
ui_pline() {
    local offset=$1; shift
    local row=$(( 6 + offset ))
    _at "$row" $(( UI_X + 3 ))
    printf '%b' "$*"
}

# ui_center_text <row_offset> <text> (centers in panel)
ui_center_text() {
    local offset=$1 text=$2
    local row=$(( 6 + offset ))
    local clean; clean=$(printf '%b' "$text" | sed 's/\x1b\[[0-9;]*m//g')
    local pad=$(( (UI_W - ${#clean}) / 2 ))
    _at "$row" $(( UI_X + pad ))
    printf '%b' "$text"
}

# ── key hints bar ─────────────────────────────────────────────────────────────
ui_hints() {
    local row=$(( UI_CONTENT_BOT + 2 ))
    _at "$row" $(( UI_X + 3 ))
    local sep=''
    for h in "$@"; do
        local key="${h%%:*}" desc="${h#*:}"
        printf '%s' "$sep"
        printf "${BG_MANTLE}${BOLD}${FG_ACCENT}${key}${R}${BG_MANTLE}${FG_DIM} ${desc}${R}"
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
            _at "$row" $(( UI_X+3 )); printf "${FG_DIM}  ${FG_WHITE}${items[$idx]}${R}"
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
                    _at "$row" $(( UI_X+3 )); printf "${FG_DIM}  ${FG_WHITE}${items[$idx]}${R}"
                fi
            else
                _fill "$row" $UI_X $UI_W
            fi
        done
        if [[ $count -gt $visible ]]; then
            _at $(( 7+visible+1 )) $(( UI_X+3 ))
            printf "${FG_DIM}$(( cursor+1 )) / ${count}${R}     "
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
        _at 7 $sx; printf "${FG_DIM}Search${R}"
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
            _at "$row" $(( UI_X+3 )); printf "${FG_DIM}  ${FG_WHITE}${items_r[$idx]}${R}"
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
                    _at "$row" $(( UI_X+3 )); printf "${FG_DIM}  ${FG_WHITE}${items_r[$idx]}${R}"
                fi
            else
                _fill "$row" $UI_X $UI_W
            fi
        done
        [[ $cnt -eq 0 ]] && { _at 12 $(( UI_X+3 )); printf "${FG_DIM}no matches      ${R}"; }
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
        _at 8 $sx; printf "${FG_SUBTEXT}${prompt}${R}"
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
        _at 8 $sx; printf "${FG_SUBTEXT}${prompt}${R}"
        _at 10 $sx; printf "${FG_DIM}${label}${R}"
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
                                _at 15 $sx; printf "${FG_RED}Passwords do not match — try again${R}"
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
        _at 8 $sx; printf "${FG_WHITE}${msg}${R}"

        _at 11 $sx
        if [[ $sel -eq 0 ]]; then
            printf "${BG_SEL}${BOLD}${FG_WHITE}  Yes  ${R}   ${FG_DIM}  No  ${R}"
        else
            printf "${FG_DIM}  Yes  ${R}   ${BG_SEL}${BOLD}${FG_WHITE}  No  ${R}"
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

    _at "$row" $sx; printf "${FG_SUBTEXT}${label}${R}"
    _at $(( row+1 )) $sx
    printf "${_BG_MAUVE}%*s${R}" "$filled" ''
    printf "${BG_SURFACE0}%*s${R}" "$empty" ''
    printf "  ${BOLD}${FG_WHITE}${pct}%%${R}"
}

# ── spinner ───────────────────────────────────────────────────────────────────
_SP=('⠋' '⠙' '⠹' '⠸' '⠼' '⠴' '⠦' '⠧' '⠇' '⠏')
_SI=0
ui_spin() {
    local offset=$1 label=$2
    _at $(( 6+offset )) $(( UI_X+3 ))
    printf "${FG_ACCENT}${_SP[$_SI]}${R}  ${FG_DIM}${label}${R}"
    _SI=$(( (_SI+1)%10 ))
}

# ── notification flash ────────────────────────────────────────────────────────
ui_flash() {
    local msg=$1 color=${2:-$FG_GREEN}
    _at $(( UI_CONTENT_BOT-1 )) $(( UI_X+3 ))
    printf "${color}${msg}${R}"
}

# ── disk visualization bar ────────────────────────────────────────────────────
# Draws a colored-band proportional partition map — no box chars needed
ui_disk_bar() {
    local disk=$1 offset=$2
    local bar_w=$(( UI_W-10 )) sx=$(( UI_X+3 ))

    local total_b; total_b=$(lsblk -dno SIZE --bytes "$disk" 2>/dev/null || echo 0)
    [[ $total_b -eq 0 ]] && return

    local -a parts=()
    while IFS= read -r line; do
        parts+=("$line")
    done < <(lsblk -Pno NAME,SIZE,FSTYPE,LABEL "$disk" 2>/dev/null | \
             grep -v "^NAME=\"${disk##*/}\"" | \
             awk -F'"' '{print $2 "|" $4 "|" $6 "|" $8}')

    # header: disk path + size
    _at $(( 6+offset )) $sx
    printf "${FG_SUBTEXT}${disk}${R}  ${FG_WHITE}$(lsblk -dno SIZE "$disk" 2>/dev/null)${R}"

    local n=${#parts[@]}
    _at $(( 7+offset )) $sx

    if [[ $n -eq 0 ]]; then
        # whole disk free
        printf "${_BG_GREEN}%*s${R}" "$bar_w" ''
        _at $(( 8+offset )) $sx; printf "${FG_GREEN}free space${R}"
        _at $(( 9+offset )) $sx
        printf "${_BG_GREEN}   ${R}${FG_GREEN} Free${R}"
        return
    fi

    # colored-band segments — proportional by partition count (simple equal split)
    local seg_w=$(( bar_w / n ))
    local i
    for (( i=0; i<n; i++ )); do
        IFS='|' read -r pname psize pfs plabel <<< "${parts[$i]}"
        local w=$seg_w
        (( i == n-1 )) && w=$(( bar_w - seg_w*(n-1) ))
        local bg
        case "$pfs" in
            vfat|fat32|fat16) bg="${_BG_YELLOW}" ;;
            btrfs|ext4|xfs)   bg="${_BG_MAUVE}"  ;;
            swap)              bg="${_BG_ORANGE}" ;;
            ntfs|fuseblk)     bg="${_BG_RED}"    ;;
            *)                 bg="${_BG_GREEN}"  ;;
        esac
        printf "${bg}%*s${R}" "$w" ''
    done

    # partition labels below bar
    for (( i=0; i<n; i++ )); do
        IFS='|' read -r pname psize pfs plabel <<< "${parts[$i]}"
        local lbl="${pname##*/}"
        local lpad=$(( seg_w*i + seg_w/2 - ${#lbl}/2 ))
        _at $(( 8+offset )) $(( sx+lpad ))
        printf "${FG_DIM}${lbl}${R}"
    done

    # color legend using colored squares (filled spaces)
    _at $(( 9+offset )) $sx
    printf "${_BG_YELLOW}   ${R}${FG_YELLOW} EFI  "
    printf "${_BG_MAUVE}   ${R}${FG_ACCENT} Linux  "
    printf "${_BG_RED}   ${R}${FG_RED} Windows  "
    printf "${_BG_GREEN}   ${R}${FG_GREEN} Free  "
    printf "${_BG_ORANGE}   ${R}${FG_ORANGE} Swap${R}"
}
