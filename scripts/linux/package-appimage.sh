#!/usr/bin/env bash
# Build a portable AppImage (Type-2, runtime downloaded on first use).
# Requires: the staged tree from scripts/linux/stage.sh and either
# appimagetool on PATH or network access to fetch it.
set -euo pipefail

repo_root="$(cd "$(dirname "$0")/../.." && pwd)"
stage="$repo_root/build/linux/stage"
appdir="$repo_root/build/linux/AppDir"
tool="$repo_root/build/linux/appimagetool-x86_64.AppImage"

cd "$repo_root"
[ -d "$stage/usr/bin" ] || ./scripts/linux/stage.sh "$stage"

echo "==> assembling AppDir"
rm -rf "$appdir"
mkdir -p "$appdir/usr"
cp -a "$stage/usr" "$appdir/"

cat > "$appdir/tpt-focus.desktop" <<'EOF'
[Desktop Entry]
Type=Application
Name=tpt-focus
GenericName=Notification Center
Comment=Rule-based notification manager with searchable history
Exec=tpt-focus-tray
Icon=tpt-focus
Terminal=false
Categories=Utility;
EOF

# 64x64 icon (same programmatic design as the tray, as a PNG).
if command -v python3 >/dev/null; then
    python3 - "$appdir" <<'PY'
import struct, sys, zlib, os
appdir = sys.argv[1]
size = 64
rows = []
for y in range(size):
    row = b"\x00" + bytes(
        v
        for x in range(size)
        for v in (
            (255, 255, 255) if (10 <= y <= 13 and abs(x - 32) <= 12) else
            (110, 70, 160) if (x - 31.5) ** 2 + (y - 31.5) ** 2 <= 30 * 30 else
            (0, 0, 0)
        )
    )
    rows.append(row)
raw = b"".join(rows)
def chunk(tag, payload):
    return (
        struct.pack(">I", len(payload)) + tag + payload +
        struct.pack(">I", zlib.crc32(tag + payload) & 0xFFFFFFFF)
    )
png = (
    b"\x89PNG\r\n\x1a\n"
    + chunk(b"IHDR", struct.pack(">IIBBBBB", size, size, 8, 2, 0, 0, 0))
    + chunk(b"IDAT", zlib.compress(raw))
    + chunk(b"IEND", b"")
)
os.makedirs(f"{appdir}/usr/share/icons/hicolor/64x64/apps", exist_ok=True)
open(f"{appdir}/usr/share/icons/hicolor/64x64/apps/tpt-focus.png", "wb").write(png)
PY
fi

echo "==> appimagetool"
if ! command -v appimagetool >/dev/null; then
    if [ ! -x "$tool" ]; then
        echo "fetching appimagetool"
        curl -fsSL -o "$tool" \
            "https://github.com/AppImage/appimagetool/releases/download/continuous/appimagetool-x86_64.AppImage"
        chmod +x "$tool"
    fi
    appimagetool="$tool"
else
    appimagetool=appimagetool
fi

mkdir -p build
"$appimagetool" "$appdir" build/tpt-focus.AppImage
echo "built build/tpt-focus.AppImage"
