/**
 * ShyExcel 客户端高速导出 — 通用唤起 SDK（零依赖，跨业务/跨框架）。
 *
 * 私有 npm 包，供任意 PC 前端（Vue2 / Vue3 / React / vanilla，axios 版本不限）`npm i` 后复用：
 * 调后端「建任务」接口拿 {jobId, token, streamPath} → 拼自定义协议深链 `shyexport://`
 * → 唤起桌面客户端「ShyExcel」拉 Arrow 流本地生成 xlsx。
 *
 * 设计取向：
 *   - 零依赖、零项目耦合：不 import 任何项目文件（无 envConst 等）。
 *   - 传输二选一：默认用原生 fetch（新项目零接线即可用）；也可传 axios-like 的 request 复用项目拦截器。
 *   - 单一桌面客户端：scheme / 下载地址 / 品牌写死在本包（所有业务系统唤起同一个 ShyExcel）。
 *
 * @example fetch（新项目，无 axios）
 *   import { launchClientExport } from '@yuanmai/client-export'
 *   await launchClientExport({
 *     baseURL: 'https://api.example.com',   // jobApi 前缀
 *     jobApi: '/mall/order/export/client',
 *     params: this.getParams(),
 *     host: 'https://api.example.com',       // 数据流 origin（桌面客户端直连）
 *     // auth 默认取 localStorage.token
 *   })
 *
 * @example 复用 axios 拦截器
 *   await launchClientExport({
 *     request: this.$axios,                  // 复用项目的 baseURL/鉴权/Result 解包
 *     jobApi: '/mall/order/export/client',
 *     params: this.getParams(),
 *     host: this.$envConst.API_ORIGIN,       // 必传：流地址需带与接口一致的前缀（如 /api）
 *     onStatus: (s) => console.log(s),       // creating | launching | launched | maybe_not_installed
 *   })
 *
 * SOURCE: @yuanmai/client-export （shy-excel-app/web-sdk/src/index.js）— 协议契约的唯一源，改协议后发新版本。
 */

export const EXPORT_SCHEME = 'shyexport'

