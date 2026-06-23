#!/usr/bin/env bash
# =============================================================================
# 上传任意文件为 cnb Release 资产（呈现在 Releases 页）
# -----------------------------------------------------------------------------
# 用途：macOS DMG 只能在 Mac 本地打包（cnb Linux 容器无法交叉构建），所以发版后需用
#       本脚本把本地产物手工传到对应 tag 的 Release，和 Windows 安装包放在同一页面。
#       （Windows 安装包由 .cnb.yml 在 tag_push 时自动上传，无需本脚本。）
#
# 用法：./scripts/upload-release-asset.sh <文件> [tag]
#   例：./scripts/upload-release-asset.sh dist/xwjd-export-client-0.1.0-arm64.dmg v0.1.0
#   tag 省略时默认 v<tauri.conf.json 里的 version>。
#
# 鉴权 token 来源（按序）：环境变量 CNB_TOKEN > git 凭据（osxkeychain 里 cnb.cool 的密码）。
# 接口为 cnb OpenAPI 三步式（与 .cnb.yml 末段一致）：
#   asset-upload-url(申请) → PUT(传对象存储) → asset-upload-confirmation(确认)。
#   确认必须 --path-as-is 保留 verify_url 中编码的 %2F 资产路径，否则 token 校验失败。
# =============================================================================
set -euo pipefail
export LANG="${LANG:-en_US.UTF-8}"; export LC_ALL="${LC_ALL:-en_US.UTF-8}"

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"; cd "$HERE"
REPO="fucksky/xwjd/xwjd-export-client"
API="https://api.cnb.cool"

FILE="${1:-}"
[ -n "$FILE" ] && [ -f "$FILE" ] || { echo "用法：$0 <文件> [tag]"; exit 1; }
VERSION="$(/usr/bin/python3 -c "import json;print(json.load(open('src-tauri/tauri.conf.json'))['version'])" 2>/dev/null || echo 0.0.0)"
TAG="${2:-v$VERSION}"
FN="$(basename "$FILE")"
SIZE="$(wc -c < "$FILE" | tr -d ' ')"

# token：优先环境变量，否则取 git 凭据
TOKEN="${CNB_TOKEN:-}"
if [ -z "$TOKEN" ]; then
  TOKEN="$(printf 'protocol=https\nhost=cnb.cool\n\n' | git credential fill 2>/dev/null | sed -n 's/^password=//p')"
fi
[ -n "$TOKEN" ] || { echo "❌ 无 token：设 CNB_TOKEN 或先在本机用 git 推过该仓库"; exit 1; }
AUTH="Authorization: Bearer $TOKEN"

echo "▶ 上传 ${FN}（${SIZE} 字节）→ Release ${TAG}"
RID="$(curl -fsS -H "$AUTH" -H "Accept: application/json" "$API/$REPO/-/releases/tags/$TAG" | /usr/bin/python3 -c 'import sys,json;print(json.load(sys.stdin).get("id",""))')"
[ -n "$RID" ] || { echo "❌ 未找到 tag=$TAG 的 Release（先打 tag 触发 cnb 建 Release，或核对 tag 名）"; exit 1; }
echo "  release_id=$RID"

RESP="$(curl -fsS -X POST -H "$AUTH" -H "Content-Type: application/json" -H "Accept: application/vnd.cnb.api+json" \
  "$API/$REPO/-/releases/$RID/asset-upload-url" -d "{\"asset_name\":\"$FN\",\"size\":$SIZE,\"overwrite\":true}")"
UPURL="$(printf '%s' "$RESP" | /usr/bin/python3 -c 'import sys,json;print(json.load(sys.stdin)["upload_url"])')"
VERIFY="$(printf '%s' "$RESP" | /usr/bin/python3 -c 'import sys,json;print(json.load(sys.stdin)["verify_url"])')"

curl -fsS -X PUT -H "Content-Type: application/octet-stream" --data-binary "@$FILE" "$UPURL" >/dev/null
curl -fsS --path-as-is -X POST -H "$AUTH" -H "Accept: application/vnd.cnb.api+json" "$VERIFY" >/dev/null

echo "✅ 已挂到 Release。当前资产："
curl -fsS -H "$AUTH" -H "Accept: application/json" "$API/$REPO/-/releases/$RID" \
  | /usr/bin/python3 -c 'import sys,json
for a in json.load(sys.stdin).get("assets",[]): print("  ·",a["name"],a["size"],"bytes ->",a.get("browser_download_url"))'
