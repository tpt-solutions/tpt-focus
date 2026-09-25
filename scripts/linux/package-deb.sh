#!/usr/bin/env bash
# Build a Debian package from the staged tree (scripts/linux/stage.sh).
# Requires: dpkg-deb (any distro with dpkg).
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
version="$(grep -m1 '^version' "$repo_root/Cargo.toml" | cut -d'"' -f2)"
stage="$repo_root/build/linux/stage"
pkg_dir="$repo_root/build/linux/deb-root"
deb="$repo_root/build/tpt-focus_${version}_amd64.deb"

cd "$repo_root"
[ -d "$stage/usr/bin" ] || ./scripts/linux/stage.sh "$stage"

echo "==> assembling deb control tree"
rm -rf "$pkg_dir"
cp -a "$stage" "$pkg_dir"
install -Dm644 /dev/null "$pkg_dir/DEBIAN/control"
cat > "$pkg_dir/DEBIAN/control" <<EOF
Package: tpt-focus
Version: $version
Section: utils
Priority: optional
Architecture: amd64
Maintainer: TPT Solutions <opensource@tpt.solutions>
Depends: libc6 (>= 2.31)
Recommends: libappindicator3-1 | gnome-shell-extension-appindicator
Description: Unified notification & focus center
 Rule-based notification manager: allows, mutes or batches every
 notification, keeps a searchable local history, and acts as the
 org.freedesktop.Notifications daemon on Linux.
EOF

cat > "$pkg_dir/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v systemctl >/dev/null 2>&1; then
    systemctl --user daemon-reload 2>/dev/null || true
fi
EOF
chmod 755 "$pkg_dir/DEBIAN/postinst"

mkdir -p "$(dirname "$deb")"
echo "==> dpkg-deb --build"
dpkg-deb --build "$pkg_dir" "$deb"
echo "built $deb"
