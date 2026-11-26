#!/usr/bin/env bash
# 仓库文本契约检查入口：自动发现并执行 scripts/test-*.py（跳过 special_checks 中登记由外部门禁执行的条目）。
#
# verify-local.sh 会 source 本脚本；也可独立运行。缺工具或输入时各检查自行打印 skipped 并以 0 退出。

if [ "${BASH_SOURCE[0]}" = "$0" ]; then
  set -uo pipefail
  cd "$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)" || exit 2
  failed=0
  note() { printf '\n=== %s ===\n' "$1"; }
  fail() { echo "FAIL: $1"; failed=1; }
  run_checks_standalone=1
else
  run_checks_standalone=0
fi

# 无法无参独立运行的检查按 `名字=负责执行或说明的文件` 登记于此。
special_checks=""

note "contract check registry"
special_names=""
registry_ok=1
for entry in $special_checks; do
  name="${entry%%=*}"
  owner="${entry#*=}"
  special_names="$special_names $name"
  if [ ! -f "scripts/test-$name.py" ]; then
    fail "special check $name: scripts/test-$name.py does not exist"
    registry_ok=0
  elif [ "${owner%.sh}" != "$owner" ]; then
    if ! grep -qE "^[^#]*python3 [^#]*scripts/test-$name\.py" "$owner" 2>/dev/null; then
      fail "special check $name: $owner does not run scripts/test-$name.py"
      registry_ok=0
    fi
  elif ! grep -qF "scripts/test-$name.py" "$owner" 2>/dev/null; then
    fail "special check $name: $owner does not document scripts/test-$name.py"
    registry_ok=0
  fi
done
[ "$registry_ok" -eq 1 ] && echo "contract check registry: every special check is run by the script it names, or documented where no gate runs it"

check_jobs="${MSIME_CHECK_JOBS:-${CARGO_BUILD_JOBS:-}}"
if [ -z "$check_jobs" ]; then
  check_jobs="$(getconf _NPROCESSORS_ONLN 2>/dev/null || echo 1)"
  [ "$check_jobs" -gt 6 ] 2>/dev/null && check_jobs=6
fi
case "$check_jobs" in '' | *[!0-9]* | 0) check_jobs=1 ;; esac
checks_dir="$(mktemp -d)"
[ "$run_checks_standalone" -eq 1 ] && trap 'rm -rf -- "$checks_dir"' EXIT
pending=""
for check in scripts/test-*.py; do
  name="${check#scripts/test-}"
  name="${name%.py}"
  case " $special_names " in *" $name "*) continue ;; esac
  pending="$pending$check"$'\n'
done
# shellcheck disable=SC2016
printf '%s' "$pending" | xargs -P "$check_jobs" -I {} sh -c \
  'out="$2/$(basename "$1" .py)"; PYTHONUNBUFFERED=1 python3 "$1" </dev/null >"$out.log" 2>&1; echo $? >"$out.status"' \
  run-check {} "$checks_dir"
for check in $pending; do
  name="${check#scripts/test-}"
  name="${name%.py}"
  note "${name//-/ }"
  cat "$checks_dir/test-$name.log" 2>/dev/null
  [ "$(cat "$checks_dir/test-$name.status" 2>/dev/null)" = 0 ] || fail "${name//-/ }"
done
rm -rf -- "$checks_dir"

if [ "$run_checks_standalone" -eq 1 ]; then
  if [ "$failed" -ne 0 ]; then
    printf '\nrun-checks: at least one contract check failed\n'
    exit 1
  fi
  printf '\nrun-checks: every contract check passed or skipped\n'
fi
