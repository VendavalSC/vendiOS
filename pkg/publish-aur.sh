#!/usr/bin/env bash
# Publish a vendiOS -git PKGBUILD to the AUR.
#   ! bash /home/vendi/vendiOS/pkg/publish-aur.sh [pkgname] [message]
#
# pkgname defaults to vendi-git (the desktop). Pass vendimessage-git to publish
# the messenger. The AUR repo published to is ssh://aur@aur.archlinux.org/<pkgname>.
set -euo pipefail

REPO=/home/vendi/vendiOS
PKG="${1:-vendi-git}"
PKGDIR="$REPO/pkg/$PKG"
[[ -f "$PKGDIR/PKGBUILD" ]] || { echo "no PKGBUILD at $PKGDIR" >&2; exit 1; }
CLONE=~/aur-$PKG
# Default the commit message to the PKGBUILD's actual pkgver so it can never
# drift from the published version (pass a 2nd argument to override).
_ver=$(sed -n 's/^pkgver=//p' "$PKGDIR/PKGBUILD")
MSG="${2:-snapshot $_ver}"

# Load the (passphrase-protected) AUR key once for this run.
if ! ssh-add -l >/dev/null 2>&1; then
    eval "$(ssh-agent)"
    ssh-add ~/.ssh/id_ed25519
fi

# Stand in a directory that won't be deleted (the shell may be sitting inside
# an old $CLONE from a prior attempt — removing the cwd breaks getcwd/clone).
cd "$REPO"
rm -rf "$CLONE"
git clone "ssh://aur@aur.archlinux.org/$PKG.git" "$CLONE"
cd "$CLONE"

cp "$PKGDIR/PKGBUILD" ./PKGBUILD
makepkg --printsrcinfo > .SRCINFO

git add PKGBUILD .SRCINFO
git commit -m "$MSG"
git push

echo
echo "==> published $PKG. The line above should read 'master -> master'."
