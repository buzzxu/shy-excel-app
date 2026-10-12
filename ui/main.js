// 前端：任务管理 + 实时进度/ETA + 设置（下载目录/每文件单数） + 文件定位 + 清理。
// 用 withGlobalTauri：window.__TAURI__.{core.invoke, event.listen, store.load, dialog}
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;
const { load } = window.__TAURI__.store;

const tasksEl = document.getElementById("tasks");
const emptyEl = document.getElementById("empty");
const MAX_TASKS = 10;

let store;
let settings = { dir: "", chunk: 70000 };
let tasks = []; // 最新在前
let trusted = []; // 已信任的数据来源（origin），持久化于 store.trusted_origins
const asking = new Map(); // origin → 进行中的信任确认；同来源的后续深链等同一个结果，不叠框
let askQueue = Promise.resolve(); // 不同来源的确认框排队依次弹出
const rejected = new Set(); // 本次运行内被拒绝的来源：其深链直接忽略、不再询问（不持久化，重启后恢复询问）

// 并发闸：同时最多 MAX_CONCURRENT 个导出，其余先进先出排队；排队中取消直接出队，不调 run_export。
const MAX_CONCURRENT = 2;
let active = 0;
const waiting = []; // { t, go }：go(true)=轮到它开始，go(false)=排队中被取消

// ---------- 工具 ----------
const fmtNum = (n) => (n || 0).toLocaleString("zh-CN");
function fmtDuration(ms) {
  const s = Math.max(0, ms / 1000);
  if (s < 60) return `${s.toFixed(s < 10 ? 1 : 0)} 秒`;
  const m = Math.floor(s / 60), r = Math.round(s % 60);
  return `${m} 分 ${r} 秒`;
}
function fmtEta(sec) {
  if (!isFinite(sec) || sec < 0) return "—";
  if (sec < 1) return "即将完成";
  if (sec < 60) return `约 ${Math.ceil(sec)} 秒`;
  return `约 ${Math.ceil(sec / 60)} 分`;
}
function estMem(chunk) {
  if (chunk >= 100000) return "≈2.5GB+/块";
  if (chunk >= 50000) return "≈1.9GB/块";
  if (chunk >= 20000) return "≈1.5GB/块";
  if (chunk >= 10000) return "≈1.2GB/块";
  return "≈0.7GB/块";
}
const esc = (s) => String(s == null ? "" : s).replace(/[&<>"]/g, (c) => ({ "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c]));
// 路径展示：短路径全显，过长则「…/父目录/文件名」（始终 LTR、保留文件名，避免 direction:rtl 的斜杠错位）。完整路径放 title。
function dispPath(p) {
  if (!p) return "";
  const parts = p.split(/[\\/]/).filter(Boolean);
  if (parts.length <= 3) return p;
  return "…/" + parts.slice(-2).join("/");
}

// ---------- 任务卡片 ----------
function cardInner(t) {
  const short = t.job ? t.job.slice(0, 8) : t.id;
  const head = (badge) =>
    `<div class="top"><span class="name">导出任务 ${esc(short)}</span>${badge}` +
    (t.status !== "running"
      ? `<button class="btn ghost icon del" data-del title="删除"><svg class="i" viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg></button>`
      : `<button class="btn ghost cancel" data-cancel title="取消导出" ${t.cancelling ? "disabled" : ""}>${t.cancelling ? "取消中…" : "取消"}</button>`) +
    `</div>`;

  if (t.status === "running") {
    const hasTotal = t.total > 0; // 进度条宽度由 updateProgress 经 CSSOM 设置：CSP 不允许 style 属性
    return (
      head(`<span class="badge run">${t.queued ? "排队中" : "进行中"}</span>`) +
      `<div class="prog">
         <div class="bar ${hasTotal ? "" : "indet"}"><i data-bar></i></div>
         <div class="meta" data-meta>${runMeta(t)}</div>
       </div>`
    );
  }
  if (t.status === "done") {
    const icon = t.isDir
      ? `<svg class="i" viewBox="0 0 24 24"><path d="M3 7a2 2 0 0 1 2-2h4l2 2h8a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z"/></svg>`
      : `<svg class="i" viewBox="0 0 24 24"><path d="M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z"/><path d="M14 3v5h5"/></svg>`;
    return (
      head(`<span class="badge done">完成</span>`) +
      `<div class="stats">
         <div class="stat"><div class="v">${t.files}</div><div class="k">个文件</div></div>
         <div class="stat"><div class="v">${fmtNum(t.rows)}</div><div class="k">行记录</div></div>
         <div class="stat"><div class="v">${fmtNum(t.orders)}</div><div class="k">单</div></div>
         <div class="stat"><div class="v">${fmtDuration(t.elapsedMs)}</div><div class="k">耗时</div></div>
       </div>
       <div class="path" data-path title="${esc(t.path)}">
         <span class="ic">${icon}</span>
         <span class="p">${esc(dispPath(t.path))}</span>
         <span class="go">${t.isDir ? "打开目录 ▸" : "打开 ▸"}</span>
       </div>`
    );
  }
  // error
  return head(`<span class="badge err">失败</span>`) + `<div class="errbox">${esc(t.error || "导出失败")}</div>`;
}

