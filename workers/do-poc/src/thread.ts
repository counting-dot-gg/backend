import { DurableObject } from 'cloudflare:workers'
import { coloOf } from './colo.ts'
import { Engine, type Snapshot } from './engine.ts'
import type { Env } from './env.ts'
import { signedHeaders } from './signing.ts'
import { closeBack } from './sockets.ts'

interface Seat {
  uuid: string
}

interface Forward {
  seq: number
  hash: string
  at: number
  decision: unknown
}

interface Ownership {
  thread: string
  epoch: number
  owning: boolean
}

interface Limits {
  burstPerSec: number
  on: boolean
}

interface Handed extends Snapshot {
  hashes?: string[]
}

interface Checkpoint extends Handed {
  seq: number
}

interface PostMessage {
  t: 'post'
  text: string
  h: string
  s: number
  r?: number
  m?: [number, number]
}

export interface PassedPost {
  author: string
  text: string
  hash: string | null
  m?: [number | null, number | null]
}

type Outcome =
  | { t: 'ok' }
  | { t: 'moved' }
  | { t: 'dup' }
  | { t: 'rate_limited' }
  | { t: 'rejected'; reason: string }
  | { t: 'unready' }
  | { t: 'passed' }

const RECENT_HASHES = 512
const RETRY_MS = 1000
const QUIET_MS = 2000
const DEFAULT_LIMITS: Limits = { burstPerSec: 25, on: true }

function asPost(raw: string | ArrayBuffer): PostMessage | null {
  if (typeof raw !== 'string') return null
  try {
    const m = JSON.parse(raw)
    if (m?.t !== 'post' || typeof m.h !== 'string') return null
    if (typeof m.text !== 'string' && Number.isInteger(m.n)) m.text = String(m.n)
    return typeof m.text === 'string' && m.text.trim() ? m : null
  } catch {
    return null
  }
}

function macroIds(m: PostMessage['m'] | PassedPost['m']): { macroPresetId?: number; macroPresetVersionId?: number } {
  const [preset, version] = Array.isArray(m) ? m : []
  return {
    ...(typeof preset === 'number' && Number.isInteger(preset) ? { macroPresetId: preset } : {}),
    ...(typeof version === 'number' && Number.isInteger(version) ? { macroPresetVersionId: version } : {}),
  }
}

