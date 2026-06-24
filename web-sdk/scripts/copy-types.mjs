// 把手写的类型声明拷到 dist，作为 package.json "types" 指向的产物。
import { copyFileSync, mkdirSync } from 'node:fs'

mkdirSync('dist', { recursive: true })
copyFileSync('src/index.d.ts', 'dist/index.d.ts')
console.log('copied src/index.d.ts -> dist/index.d.ts')
