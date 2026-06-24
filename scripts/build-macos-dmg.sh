#!/usr/bin/env bash
# =============================================================================
# ShyExcel · 数据导出 —— 本地构建 macOS DMG（企业内部分发，不上架 App Store）
# -----------------------------------------------------------------------------
# 为什么是本地脚本而不是 cnb/Docker：
#   macOS 的 .app/.dmg 必须在 macOS 上打包（需 Apple 工具链 + hdiutil/codesign），
#   无法在 Linux 容器里交叉构建。Windows 那条线在 cnb 用 cargo-xwin 交叉编译；
#   macOS 这条线只能在 Mac 上跑此脚本。
#
# 用法：
#   ./scripts/build-macos-dmg.sh                  # 默认 arm64（Apple Silicon，最快）
#   ARCH=universal ./scripts/build-macos-dmg.sh   # Intel+Apple 芯片通用（含 Intel 老机型）
#   ARCH=x86_64    ./scripts/build-macos-dmg.sh   # 仅 Intel
#
# 签名（可选，按需设环境变量，脚本自动识别）：
#   不设       → ad-hoc 未签名：企业内部可用，用户首次打开需绕过 Gatekeeper（见末尾提示）。
#   Developer ID → 设 APPLE_SIGNING_IDENTITY="Developer ID Application: 公司 (TEAMID)"
#   公证(彻底无提示) → 另设 APPLE_ID / APPLE_PASSWORD(应用专用密码) / APPLE_TEAM_ID
#   （Tauri 会自动读取以上环境变量完成签名与公证。）
#
# 产物：dist/shy-export-client-<version>-<arch>.dmg
# =============================================================================
set -euo pipefail
# 强制 UTF-8 locale：非交互/后台 shell 可能是 C locale，会导致 `$VAR中文` 紧邻全角字时
# bash 误把后续字节并入变量名（配合 set -u 直接报 "unbound variable"）。
export LANG="${LANG:-en_US.UTF-8}"
export LC_ALL="${LC_ALL:-en_US.UTF-8}"

# --- 仓库根（本脚本在 scripts/ 下）---
HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "$HERE"

ARCH="${ARCH:-arm64}"
PRODUCT="ShyExcel"
DIST="$HERE/dist"
CONF="src-tauri/tauri.conf.json"

# --- 0. 环境守卫 ---
[ "$(uname)" = "Darwin" ] || { echo "❌ 只能在 macOS 上打 DMG（当前 $(uname)）"; exit 1; }
command -v cargo   >/dev/null 2>&1 || { echo "❌ 未安装 Rust/cargo"; exit 1; }
command -v rustup  >/dev/null 2>&1 || { echo "❌ 未安装 rustup"; exit 1; }
command -v hdiutil >/dev/null 2>&1 || { echo "❌ 缺 hdiutil（装 Xcode Command Line Tools）"; exit 1; }

VERSION="$(/usr/bin/python3 -c "import json;print(json.load(open('$CONF'))['version'])" 2>/dev/null || echo "0.0.0")"

# --- 1. arch → target triple ---
case "$ARCH" in
  universal)      TRIPLES="aarch64-apple-darwin x86_64-apple-darwin"; TARGET="universal-apple-darwin" ;;
  arm64|aarch64)  TRIPLES="aarch64-apple-darwin";                     TARGET="aarch64-apple-darwin" ;;
  x86_64|intel)   TRIPLES="x86_64-apple-darwin";                      TARGET="x86_64-apple-darwin" ;;
  *) echo "❌ ARCH 仅支持 universal | arm64 | x86_64（收到 '$ARCH'）"; exit 1 ;;
esac
echo "▶ 目标架构 ARCH=$ARCH  →  --target $TARGET   版本 v$VERSION"

# --- 2. 工具链 ---
for t in $TRIPLES; do
  rustup target list --installed 2>/dev/null | grep -qx "$t" || { echo "  + rustup target add $t"; rustup target add "$t"; }
done
if ! command -v cargo-tauri >/dev/null 2>&1; then
  echo "  + 安装 tauri-cli（首次较慢）"; cargo install --locked tauri-cli --version "^2"
fi

