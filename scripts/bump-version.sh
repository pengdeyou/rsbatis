#!/usr/bin/env bash
# 改某个包的版本号，并且同步改掉所有"依赖这个包"的 Cargo.toml 里的版本要求。
#
# 为什么单独写脚本：`sed -i 's/0.1.0-alpha.8/0.1.0-alpha.9/g'` 这种整串替换会误伤
# 其它包里恰好同号的依赖行（本项目踩过两次，一次把 orm 的版本改了两次、一次把
# 不相干的 chrono 版本匹配进去）。所以这里按"结构位置"改，不按字符串改。
#
# 用法：
#   ./scripts/bump-version.sh rsbatis-core 0.1.0-alpha.10
#   ./scripts/bump-version.sh rsbatis       0.1.0-alpha.17
set -euo pipefail

die() { echo "错误：$*" >&2; exit 1; }

[ $# -eq 2 ] || die "用法：$0 <crate-name> <new-version>"
CRATE=$1
NEW=$2

[[ "$NEW" =~ ^[0-9]+\.[0-9]+\.[0-9]+(-(alpha|beta|rc)\.[0-9]+)?$ ]] || die "版本号格式不对：$NEW"

cd "$(git rev-parse --show-toplevel 2>/dev/null || dirname "$(dirname "$(readlink -f "$0")")")"

python3 - "$CRATE" "$NEW" <<'PY'
import pathlib, re, sys

crate, new = sys.argv[1], sys.argv[2]
root = pathlib.Path("crates")

def find_pkg(name):
    for f in root.glob("*/Cargo.toml"):
        if re.search(rf'^name\s*=\s*"{re.escape(name)}"$', f.read_text(), re.M):
            return f
    return None

target = find_pkg(crate)
if target is None:
    sys.exit(f"找不到包 {crate}")

changed = []

# 1) 改自己的 [package] version —— 只改 [package] 段里的第一处，
#    不能碰 [dependencies] 里的同名键。
lines = target.read_text().splitlines(keepends=True)
in_pkg = False; done = False
for i, ln in enumerate(lines):
    s = ln.strip()
    if s.startswith("["):
        in_pkg = (s == "[package]")
        continue
    if in_pkg and not done and re.match(r'^version\s*=', s):
        lines[i] = re.sub(r'"[^"]*"', f'"{new}"', ln, count=1)
        done = True
        break
if not done:
    sys.exit(f"{target} 的 [package] 段里没找到 version")
target.write_text("".join(lines))
changed.append(str(target))

# 2) 同步所有引用它的依赖行（靠 path = "../<crate>" 定位，逐行改）
for f in root.glob("*/Cargo.toml"):
    if f == target:
        continue
    src = f.read_text()
    out = []
    hit = False
    for ln in src.splitlines(keepends=True):
        # 依赖行形如：rsbatis-core = { version = "0.1.0-alpha.9", path = "../rsbatis-core" }
        if f'path = "../{crate}"' in ln and "version" in ln:
            new_ln = re.sub(r'version\s*=\s*"[^"]*"', f'version = "{new}"', ln)
            out.append(new_ln); hit = True
            print(f"  {f}: {ln.strip()}  ->  {new_ln.strip()}")
        else:
            out.append(ln)
    if hit:
        f.write_text("".join(out)); changed.append(str(f))

print(f"改完 {crate} -> {new}，涉及 {len(changed)} 个文件")
PY

echo
echo "下一步：git diff 复核，然后 ./scripts/publish.sh --from $CRATE"
