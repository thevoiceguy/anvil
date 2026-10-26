#!/usr/bin/env bash
# The Linux installers from a release bundle: a .deb (installed in
# /opt/anvil, `anvil` on the path) and an AppImage (one file, runs in
# place). Needs dpkg-deb, and appimagetool with the type2 runtime (paths in
# APPIMAGETOOL and APPIMAGE_RUNTIME).
#
#   packaging/linux.sh <version> <label> <bundle dir> <out dir>
set -euo pipefail
umask 022

version=$1 label=$2 bundle=$3 out=$4
here=$(cd "$(dirname "$0")" && pwd)
app=$(cd "$here/.." && pwd)
repo=$(cd "$app/.." && pwd)
id=com.thevoiceguy.anvil
mkdir -p "$out"
work=$(mktemp -d)

# --- .deb -----------------------------------------------------------------
root="$work/deb"
mkdir -p "$root/DEBIAN" "$root/opt/anvil" "$root/usr/bin" \
  "$root/usr/share/applications" "$root/usr/share/icons/hicolor/512x512/apps" \
  "$root/usr/share/doc/anvil"
cp -a "$bundle/." "$root/opt/anvil/"
ln -s /opt/anvil/anvil "$root/usr/bin/anvil"
cp "$app/linux/packaging/anvil.desktop" "$root/usr/share/applications/$id.desktop"
cp "$app/linux/packaging/anvil.png" "$root/usr/share/icons/hicolor/512x512/apps/$id.png"
cat "$repo/LICENSE-MIT" "$repo/LICENSE-APACHE" > "$root/usr/share/doc/anvil/copyright"
cat > "$root/DEBIAN/control" <<CONTROL
Package: anvil
Version: $version
Architecture: amd64
Maintainer: Anvil <noreply@github.com>
Homepage: https://github.com/thevoiceguy/anvil
Section: net
Priority: optional
Installed-Size: $(du -sk "$root/opt" | cut -f1)
Depends: libgtk-3-0t64 | libgtk-3-0, libasound2t64 | libasound2, libayatana-appindicator3-1
Description: Anvil, the softphone for FCP
 Calls, transfer, voicemail, presence and the directory from an FCP account,
 on the desktop.
CONTROL
# A branded package's profile (brand.py) is conffiles in /etc/anvil/profile:
# a purge removes the files, even under this package, but not the folders,
# which no package owns once this one has replaced it. Only empty ones go,
# so a profile put there by hand stays.
cat > "$root/DEBIAN/postrm" <<'POSTRM'
#!/bin/sh
set -e
if [ "$1" = purge ]; then
  rmdir /etc/anvil/profile 2>/dev/null || true
  rmdir /etc/anvil 2>/dev/null || true
fi
POSTRM
chmod 755 "$root/DEBIAN/postrm"
dpkg-deb --root-owner-group --build "$root" "$out/anvil_${label}_amd64.deb"

# --- AppImage --------------------------------------------------------------
appdir="$work/Anvil.AppDir"
mkdir -p "$appdir/usr/lib/anvil"
cp -a "$bundle/." "$appdir/usr/lib/anvil/"
cat > "$appdir/AppRun" <<'APPRUN'
#!/bin/sh
here="$(dirname "$(readlink -f "$0")")"
exec "$here/usr/lib/anvil/anvil" "$@"
APPRUN
chmod +x "$appdir/AppRun"
cp "$app/linux/packaging/anvil.desktop" "$appdir/$id.desktop"
cp "$app/linux/packaging/anvil.png" "$appdir/$id.png"
ARCH=x86_64 VERSION=$version "$APPIMAGETOOL" --no-appstream \
  --runtime-file "$APPIMAGE_RUNTIME" "$appdir" "$out/Anvil-${label}-x86_64.AppImage"

ls -l "$out"
