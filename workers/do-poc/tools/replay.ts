import { readFileSync } from 'node:fs'
import { signedHeaders } from '../src/signing.ts'
import { arg, nextMessage, open, summary, tokenFor } from './common.ts'

interface Planned {
  at: number
  author: string
}

const base = arg('base', 'ws://127.0.0.1:8787')
const thread = arg('thread', `replay-${Date.now()}`)
const region = arg('region', arg('hint', 'enam'))
const speed = Number(arg('speed', '1'))
const file = arg('file', '')

function synthetic(): Planned[] {
  const rate = Number(arg('rate', '5'))
  const seconds = Number(arg('seconds', '30'))
  const authors = Number(arg('authors', '4'))
  return Array.from({ length: Math.round(rate * seconds) }, (_, i) => ({
    at: (i * 1000) / rate,
    author: `author-${i % authors}`,
  }))
}

function fromFile(path: string): Planned[] {
  const rows = readFileSync(path, 'utf8')
    .split('\n')
    .filter(line => line.trim())
    .map(line => JSON.parse(line) as { t: number; a: string })
    .sort((x, y) => x.t - y.t)
  const start = rows[0]?.t ?? 0
  return rows.map(r => ({ at: r.t - start, author: r.a }))
}

async function adopt(): Promise<void> {
  const secret = process.env.DO_SHARED_SECRET
  if (!secret) {
    console.error('Set DO_SHARED_SECRET to the Worker\'s value (.dev.vars or .prod.vars).')
    process.exit(2)
  }
  const body = JSON.stringify({ thread, region, epoch: 1, last: 0 })
  const httpBase = base.replace(/^ws/, 'http')
  const response = await fetch(`${httpBase}/admin/adopt`, { method: 'POST', headers: await signedHeaders(secret, body), body })
  if (!response.ok) throw new Error(`adopt failed: ${response.status} ${await response.text()}`)
}

const plan = file ? fromFile(file) : synthetic()
const authors = [...new Set(plan.map(p => p.author))]
await adopt()
const sockets = new Map<string, WebSocket>()
for (const author of authors) {
  const query = `t=${encodeURIComponent(await tokenFor(author))}&region=${region}`
  sockets.set(author, await open(`${base}/thread/${encodeURIComponent(thread)}?${query}`))
}

const first = sockets.get(authors[0])!
first.send('where')
const where = JSON.parse(await nextMessage(first, d => d.startsWith('{"t":"where"'), 15000))
console.log(`thread=${thread} region=${region} colo=${where.colo} posts=${plan.length} authors=${authors.length} speed=${speed}x`)

const rtts: number[] = []
const outcomes = { valid: 0, invalid: 0, dup: 0, moved: 0, lost: 0 }
const waiting = new Map<string, number>()
for (const ws of sockets.values()) {
  ws.addEventListener('message', event => {
    const m = JSON.parse(String(event.data))
    if (typeof m.h !== 'string' || !waiting.has(m.h)) return
    rtts.push(performance.now() - waiting.get(m.h)!)
    waiting.delete(m.h)
    if (m.t === 'ok') outcomes[m.valid ? 'valid' : 'invalid']++
    else if (m.t === 'dup' || m.t === 'moved') outcomes[m.t as 'dup' | 'moved']++
  })
}

const started = performance.now()
let n = where.last
for (const post of plan) {
  const delay = started + post.at / speed - performance.now()
  if (delay > 0) await new Promise(r => setTimeout(r, delay))
  n += 1
  const hash = `${thread}:${n}`
  waiting.set(hash, performance.now())
  sockets.get(post.author)!.send(JSON.stringify({ t: 'post', text: String(n), h: hash, s: Date.now() }))
}

await new Promise(r => setTimeout(r, 3000))
outcomes.lost = waiting.size
for (const ws of sockets.values()) ws.close(1000)
console.log(Object.entries(outcomes).map(([k, v]) => `${k}=${v}`).join(' '))
console.log(`round trip ${summary(rtts)}`)
