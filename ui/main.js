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
    (t.status !== "running" ? `<button class="btn ghost icon del" data-del title="删除"><svg class="i" viewBox="0 0 24 24"><path d="M6 6l12 12M18 6L6 18"/></svg></button>` : "") +
    `</div>`;

  if (t.status === "running") {
    const hasTotal = t.total > 0;
    const p = hasTotal ? Math.min(99, Math.round((t.orders / t.total) * 100)) : 0;
    return (
      head(`<span class="badge run">进行中</span>`) +
      `<div class="prog">
         <div class="bar ${hasTotal ? "" : "indet"}"><i data-bar style="width:${p}%"></i></div>
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
         <div class="stat"><div class="v">${fmtNum(t.orders)}</div><div class="k">订单（单）</div></div>
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
  const path = t.el.querySelector("[data-path]");
  if (path) path.onclick = () => invoke("reveal_path", { path: t.path, isDir: t.isDir }).catch(() => {});
}

function mountTask(t) {
  const el = document.createElement("div");
  el.className = "card";
  el.innerHTML = cardInner(t);
  t.el = el;
  tasksEl.prepend(el);
  bindCard(t);
  // 淘汰最旧
  while (tasks.length > MAX_TASKS) {
    const old = tasks.pop();
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

// ---------- 导出流程 ----------
let seq = 0;
function createTask(job) {
  const t = { id: "t" + Date.now() + "_" + ++seq, job, status: "running", orders: 0, rows: 0, total: 0, startedAt: Date.now() };
  tasks.unshift(t);
  mountTask(t);
  return t;
}

function parseDeepLink(url) {
  // swiftexport://export?job=..&token=..&url=完整流地址（含鉴权参数，仅内部使用，绝不展示）
  const q = new URLSearchParams(url.split("?")[1] || "");
  const job = q.get("job");
  let streamUrl = q.get("url");
  if (!streamUrl) {
    const host = (q.get("host") || "").replace(/\/$/, "");
    const token = q.get("token");
    streamUrl = `${host}/shy/export/stream?job=${encodeURIComponent(job)}&token=${encodeURIComponent(token)}`;
  }
  return { job, streamUrl };
}

async function handleDeepLink(url) {
  const { job, streamUrl } = parseDeepLink(url);
  if (!streamUrl) return;
  // 去重：同一 job 正在进行中则忽略（防同条深链既被缓冲又被事件重复投递 / 用户重复点击）
  if (job && tasks.some((t) => t.job === job && t.status === "running")) return;
  if (!settings.dir) {
    openSettings();
    return;
  }
  const t = createTask(job);
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
  }
  refreshCard(t);
  saveTasks();
}

// ---------- 设置 ----------
const overlay = document.getElementById("settings");
const dirEl = document.getElementById("dir");
const chunkEl = document.getElementById("chunk");
const memhint = document.getElementById("memhint");

function openSettings() {
  dirEl.value = settings.dir;
  chunkEl.value = settings.chunk;
  memhint.textContent = "预估峰值内存 " + estMem(settings.chunk);
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

  // 下载目录：无配置则取系统默认下载目录
  let dir = await store.get("download_dir");
  if (!dir) {
    try { dir = await invoke("default_download_dir"); } catch (_) { dir = ""; }
    if (dir) await store.set("download_dir", dir);
  }
  settings.dir = dir || "";
  settings.chunk = (await store.get("orders_per_file")) || 70000;

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
