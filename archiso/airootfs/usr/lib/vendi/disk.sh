#!/usr/bin/env bash
# vendiOS disk library

FIRMWARE='bios'
[[ -d /sys/firmware/efi/efivars ]] && FIRMWARE='uefi'

# ── disk enumeration ──────────────────────────────────────────────────────────
disk_list() {
    lsblk -dno NAME,SIZE,MODEL,TYPE 2>/dev/null > /tmp/lsblk.log
    local name size model
    while read -r name; do
        [[ -z "$name" ]] && continue
        size=$(lsblk -dno SIZE "/dev/${name}" 2>/dev/null)
        model=$(lsblk -dno MODEL "/dev/${name}" 2>/dev/null | sed 's/^[[:space:]]*//;s/[[:space:]]*$//')
        [[ -z "$model" ]] && model="Unknown"
        echo "/dev/${name}|${size}|${model}"
    done < <(lsblk -dno NAME,TYPE 2>/dev/null | awk '$NF=="disk" {print $1}')
}

disk_is_nvme() { [[ "$1" == *nvme* || "$1" == *mmcblk* ]]; }
disk_part()    { disk_is_nvme "$1" && echo "${1}p${2}" || echo "${1}${2}"; }

# Human-readable bytes → "1.8 TiB" / "512 MiB". Used everywhere we show a size.
disk_human() {
    local b=${1:-0}
    if   (( b >= 1024**4 )); then awk "BEGIN{printf \"%.1f TiB\", $b/1024^4}"
    elif (( b >= 1024**3 )); then awk "BEGIN{printf \"%.0f GiB\", $b/1024^3}"
    elif (( b >= 1024**2 )); then awk "BEGIN{printf \"%.0f MiB\", $b/1024^2}"
    elif (( b >= 1024 ));    then awk "BEGIN{printf \"%.0f KiB\", $b/1024}"
    else echo "${b} B"; fi
}

# disk_map <disk> — the single source of truth for the disk layout, in physical
# order, including free-space gaps. parted's byte output gives true geometry
# (the old equal-split bar lied about proportions). One row per segment:
#
#   KIND|NUM|START_B|SIZE_B|DEV|FS|LABEL|MOUNT
#
# KIND is 'part' or 'free'. For free gaps only START_B/SIZE_B are set. Gaps
# smaller than 2 MiB (alignment slack) are dropped so they don't clutter.
disk_map() {
    local disk=$1 line
    parted -sm "$disk" unit B print free 2>/dev/null | tail -n +3 | while IFS= read -r line; do
        line=${line%;}
        local num s e size type
        IFS=':' read -r num s e size type _ <<<"$line"
        local start=${s%B} bytes=${size%B}
        if [[ "$type" == free ]]; then
            (( bytes < 2*1024*1024 )) && continue
            echo "free||${start}|${bytes}||||"
        else
            local dev fs label mount
            dev=$(disk_part "$disk" "$num")
            fs=$(lsblk -dno FSTYPE "$dev" 2>/dev/null | head -1)
            label=$(lsblk -dno LABEL "$dev" 2>/dev/null | head -1)
            mount=$(lsblk -dno MOUNTPOINT "$dev" 2>/dev/null | head -1)
            echo "part|${num}|${start}|${bytes}|${dev}|${fs}|${label}|${mount}"
        fi
    done
}

# ── free space detection ──────────────────────────────────────────────────────
disk_largest_free() {
    local disk=$1
    # parted unit MiB print free:
    # Col 1: Start, Col 2: End, Col 3: Size
    parted -s "$disk" unit MiB print free 2>/dev/null | \
    awk '/Free Space/ {
        gsub(/MiB/,"")
        start = $1
        end = $2
        size = $3
        if (size > max) { max=size; best_start=start; best_end=end }
    } END {
        if (max > 0) print best_start, best_end, max
    }'
}

disk_free_ok() {
    local result; result=$(disk_largest_free "$1")
    local size_mib; size_mib=$(echo "$result" | awk '{print $3}')
    [[ -n "$size_mib" && "${size_mib%.*}" -ge 8192 ]]
}

disk_list_partitions() {
    local disk=$1
    lsblk -lnp -o NAME,SIZE,FSTYPE,LABEL,MOUNTPOINT "$disk" | grep -v "^$disk$"
}