# --- 3. macOS 图标：bundler 需 icon.icns，缺则用原生 sips+iconutil 从 icon.png 生成（不动其它图标）---
ICNS="src-tauri/icons/icon.icns"
if [ ! -f "$ICNS" ] && [ -f "src-tauri/icons/icon.png" ]; then
  echo "  + 生成 ${ICNS}（从 icon.png）"
  ISET="$(mktemp -d)/icon.iconset"; mkdir -p "$ISET"
  for s in 16 32 128 256 512; do
    sips -z "$s"        "$s"        src-tauri/icons/icon.png --out "$ISET/icon_${s}x${s}.png"    >/dev/null 2>&1 || true
    sips -z "$((s*2))"  "$((s*2))"  src-tauri/icons/icon.png --out "$ISET/icon_${s}x${s}@2x.png" >/dev/null 2>&1 || true
  done
  iconutil -c icns "$ISET" -o "$ICNS" 2>/dev/null && echo "    ✅ $ICNS" || echo "    ⚠️ icns 生成失败，交给 tauri 自动处理"
fi

# --- 4. 签名识别（仅提示；Tauri 自动读取这些环境变量）---
if [ -n "${APPLE_SIGNING_IDENTITY:-}" ]; then
  echo "🔏 Developer ID 签名：$APPLE_SIGNING_IDENTITY"
  [ -n "${APPLE_ID:-}" ] && echo "🍎 将尝试公证（APPLE_ID=${APPLE_ID}）" || echo "ℹ️ 未设 APPLE_ID → 仅签名不公证"
else
  echo "ℹ️ 未设 APPLE_SIGNING_IDENTITY → ad-hoc 未签名（企业内部可用，用户首次打开需绕过 Gatekeeper）"
fi

# --- 5. 构建 DMG ---
# DMG 的「美化窗口布局」由 Finder/AppleScript 完成，无 GUI 会话（后台/SSH/CI）会失败。
# 设 CI=true → tauri-bundler 给 bundle_dmg.sh 传 --skip-jenkins，跳过美化、产出朴素但可靠
# 的 DMG（仍含 .app + 拖到 Applications 的快捷方式）。想要自定义布局的漂亮 DMG，请在有
# 图形界面的会话里执行：PRETTY_DMG=1 ./scripts/build-macos-dmg.sh
[ -n "${PRETTY_DMG:-}" ] || export CI=true
echo "🛠  cargo tauri build --bundles dmg --target ${TARGET}  (CI=${CI:-未设})…"
cargo tauri build --bundles dmg --target "$TARGET"

# --- 6. 收集产物 ---
SRC_DMG="$(find "target/$TARGET/release/bundle/dmg" -maxdepth 1 -name '*.dmg' 2>/dev/null | head -1)"
[ -n "$SRC_DMG" ] || { echo "❌ 未找到 DMG，构建可能失败"; find target -name '*.dmg' 2>/dev/null; exit 1; }
mkdir -p "$DIST"
OUT="$DIST/shy-export-client-${VERSION}-${ARCH}.dmg"
cp -f "$SRC_DMG" "$OUT"

# --- 7. 自检 + 分发说明 ---
echo ""
echo "✅ 产物：$OUT  （$(du -h "$OUT" | cut -f1)）"
echo "   源 DMG：$SRC_DMG"
echo "   签名状态：$(codesign -dvv "$SRC_DMG" 2>&1 | grep -iE 'Signature|Authority' | head -1 || echo '未签名 / ad-hoc')"
echo ""
echo "── 企业内部分发说明 ─────────────────────────────────────────────"
if [ -z "${APPLE_SIGNING_IDENTITY:-}" ]; then
  cat <<EOF
本 DMG 未签名/未公证。用户从内网下载后首次打开会被 Gatekeeper 拦，二选一：
  a) 在 访达 里右键 ${PRODUCT}.app → 打开 → 再点「打开」（仅首次）
  b) 终端执行：xattr -dr com.apple.quarantine "/Applications/${PRODUCT}.app"
若要彻底无任何提示：申请 Apple Developer ID 证书并设 APPLE_SIGNING_IDENTITY
+ APPLE_ID/APPLE_PASSWORD/APPLE_TEAM_ID 后重跑本脚本（自动签名+公证）。
EOF
fi
echo "────────────────────────────────────────────────────────────────"
