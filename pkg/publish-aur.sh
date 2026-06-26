#!/usr/bin/env bash
# Publish the current vendi-git PKGBUILD to the AUR.
# Run it from your terminal:  ! bash /home/vendi/vendiOS/pkg/publish-aur.sh
set -euo pipefail

REPO=/home/vendi/vendiOS
CLONE=~/aur-vendi-git
# Default the commit message to the PKGBUILD's actual pkgver so it can never
# drift from the published version (pass an argument to override).
_ver=$(sed -n 's/^pkgver=//p' "$REPO/pkg/vendi-git/PKGBUILD")
MSG="${1:-snapshot $_ver}"

# Load the (passphrase-protected) AUR key once for this run.
if ! ssh-add -l >/dev/null 2>&1; then
    eval "$(ssh-agent)"
    ssh-add ~/.ssh/id_ed25519
fi

# Stand in a directory that won't be deleted (the shell may be sitting inside
# an old $CLONE from a prior attempt — removing the cwd breaks getcwd/clone).
cd "$REPO"
rm -rf "$CLONE"
git clone ssh://aur@aur.archlinux.org/vendi-git.git "$CLONE"
cd "$CLONE"

cp "$REPO/pkg/vendi-git/PKGBUILD" ./PKGBUILD
makepkg --printsrcinfo > .SRCINFO

git add PKGBUILD .SRCINFO
git commit -m "$MSG"
git push

echo
echo "==> published. The line above should read 'master -> master'."
