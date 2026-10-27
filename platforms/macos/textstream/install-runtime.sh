#!/bin/bash
# 幂等安装 TextStream 工作进程的运行环境与模型。
# 模型：Qwen/Qwen3-ASR-0.6B @ 5eb144179a02acc5e5ba31e748d22b0cf3e303b0（与 OpenLess 内置清单一致）。
set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT="${SHURUFA_TEXTSTREAM_ROOT:-$HOME/Library/Application Support/Shurufa/textstream}"
VENV="$ROOT/venv"
MODEL="$ROOT/model"
READY="$ROOT/.ready"
OPENLESS_MODEL="${OPENLESS_TEXTSTREAM_MODEL:-$HOME/Library/Application Support/OpenLess/models/experimental-textstream-qwen3}"
UV="${UV:-$(command -v uv || echo "$HOME/.local/bin/uv")}"
REPO="Qwen/Qwen3-ASR-0.6B"
REV="5eb144179a02acc5e5ba31e748d22b0cf3e303b0"
PYTHON_VERSION="3.12"

# 文件名 sha256
MODEL_FILES="chat_template.json 75a8cfca24f00de72d796fbfed6858fc9614ef3dabd8696684cc3bc03a9c58ff
config.json 76d3ae4601ce939830b2517f4a6cadb86cc51316c3900af6b020b051c21a478c
generation_config.json 1da527824d81e07118facff437e03f2e24a23311e3bdeb2368973fe77e5f275c
merges.txt 8831e4f1a044471340f7c0a83d7bd71306a5b867e95fd870f74d0c5308a904d5
model.safetensors 79d6cbd4c98c7bbffe9db2edac07f56cd6637d0d5944b27f6c2b8353840323ea
preprocessor_config.json 45e120a4eda2c20c5d7f2ea9354e63536bf35e27aa573fb7cdf78017b378770d
tokenizer_config.json 4942d005604266809309cabc9f4e9cb89ce855d59b14681fdc0e1cc62ea26c4c
vocab.json ca10d7e9fb3ed18575dd1e277a2579c16d108e32f27439684afa0e10b1440910"

LOCK_HASH="$(shasum -a 256 "$HERE/requirements.lock" | cut -d' ' -f1)"
STAMP="$LOCK_HASH $REV"

if [ "$(cat "$READY" 2>/dev/null || true)" = "$STAMP" ] && [ -x "$VENV/bin/python" ] && [ "${1:-}" != "--force" ]; then
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
    curl -fL --retry 3 -o "$dest.part" "https://huggingface.co/$REPO/resolve/$REV/$name"
  fi
  sha_ok "$dest.part" "$sum" || { echo "checksum mismatch: $name" >&2; rm -f "$dest.part"; exit 1; }
  mv "$dest.part" "$dest"
done <<< "$MODEL_FILES"

echo "== import check"
"$VENV/bin/python" -c "import mlx.core, mlx_qwen3_asr, silero_vad_lite, numpy"

printf '%s' "$STAMP" > "$READY"
echo "installed: $ROOT"
