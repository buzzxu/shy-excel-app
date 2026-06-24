import { defineConfig } from 'tsup'

// 源码是零依赖的单文件 ESM；产出 ESM + CJS 双格式，类型用手写的 src/index.d.ts（构建后拷贝）。
export default defineConfig({
  entry: { index: 'src/index.js' },
  format: ['esm', 'cjs'],
  target: 'es2018', // 内部现代 Chrome 足够；如需更老浏览器，消费方把本包加入 transpileDependencies
  outExtension({ format }) {
    return { js: format === 'cjs' ? '.cjs' : '.js' }
  },
  clean: true,
  dts: false, // 用手写 d.ts，见 scripts/copy-types.mjs
  sourcemap: false,
  minify: false,
})