function runMeta(t) {
  if (t.queued) return `<span>排队中…</span>`;
  const elapsed = (Date.now() - t.startedAt) / 1000;
  const left = `已导出 ${fmtNum(t.orders)} 单 · ${fmtNum(t.rows)} 行`;
  let right;
  if (t.total > 0 && t.orders > 0) {
    const frac = Math.min(t.orders / t.total, 0.999);
    const remain = (elapsed * (1 - frac)) / frac;
    right = `预计剩 ${fmtEta(remain)}`;
  } else {
    const rate = elapsed > 0 ? Math.round(t.rows / elapsed) : 0;
    right = rate > 0 ? `${fmtNum(rate)} 行/秒` : "处理中…";
  }
  return `<span>${left}</span><span>${right}</span>`;
}

function bindCard(t) {
  const del = t.el.querySelector("[data-del]");
  if (del) del.onclick = () => removeTask(t);
  const cancel = t.el.querySelector("[data-cancel]");
  if (cancel) cancel.onclick = () => {
    const q = waiting.findIndex((w) => w.t === t);
    if (q >= 0) {
      waiting.splice(q, 1)[0].go(false); // 排队中：出队即可，不调 run_export
      return;
    }
    t.cancelling = true; // 记在任务上，重绘卡片后仍显示「取消中…」
    refreshCard(t);
    invoke("cancel_export", { taskId: t.id }).catch(() => {});
  };
  const path = t.el.querySelector("[data-path]");
  if (path) path.onclick = () => invoke("reveal_path", { path: t.path }).catch(() => {});
  updateProgress(t);
}

function mountTask(t) {
  const el = document.createElement("div");
  el.className = "card";
  el.innerHTML = cardInner(t);
  t.el = el;
  tasksEl.prepend(el);
  bindCard(t);
  // 淘汰最旧的非进行中任务：进行中/排队中的绝不淘汰（否则取消按钮随卡片消失），列表可暂时超过上限
  for (let i = tasks.length - 1; tasks.length > MAX_TASKS && i >= 0; i--) {
    if (tasks[i].status === "running") continue;
    const [old] = tasks.splice(i, 1);
    if (old.el) old.el.remove();
  }
  updateEmpty();
}
function refreshCard(t) {
  if (!t.el) return;
  t.el.innerHTML = cardInner(t);
  bindCard(t);
}
function updateProgress(t) {
  if (t.status !== "running" || !t.el) return;
  const bar = t.el.querySelector("[data-bar]");
  const meta = t.el.querySelector("[data-meta]");
  if (bar) bar.parentElement.classList.toggle("indet", !(t.total > 0)); // 卡片首绘时总数未知；拿到总数后改显真实进度
  if (bar && t.total > 0) bar.style.width = Math.min(99, Math.round((t.orders / t.total) * 100)) + "%";
  if (meta) meta.innerHTML = runMeta(t);
}
function removeTask(t) {
  if (t.el) t.el.remove();
  tasks = tasks.filter((x) => x !== t);
  updateEmpty();
  saveTasks();
}
function updateEmpty() {
  emptyEl.style.display = tasks.length ? "none" : "";
}

// ---------- 持久化（仅保存终态，重启后 running 视为中断）----------
function serialize(t) {
  return { id: t.id, job: t.job, status: t.status, orders: t.orders, rows: t.rows, total: t.total,
    files: t.files, path: t.path, isDir: t.isDir, elapsedMs: t.elapsedMs, error: t.error, startedAt: t.startedAt };
}
async function saveTasks() {
  if (store) await store.set("tasks", tasks.map(serialize));
}
async function saveTrusted() {
  if (store) await store.set("trusted_origins", trusted);
}

// ---------- 导出流程 ----------
let seq = 0;
function createTask(job) {
  const t = { id: "t" + Date.now() + "_" + ++seq, job, status: "running", orders: 0, rows: 0, total: 0, startedAt: Date.now() };
  tasks.unshift(t);
  mountTask(t);
  return t;
}

