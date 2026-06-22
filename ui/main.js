// 前端：监听深链事件 → 解析 job/token/url → 调 run_export → 渲染任务。
// 用 withGlobalTauri：window.__TAURI__.{core.invoke, event.listen, store, dialog}
const { invoke } = window.__TAURI__.core;
const { listen } = window.__TAURI__.event;
const dialog = window.__TAURI__.dialog;
const { load } = window.__TAURI__.store;

const tasksEl = document.getElementById("tasks");
const dirEl = document.getElementById("dir");
const chunkEl = document.getElementById("chunk");
const memhint = document.getElementById("memhint");
let store;

// 分块粒度 → 预估峰值内存（M2 实测曲线）
function estMem(chunk) {
  if (chunk >= 100000) return "≈2.5GB+/块";
  if (chunk >= 70000) return "≈1.9GB/块";
  if (chunk >= 50000) return "≈1.9GB/块";
  if (chunk >= 20000) return "≈1.5GB/块";
  if (chunk >= 10000) return "≈1.2GB/块";
  return "≈0.7GB/块";
}
function refreshHint() { memhint.textContent = "预估峰值内存 " + estMem(+chunkEl.value || 70000); }

async function initSettings() {
  store = await load("settings.json", { autoSave: true });
  const dir = (await store.get("out_dir")) || "";
  const chunk = (await store.get("orders_per_file")) || 70000;
  dirEl.value = dir;
  chunkEl.value = chunk;
  refreshHint();
}
chunkEl.addEventListener("change", async () => {
  refreshHint();
  if (store) await store.set("orders_per_file", +chunkEl.value || 70000);
});
document.getElementById("pick").addEventListener("click", async () => {
  const picked = await dialog.open({ directory: true, multiple: false });
  if (picked) { dirEl.value = picked; if (store) await store.set("out_dir", picked); }
});

let seq = 0;
function addTask(label) {
  const id = "t" + ++seq;
  const empty = tasksEl.querySelector(".empty");
  if (empty) empty.remove();
  const el = document.createElement("div");
  el.className = "task";
  el.id = id;
  el.innerHTML = `<div><b>${label}</b> <span class="st-running" data-st>进行中…</span></div><div class="meta" data-meta></div>`;
  tasksEl.prepend(el);
  return id;
}
function setTask(id, cls, st, meta) {
  const el = document.getElementById(id);
  if (!el) return;
  const s = el.querySelector("[data-st]");
  s.className = cls; s.textContent = st;
  if (meta) el.querySelector("[data-meta]").textContent = meta;
}

function parseDeepLink(url) {
  // xwjdexport://export?job=..&token=..&host=https://api.boss.xw-jd.com  (或 &url=完整流地址)
  const q = new URLSearchParams(url.split("?")[1] || "");
  const job = q.get("job"), token = q.get("token");
  let streamUrl = q.get("url");
  if (!streamUrl) {
    const host = (q.get("host") || "").replace(/\/$/, "");
    streamUrl = `${host}/mall/order/export/stream?job=${encodeURIComponent(job)}&token=${encodeURIComponent(token)}`;
  }
  return { job, streamUrl };
}

async function handleDeepLink(url) {
  if (!dirEl.value) { addTask("导出"); alert("请先在上方选择导出目录"); return; }
  const { job, streamUrl } = parseDeepLink(url);
  const id = addTask("导出任务 " + (job ? job.slice(0, 8) : ""));
  try {
    const r = await invoke("run_export", {
      streamUrl,
      outDir: dirEl.value,
      baseName: "导出订单_" + new Date().toISOString().slice(0, 10),
      ordersPerFile: +chunkEl.value || 70000,
    });
    setTask(id, "st-done", "完成", `${r.files} 个文件 · ${r.orders} 订单 · ${r.rows} 行`);
  } catch (e) {
    setTask(id, "st-error", "失败", String(e));
  }
}

listen("deep-link", (e) => handleDeepLink(e.payload));
initSettings();
