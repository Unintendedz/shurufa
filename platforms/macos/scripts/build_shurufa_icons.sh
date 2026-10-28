#!/usr/bin/env bash
# 从已提交的 PNG 母版和 SVG 路径生成完整 icns 与模板 TIFF，原版图标不改动。
set -euo pipefail
root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../../.." && pwd)"
source_root="$root/platforms/macos/resources/shurufa"
staging="$root/target/macos-isolated/shurufa-icon-build"
mkdir -p "$staging/Shurufa.iconset"
export SDKROOT=/Library/Developer/CommandLineTools/SDKs/MacOSX26.5.sdk
xcrun swift "$root/platforms/macos/scripts/render_shurufa_icons.swift" "$source_root" "$staging"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" "$source_root/app-icon-1024.png" --out "$staging/Shurufa.iconset/icon_${size}x${size}.png" >/dev/null
  sips -z "$((size * 2))" "$((size * 2))" "$source_root/app-icon-1024.png" --out "$staging/Shurufa.iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$staging/Shurufa.iconset" -o "$source_root/MSIMEClientInputMethod.icns"
cp "$staging/"*.tiff "$source_root/"
python3 - "$source_root" <<'PY'
import hashlib
import json
import sys
from pathlib import Path
root = Path(sys.argv[1])
files = sorted([root / "MSIMEClientInputMethod.icns", *root.glob("*.tiff")])
manifest = {file.name: hashlib.sha256(file.read_bytes()).hexdigest() for file in files}
(root / "icon-sha256.json").write_text(json.dumps(manifest, indent=2) + "\n")
PY
