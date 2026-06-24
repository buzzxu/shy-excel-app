# ShyExcel · 数据导出

通用高速数据导出桌面客户端（Windows / macOS）。由浏览器 `shyexport://` 深链唤起，
从业务服务端 **Arrow 流式**拉数据，在用户本机**流式生成多层合并的 Excel**——
峰值内存与数据总量无关，轻松导出数十万订单 / 数百万行。

```
业务前端 ──shyexport://──▶ ShyExcel客户端 ──GET Arrow 流──▶ 业务后端
                                  │
                                  ▼ 本地生成 .xlsx（多层合并 + 多文件分块）
```

**核心特性**

- **零业务硬编码**：导出的标题/列/合并/层级全部由 Arrow schema **自描述**，新增导出无需改客户端、无需发版。
- **真流式 + 多文件分块**：边下边生成边落盘，内存可控。
- **开箱即用**：托盘常驻、单实例、冷启动不丢任务、同任务去重、进度/ETA、产物一键定位。

## 给业务系统接入方

👉 **[集成接入指南](docs/集成接入指南.md)** —— 前端如何唤起、后端 Arrow 流契约、自描述 schema、Java 服务端要点、联调 checklist。

一分钟版（前端唤起）：

```js
const link = `shyexport://export?job=${encodeURIComponent(jobId)}`
           + `&url=${encodeURIComponent(streamUrl)}`; // streamUrl 必须整体编码
window.location.href = link;
```

## 安装

从 [Releases](../../releases) 下载：Windows `*-setup.exe`（NSIS，自动装 WebView2）、macOS `*.dmg`（Apple Silicon）。

> macOS 当前为未签名构建，首次打开请右键 App →「打开」，或
> `xattr -dr com.apple.quarantine "/Applications/ShyExcel.app"`。

## 开发 / 构建

Tauri v2 + Rust workspace：

```
crates/shy-xlsx-core    # Arrow IPC 流 → 多层合并 xlsx 生成核心（可独立复用）
crates/shy-export-cli   # headless 拉流 + 生成核心（含 generate_local 离线联调）
src-tauri                # Tauri v2 GUI 壳（深链 / 托盘 / 进度事件）
ui                       # 静态前端（编译期内嵌，无需 node 构建）
```

```bash
cargo tauri dev                              # 本地调试
cargo tauri build                            # 本机平台打包
./scripts/build-macos-dmg.sh                 # macOS 本地打 DMG（可选签名/公证）
```

CI：推送 `v*` tag 由 GitHub Actions 自动构建 macOS / Windows 并发布到 Releases（见 `.github/workflows/release.yml`）。

## 许可

[AGPL-3.0](LICENSE)