// 拿并发名额：有空位立即返回 true；否则排队，轮到时 true、排队中被取消时 false。
function acquire(t) {
  if (active < MAX_CONCURRENT) {
    active++;
    return Promise.resolve(true);
  }
  t.queued = true;
  refreshCard(t);
  return new Promise((go) => waiting.push({ t, go }));
}
// 归还名额：有人排队则直接转给队首，否则空出一个位置。
function release() {
  const next = waiting.shift();
  if (!next) {
    active--;
    return;
  }
  next.t.queued = false;
  next.t.startedAt = Date.now(); // 吞吐/ETA 从真正开始拉流算起
  refreshCard(next.t);
  next.go(true);
}

function parseDeepLink(url) {
  // shyexport://export?job=..&token=..&url=完整流地址（含鉴权参数，仅内部使用，绝不展示）
  // 可选 &theme=主题 JSON：只交给 parseTheme 校验，不渲染
  const q = new URLSearchParams(url.split("?")[1] || "");
  const job = q.get("job");
  let streamUrl = q.get("url");
  if (!streamUrl) {
    const host = (q.get("host") || "").replace(/\/$/, "");
    const token = q.get("token");
    streamUrl = `${host}/shy/export/stream?job=${encodeURIComponent(job)}&token=${encodeURIComponent(token)}`;
  }
  return { job, streamUrl, theme: q.get("theme") };
}

async function handleDeepLink(url) {
  const { job, streamUrl, theme } = parseDeepLink(url);
  if (!streamUrl) return;
  // 去重：同一 job 正在进行中则忽略（防同条深链既被缓冲又被事件重复投递 / 用户重复点击）
  if (job && tasks.some((t) => t.job === job && t.status === "running")) return;
  if (!settings.dir) {
    openSettings();
    return;
  }
  // 信任边界：只放行 http(s) 流地址，且其来源（origin）须经用户首次确认后才会拉取、写文件。卡片绝不展示 URL。
  let src = null;
  try { src = new URL(streamUrl); } catch (_) {}
  if (!src || (src.protocol !== "http:" && src.protocol !== "https:")) {
    const bad = createTask(job);
    bad.status = "error";
    bad.error = "导出链接无效";
    refreshCard(bad);
    saveTasks();
    return;
  }
  if (rejected.has(src.origin)) return;
  if (!trusted.includes(src.origin)) {
    let asked = asking.get(src.origin);
    if (!asked) {
      asked = askQueue = askQueue
        .then(() =>
          dialog.ask(
            `是否信任以下数据来源并开始导出？\n\n${src.origin}\n\n` +
              "信任后，来自该来源的导出链接将直接从该地址下载数据并保存到下载目录，不再询问。" +
              (src.protocol === "http:" ? "\n\n注意：该来源使用 http，连接未加密。" : ""),
            { title: "新的数据来源", kind: "warning", okLabel: "信任并导出", cancelLabel: "拒绝" }
          )
        )
        .catch(() => null); // 确认框本身弹不出（插件/权限异常）≠ 用户拒绝
      asking.set(src.origin, asked);
    }
    const ok = await asked;
    const first = asking.get(src.origin) === asked; // 同一次确认的多个等待者里，只有第一个负责出错误卡片
    asking.delete(src.origin);
    if (ok === null) {
      if (first) {
        const bad = createTask(job);
        bad.status = "error";
        bad.error = "无法显示信任确认框，请重试";
        refreshCard(bad);
        saveTasks();
      }
      return;
    }
    if (!ok) {
      rejected.add(src.origin);
      return;
    }
    if (!trusted.includes(src.origin)) {
      trusted.push(src.origin);
      saveTrusted();
    }
    // 等待确认期间同一 job 可能已由另一条深链启动
    if (job && tasks.some((t) => t.job === job && t.status === "running")) return;
  }
  // 主题只在来源受信任后生效；走到这里的每条深链都决定窗口主题（不带或无效 theme 即恢复默认）
  const th = parseTheme(theme);
  applyTheme(th);
  if (store) store.set("theme", th);
  const t = createTask(job);
  if (!(await acquire(t))) {
    t.queued = false;
    t.status = "error";
    t.error = "已取消导出";
    refreshCard(t);
    saveTasks();
    return;
  }
  try {
    const out = await invoke("run_export", {
      taskId: t.id,
      jobId: job || t.id,
      streamUrl, // 内部传递，不渲染
      downloadDir: settings.dir,
      baseName: "导出_" + new Date().toISOString().slice(0, 10),
      ordersPerFile: settings.chunk,
    });
    t.status = "done";
    t.files = out.files;
    t.orders = out.orders;
    t.rows = out.rows;
    t.path = out.path;
    t.isDir = out.is_dir;
    t.elapsedMs = out.elapsed_ms;
  } catch (e) {
    t.status = "error";
    t.error = typeof e === "string" ? e : e && e.message ? e.message : "导出失败";
  } finally {
    release();
  }
  refreshCard(t);
  saveTasks();
}

