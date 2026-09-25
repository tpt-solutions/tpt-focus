#!/usr/bin/env bash
# Build release binaries and stage a Linux install tree under build/linux/.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
out_dir="${1:-$repo_root/build/linux/stage}"
version="$(grep -m1 '^version' "$repo_root/Cargo.toml" | cut -d'"' -f2)"

cd "$repo_root"

echo "==> cargo build --release"
cargo build --release --workspace

echo "==> staging $out_dir"
rm -rf "$out_dir"
install -Dm755 target/release/tpt-focus-tray "$out_dir/usr/bin/tpt-focus-tray"
install -Dm755 target/release/tpt-focus      "$out_dir/usr/bin/tpt-focus"
install -Dm644 packaging/linux/tpt-focus.service \
    "$out_dir/usr/lib/systemd/user/tpt-focus.service"
install -Dm644 packaging/linux/tpt-focus.desktop \
    "$out_dir/usr/share/applications/tpt-focus.desktop"
install -Dm644 packaging/linux/tpt-focus.appdata.xml \
    "$out_dir/usr/share/metainfo/tpt-focus.appdata.xml"
install -Dm644 README.md "$out_dir/usr/share/doc/tpt-focus/README.md"

echo "staged tree ready for packaging ($out_dir)"
echo "version: $version"
echo
echo "next steps:"
echo "  scripts/linux/package-deb.sh   # build/tpt-focus_${version}_amd64.deb"
echo "  scripts/linux/package-rpm.sh   # build/tpt-focus-${version}.x86_64.rpm"
echo "  scripts/linux/package-appimage.sh"
