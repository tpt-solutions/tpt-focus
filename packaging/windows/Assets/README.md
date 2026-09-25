# Package assets

Required by `AppxManifest.xml` when packing the sparse package
(`scripts/windows/package-sparse.ps1`):

| File | Size |
|------|------|
| `logo.png` | 512×512 (Store/Properties logo) |
| `logo-150.png` | 150×150 |
| `logo-44.png` | 44×44 |

Final tray/icon artwork lands in Phase 8; until then generate placeholders
from the TPT logo before packing:

```powershell
# example with ImageMagick
magick logo.png -resize 150x150 logo-150.png
magick logo.png -resize 44x44 logo-44.png
```