// ---------- 设置 ----------
const overlay = document.getElementById("settings");
const dirEl = document.getElementById("dir");
const chunkEl = document.getElementById("chunk");
const memhint = document.getElementById("memhint");
const originsEl = document.getElementById("origins");

function renderOrigins() {
  originsEl.innerHTML = trusted.length
    ? trusted
        .map((o, i) =>
          `<div class="origin"><span class="o" title="${esc(o)}">${esc(o)}</span>` +
          `<button class="btn ghost icon" data-rm="${i}" title="移除"><svg class="i" viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg></button></div>`)
        .join("")
    : `<div class="hint">暂无。首次从新来源导出时会询问是否信任。</div>`;
}
originsEl.onclick = (e) => {
  const b = e.target.closest("[data-rm]");
  if (!b) return;
  trusted.splice(+b.dataset.rm, 1);
  saveTrusted();
  renderOrigins();
};

function openSettings() {
  dirEl.value = settings.dir;
  chunkEl.value = settings.chunk;
  memhint.textContent = "预估峰值内存 " + estMem(settings.chunk);
  renderOrigins();
  overlay.classList.add("show");
}
function closeSettings() {
  overlay.classList.remove("show");
}
document.getElementById("openSettings").onclick = openSettings;
document.getElementById("closeSettings").onclick = closeSettings;
overlay.addEventListener("click", (e) => { if (e.target === overlay) closeSettings(); });
document.addEventListener("keydown", (e) => { if (e.key === "Escape") closeSettings(); });

document.getElementById("pick").onclick = async () => {
  const picked = await dialog.open({ directory: true, multiple: false });
  if (picked) {
    settings.dir = picked;
    dirEl.value = picked;
    if (store) await store.set("download_dir", picked);
  }
};
chunkEl.addEventListener("change", async () => {
  settings.chunk = Math.max(1000, +chunkEl.value || 70000);
  chunkEl.value = settings.chunk;
  memhint.textContent = "预估峰值内存 " + estMem(settings.chunk);
  if (store) await store.set("orders_per_file", settings.chunk);
});

// 清理已完成 / 失败
document.getElementById("clearDone").onclick = () => {
  tasks.filter((t) => t.status !== "running").forEach((t) => t.el && t.el.remove());
  tasks = tasks.filter((t) => t.status === "running");
  updateEmpty();
  saveTasks();
};

// ---------- 初始化 ----------
async function init() {
  store = await load("settings.json", { autoSave: true });
  // 恢复上次导出的窗口主题；store 文件可被手改，读回后重新校验
  applyTheme(sanitizeTheme(await store.get("theme")));

  // 下载目录：无配置则取系统默认下载目录
  let dir = await store.get("download_dir");
  if (!dir) {
    try { dir = await invoke("default_download_dir"); } catch (_) { dir = ""; }
    if (dir) await store.set("download_dir", dir);
  }
  settings.dir = dir || "";
  settings.chunk = (await store.get("orders_per_file")) || 70000;
  trusted = (await store.get("trusted_origins")) || [];

  // 恢复历史任务（running 视为中断）
  const saved = (await store.get("tasks")) || [];
  saved.reverse().forEach((s) => {
    if (s.status === "running") { s.status = "error"; s.error = "已中断（应用重启）"; }
    tasks.unshift(s);
    mountTask(s);
  });
  updateEmpty();

  // 进度事件
  await listen("export-progress", (e) => {
    const p = e.payload;
    const t = tasks.find((x) => x.id === p.task_id);
    if (!t || t.status !== "running") return;
    t.orders = p.orders; t.rows = p.rows; t.total = p.total;
    updateProgress(t);
  });

  // 深链：先挂运行期监听，再领取「冷启动时缓冲的深链」（首次点击、客户端未开那条就在这里被取到）
  await listen("deep-link", (e) => handleDeepLink(e.payload));
  await listen("open-settings", () => openSettings());
  try {
    const pending = await invoke("frontend_ready");
    (pending || []).forEach((u) => handleDeepLink(u));
  } catch (_) {}

  // 运行中每秒刷新 ETA（吞吐/剩余时间随时间走）
  setInterval(() => tasks.forEach((t) => t.status === "running" && updateProgress(t)), 1000);
}
init();
