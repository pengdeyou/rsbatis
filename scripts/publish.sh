#!/usr/bin/env bash
# 按依赖顺序发布 rsbatis 全套包，并在每步之后去 crates.io 核实"真的上去了"。
#
# 为什么不是一句 `cargo publish` 循环：
#   1. 顺序不能错——上游包没进索引时，下游 publish 会报 "failed to find version"；
#   2. crates.io 对新 crate 有速率限制，失败后要知道从哪个包续跑，而不是从头再来；
#   3. 发布是外网写操作，"命令返回 0"不等于"版本存在"，必须用 API 复核。
#
# 用法：
#   ./scripts/publish.sh                     # 按顺序全部发布
#   ./scripts/publish.sh --from rsbatis-orm  # 从失败的那个包续跑
#   ./scripts/publish.sh --dry-run           # 只做 tarball 校验，不真的发布
set -uo pipefail

ORDER=(rsbatis-core rsbatis-macros rsbatis-driver rsbatis-orm rsbatis-migrate rsbatis)
FROM=""
DRY=0

while [ $# -gt 0 ]; do
  case "$1" in
    --from) FROM=${2:-}; shift 2 ;;
    --dry-run) DRY=1; shift ;;
    *) echo "未知参数 $1" >&2; exit 2 ;;
  esac
done

cd "$(dirname "$(dirname "$(readlink -f "$0")")")"
# 非交互 shell 里没有 ~/.cargo/bin，必须自己 source，否则 cargo: command not found
[ -f "$HOME/.cargo/env" ] && source "$HOME/.cargo/env"

ver_of() {  # 从 [package] 段读版本，避免读到依赖行里的 version
  python3 - "$1" <<'PY'
import re, sys, pathlib
for f in pathlib.Path("crates").glob("*/Cargo.toml"):
    t = f.read_text()
    if re.search(rf'^name\s*=\s*"{sys.argv[1]}"$', t, re.M):
        pkg = t.split("[package]")[1].split("\n[")[0]
        print(re.search(r'^version\s*=\s*"([^"]+)"', pkg, re.M).group(1))
        break
PY
}

echo "== 前置检查 =="
[ -f "$HOME/.cargo/credentials.toml" ] || { echo "没有 ~/.cargo/credentials.toml，先 cargo login" >&2; exit 1; }
# 不打印 token，只确认文件存在且权限收紧
[ "$(stat -c %a "$HOME/.cargo/credentials.toml")" = "600" ] || echo "提示：credentials.toml 权限不是 600"
cargo fmt --all -- --check || { echo "fmt 不通过，先 cargo fmt" >&2; exit 1; }
echo "凭据文件存在，格式检查通过"

# 算出从第几个包开始（--from 续跑用），比在循环里做字符串比对可靠
START=0
if [ -n "$FROM" ]; then
  FOUND=0
  for i in "${!ORDER[@]}"; do
    if [ "${ORDER[$i]}" = "$FROM" ]; then START=$i; FOUND=1; fi
  done
  [ $FOUND -eq 1 ] || { echo "--from $FROM 不在发布列表里" >&2; exit 2; }
fi

for i in "${!ORDER[@]}"; do
  c=${ORDER[$i]}
  if [ "$i" -lt "$START" ]; then echo "-- 跳过 $c（已发布）"; continue; fi


  V=$(ver_of "$c")
  [ -n "$V" ] || { echo "读不到 $c 的版本" >&2; exit 1; }
  echo
  echo "== $c $V =="

  if [ $DRY -eq 1 ]; then
    cargo publish -p "$c" --allow-dirty --dry-run || { echo "$c dry-run 失败" >&2; exit 1; }
    continue
  fi

  if ! cargo publish -p "$c" --allow-dirty 2>&1 | tee /tmp/pub-$$.log; then
    # 常见两种失败：索引还没更新 / 被限流。都不是致命错，等一会儿重试一次。
    if grep -qE "already uploaded|already exists" /tmp/pub-$$.log; then
      echo "$c $V 已经发过了，继续"
    else
      echo "首次失败，等 20s 重试一次（索引更新/限流都需要时间）..." >&2
      sleep 20
      cargo publish -p "$c" --allow-dirty || { echo "$c 发布失败，从它续跑：./scripts/publish.sh --from $c" >&2; exit 1; }
    fi
  fi

  # 用 API 复核，而不是相信退出码
  for i in 1 2 3 4 5; do
    if curl -sS -A "rsbatis-publish/0.1" "https://crates.io/api/v1/crates/$c/$V" | grep -q '"version"'; then
      echo "✔ $c $V 已在 crates.io"
      break
    fi
    [ $i -eq 5 ] && { echo "✘ $c $V 查不到，索引可能延迟，稍后手动核实" >&2; exit 1; }
    sleep 6
  done
done

echo
echo "全部完成。别忘了："
echo "  1) 把 verify-rsbatis-* 消费者工程的依赖版本改到最新，重跑真实库验证；"
echo "  2) 更新 README 的版本表；"
echo "  3) 若这是稳定版，考虑轮换 crates.io token。"
