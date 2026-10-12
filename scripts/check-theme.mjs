// ui/theme.js 主题校验自检（无依赖）：node scripts/check-theme.mjs
import { readFileSync } from "node:fs";
import vm from "node:vm";
import assert from "node:assert/strict";

const ctx = vm.createContext({});
vm.runInContext(readFileSync(new URL("../ui/theme.js", import.meta.url), "utf8"), ctx);
const { parseTheme, sanitizeTheme, onPrimary, mixWhite } = ctx;

// vm 里创建的对象原型属于另一个 realm，deepEqual 会比原型；复制到本 realm 再比
const own = (r) => (r ? { ...r } : r);
const p = (o) => own(parseTheme(JSON.stringify(o)));

// 合法完整主题；大写规范化为小写
assert.deepEqual(p({ primary: "#2D8CF0", radius: 4, flat: true }), { primary: "#2d8cf0", radius: 4, flat: true });
// 3 位色展开
assert.deepEqual(p({ primary: "#ABC" }), { primary: "#aabbcc" });
// 非法色值丢弃
for (const bad of ["red", "#12345", "#gggggg", "url(x)", "#2d8cf0;x", " #2d8cf0", 123]) {
  assert.deepEqual(p({ primary: bad, radius: 4 }), { radius: 4 }, String(bad));
  assert.equal(parseTheme(JSON.stringify({ primary: bad })), null, String(bad));
}
// 圆角：0 与 24 是边界内，其余丢弃
assert.deepEqual(p({ radius: 0 }), { radius: 0 });
assert.deepEqual(p({ radius: 24 }), { radius: 24 });
for (const bad of [25, -1, 4.5, "4", null, true]) {
  assert.deepEqual(p({ radius: bad, flat: true }), { flat: true }, String(bad));
}
// flat 只认 true
for (const bad of ["true", 1, false]) assert.equal(parseTheme(JSON.stringify({ flat: bad })), null, String(bad));
// 未知键忽略
assert.deepEqual(p({ primary: "#fff", css: "body{display:none}", foo: 1 }), { primary: "#ffffff" });
// 非对象 / 空对象 JSON → null
for (const raw of ["[]", "1", "null", '"#2d8cf0"', "true", "{}"]) assert.equal(parseTheme(raw), null, raw);
// 非法 JSON → null
for (const raw of ["{", "{primary:'#fff'}", "#2d8cf0"]) assert.equal(parseTheme(raw), null, raw);
// 非字符串 / 空串 → null
for (const raw of [null, undefined, "", 1, {}]) assert.equal(parseTheme(raw), null, String(raw));
// 长度上限 512
const ofLen = (n) => {
  const base = JSON.stringify({ primary: "#2d8cf0", pad: "" }).length;
  const s = JSON.stringify({ primary: "#2d8cf0", pad: "x".repeat(n - base) });
  assert.equal(s.length, n);
  return s;
};
assert.deepEqual(own(parseTheme(ofLen(512))), { primary: "#2d8cf0" });
assert.equal(parseTheme(ofLen(513)), null);
// 全部无效 → null；部分有效保留有效字段
assert.equal(parseTheme(JSON.stringify({ primary: "red", radius: 99, flat: "yes" })), null);
assert.deepEqual(p({ primary: "red", radius: 8, flat: "yes" }), { radius: 8 });
assert.deepEqual(p({ primary: "#10b981", radius: 99, flat: true }), { primary: "#10b981", flat: true });
// sanitizeTheme 直接调用（store 读回的值）
for (const v of [null, undefined, [], "x", 1, true]) assert.equal(sanitizeTheme(v), null, String(v));
assert.deepEqual(own(sanitizeTheme({ primary: "#FFF", radius: 2 })), { primary: "#ffffff", radius: 2 });

// 主色上的文字色
assert.equal(onPrimary("#10b981"), "#fff");
assert.equal(onPrimary("#409eff"), "#fff");
assert.equal(onPrimary("#2d8cf0"), "#fff");
assert.equal(onPrimary("#fadb14"), "#1f2329");
assert.equal(onPrimary("#ffffff"), "#1f2329");

// 深色模式按钮底 = color-mix(in srgb, 主色 75%, #fff)：逐通道 c*.75 + 255*.25，四舍五入
assert.equal(mixWhite("#000000", 0.75), "#404040"); // 63.75 → 64
assert.equal(mixWhite("#ffffff", 0.75), "#ffffff");
assert.equal(mixWhite("#2d8cf0", 0.75), "#62a9f4"); // 97.5/168.75/243.75 → 98/169/244
assert.equal(mixWhite("#f5a623", 0.75), "#f8bc5a");
// 橙色：浅色模式主色底上白字，深色模式变浅后改深色字
assert.equal(onPrimary("#f5a623"), "#fff");
assert.equal(onPrimary(mixWhite("#f5a623", 0.75)), "#1f2329");
assert.equal(onPrimary(mixWhite("#2d8cf0", 0.75)), "#fff");

console.log("check-theme: ok");
