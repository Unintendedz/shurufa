#!/usr/bin/env bash
# 运行 Shurufa 端侧两阶段（Tier 0 + Tier 1 Qwen3-0.6B Metal）真实模型门禁评测。
# 默认自动发现本机已暂存词库、万象语法包、ShurufaNeuralService 与 GGUF 模型；
# 若缺少本机 GGUF 模型且未传 `--require`，打印 `skipped:` 并以 0 退出。
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$root"

if [[ -d "/opt/homebrew/opt/python@3.14/bin" ]]; then
  export PATH="/opt/homebrew/opt/python@3.14/bin:$PATH"
fi
export CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target/macos-cargo}"
if [[ -d "/private/tmp" ]]; then
  export TMPDIR="/private/tmp"
fi

require_run=0
extra_args=()
for arg in "$@"; do
  case "$arg" in
    --require) require_run=1 ;;
    *) extra_args+=("$arg") ;;
  esac
done

resources=""
resource_lock=""
if [[ -f "$root/target/macos/EngineResources/msime-pinyin.db" ]]; then
  resources="$root/target/macos/EngineResources"
  resource_lock="$root/resources/editions/shurufa.lock.json"
elif [[ -f "$HOME/Library/Input Methods/Shurufa.app/Contents/Resources/EngineResources/msime-pinyin.db" ]]; then
  resources="$HOME/Library/Input Methods/Shurufa.app/Contents/Resources/EngineResources"
  resource_lock="$root/resources/editions/shurufa.lock.json"
fi

gram="${SHURUFA_GRAMMAR_MODEL:-$HOME/Library/Application Support/Shurufa/resource-packs/grammar-model-wanxiang/wanxiang-lts-zh-hans.gram}"
model_dir="${SHURUFA_NEURAL_MODEL_DIR:-$HOME/Library/Application Support/Shurufa/resource-packs/neural-lm}"

helper=""
if [[ -x "$root/target/macos-isolated/ShurufaNeuralService" ]]; then
  helper="$root/target/macos-isolated/ShurufaNeuralService"
elif [[ -x "$HOME/Library/Input Methods/Shurufa.app/Contents/Helpers/ShurufaNeuralService" ]]; then
  helper="$HOME/Library/Input Methods/Shurufa.app/Contents/Helpers/ShurufaNeuralService"
fi

skip_or_fail() {
  if [[ "$require_run" -eq 1 ]]; then
    echo "FAIL: $1" >&2
    exit 1
  fi
  echo "skipped: $1"
  exit 0
}

[[ -n "$resources" ]] || skip_or_fail "staged EngineResources (msime-pinyin.db) not found"
[[ -f "$gram" ]] || skip_or_fail "wanxiang grammar model not found at $gram"
[[ -n "$helper" ]] || skip_or_fail "ShurufaNeuralService binary not found"
if ! compgen -G "$model_dir/*.gguf" >/dev/null && ! compgen -G "$model_dir/*.GGUF" >/dev/null; then
  skip_or_fail "no .gguf neural model found in $model_dir"
fi

report_out="${SHURUFA_TIER1_REPORT:-$root/target/tier1-eval-latest.json}"

cargo run --quiet --release -p msime-input-runtime --example tier1_eval --locked -- \
  --resources "$resources" \
  --resource-lock "$resource_lock" \
  --grammar-model "$gram" \
  --helper "$helper" \
  --model-dir "$model_dir" \
  --page-size 6 \
  --suite "spoken-v1=$root/resources/eval/shurufa-spoken-v1.tsv" \
  --suite "guardrails-v1=$root/resources/eval/shurufa-tier1-guardrails-v1.tsv" \
  --suite "sentences-v2-4char=$root/resources/eval/sentences-v2.tsv:max_syl=4" \
  --report "$report_out" \
  --gate \
  ${extra_args[@]+"${extra_args[@]}"} >/dev/null