/** 把可能为相对路径的 host 归一为绝对 origin（去尾斜杠）。 */
function absOrigin(host) {
  const h = (host || '').trim()
  if (/^https?:\/\//.test(h)) return h.replace(/\/+$/, '')
  return (window.location.origin + h).replace(/\/+$/, '')
}

/**
 * 唤起自定义协议并粗略判断是否唤起成功。
 * 浏览器无法直接确认协议被处理；以「超时点本页是否仍持有焦点」近似判断：成功唤起会让客户端
 * 抢占前台、本页失焦；而「找不到文件」等系统错误弹窗只是瞬时夺焦，焦点很快回到本页。
 * 不再把「发生过 blur」当成功——否则错误弹窗的瞬时夺焦会把「未安装」误判为「已唤起」、不弹下载页。
 * @returns {Promise<boolean>} 失焦(被客户端接管)返回 true；超时仍在前台返回 false（未安装/未唤起→弹下载页）。
 */
export function launchProtocol(url, timeout = 1600) {
  return new Promise((resolve) => {
    let done = false
    let backTimer = null
    const finish = (ok) => {
      if (done) return
      done = true
      document.removeEventListener('visibilitychange', onHide)
      window.removeEventListener('focus', onBack)
      clearTimeout(timer)
      if (backTimer) clearTimeout(backTimer)
      resolve(ok)
    }
    // 切到后台（移动端/部分桌面）→ 已唤起。
    const onHide = () => { if (document.hidden) finish(true) }
    // 失焦后焦点又回到本页 → 说明刚才是系统错误弹窗（如「找不到文件」），并非客户端 → 判未唤起。
    const onBack = () => finish(false)
    document.addEventListener('visibilitychange', onHide)
    try {
      // 用隐藏 iframe 尝试唤起：协议未注册/损坏时错误发生在 iframe 内，不会把当前 SPA 导航走。
      const iframe = document.createElement('iframe')
      iframe.style.display = 'none'
      iframe.src = url
      document.body.appendChild(iframe)
      setTimeout(() => { try { iframe.remove() } catch (e) { /* ignore */ } }, 1200)
    } catch (e) {
      try { window.location.href = url } catch (e2) { /* ignore */ }
    }
    // 两段判定，兼容「系统错误弹窗瞬时/持续夺焦」两种情况：
    // ① 超时点仍持有焦点 → 客户端没接管 → 未唤起 → 上层弹下载页。
    // ② 超时点失焦 → 可能客户端接管，也可能「找不到文件」错误弹窗仍开着；再等焦点是否回来：
    //    回来 = 错误弹窗（用户点了「确定」）→ 未唤起（弹下载页）；一直不回来 = 客户端已接管。
    const timer = setTimeout(() => {
      const focused = typeof document.hasFocus !== 'function' || document.hasFocus()
      if (focused) { finish(false); return }
      window.addEventListener('focus', onBack)
      backTimer = setTimeout(() => finish(true), 2500)
    }, timeout)
  })
}

/** 客户端下载直链（企业内部分发，固定地址）。换域名/路径在此修改即可。 */
export const CLIENT_DOWNLOAD = {
  windows: 'https://file.xw-jd.com/app/shy-execl/shy-export-client-x64.exe',
  macos: 'https://file.xw-jd.com/app/shy-execl/shy-export-client-arm64.dmg',
}

/** 粗略识别当前系统，用于在下载页高亮推荐项。 */
function detectOS() {
  const ua = (navigator.userAgent || '') + ' ' + (navigator.platform || '')
  if (/Mac|iPhone|iPad|iPod/i.test(ua)) return 'macos'
  if (/Win/i.test(ua)) return 'windows'
  return ''
}

let _dlEl = null
/**
 * 未检测到客户端时弹出的「下载客户端」页（自包含、跨页面复用、Liquid Glass 风格）。
 * @param {Object} [o]
 * @param {Function} [o.onRetry] 点击「我已安装，重新导出」时回调（通常重跑一次导出）。
 */
export function showClientDownloadDialog(o = {}) {
  const onRetry = o.onRetry
  if (_dlEl) { try { _dlEl.remove() } catch (e) { /* ignore */ } _dlEl = null }
  const rec = detectOS()
  const winUrl = CLIENT_DOWNLOAD.windows
  const macUrl = CLIENT_DOWNLOAD.macos
  const LOGO = '<svg viewBox="0 0 32 32" fill="none"><defs><linearGradient id="shyxDlLogo" x1="3" y1="3" x2="29" y2="29" gradientUnits="userSpaceOnUse"><stop stop-color="#6CF6D2"/><stop offset="1" stop-color="#0E9E6B"/></linearGradient></defs><rect x="2" y="2" width="28" height="28" rx="9" fill="url(#shyxDlLogo)"/><rect x="2.7" y="2.7" width="26.6" height="26.6" rx="8.3" stroke="#fff" stroke-opacity=".55"/><rect x="8.5" y="8" width="15" height="16" rx="3" fill="#fff" fill-opacity=".96"/><path d="M8.5 13h15" stroke="#13B98A" stroke-opacity=".55" stroke-width="1.4"/><path d="M16 8v16M8.5 18.5h15" stroke="#13B98A" stroke-opacity=".3" stroke-width="1.1"/></svg>'
  const WIN = '<svg viewBox="0 0 24 24" fill="#2b8fd6"><rect x="2" y="2" width="9" height="9" rx="1"/><rect x="13" y="2" width="9" height="9" rx="1"/><rect x="2" y="13" width="9" height="9" rx="1"/><rect x="13" y="13" width="9" height="9" rx="1"/></svg>'
  const APPLE = '<svg viewBox="0 0 24 24" fill="#1d1d1f"><path d="M16.365 1.43c0 1.14-.493 2.27-1.177 3.08-.744.9-1.99 1.57-2.987 1.57-.12 0-.23-.02-.3-.03-.01-.06-.04-.22-.04-.39 0-1.15.572-2.27 1.206-2.98.804-.94 2.142-1.64 3.248-1.68.03.13.04.28.04.43zm4.565 15.71c-.03.07-.463 1.58-1.518 3.12-.945 1.34-1.94 2.71-3.43 2.71-1.517 0-1.9-.88-3.63-.88-1.698 0-2.302.91-3.67.91-1.377 0-2.332-1.26-3.428-2.8-1.287-1.82-2.323-4.63-2.323-7.28 0-4.28 2.797-6.55 5.552-6.55 1.448 0 2.675.95 3.6.95.865 0 2.222-1.01 3.902-1.01.613 0 2.886.06 4.374 2.19-.13.09-2.383 1.37-2.383 4.19 0 3.26 2.854 4.42 2.957 4.45z"/></svg>'
  const badge = '<span class="shyx-dl-badge">为你推荐</span>'
  // Shadow DOM 隔离：内部样式不会泄漏到宿主页面，宿主/ViewUI 的全局样式也不会影响本弹窗。
  const el = document.createElement('div')
  el.className = 'shyx-dl-host'
  const root = el.attachShadow({ mode: 'open' })
  root.innerHTML =
    '<style>' +
    '*{box-sizing:border-box}' +
    '.shyx-dl-mask{position:fixed;inset:0;z-index:99999;display:flex;align-items:center;justify-content:center;background:rgba(8,28,22,.46);backdrop-filter:blur(8px) saturate(120%);-webkit-backdrop-filter:blur(8px) saturate(120%);font-family:-apple-system,BlinkMacSystemFont,"Segoe UI","PingFang SC","Microsoft YaHei",system-ui,sans-serif;letter-spacing:normal;line-height:1.5;text-align:left;animation:shyxFade .22s ease}' +
    '@keyframes shyxFade{from{opacity:0}to{opacity:1}}' +
    '.shyx-dl-card{position:relative;width:540px;max-width:calc(100vw - 32px);padding:30px 30px 24px;border-radius:26px;background:radial-gradient(120% 90% at 12% 0%,rgba(95,233,194,.18),transparent 55%),linear-gradient(180deg,rgba(255,255,255,.97),rgba(244,253,249,.95));border:1px solid rgba(255,255,255,.7);box-shadow:0 30px 80px rgba(4,30,22,.35),inset 0 1px 0 rgba(255,255,255,.9);color:#102019;animation:shyxRise .28s cubic-bezier(.22,.61,.36,1)}' +
    '@keyframes shyxRise{from{opacity:0;transform:translateY(12px) scale(.98)}to{opacity:1;transform:none}}' +
    '.shyx-dl-x{position:absolute;top:16px;right:16px;width:30px;height:30px;border:none;border-radius:10px;background:rgba(8,42,30,.06);color:#5b6b62;font-size:14px;cursor:pointer;transition:background .15s}' +
    '.shyx-dl-x:hover{background:rgba(8,42,30,.12)}' +
    '.shyx-dl-head{display:flex;align-items:center;gap:12px}' +
    '.shyx-dl-logo{width:44px;height:44px;display:block;filter:drop-shadow(0 6px 14px rgba(8,120,90,.35))}.shyx-dl-logo svg{width:44px;height:44px;display:block}' +
    '.shyx-dl-brand{font-weight:700;font-size:17px;letter-spacing:.2px}.shyx-dl-brand span{color:#8a988f;font-weight:500;font-size:13px;margin-left:6px}' +
    '.shyx-dl-title{margin:18px 0 6px;font-size:21px;font-weight:680;letter-spacing:.2px}' +
    '.shyx-dl-sub{margin:0 0 20px;font-size:13.5px;line-height:1.6;color:#566b62}' +
    '.shyx-dl-os{display:grid;grid-template-columns:1fr 1fr;gap:14px}' +
    '.shyx-dl-osc{position:relative;display:flex;flex-direction:column;align-items:flex-start;gap:4px;padding:18px;border-radius:18px;text-decoration:none;color:inherit;cursor:pointer;background:rgba(255,255,255,.72);border:1px solid rgba(8,42,30,.10);box-shadow:inset 0 1px 0 rgba(255,255,255,.9);transition:transform .16s ease,box-shadow .16s ease,border-color .16s}' +
    '.shyx-dl-osc:hover{transform:translateY(-2px);box-shadow:0 14px 30px rgba(6,40,30,.16),inset 0 1px 0 rgba(255,255,255,.9)}' +
    '.shyx-dl-osc.rec{border-color:rgba(16,185,129,.55);box-shadow:0 0 0 3px rgba(16,185,129,.16),inset 0 1px 0 rgba(255,255,255,.9)}' +
    '.shyx-dl-badge{position:absolute;top:-9px;right:14px;font-size:11px;font-weight:600;color:#fff;background:linear-gradient(155deg,#34e2b0,#0e9e6b);padding:3px 9px;border-radius:999px;box-shadow:0 4px 10px rgba(14,158,107,.4)}' +
    '.shyx-dl-osi{width:34px;height:34px;display:block}.shyx-dl-osi svg{width:34px;height:34px;display:block}' +
    '.shyx-dl-osn{font-weight:650;font-size:15px;margin-top:6px}.shyx-dl-osd{font-size:12px;color:#8a988f}' +
    '.shyx-dl-cta{margin-top:10px;font-size:13px;font-weight:650;color:#0a8f63}.shyx-dl-osc:hover .shyx-dl-cta{color:#0e9e6b}' +
    '.shyx-dl-steps{display:flex;align-items:center;justify-content:center;gap:10px;margin:22px 0 6px;flex-wrap:wrap}' +
    '.shyx-dl-step{display:flex;align-items:center;gap:7px;font-size:12.5px;color:#566b62}' +
    '.shyx-dl-step i{width:20px;height:20px;border-radius:50%;display:grid;place-items:center;font-style:normal;font-size:11px;font-weight:700;color:#fff;background:linear-gradient(155deg,#34e2b0,#10b981)}' +
    '.shyx-dl-arrow{color:#c2d2cb;font-size:12px}' +
    '.shyx-dl-note{margin:10px 0 0;font-size:11.5px;color:#9aa8a0;text-align:center}' +
    '.shyx-dl-foot{display:flex;justify-content:flex-end;gap:10px;margin-top:20px}' +
    '.shyx-dl-btn{font-family:inherit;font-size:14px;padding:9px 18px;border-radius:12px;cursor:pointer;border:1px solid transparent;transition:filter .15s,background .15s}' +
    '.shyx-dl-btn.ghost{background:transparent;color:#566b62;border-color:rgba(8,42,30,.14)}.shyx-dl-btn.ghost:hover{background:rgba(8,42,30,.05)}' +
    '.shyx-dl-btn.primary{color:#fff;font-weight:600;background:linear-gradient(155deg,#34e2b0,#10b981 55%,#0a8f63);box-shadow:0 8px 20px rgba(16,185,129,.4),inset 0 1px 0 rgba(255,255,255,.45)}.shyx-dl-btn.primary:hover{filter:brightness(1.05)}' +
    '@media (max-width:520px){.shyx-dl-os{grid-template-columns:1fr}}' +
    '</style>' +
    '<div class="shyx-dl-mask" data-backdrop>' +
    '<div class="shyx-dl-card" role="dialog" aria-modal="true">' +
    '<button class="shyx-dl-x" data-close aria-label="关闭">✕</button>' +
    '<div class="shyx-dl-head"><span class="shyx-dl-logo">' + LOGO + '</span>' +
    '<div class="shyx-dl-brand">ShyExcel<span>· 数据导出</span></div></div>' +
    '<h2 class="shyx-dl-title">需要先安装 ShyExcel 客户端</h2>' +
    '<p class="shyx-dl-sub">大数据量导出由本地客户端高速生成。请先下载安装客户端，安装完成后回到本页重新点击「导出」即可。</p>' +
    '<div class="shyx-dl-os">' +
    '<a class="shyx-dl-osc' + (rec === 'windows' ? ' rec' : '') + '" href="' + winUrl + '" target="_blank" rel="noopener">' +
    (rec === 'windows' ? badge : '') +
    '<span class="shyx-dl-osi">' + WIN + '</span><span class="shyx-dl-osn">Windows</span><span class="shyx-dl-osd">.exe 安装包</span><span class="shyx-dl-cta">下载 ↓</span></a>' +
    '<a class="shyx-dl-osc' + (rec === 'macos' ? ' rec' : '') + '" href="' + macUrl + '" target="_blank" rel="noopener">' +
    (rec === 'macos' ? badge : '') +
    '<span class="shyx-dl-osi">' + APPLE + '</span><span class="shyx-dl-osn">macOS</span><span class="shyx-dl-osd">Apple 芯片 · .dmg</span><span class="shyx-dl-cta">下载 ↓</span></a>' +
    '</div>' +
    '<div class="shyx-dl-steps"><span class="shyx-dl-step"><i>1</i>下载并安装</span><span class="shyx-dl-arrow">→</span><span class="shyx-dl-step"><i>2</i>打开客户端</span><span class="shyx-dl-arrow">→</span><span class="shyx-dl-step"><i>3</i>回本页重新导出</span></div>' +
    '<p class="shyx-dl-note">macOS 首次打开：右键 App →「打开」即可（企业内部分发，暂未签名）。</p>' +
    '<div class="shyx-dl-foot"><button class="shyx-dl-btn ghost" data-close>关闭</button><button class="shyx-dl-btn primary" data-retry>我已安装，重新导出</button></div>' +
    '</div></div>'
  document.body.appendChild(el)
  _dlEl = el

  const close = () => {
    try { el.remove() } catch (e) { /* ignore */ }
    if (_dlEl === el) _dlEl = null
    document.removeEventListener('keydown', onKey)
  }
  const onKey = (e) => { if (e.key === 'Escape') close() }
  document.addEventListener('keydown', onKey)
  root.querySelectorAll('[data-close]').forEach((b) => b.addEventListener('click', close))
  const mask = root.querySelector('[data-backdrop]')
  if (mask) mask.addEventListener('click', (e) => { if (e.target === mask) close() })
  const retry = root.querySelector('[data-retry]')
  if (retry) retry.addEventListener('click', () => { close(); if (onRetry) onRetry() })
}

/** 把对象拼成查询串（跳过 undefined/null）；fetch 路径用，与 axios 的 params 序列化对齐基础场景。 */
function buildQuery(params) {
  const p = params || {}
  const parts = []
  for (const k in p) {
    if (!Object.prototype.hasOwnProperty.call(p, k)) continue
    const v = p[k]
    if (v === undefined || v === null) continue
    parts.push(encodeURIComponent(k) + '=' + encodeURIComponent(v))
  }
  return parts.length ? '?' + parts.join('&') : ''
}

/**
 * 双传输建任务：传了 request 走 axios-like（复用项目拦截器/鉴权/Result 解包）；否则走原生 fetch。
 * 两路都用同一个「从外到内找第一个带 jobId 的对象」启发式解包，兼容三种返回形态：
 *   1) 拦截器已解包（res 直接是 Dispatch）  2) Result 作为 body（res.data 是 Dispatch）
 *   3) 原始响应（res.data.data 是 Dispatch）
 * @returns {Promise<{jobId:String, token:String, streamPath:String}>}
 */
export async function createJob(o) {
  const { jobApi, params = {}, method = 'GET', request, fetchImpl, baseURL = '', headers, auth } = o || {}
  if (!jobApi) throw new Error('createJob: 缺少 jobApi')
  const has = (x) => x && typeof x === 'object' && Object.prototype.hasOwnProperty.call(x, 'jobId')

  let res
  if (request) {
    res = method === 'POST'
      ? await request.post(jobApi, params)
      : await request.get(jobApi, { params })
  } else {
    const f =
      fetchImpl ||
      (typeof window !== 'undefined' && window.fetch && window.fetch.bind(window)) ||
      (typeof fetch !== 'undefined' ? fetch : null)
    if (!f) throw new Error('launchClientExport: 当前环境无 fetch，请传 request 或 fetchImpl')
    const authToken = auth || (typeof localStorage !== 'undefined' && localStorage.token) || ''
    const hdr = Object.assign({}, authToken ? { Authorization: authToken } : {}, headers || {})
    let url, init
    if (method === 'POST') {
      url = baseURL + jobApi
      init = { method: 'POST', headers: Object.assign({ 'Content-Type': 'application/json' }, hdr), body: JSON.stringify(params) }
    } else {
      url = baseURL + jobApi + buildQuery(params)
      init = { method: 'GET', headers: hdr }
    }
    const r = await f(url, init)
    if (!r || !r.ok) throw new Error('建任务失败：HTTP ' + (r ? r.status : 'no-response'))
    res = await r.json()
  }

  const d = [res, res && res.data, res && res.data && res.data.data].find(has) || {}
  const jobId = d.jobId
  const token = d.token
  const streamPath = d.streamPath || '/shy/export/stream'
  if (!jobId || !token) throw new Error('建任务失败：返回缺少 jobId/token')
  return { jobId, token, streamPath }
}

/**
 * 由建任务结果拼出数据流地址与唤起深链。
 * 数据端点经 Shiro 鉴权：服务端 GetToken 依次从 header → query(Authorization) → cookie 取登录 token。
 * 桌面客户端不带浏览器会话，故把登录 token 作为 Authorization 查询参数附到流地址上，
 * 客户端原样请求即可通过 Shiro 校验（无需放开白名单）。越权仍由建任务时固定的 filter 拦住。
 * @returns {{streamUrl:String, deepLink:String}}
 */
export function buildDeepLink(o) {
  const { host, streamPath = '/shy/export/stream', jobId, token, auth } = o || {}
  const authToken = auth || (typeof localStorage !== 'undefined' && localStorage.token) || ''
  const streamUrl =
    absOrigin(host) + streamPath +
    `?job=${encodeURIComponent(jobId)}&token=${encodeURIComponent(token)}` +
    (authToken ? `&Authorization=${encodeURIComponent(authToken)}` : '')
  const deepLink =
    `${EXPORT_SCHEME}://export?job=${encodeURIComponent(jobId)}` +
    `&token=${encodeURIComponent(token)}&url=${encodeURIComponent(streamUrl)}`
  return { streamUrl, deepLink }
}

/**
 * 发起一次客户端高速导出（建任务 → 唤起客户端 → 未安装则弹下载页）。
 * @param {Object} o
 * @param {String}   o.jobApi      必填：建任务接口（默认 GET），返回体含 {jobId, token, streamPath}（兼容被 Result 包一层）
 * @param {Object}   [o.params]    导出筛选条件（与现有列表查询一致）
 * @param {String}   [o.method]    建任务请求方法：'GET'(默认) | 'POST'
 * @param {Object}   [o.request]   axios-like 实例（有 .get/.post）；传了则复用项目拦截器/鉴权/解包
 * @param {Function} [o.fetchImpl] 自定义 fetch（默认 window.fetch）；仅在未传 request 时用
 * @param {String}   [o.baseURL]   fetch 路径下给 jobApi 拼的前缀（如 'https://api.example.com'）
 * @param {Object}   [o.headers]   fetch 路径下附加请求头
 * @param {String}   [o.host]      数据端点 origin（桌面客户端直连），默认 window.location.origin。
 *                                 注意：若接口走 axios 的 baseURL 前缀（如 /api），此处须显式传同样带前缀的 origin。
 * @param {String}   [o.auth]      登录 token，作为 ?Authorization= 透传给客户端，默认 localStorage.token
 * @param {Function} [o.onStatus]  状态回调：'creating' | 'launching' | 'launched' | 'maybe_not_installed'
 * @param {Boolean}  [o.autoPrompt] 未唤起时是否自动弹下载页，默认 true
 * @returns {Promise<{jobId:String, deepLink:String, streamUrl:String, launched:Boolean}>}
 */
export async function launchClientExport(o) {
  const {
    request, jobApi, params = {}, method = 'GET',
    fetchImpl, baseURL, headers,
    host = (typeof window !== 'undefined' ? window.location.origin : ''),
    auth, onStatus, autoPrompt = true,
  } = o || {}
  if (!jobApi) throw new Error('launchClientExport: 缺少 jobApi')

  onStatus && onStatus('creating')
  const { jobId, token, streamPath } = await createJob({ jobApi, params, method, request, fetchImpl, baseURL, headers, auth })

  const { streamUrl, deepLink } = buildDeepLink({ host, streamPath, jobId, token, auth })

  onStatus && onStatus('launching')
  const launched = await launchProtocol(deepLink)
  onStatus && onStatus(launched ? 'launched' : 'maybe_not_installed')
  // 未检测到客户端 → 弹出精美下载页（Win/macOS），并提供「我已安装，重新导出」一键重试。
  if (!launched && autoPrompt) {
    showClientDownloadDialog({ onRetry: () => launchClientExport(o) })
  }
  return { jobId, deepLink, streamUrl, launched }
}

export default launchClientExport
