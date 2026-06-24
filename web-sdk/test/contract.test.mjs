// 协议契约测试 —— CI 发布前的闸门，纯 node:test，无外部依赖。
// 校验深链/流地址拼装、双传输解包一致、错误路径、fetch URL 构造。
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { buildDeepLink, createJob, EXPORT_SCHEME } from '../src/index.js'

// 默认 auth 路径会读 localStorage.token；Node 下打桩。
globalThis.localStorage = { token: 'LS_TOKEN' }

// 独立复刻原始公式作为 oracle（host 绝对，window 不会被触碰）。
function oracle({ host, streamPath, jobId, token, auth }) {
  const authToken = auth || (globalThis.localStorage && globalThis.localStorage.token) || ''
  const streamUrl =
    host.replace(/\/+$/, '') + streamPath +
    `?job=${encodeURIComponent(jobId)}&token=${encodeURIComponent(token)}` +
    (authToken ? `&Authorization=${encodeURIComponent(authToken)}` : '')
  const deepLink =
    `shyexport://export?job=${encodeURIComponent(jobId)}` +
    `&token=${encodeURIComponent(token)}&url=${encodeURIComponent(streamUrl)}`
  return { streamUrl, deepLink }
}

test('buildDeepLink byte-identical to original formula (incl. special chars)', () => {
  for (const c of [
    { host: 'https://boss.xw-jd.com/api', streamPath: '/shy/export/stream', jobId: 'J-1', token: 'T1', auth: 'AUTH1' },
    { host: 'https://boss.xw-jd.com/api/', streamPath: '/shy/export/stream', jobId: 'a/b?c&d', token: 't=x y', auth: 'au th&v' },
    { host: 'https://boss.xw-jd.com/api', streamPath: '/shy/export/stream', jobId: 'J2', token: 'T2', auth: '' },
  ]) {
    const got = buildDeepLink(c)
    const exp = oracle(c)
    assert.equal(got.streamUrl, exp.streamUrl)
    assert.equal(got.deepLink, exp.deepLink)
  }
})

test('deepLink matches client contract + preserves host prefix', () => {
  const { deepLink, streamUrl } = buildDeepLink({ host: 'https://x.com/api', streamPath: '/shy/export/stream', jobId: 'JID', token: 'TK', auth: 'A' })
  assert.ok(deepLink.startsWith(EXPORT_SCHEME + '://export?job=JID&token=TK&url='))
  assert.equal(decodeURIComponent(deepLink.split('&url=')[1]), streamUrl)
  assert.ok(streamUrl.includes('/api/shy/export/stream'))
})

test('dual-transport unwrap consistency across 3 response forms', async () => {
  const D = { jobId: 'JOB9', token: 'TOK9', streamPath: '/shy/export/stream' }
  const forms = [D, { code: 200, data: D }, { data: { code: 200, data: D } }]
  for (const body of forms) {
    const viaRequest = await createJob({ jobApi: '/x', request: { get: async () => body, post: async () => body } })
    const viaFetch = await createJob({ jobApi: '/x', baseURL: 'https://x.com', auth: 'A', fetchImpl: async () => ({ ok: true, status: 200, json: async () => body }) })
    assert.deepEqual(viaRequest, D)
    assert.deepEqual(viaFetch, D)
  }
})

test('default streamPath applied when absent', async () => {
  const r = await createJob({ jobApi: '/x', request: { get: async () => ({ jobId: 'J', token: 'T' }) } })
  assert.equal(r.streamPath, '/shy/export/stream')
})

test('error paths reject', async () => {
  await assert.rejects(createJob({ jobApi: '/x', request: { get: async () => ({ token: 'T' }) } }), /jobId/)
  await assert.rejects(createJob({ jobApi: '/x', fetchImpl: async () => ({ ok: false, status: 401 }) }), /HTTP 401/)
})

test('fetch GET builds URL with query + Authorization header', async () => {
  let seenUrl, seenInit
  await createJob({
    jobApi: '/mall/order/export/client', baseURL: 'https://api.example.com', auth: 'BEARER',
    params: { a: 1, b: 'x y', skip: undefined, nil: null },
    fetchImpl: async (url, init) => { seenUrl = url; seenInit = init; return { ok: true, json: async () => ({ jobId: 'J', token: 'T' }) } },
  })
  assert.equal(seenUrl, 'https://api.example.com/mall/order/export/client?a=1&b=x%20y')
  assert.equal(seenInit.headers.Authorization, 'BEARER')
})
