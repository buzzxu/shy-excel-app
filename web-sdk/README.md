# @yuanmai/client-export

唤起桌面客户端 **ShyExcel** 进行大数据量导出的前端 SDK：建任务 → `shyexport://` 深链唤起
→ 客户端直连拉 Apache Arrow 流 → 本地高速生成 `.xlsx`。**零依赖**，跨框架（Vue2 / Vue3 / React / vanilla）。

> 私有包。这里是协议契约的**唯一源**（scheme / 下载地址 / 流地址格式）；改契约就在此发新版本。
> 服务端（建任务接口 + Arrow 流 + schema）的对接见仓库内 [`docs/集成接入指南.md`](../docs/集成接入指南.md)。

## 安装

```bash
npm i @yuanmai/client-export
# 需先把私有 scope 指到你们的 registry，见下「私有 registry 配置」
```

产物为 **ESM + CJS 双格式**并自带类型声明，`import` / `require` 均可。

## 它需要后端提供什么

- **建任务接口**（你传给 `jobApi`）：返回 `{ jobId, token, streamPath }`（可被业务 `Result` 包一层，SDK 会自动从外到内解包）。
- **数据流接口**（`streamPath`，默认 `/shy/export/stream`）：对 `GET ?job=&token=&Authorization=` 返回 **Apache Arrow IPC 流**，
  `Content-Type` **不得含 `json`**；可选响应头 `X-Export-Total-Orders: <n>` 用于进度/完整性校验。
  错误用 HTTP 4xx/5xx 或 `Content-Type: application/json` + `{message|error|msg}` 表达。

## 用法

### 配方 A — fetch（新项目，无 axios，零接线）

SDK 默认用原生 `fetch`，自己负责拼 URL、附 `Authorization` 头、解包返回体。

```js
import { launchClientExport } from '@yuanmai/client-export'

await launchClientExport({
  baseURL: 'https://api.example.com',      // 给 jobApi 拼前缀
  jobApi: '/mall/order/export/client',
  params: getFilters(),
  host: 'https://api.example.com',          // 数据流 origin（桌面客户端直连，需与服务端可达地址一致）
  // auth 默认取 localStorage.token；如不同请显式传 auth
  onStatus: (s) => console.log(s),
})
```

### 配方 B — 复用项目的 axios 拦截器（推荐用于已有 axios 的项目）

传 `request`，SDK 就用它发建任务请求，从而复用项目的 baseURL、鉴权头注入、`Result` 解包。

```js
import { launchClientExport } from '@yuanmai/client-export'

await launchClientExport({
  request: this.$axios,                     // 复用拦截器
  jobApi: '/mall/order/export/client',
  params: this.getParams(),
  host: this.$envConst.API_ORIGIN,          // ⚠ 必传：见下「关于 host」
  onStatus: (s) => {
    if (s === 'launched') this.$Message.success('已唤起客户端，请在客户端查看导出进度')
  },
})
```

### 配方 C — 自定义传输 / 鉴权

```js
import { launchClientExport } from '@yuanmai/client-export'

await launchClientExport({
  fetchImpl: myFetch,                        // 自定义 fetch（默认 window.fetch）
  headers: { 'X-Tenant': 't1' },             // fetch 路径附加头
  auth: getSsoToken(),                        // 覆盖默认的 localStorage.token
  baseURL: API,
  jobApi: '/export/client',
  host: API,
  method: 'POST',                            // 建任务用 POST，params 作为 JSON body
})
```

## ⚠ 关于 `host`（最常见的坑）

`host` 决定桌面客户端要去**直连**的数据流 origin，**默认是 `window.location.origin`**。

如果你的接口是经由 axios `baseURL` 带前缀访问的（例如 `baseURL = https://boss.xw-jd.com/api`），
那 `window.location.origin`（`https://boss.xw-jd.com`，**没有 `/api`**）会让流地址少掉前缀、导致客户端 404。
此时**必须显式传** `host`，带上同样的前缀：

```js
host: this.$envConst.API_ORIGIN   // e.g. 'https://boss.xw-jd.com/api'
```

只有当数据流确实挂在站点根 origin 下时，才可以省略 `host`。

## API

| 导出 | 说明 |
| --- | --- |
| `launchClientExport(opts)` | 主入口：建任务 → 唤起 → 未安装弹下载页。返回 `{jobId, deepLink, streamUrl, launched}` |
| `createJob(opts)` | 仅建任务（双传输），返回 `{jobId, token, streamPath}` |
| `buildDeepLink({host, streamPath, jobId, token, auth})` | 仅拼 `{streamUrl, deepLink}`，不发请求 |
| `launchProtocol(url, timeout?)` | 仅唤起协议并启发式判断是否成功 |
| `showClientDownloadDialog({onRetry?})` | 仅弹下载页 |
| `EXPORT_SCHEME` / `CLIENT_DOWNLOAD` | 常量：scheme 名 / 下载直链 |

`opts` 完整字段见随包的类型声明 `index.d.ts`。

## 深链契约（与桌面客户端约定，勿改）

```
shyexport://export?job=<jobId>&token=<token>&url=<encodeURIComponent(streamUrl)>
streamUrl = <host><streamPath>?job=<jobId>&token=<token>&Authorization=<auth>
```

`url` 参数**必须** `encodeURIComponent`，否则内部的 `?`/`&` 会被当成深链参数解析。
客户端解析见仓库 `ui/main.js` 的 `parseDeepLink`。

## 已知局限

- **唤起检测是启发式的**：浏览器无法确证自定义协议是否被接管，SDK 以「超时后本页是否仍持有焦点」近似判断。
  误判为「未唤起」时会弹下载页，用户可点「我已安装，重新导出」重试。
- **跨域**：fetch 路径直连建任务接口时需后端允许对应来源并接受 query/header 鉴权；需要复用浏览器会话/拦截器的项目请用配方 B。
- **老浏览器**：产物 target 为 `es2018`。若需支持更老环境，消费方把 `@yuanmai/client-export` 加进各自构建的 `transpileDependencies`（vue-cli）/ 等价配置。

---

## 维护者：构建与发布

registry 已指向 cnb.cool 私有源：`https://npm.cnb.cool/fucksky/yuanmai/registry/`（见 `package.json` 的 `publishConfig`）。

### 方式一：CI 自动发布（推荐）

工作流 `.github/workflows/publish-web-sdk.yml`：

1. 一次性在仓库 **Settings → Secrets and variables → Actions** 配置 `NPM_TOKEN`（cnb.cool 生成的、对该 registry 有写权限的令牌）。
2. 发版：
   ```bash
   # 改 web-sdk/package.json 的 version 后
   git tag web-sdk-v0.2.0
   git push origin web-sdk-v0.2.0     # → CI 校验版本一致 → npm test → 构建 → npm publish
   ```
3. 想先验证不发版：Actions 页手动「Run workflow」→ 只做 `npm publish --dry-run`。

> tag 用 `web-sdk-v*` 前缀，与桌面端 `release.yml` 的 `v*`（构建 .dmg/.exe）解耦，互不触发。

### 方式二：本地手动发布

```bash
cd web-sdk
cp .npmrc.example .npmrc            # 已被 .gitignore；勿提交明文 token
npm install
npm run pack:check                  # 构建 + npm pack --dry-run，检查待发布文件清单
NPM_TOKEN=xxxxx npm publish         # prepublishOnly 自动 npm test + 构建
```

发版即改 `version`（建议跟 ShyExcel 客户端协议版本对齐），契约不变的纯前端改动按 SemVer patch/minor 递增。
