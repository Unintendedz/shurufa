#!/usr/bin/env bash
# 暂存 Shurufa 运行时核心词库目录（仅含 shurufa.lock.json 固定的核心拼音、英文、表情与 n-gram 资源）。
set -euo pipefail
umask 077

repo_root=$(cd "$(dirname "$0")/../.." && pwd)
cd "$repo_root"
source_dir=${1:?usage: [MSIME_EDITION=<id>] stage-resources.sh <verified-resource-directory>}
source_dir=$(cd "$source_dir" && pwd)
destination="$repo_root/target/macos/EngineResources"

edition="${MSIME_EDITION:-shurufa}"
edition_flags=()
if [ "$edition" != full ]; then
  edition_lock="$repo_root/resources/editions/$edition.lock.json"
  [ -f "$edition_lock" ] || { echo "unknown edition $edition: $edition_lock does not exist" >&2; exit 1; }
  artifacts=$(python3 -c 'import json, sys; print("\n".join(artifact["name"] for artifact in json.load(open(sys.argv[1]))["artifacts"]))' "$edition_lock")
  edition_flags=(--edition "$edition")
else
  artifacts=$(cargo run --quiet -p msime-client-core --example verify_resources --locked -- "$source_dir")
fi

rm -rf "$destination" "$repo_root/target/macos/settled-model" "$repo_root/target/macos/language-dictionaries"
mkdir -p "$destination"
while IFS= read -r artifact; do
  [ -n "$artifact" ] || continue
  cp "$source_dir/$artifact" "$destination/$artifact"
done <<< "$artifacts"

cargo run --quiet -p msime-client-core --example verify_resources --locked -- ${edition_flags[@]+"${edition_flags[@]}"} "$destination" >/dev/null
stroke_source="${2:-$repo_root/target/language-dictionaries}"
if [ -f "$stroke_source/msime-stroke.db" ] && [ -f "$stroke_source/msime-rime_stroke_LICENSE.txt" ]; then
  stroke_dest="$repo_root/target/macos/language-dictionaries"
  mkdir -p "$stroke_dest"
  cp "$stroke_source/msime-stroke.db" "$stroke_dest/msime-stroke.db"
  cp "$stroke_source/msime-rime_stroke_LICENSE.txt" "$stroke_dest/msime-rime_stroke_LICENSE.txt"
fi
if [ -f "$stroke_source/shurufa-lexicon-calibration.bin" ]; then
  mkdir -p "$repo_root/target/macos/language-dictionaries"
  cp "$stroke_source/shurufa-lexicon-calibration.bin" "$repo_root/target/macos/language-dictionaries/shurufa-lexicon-calibration.bin"
fi
echo "macOS resources staged for $edition: $destination"