disk_delete_partition() {
    local part=$1
    local disk; disk=$(lsblk -no PKNAME "$part" | head -1)
    [[ -z "$disk" ]] && disk=$(echo "$part" | sed -E 's/p?[0-9]+$//')
    [[ "$disk" != /* ]] && disk="/dev/$disk"
    
    local num; num=$(cat "/sys/class/block/${part##*/}/partition" 2>/dev/null)
    [[ -z "$num" ]] && return 1

    if [[ "$FIRMWARE" == 'uefi' ]]; then
        sgdisk --delete "$num" "$disk" >/dev/null 2>&1
    else
        parted -s "$disk" rm "$num" >/dev/null 2>&1
    fi
    udevadm settle
    partprobe "$disk"
}

disk_find_esp() {
    local disk=$1
    # Check for EFI System Partition GUID on GPT
    lsblk -no NAME,PARTTYPE "$disk" 2>/dev/null | \
        awk '/c12a7328-f81f-11d2-ba4b-00a0c93ec93b/{print "/dev/"$1}' | head -1
}

# ── partitioning ──────────────────────────────────────────────────────────────
# IMPORTANT: limine only reads FAT32 (and ISO9660). The kernel, initramfs,
# limine.conf, and limine-bios.sys must live on a FAT32 partition.
# Layout for both BIOS and UEFI: p1 = FAT32 /boot (513 MiB), p2 = root.

disk_wipe() {
    wipefs -af "$1" >/dev/null 2>&1
    sgdisk --zap-all "$1" >/dev/null 2>&1
}

disk_partition_full_uefi() {
    local disk=$1
    # Use sgdisk for better scripting reliability on UEFI/GPT
    sgdisk --clear \
           --new=1:1MiB:513MiB  --typecode=1:ef00 --change-name=1:EFI \
           --new=2:514MiB:0     --typecode=2:8304 --change-name=2:ROOT \
           "$disk" >/dev/null 2>&1
}

disk_partition_full_bios() {
    local disk=$1
    parted -s "$disk" mklabel msdos \
        mkpart primary fat32  1MiB  513MiB set 1 boot on \
        mkpart primary       513MiB 100%
}

# MODE: install into largest free space (UEFI only)
disk_partition_free_space() {
    local disk=$1

    if [[ "$FIRMWARE" != 'uefi' ]]; then
        echo ""
        return 1
    fi

    # ── Snapshot + Sort Method ──
    # 1. Take snapshot of existing partitions
    local before; before=$(lsblk -ln -o NAME "$disk" | sort)

    # Always carve vendiOS its OWN 1 GiB EFI partition + ROOT inside the free
    # block. We deliberately do NOT reuse a pre-existing (Windows) ESP: those
    # are typically 100 MiB, too small for our kernel+initramfs, and writing the
    # kernel there risks running it out of space or colliding with another
    # distro's `vmlinuz-linux`. A dedicated ESP keeps vendiOS self-contained and
    # never touches the Windows bootloader.
    #
    # IMPORTANT: feed sgdisk's OWN free-space detection (start/end = 0), NOT
    # parted's MiB figures. parted rounds its print-free output to whole MiB,
    # so the "start" it reports truncates DOWN into the preceding partition and
    # the "end" rounds UP past the GPT backup header at the disk tail — both
    # make sgdisk refuse to create the partition (the dual-boot install bug).
    # sgdisk's 0:0:0 picks the largest free block with sector-exact, 2048-
    # aligned boundaries and correctly reserves the GPT tail.
    sgdisk --new=0:0:+1GiB --typecode=0:ef00 --change-name=0:VENDIBOOT \
           --new=0:0:0     --typecode=0:8304 --change-name=0:ROOT \
           "$disk" >/dev/null 2>&1

    # Correct order: partprobe → udevadm (settle waits for events partprobe triggers)
    partprobe "$disk" 2>/dev/null || true
    partx -u  "$disk" 2>/dev/null || true
    udevadm trigger --action=add --subsystem-match=block 2>/dev/null || true
    udevadm settle --timeout=15

    # 2. Identify new partitions
    local after; after=$(lsblk -ln -o NAME "$disk" | sort)
    local new_parts; new_parts=$(comm -13 <(echo "$before") <(echo "$after"))

    # 3. Sort by physical start position (EFI sits below ROOT in the block)
    local sorted_parts; sorted_parts=$(
        for p in $new_parts; do
            echo "/dev/$p $(cat "/sys/class/block/$p/start")"
        done | sort -nk2 | awk '{print $1}'
    )

    # EFI = parts[0] (lower start), ROOT = parts[1]; IS_NEW_EFI=1 → format it.
    local parts=($sorted_parts)
    echo "${parts[0]} ${parts[1]} 1"
}

# ── formatting ────────────────────────────────────────────────────────────────
disk_format_boot()       { mkfs.fat -F32 -n VENDIBOOT "$1" >/dev/null 2>&1; }
disk_format_root_btrfs() { mkfs.btrfs -f -L vendios "$1" >/dev/null 2>&1; }
disk_format_root_ext4()  { mkfs.ext4  -F -L vendios "$1" >/dev/null 2>&1; }

# ── LUKS encryption ───────────────────────────────────────────────────────────
# disk_luks_format <partition> <passphrase>  → creates LUKS container
# disk_luks_open   <partition> <passphrase>  → opens at /dev/mapper/cryptroot
# disk_luks_uuid   <partition>               → UUID of the LUKS header
disk_luks_format() {
    local part=$1 pass=$2
    printf '%s' "$pass" | cryptsetup --batch-mode --type luks2 \
        --cipher aes-xts-plain64 --key-size 512 --hash sha512 \
        --pbkdf argon2id luksFormat "$part" -
}

disk_luks_open() {
    local part=$1 pass=$2
    printf '%s' "$pass" | cryptsetup --batch-mode open "$part" cryptroot -
}

disk_luks_uuid() { blkid -s UUID -o value "$1" 2>/dev/null; }

# Legacy alias — used by older code paths
disk_format_efi() { disk_format_boot "$1"; }

# ── mounting ──────────────────────────────────────────────────────────────────
# Both BIOS and UEFI: boot partition mounts at /mnt/boot. The kernel installed
# by pacstrap lands directly on the FAT32 partition where Limine can read it.

disk_mount_btrfs() {
    local root=$1 boot=${2:-}
    {
        mount "$root" /mnt
        btrfs subvolume create /mnt/@
        btrfs subvolume create /mnt/@home
        btrfs subvolume create /mnt/@var
        btrfs subvolume create /mnt/@snapshots
        umount /mnt

        local opts='noatime,compress=zstd:1,space_cache=v2'
        mount -o "${opts},subvol=@"          "$root" /mnt
        mkdir -p /mnt/{home,var,.snapshots,boot}
        mount -o "${opts},subvol=@home"      "$root" /mnt/home
        mount -o "${opts},subvol=@var"       "$root" /mnt/var
        mount -o "${opts},subvol=@snapshots" "$root" /mnt/.snapshots
        if [[ -n "$boot" ]]; then
            mount "$boot" /mnt/boot
        fi
    } >/dev/null 2>&1
}

disk_mount_ext4() {
    local root=$1 boot=${2:-}
    {
        mount "$root" /mnt
        if [[ -n "$boot" ]]; then
            mkdir -p /mnt/boot
            mount "$boot" /mnt/boot
        fi
    } >/dev/null 2>&1
}

# ── limine install ────────────────────────────────────────────────────────────
# Unified: kernel + initramfs + limine.conf + (limine-bios.sys on BIOS) all
# live on the FAT32 /boot partition. Pacstrap put the kernel there already.

limine_install() {
    # Args: <disk> <firmware> <root_partuuid> <fs> [luks_uuid]
    # When luks_uuid is set, the cmdline uses cryptdevice= + /dev/mapper/cryptroot
    # so plymouth-encrypt unlocks the root before mounting.
    local disk=$1 firmware=$2 root_partuuid=$3 fs=$4 luks_uuid=${5:-}
    local cmdline=''

    if [[ -n "$luks_uuid" ]]; then
        cmdline="cryptdevice=UUID=${luks_uuid}:cryptroot root=/dev/mapper/cryptroot"
    else
        cmdline="root=PARTUUID=${root_partuuid}"
    fi
    cmdline+=" rw quiet loglevel=3"                            # only warnings+errors
    cmdline+=" rd.systemd.show_status=false rd.udev.log_level=0"  # silence udev in initramfs
    cmdline+=" systemd.show_status=false udev.log_level=0"
    cmdline+=" vt.global_cursor_default=0 fbcon=nodefer"        # no cursor, splash unhidden
    cmdline+=" splash bgrt_disable"                              # Plymouth, no UEFI bgrt
    [[ "$fs" == 'btrfs' ]] && cmdline+=' rootflags=subvol=@'

    local boot=/mnt/boot

    # write limine.conf — same format for both BIOS and UEFI
    {
        echo "timeout: 0"
        echo "quiet: yes"      # suppress Limine's "loading kernel…" boot chatter
        echo "serial: no"
        echo ""
        echo "/vendiOS"
        echo "    protocol: linux"
        echo "    kernel_path: boot():/vmlinuz-linux"
        echo "    cmdline: ${cmdline}"
        echo "    module_path: boot():/initramfs-linux.img"
        [[ -f "${boot}/amd-ucode.img"   ]] && echo "    module_path: boot():/amd-ucode.img"
        [[ -f "${boot}/intel-ucode.img" ]] && echo "    module_path: boot():/intel-ucode.img"
    } > "${boot}/limine.conf"

    if [[ "$firmware" == 'uefi' ]]; then
        # Place the bootloader at BOTH our named path AND the removable-media
        # fallback (/EFI/BOOT/BOOTX64.EFI). vendiOS owns this ESP outright, so
        # the fallback is safe and means the disk boots even if the firmware
        # drops/ignores the NVRAM entry. Copy failures here are FATAL (a full or
        # unwritable ESP must not be silently ignored).
        mkdir -p "${boot}/EFI/vendiOS" "${boot}/EFI/BOOT"
        cp /usr/share/limine/BOOTX64.EFI "${boot}/EFI/vendiOS/BOOTX64.EFI" || {
            echo "limine: FAILED to copy BOOTX64.EFI to ${boot}/EFI/vendiOS (ESP full/unwritable?)" >&2
            return 1
        }
        cp /usr/share/limine/BOOTX64.EFI "${boot}/EFI/BOOT/BOOTX64.EFI" || {
            echo "limine: FAILED to copy BOOTX64.EFI to ${boot}/EFI/BOOT" >&2
            return 1
        }
        cp /usr/share/limine/BOOTIA32.EFI "${boot}/EFI/vendiOS/BOOTIA32.EFI" 2>/dev/null || true
        sync

        # Verify the kernel + config actually landed on the ESP before we bother
        # registering a boot entry that would point at a broken install.
        local f
        for f in vmlinuz-linux initramfs-linux.img limine.conf; do
            [[ -f "${boot}/${f}" ]] || {
                echo "limine: FATAL ${boot}/${f} missing — kernel/config not on ESP" >&2
                return 1
            }
        done

        # Remove any stale vendiOS NVRAM entries (exact-label match on the
        # "BootXXXX* vendiOS" line) so reinstalls don't pile up duplicates.
        # Tolerant match: optional '*', one-or-more spaces, trailing whitespace.
        local n
        for n in $(efibootmgr 2>/dev/null | sed -n 's/^Boot\([0-9A-Fa-f]\{4\}\)\*\?[[:space:]]\{1,\}vendiOS[[:space:]]*$/\1/p'); do
            efibootmgr -b "$n" -B >/dev/null 2>&1 || true
        done

        # Resolve the ESP's partition number on $disk for the boot entry.
        local dev; dev=$(findmnt -no SOURCE "$boot")
        local partnum; partnum=$(cat "/sys/class/block/${dev##*/}/partition" 2>/dev/null)
        [[ -n "$partnum" ]] || {
            echo "limine: FATAL could not determine ESP partition number for ${dev}" >&2
            return 1
        }

        # Create the entry (backslash loader path — the UEFI spec form). NOT
        # wrapped in `|| true`: if NVRAM can't be written we want to know.
        if ! efibootmgr --disk "$disk" --part "$partnum" \
                --create --label "vendiOS" \
                --loader '\EFI\vendiOS\BOOTX64.EFI'; then
            echo "limine: WARNING efibootmgr could not create the NVRAM entry." >&2
            echo "limine: the /EFI/BOOT fallback is in place, so the disk should" >&2
            echo "limine: still boot via the firmware's removable-media path." >&2
            # Not fatal — the fallback loader covers this case.
        fi

        # Confirm the entry exists (best-effort verification for the log).
        if efibootmgr 2>/dev/null | grep -q ' vendiOS$'; then
            echo "limine: NVRAM boot entry 'vendiOS' registered."
        else
            echo "limine: NVRAM entry not present; relying on /EFI/BOOT fallback." >&2
        fi
    else
        cp /usr/share/limine/limine-bios.sys "${boot}/"
        sync
        # Stage 1 MBR install — patches MBR with pointers to limine-bios.sys
        # which it locates by scanning partitions for the FAT32 we just wrote.
        limine bios-install "$disk"
    fi
}

disk_partuuid() { blkid -s PARTUUID -o value "$1" 2>/dev/null; }
