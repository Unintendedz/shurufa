#!/bin/bash
# 幂等安装 TextStream 工作进程的运行环境与模型。
# 模型：Qwen/Qwen3-ASR-1.7B @ 7278e1e70fe206f11671096ffdd38061171dd6e5（Apache-2.0）。
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${SHURUFA_TEXTSTREAM_ROOT:-$HOME/Library/Application Support/Shurufa/textstream}"
VENV="$ROOT/venv"
MODEL="$ROOT/model"
READY="$ROOT/.ready"
OPENLESS_MODEL="${OPENLESS_TEXTSTREAM_MODEL:-$HOME/Library/Application Support/OpenLess/models/experimental-textstream-qwen3}"
UV="${UV:-$(command -v uv || echo "$HOME/.local/bin/uv")}"
REPO="Qwen/Qwen3-ASR-1.7B"
REV="7278e1e70fe206f11671096ffdd38061171dd6e5"
MS_REV="d69410f1c275f2b0fa60cbb9960edfcdb0ae0aec"
PYTHON_VERSION="3.12"

# 文件名 sha256
MODEL_FILES="chat_template.json 75a8cfca24f00de72d796fbfed6858fc9614ef3dabd8696684cc3bc03a9c58ff
config.json 2e74a751548b8ad7d7526d29365ad8144c345d8b412b1152d25dc6698452712f
generation_config.json 1da527824d81e07118facff437e03f2e24a23311e3bdeb2368973fe77e5f275c
merges.txt 8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5
model-00001-of-00002.safetensors a4cd1f1a04d90b757dc7f7dd26254e69a013b19e80efe590a83c6a3bde8608d6
model-00002-of-00002.safetensors 6e0b9d9e09e2e0238e7ef3cc8a484ab387e91b90f1900bedf88bc92d7929ccfc
model.safetensors.index.json f994739fe38e5210b9e3e8ce6c6307315e2ceac3cb630e7b7414d69dce520f60
preprocessor_config.json 45e120a4eda2c20c5d7f2ea9354e63536bf35e27aa573fb7cdf78017b378770d
tokenizer_config.json 4942d005604266809309cabc9f4e9cb89ce855d59b14681fdc0e1cc62ea26c4c
vocab.json ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910"

LOCK_HASH="$(shasum -a 256 "$HERE/requirements.lock" | cut -d' ' -f1)"
STAMP="$LOCK_HASH $REV"

if [ "$(cat "$READY" 2>/dev/null || true)" = "$STAMP" ] && [ -x "$VENV/bin/python" ] &&
   [ -f "$MODEL/config.json" ] && [ -f "$MODEL/model.safetensors.index.json" ] &&
   [ -f "$MODEL/model-00001-of-00002.safetensors" ] && [ -f "$MODEL/model-00002-of-00002.safetensors" ] &&
   [ "${1:-}" != "--force" ]; then
  echo "already installed: $ROOT"
  exit 0
fi

mkdir -p "$ROOT"
rm -f "$READY"

sha_ok() { [ -f "$1" ] && [ "$(shasum -a 256 "$1" | cut -d' ' -f1)" = "$2" ]; }

[ -x "$UV" ] || { echo "找不到 uv，请先安装：brew install uv" >&2; exit 1; }

echo "== python venv"
"$UV" venv --python "$PYTHON_VERSION" --allow-existing "$VENV"
"$UV" pip sync --python "$VENV/bin/python" --require-hashes --only-binary :all: "$HERE/requirements.lock"

echo "== model"
mkdir -p "$MODEL"
# 先清理不在当前清单里的旧文件（例如升级前遗留的 0.6B 单文件 model.safetensors），避免 mlx 加载全部 *.safetensors 时冲突并提前释放磁盘空间。
for existing in "$MODEL"/* "$MODEL"/.*; do
  [ -e "$existing" ] || continue
  base="$(basename "$existing")"
  if [ "$base" = "." ] || [ "$base" = ".." ]; then
    continue
  fi
  if ! awk -v f="$base" '$1 == f { found = 1 } END { exit !found }' <<< "$MODEL_FILES"; then
    echo "remove stale $base"
    rm -rf "$existing"
  fi
done
while read -r name sum; do
  dest="$MODEL/$name"
  if sha_ok "$dest" "$sum"; then continue; fi
  rm -f "$dest" "$dest.part"
  src="$OPENLESS_MODEL/$name"
  if sha_ok "$src" "$sum"; then
    echo "clone $name from OpenLess"
    cp -c "$src" "$dest.part"
  else
    echo "download $name"
    if [ -n "${HF_ENDPOINT:-}" ]; then
      if ! curl -fL --connect-timeout 15 --retry 3 -o "$dest.part" "$HF_ENDPOINT/$REPO/resolve/$REV/$name"; then
        rm -f "$dest.part"
        exit 1
      fi
    elif ! curl -fL --connect-timeout 15 --retry 3 -o "$dest.part" "https://modelscope.cn/models/$REPO/resolve/$MS_REV/$name"; then
      echo "fallback Hugging Face for $name"
      if ! curl -fL --connect-timeout 15 --retry 3 -o "$dest.part" "https://huggingface.co/$REPO/resolve/$REV/$name"; then
        rm -f "$dest.part"
        exit 1
      fi
    fi
  fi
  sha_ok "$dest.part" "$sum" || { echo "checksum mismatch: $name" >&2; rm -f "$dest.part"; exit 1; }
  mv "$dest.part" "$dest"
done <<< "$MODEL_FILES"

echo "== import check"
"$VENV/bin/python" -c "import mlx.core, mlx_qwen3_asr, silero_vad_lite, numpy"

printf '%s' "$STAMP" > "$READY"
echo "installed: $ROOT"
