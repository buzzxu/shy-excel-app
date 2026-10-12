// 界面主题：业务系统经深链 theme 参数（可选）传入 { primary, radius, flat }，让窗口贴近其设计体系。
// 信任边界：深链可由任意网页触发。这里只做白名单 + 格式校验，只认上述三个字段，绝不接受任意 CSS；
// 校验后的值只经 CSSOM 写入 CSS 变量和类名，从不拼进 HTML。
const HEX_RE = /^#([0-9a-f]{3}|[0-9a-f]{6})$/i;

// raw：深链里解码后的 theme 字符串。无效 → null（= 默认主题）
function parseTheme(raw) {
  if (typeof raw !== "string" || !raw || raw.length > 512) return null;
  try { return sanitizeTheme(JSON.parse(raw)); } catch (_) { return null; }
}

// 逐字段校验：无效字段单独丢弃，合法的保留，未知键忽略；一个都不剩 → null
function sanitizeTheme(o) {
  if (typeof o !== "object" || o === null || Array.isArray(o)) return null;
  const th = {};
  if (typeof o.primary === "string" && HEX_RE.test(o.primary)) {
    let h = o.primary.slice(1).toLowerCase();
    if (h.length === 3) h = h.replace(/./g, "$&$&"); // #abc → #aabbcc
    th.primary = "#" + h;
  }
  if (Number.isInteger(o.radius) && o.radius >= 0 && o.radius <= 24) th.radius = o.radius;
  if (o.flat === true) th.flat = true;
  return Object.keys(th).length ? th : null;
}

// 主色上的文字色。hex 为已规范化的 #rrggbb；按 WCAG 相对亮度算与白字的对比度
function onPrimary(hex) {
  const lin = (i) => {
    const c = parseInt(hex.slice(i, i + 2), 16) / 255;
    return c <= 0.03928 ? c / 12.92 : Math.pow((c + 0.055) / 1.055, 2.4);
  };
  const L = 0.2126 * lin(1) + 0.7152 * lin(3) + 0.0722 * lin(5);
  // 阈值取 2:1：主流设计体系即便只有约 2.8:1 也在主色按钮上用白字（如 Element #409eff），只有很浅的主色（如黄色）才改深色字
  return 1.05 / (L + 0.05) < 2 ? "#1f2329" : "#fff";
}

// = CSS color-mix(in srgb, hex p, #fff)：逐通道在 sRGB 值上线性插值。hex 为 #rrggbb，p 为 0~1
function mixWhite(hex, p) {
  let out = "#";
  for (let i = 1; i < 7; i += 2) {
    out += Math.round(parseInt(hex.slice(i, i + 2), 16) * p + 255 * (1 - p)).toString(16).padStart(2, "0");
  }
  return out;
}

// th：sanitizeTheme 的结果或 null（null = 恢复默认翡翠玻璃）
function applyTheme(th) {
  const root = document.documentElement;
  const p = th && th.primary;
  root.classList.toggle("themed", !!p);
  if (p) {
    root.style.setProperty("--theme-primary", p);
    // 按钮底色浅色模式是主色本身，深色模式是 --em（index.html 里的主色 75% + 白，两处比例须一致）。
    // 内联样式优先级高于样式表，所以两套文字色都写成变量，由样式表按配色方案选用
    root.style.setProperty("--on-primary-light", onPrimary(p));
    root.style.setProperty("--on-primary-dark", onPrimary(mixWhite(p, 0.75)));
  } else {
    root.style.removeProperty("--theme-primary");
    root.style.removeProperty("--on-primary-light");
    root.style.removeProperty("--on-primary-dark");
  }
  root.classList.toggle("flat", !!(th && th.flat));
  if (th && th.radius != null) root.style.setProperty("--radius", th.radius + "px");
  else root.style.removeProperty("--radius");
}