export class ThreadDO extends DurableObject<Env> {
  private seq = 0
  private restored = false
  private ownership: Ownership | null = null
  private engine = new Engine()
  private limits: Limits = DEFAULT_LIMITS
  private recent = new Set<string>()
  private bursts = new Map<string, { since: number; n: number }>()
  private unacked: Forward[] = []
  private backlogStored = false
  private dirty = false
  private quiet: ReturnType<typeof setTimeout> | null = null
  private broken = false
  private slowMs = 0
  private cut = false
  private forwardFailed = false
  private retryScheduled = false
  private unreadySince = 0
  private sending: Promise<void> | null = null
  private pulling: Promise<boolean> | null = null
  private fetchingCounters = new Map<string, Promise<boolean>>()
  private lateChain: Promise<unknown> = Promise.resolve()
  private lateFailed = new Set<string>()
  private serial: Promise<unknown> = Promise.resolve()

  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env)
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair('ping', 'pong'))
  }

  async adopt(
    thread: string,
    epoch: number,
    snapshot: Handed,
    limits: Limits | undefined,
  ): Promise<{ ok: boolean; colo: string; reason?: string }> {
    await this.restore()
    if (!(await this.reachesVps())) return { ok: false, colo: await coloOf(this.ctx.storage), reason: 'no_return_path' }
    this.ownership = { thread, epoch, owning: true }
    this.broken = false
    this.slowMs = 0
    this.unreadySince = 0
    this.seq = 0
    this.unacked = []
    this.recent.clear()
    this.learnHashes(snapshot.hashes)
    this.lateFailed.clear()
    this.limits = limits ?? DEFAULT_LIMITS
    const kept = this.engine.loaded ? this.engine.snapshot().counters ?? [] : []
    const ok = this.engine.load({ ...snapshot, counters: kept })
    this.dirty = false
    await this.ctx.storage.put({
      ownership: this.ownership,
      unacked: [],
      broken: false,
      limits: this.limits,
      dirty: false,
      checkpoint: { ...this.engine.snapshot(), seq: 0, hashes: [...this.recent] } satisfies Checkpoint,
    })
    this.backlogStored = false
    this.tellAll({ t: 'owner', owning: true, epoch })
    this.ctx.waitUntil(this.learnSeats())
    return { ok, colo: await coloOf(this.ctx.storage) }
  }

  async meta(epoch: number, meta: unknown): Promise<{ ok: boolean }> {
    await this.restore()
    if (!this.owns(epoch) || !this.engine.loaded) return { ok: false }
    const ok = this.engine.setMeta(meta)
    this.changed()
    return { ok }
  }

  async counter(epoch: number, counter: unknown): Promise<{ ok: boolean }> {
    await this.restore()
    if (!this.owns(epoch)) return { ok: false }
    const ok = this.engine.addCounter(counter)
    this.changed()
    return { ok }
  }

  async status(epoch: number): Promise<{ owning: boolean; lastSeq: number; stuckMs: number }> {
    await this.restore()
    if (this.broken) throw new Error('broken')
    const now = Date.now()
    const forwarding = this.unacked.length > 0 ? now - this.unacked[0].at : 0
    const judging = this.unreadySince ? now - this.unreadySince : 0
    return { owning: this.owns(epoch), lastSeq: this.seq, stuckMs: Math.max(forwarding, judging) }
  }

  async unforwarded(epoch: number): Promise<{ lastSeq: number; posts: Forward[] }> {
    await this.restore()
    if (this.ownership?.epoch !== epoch) return { lastSeq: 0, posts: [] }
    return { lastSeq: this.seq, posts: this.unacked }
  }

  async fault(mode: string, ms: number): Promise<{ mode: string }> {
    await this.restore()
    this.broken = mode === 'broken'
    this.slowMs = mode === 'slow' ? Math.min(Math.max(ms, 0), 60_000) : 0
    this.cut = mode === 'cut'
    await this.ctx.storage.put('broken', this.broken)
    if (this.broken) for (const ws of this.ctx.getWebSockets()) ws.close(1011, 'broken')
    return { mode }
  }

  async release(epoch: number): Promise<{ lastSeq: number }> {
    await this.restore()
    if (this.broken) throw new Error('broken')
    if (this.ownership && this.ownership.epoch === epoch) {
      this.ownership.owning = false
      await this.lag()
      await this.drain()
      await this.ctx.storage.put({ ownership: this.ownership })
      this.tellAll({ t: 'moved', epoch })
    }
    return { lastSeq: this.seq }
  }

  async fetch(request: Request): Promise<Response> {
    await this.restore()
    if (this.broken) return new Response('broken', { status: 503 })
    const uuid = request.headers.get('x-cgg-uuid') ?? 'anonymous'
    const [client, server] = Object.values(new WebSocketPair())
    this.ctx.acceptWebSocket(server)
    server.serializeAttachment({ uuid } satisfies Seat)
    server.send(JSON.stringify({ t: 'hello', owning: this.ownership?.owning ?? false }))
    if (this.ownership?.owning) this.ctx.waitUntil(this.learnCounter(uuid))
    return new Response(null, { status: 101, webSocket: client })
  }

  async webSocketMessage(ws: WebSocket, raw: string | ArrayBuffer): Promise<void> {
    await this.restore()
    if (this.broken) {
      ws.close(1011, 'broken')
      return
    }
    if (raw === 'where') {
      ws.send(JSON.stringify({ t: 'where', colo: await coloOf(this.ctx.storage) }))
      return
    }
    const post = asPost(raw)
    if (!post) return
    const { uuid } = ws.deserializeAttachment() as Seat
    const outcome = await this.inOrder(() => this.take(uuid, post.text, post.h, !!post.r, post.m, true))
    if (outcome.t === 'unready') this.unreadySince ||= Date.now()
    else this.unreadySince = 0
    if (outcome.t !== 'unready') ws.send(JSON.stringify({ ...outcome, h: post.h, s: post.s }))
  }

  async posts(epoch: number, posts: PassedPost[]): Promise<{ rejected: { author: string; hash: string | null; reason: string }[] }> {
    await this.restore()
    const rejected: { author: string; hash: string | null; reason: string }[] = []
    if (!this.owns(epoch)) throw new Error('not owner')
    for (const p of posts) {
      if (typeof p.text !== 'string' || !p.text.trim() || typeof p.author !== 'string') continue
      const hash = typeof p.hash === 'string' && p.hash ? p.hash : `vps-${crypto.randomUUID()}`
      const outcome = await this.inOrder(() => this.take(p.author, p.text, hash, false, p.m, false))
      if (outcome.t === 'unready') throw new Error('not ready')
      if (outcome.t === 'rejected') rejected.push({ author: p.author, hash: p.hash, reason: outcome.reason })
    }
    return { rejected }
  }

  private inOrder<T>(work: () => Promise<T>): Promise<T> {
    const run = this.serial.then(work, work)
    this.serial = run.catch(() => undefined)
    return run
  }

  private async take(
    uuid: string,
    text: string,
    hash: string,
    retry: boolean,
    m: PostMessage['m'] | PassedPost['m'],
    limited: boolean,
  ): Promise<Outcome> {
    if (this.recent.has(hash) && !retry) return { t: 'dup' }
    if (!this.ownership?.owning) return (await this.passLate(uuid, text, hash, m)) ? { t: 'passed' } : { t: 'moved' }
    if (limited && !this.admitted(uuid)) return { t: 'rate_limited' }
    if (!(await this.ready()) || !(await this.learnCounter(uuid))) return { t: 'unready' }
    if (!this.ownership?.owning) return { t: 'moved' }
    await this.markDirty()
    const judged = this.engine.judge({ author: uuid, text, hash, ...macroIds(m) })
    if (judged.error) return { t: 'unready' }
    this.remember(hash)
    if (judged.rejected) return { t: 'rejected', reason: judged.rejected }
    this.seq += 1
    this.unacked.push({ seq: this.seq, hash, at: Date.now(), decision: judged.decision })
    this.tellAll({ t: 'post', ...judged.message })
    if (judged.threadUpdate) this.tellAll({ t: 'thread_update', ...judged.threadUpdate })
    this.changed()
    this.ctx.waitUntil(this.pump())
    return { t: 'ok' }
  }

  async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    closeBack(ws, code)
  }

  async alarm(): Promise<void> {
    this.retryScheduled = false
    await this.restore()
    await this.pump()
  }

  private owns(epoch: number): boolean {
    return !!this.ownership?.owning && this.ownership.epoch === epoch
  }

  private async lag(): Promise<void> {
    if (this.slowMs > 0) await new Promise(r => setTimeout(r, this.slowMs))
  }

  private admitted(uuid: string): boolean {
    if (!this.limits.on) return true
    const now = Date.now()
    const burst = this.bursts.get(uuid)
    if (!burst || now - burst.since >= 1000) {
      this.bursts.set(uuid, { since: now, n: 1 })
      return true
    }
    burst.n += 1
    return burst.n <= this.limits.burstPerSec
  }

  private async restore(): Promise<void> {
    if (this.restored) return
    this.restored = true
    const stored = await this.ctx.storage.get(['ownership', 'broken', 'unacked', 'limits', 'dirty', 'checkpoint'])
    this.ownership = (stored.get('ownership') as Ownership | undefined) ?? null
    this.broken = (stored.get('broken') as boolean | undefined) ?? false
    this.unacked = (stored.get('unacked') as Forward[] | undefined) ?? []
    this.backlogStored = this.unacked.length > 0
    this.limits = (stored.get('limits') as Limits | undefined) ?? DEFAULT_LIMITS
    this.dirty = (stored.get('dirty') as boolean | undefined) ?? false
    const checkpoint = stored.get('checkpoint') as Checkpoint | undefined
    this.learnHashes(checkpoint?.hashes)
    if (checkpoint && !this.dirty) {
      this.engine.load(checkpoint)
      this.seq = Math.max(checkpoint.seq, ...this.unacked.map(f => f.seq))
    }
  }

  private ready(): Promise<boolean> {
    if (this.engine.loaded) return Promise.resolve(true)
    if (!this.pulling) {
      this.pulling = this.pull().finally(() => {
        this.pulling = null
      })
    }
    return this.pulling
  }

  private async pull(): Promise<boolean> {
    if (!this.ownership?.owning) return false
    await this.drain()
    if (this.unacked.length > 0) return false
    const reply = await this.askVps<Handed & { limits?: Limits; lastSeq?: number }>('snapshot', {
      thread: this.ownership.thread,
      epoch: this.ownership.epoch,
    })
    if (reply.status === 409) {
      await this.lose()
      return false
    }
    if (!reply.body || !this.engine.load(reply.body)) return false
    this.learnHashes(reply.body.hashes)
    this.limits = reply.body.limits ?? this.limits
    this.seq = Math.max(this.seq, reply.body.lastSeq ?? 0)
    this.dirty = false
    await this.checkpoint()
    this.ctx.waitUntil(this.learnSeats())
    return true
  }

  private passLate(uuid: string, text: string, hash: string, m: PostMessage['m'] | PassedPost['m']): Promise<boolean> {
    const ownership = this.ownership
    if (!ownership) return Promise.resolve(false)
    const passed = this.lateChain.then(async () => {
      if (this.lateFailed.has(uuid)) return false
      const reply = await this.askVps<{ ok?: boolean }>('late', {
        thread: ownership.thread,
        epoch: ownership.epoch,
        posts: [{ author: uuid, text, hash, m: Array.isArray(m) ? m : null }],
      })
      if (reply.body?.ok !== true) {
        this.lateFailed.add(uuid)
        return false
      }
      this.remember(hash)
      return true
    })
    this.lateChain = passed.catch(() => false)
    return passed
  }

  private learnCounter(uuid: string): Promise<boolean> {
    if (this.engine.knows(uuid)) return Promise.resolve(true)
    let fetching = this.fetchingCounters.get(uuid)
    if (!fetching) {
      fetching = this.askVps<{ counter?: unknown }>('counter', { uuid })
        .then(reply => !!reply.body?.counter && this.engine.addCounter(reply.body.counter))
        .finally(() => this.fetchingCounters.delete(uuid))
      this.fetchingCounters.set(uuid, fetching)
    }
    return fetching
  }

  private async learnSeats(): Promise<void> {
    const seats = new Set(this.ctx.getWebSockets().map(ws => (ws.deserializeAttachment() as Seat | null)?.uuid))
    await Promise.all([...seats].filter((u): u is string => !!u && u !== 'anonymous').map(u => this.learnCounter(u)))
  }

  private async reachesVps(): Promise<boolean> {
    const { status } = await this.askVps('counter', { uuid: 'return-path' })
    return status >= 200 && status < 500
  }

  private async askVps<T>(what: 'counter' | 'snapshot' | 'late', ask: object): Promise<{ status: number; body: T | null }> {
    if (!this.env.INGEST_URL || this.cut) return { status: 0, body: null }
    const body = JSON.stringify(ask)
    try {
      const response = await fetch(new URL(what, this.env.INGEST_URL), {
        method: 'POST',
        headers: await signedHeaders(this.env.DO_SHARED_SECRET, body),
        body,
      })
      return { status: response.status, body: response.ok ? ((await response.json()) as T) : null }
    } catch {
      return { status: 0, body: null }
    }
  }

  private async markDirty(): Promise<void> {
    if (this.dirty) return
    this.dirty = true
    await this.ctx.storage.put('dirty', true)
  }

  private changed(): void {
    if (this.quiet) clearTimeout(this.quiet)
    this.quiet = setTimeout(() => {
      this.quiet = null
      this.ctx.waitUntil(this.settleQuietly())
    }, QUIET_MS)
  }

  private async settleQuietly(): Promise<void> {
    await this.pump()
    if (this.unacked.length === 0 && this.engine.loaded) await this.checkpoint()
  }

  private async checkpoint(): Promise<void> {
    this.dirty = false
    await this.ctx.storage.put({
      checkpoint: { ...this.engine.snapshot(), seq: this.seq, hashes: [...this.recent] } satisfies Checkpoint,
      dirty: false,
    })
  }

  private learnHashes(hashes: unknown): void {
    if (!Array.isArray(hashes)) return
    for (const h of hashes) if (typeof h === 'string' && h) this.remember(h)
  }

  private remember(hash: string): void {
    this.recent.add(hash)
    if (this.recent.size > RECENT_HASHES) this.recent.delete(this.recent.values().next().value as string)
  }

  private tellAll(message: object): void {
    const text = JSON.stringify(message)
    for (const ws of this.ctx.getWebSockets()) {
      try {
        ws.send(text)
      } catch {
        continue
      }
    }
  }

  private async lose(): Promise<void> {
    if (!this.ownership) return
    this.ownership.owning = false
    this.unacked = []
    await this.ctx.storage.put({ ownership: this.ownership, unacked: [] })
    this.backlogStored = false
    this.tellAll({ t: 'moved', epoch: this.ownership.epoch })
  }

  private pump(): Promise<void> {
    if (!this.sending) {
      this.sending = this.sendAll().finally(() => {
        this.sending = null
      })
    }
    return this.sending
  }

  private async drain(): Promise<void> {
    for (let attempt = 0; attempt < 5 && this.unacked.length > 0; attempt++) {
      await this.pump()
      if (this.unacked.length === 0 || this.forwardFailed) return
      await new Promise(r => setTimeout(r, 200))
    }
  }

  private async sendAll(): Promise<void> {
    while (this.unacked.length > 0 && this.ownership && !this.broken) {
      const batch = this.unacked.slice(0, 200)
      const acked = await this.send(batch)
      this.forwardFailed = acked === null
      if (acked === null || batch.every(f => f.seq > acked)) {
        await this.retryLater()
        return
      }
      this.unacked = this.unacked.filter(f => f.seq > acked)
    }
    if (this.backlogStored && this.unacked.length === 0) {
      await this.ctx.storage.put('unacked', [])
      this.backlogStored = false
    }
  }

  private async retryLater(): Promise<void> {
    if (this.retryScheduled) return
    this.retryScheduled = true
    await this.ctx.storage.put('unacked', this.unacked)
    this.backlogStored = true
    await this.ctx.storage.setAlarm(Date.now() + RETRY_MS)
  }

  private async send(batch: Forward[]): Promise<number | null> {
    if (!this.env.INGEST_URL || !this.ownership || this.cut) return null
    await this.lag()
    const body = JSON.stringify({ thread: this.ownership.thread, epoch: this.ownership.epoch, posts: batch })
    try {
      const response = await fetch(this.env.INGEST_URL, {
        method: 'POST',
        headers: await signedHeaders(this.env.DO_SHARED_SECRET, body),
        body,
      })
      if (response.status === 409) {
        await this.lose()
        return Number.MAX_SAFE_INTEGER
      }
      if (!response.ok) return null
      const reply = (await response.json()) as { lastSeq?: number }
      return typeof reply.lastSeq === 'number' ? reply.lastSeq : null
    } catch {
      return null
    }
  }
}
