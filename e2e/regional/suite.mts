import { createHmac } from 'node:crypto'
import { readFileSync } from 'node:fs'
import net from 'node:net'
import mysql from 'mysql2/promise'
import { io, type Socket } from 'socket.io-client'
import { sign } from '../../workers/do-poc/src/token.ts'

interface Config {
  socketUrl: string
  workerUrl: string
  control: string
  origin: string
  nestPort: number
  sharedSecret: string
  tokenSecret: string
  mysql: { host: string; port: number; user: string; password: string; database: string }
  threads: { main: string; noDoubles: string }
  accounts: { uuid: string; cookie: string }[]
  moderator: number
  clientLib: string
}

const config = JSON.parse(readFileSync(process.env.REGIONAL_CONFIG ?? '', 'utf8')) as Config
const SOCKET_URL = config.socketUrl
const ORIGIN = config.origin
const WORKER = config.workerUrl
const NEST_PORT = config.nestPort
const SHARED = config.sharedSecret
const TOKENS = config.tokenSecret
const LIB = config.clientLib

type Region = 'enam' | 'wnam' | 'weur' | 'eeur' | 'apac' | 'apac-ne' | 'apac-se' | 'oc'
type Profile = Partial<Record<Region | 'vps', number>>

interface Owner {
  thread_name: string
  movable: boolean
  region: Region | null
  epoch: number
  moving: boolean
  worker: string | null
  cooldownUntil: number
  regionsOpen?: boolean
  auto?: boolean
  pausedUntil?: number | null
  pending?: { target: string; inMs: number } | null
}

interface Lib {
  attachSocket(s: unknown): void
  configureRegional(o: object): void
  sendPost(s: unknown, p: object): void
  setOwner(o: Owner): void
  confirmPost(h?: string | null): void
  postQueued(h?: string | null): void
  refusePost(t: string, h: string | undefined, by: null): void
  pendingCount(): number
  heldCount(): number
  ownerOf(t: string): Owner | null
  stalePending(t: string, ms: number): { h: string; text: string }[]
  reconcile(t: string, known: string[], connectedAt: number): void
}

const textOf = new Map<string, string>()
const authorOf = new Map<string, string>()
const startIds = new Map<string, number>()

const db = await mysql.createConnection({ ...config.mysql, supportBigNumbers: true })

const t0 = Date.now()
const log = (...p: unknown[]) => console.log(`${((Date.now() - t0) / 1000).toFixed(1).padStart(6)}s`, ...p)
const sleep = (ms: number) => new Promise(r => setTimeout(r, ms))
const traced: string[] = []
const trace = (...p: unknown[]) => {
  const line = `T ${Date.now() - t0} ${p.join(' ')}`
  if (process.env.TRACE) console.log(line)
  traced.push(line)
  if (traced.length > 5000) traced.shift()
}

const RealWS = globalThis.WebSocket
const lagByToken = new Map<string, Profile>()
const wsByToken = new Map<string, Set<LaggyWS>>()

class LaggyWS {
  static CONNECTING = 0
  static OPEN = 1
  static CLOSING = 2
  static CLOSED = 3
  private ws: WebSocket
  private lag: number
  private token: string
  private region: string
  constructor(url: string) {
    const u = new URL(url)
    this.token = u.searchParams.get('t') ?? ''
    const region = (u.searchParams.get('region') ?? '') as Region
    this.region = region
    this.lag = lagByToken.get(this.token)?.[region] ?? 0
    this.ws = new RealWS(url)
    if (!wsByToken.has(this.token)) wsByToken.set(this.token, new Set())
    wsByToken.get(this.token)!.add(this)
    this.ws.addEventListener('close', () => wsByToken.get(this.token)?.delete(this))
    if (process.env.DEBUG_WS) this.ws.addEventListener('message', e => String(e.data) !== 'pong' && console.log('  <<', String(e.data).slice(0, 240)))
  }
  get readyState() {
    return this.ws.readyState
  }
  send(m: string) {
    if (process.env.DEBUG_WS && m !== 'ping') console.log('  >>', this.region, m.slice(0, 120))
    if (m !== 'ping') trace('ws>', this.region, m.match(/"text":"([^"]*)"/)?.[1] ?? m.slice(0, 40))
    if (!this.lag) return this.ws.send(m)
    setTimeout(() => this.ws.readyState === 1 && this.ws.send(m), this.lag)
  }
  close(code?: number, reason?: string) {
    this.ws.close(code, reason)
  }
  addEventListener(type: string, fn: (e: Event) => void, opts?: AddEventListenerOptions) {
    if (type === 'message' && this.lag) this.ws.addEventListener(type, e => setTimeout(() => fn(e), this.lag), opts)
    else this.ws.addEventListener(type, fn, opts)
  }
  removeEventListener(type: string, fn: (e: Event) => void) {
    this.ws.removeEventListener(type, fn)
  }
}
;(globalThis as { WebSocket: unknown }).WebSocket = LaggyWS

let instances = 0

class User {
  name: string
  uuid = ''
  lib!: Lib
  socket!: Socket
  profile: Profile
  watched = new Set<string>()
  rejected: { hash: string; reason: string }[] = []
  refusals: string[] = []
  confirmedAt = new Map<string, number>()
  sentAt = new Map<string, number>()
  tokens: string[] = []
  private shortTokens: boolean
  sender: { emit: (ev: string, d: unknown) => void }

  constructor(name: string, profile: Profile = {}, shortTokens = false) {
    this.name = name
    this.profile = profile
    this.shortTokens = shortTokens
    this.sender = {
      emit: (ev, d) => {
        if (ev === 'post') trace('vps>', this.name, (d as { text?: string }).text)
        this.later(() => this.socket.emit(ev, d))
      },
    }
  }

  private later(fn: () => void) {
    const ms = this.profile.vps ?? 0
    if (ms) setTimeout(fn, ms)
    else fn()
  }

  async start(account: number): Promise<User> {
    const { cookie, uuid } = config.accounts[account]
    this.uuid = uuid
    this.lib = (await import(`${LIB}?u=${++instances}`)) as Lib
    const headers = { Cookie: cookie, Origin: ORIGIN }
    this.lib.configureRegional({
      tokenSource: async () => {
        const exp = Date.now() + (this.shortTokens ? 20_000 : 30 * 60_000)
        const fresh = { token: await sign({ uuid, exp }, TOKENS), exp }
        lagByToken.set(fresh.token, this.profile)
        this.tokens.push(fresh.token)
        return fresh
      },
      onUnreachable: (thread: string, region: string) =>
        this.sender.emit('thread_region_unreachable', { thread_name: thread, region }),
    })
    this.socket = io(SOCKET_URL, { transports: ['websocket'], extraHeaders: headers, reconnection: true, reconnectionDelay: 300 })
    this.lib.attachSocket(this.socket)
    const internal = this.socket as unknown as { emitEvent(args: unknown[]): void }
    const arrive = internal.emitEvent.bind(this.socket)
    internal.emitEvent = (args: unknown[]) => this.later(() => arrive(args))
    let connectedAt = Date.now()
    const checkStatus = () => {
      if (!this.socket.connected) return
      for (const t of this.watched) {
        const posts = this.lib.stalePending(t, 4000)
        if (posts.length > 0) this.sender.emit('post_status', { thread_name: t, posts })
      }
    }
    setInterval(checkStatus, 3000).unref()
    this.socket.on('connect', () => {
      connectedAt = Date.now()
      this.watched.forEach(t => this.socket.emit('watch', t))
      checkStatus()
    })
    this.socket.on('post_status', (m: { thread_name: string; known?: string[] }) => {
      trace('status', this.name, (m.known ?? []).length, 'known; pending', this.lib.pendingCount(), 'held', this.lib.heldCount())
      this.lib.reconcile(m.thread_name, m.known ?? [], connectedAt)
    })
    this.socket.on('thread_owner', (o: Owner) => {
      trace('owner', this.name, o.region, o.moving ? 'moving' : 'settled')
      this.lib.setOwner(o)
    })
    this.socket.on('post', (m: { post?: { post_hash?: string } }) => {
      const h = m?.post?.post_hash
      if (h && this.sentAt.has(h) && !this.confirmedAt.has(h)) this.confirmedAt.set(h, Date.now())
      this.lib.confirmPost(h)
    })
    this.socket.on('post_rejected', (m: { post_hash: string; reason: string }) => {
      this.rejected.push({ hash: m.post_hash, reason: m.reason })
      this.lib.confirmPost(m.post_hash)
    })
    this.socket.on('post_queued', (m: { post_hash?: string }) => this.lib.postQueued(m?.post_hash))
    this.socket.on('post_moved', (m: { thread_name: string; post_hash?: string }) =>
      this.lib.refusePost(m.thread_name, m.post_hash, null),
    )
    this.socket.on('thread_region_refused', (m: { reason: string }) => this.refusals.push(m.reason))
    await new Promise<void>((res, rej) => {
      this.socket.once('connect', () => res())
      this.socket.once('connect_error', rej)
    })
    return this
  }

  watch(thread: string) {
    this.watched.clear()
    this.watched.add(thread)
    this.socket.emit('watch', thread)
  }

  post(thread: string, text: string, hash: string, extra: object = {}) {
    trace('typed', this.name, text)
    this.sentAt.set(hash, Date.now())
    textOf.set(hash, text.trim())
    authorOf.set(hash, this.uuid)
    this.lib.sendPost(this.sender, { thread_name: thread, text, post_hash: hash, ...extra })
  }

  dropConnections() {
    this.socket.io.engine.close()
    for (const t of this.tokens) for (const ws of wsByToken.get(t) ?? []) ws.close(4000, 'dropped')
  }

  close() {
    this.socket.close()
  }
}

interface Seen {
  hash: string
  valid: boolean
  raw: string
  text: string
  uuid: string
  at: number
}

const seen = new Map<string, Seen[]>()
const owners = new Map<string, Owner>()
const reactionUpdates: { post_uuid: string; reactions: object }[] = []
const commentDeletes: { uuid: string; rawText: string }[] = []
const viewers = new Map<string, Socket>()
const moves = new Map<string, { start: number; end?: number }[]>()
const BEFORE_SWITCH_MS = 500
const AFTER_SWITCH_MS = 1000

function swapsOutsideSwitches(thread: string, rows: { hash: string }[], sendOrder: string[], users: User[]) {
  const index = new Map(sendOrder.map((h, i) => [h, i]))
  const sentAt = (h: string) => users.map(u => u.sentAt.get(h)).find(t => t !== undefined) ?? NaN
  const windows = (moves.get(thread) ?? []).map(m => [m.start - BEFORE_SWITCH_MS, (m.end ?? Date.now()) + AFTER_SWITCH_MS])
  const inside = (t: number) => windows.some(([a, b]) => t >= a && t <= b)
  const swaps: { inside: boolean }[] = []
  for (let i = 1; i < rows.length; i++) {
    if ((index.get(rows[i].hash) ?? 0) >= (index.get(rows[i - 1].hash) ?? 0)) continue
    swaps.push({ inside: inside(sentAt(rows[i].hash)) && inside(sentAt(rows[i - 1].hash)) })
  }
  return { swaps: swaps.length, outside: swaps.filter(x => !x.inside).length, switches: windows.length }
}

async function blindCrowd(name: string, route: readonly (Region | 'vps')[]): Promise<Result> {
  const crowd = [await user('b1', 0), await user('b2', 1), await user('b3', 2)]
  for (const u of crowd) u.watch(T)
  await resync(T)
  moves.set(T, [])
  const details: string[] = []
  const hashes: string[] = []
  let stop = false
  const posting = (async () => {
    for (let i = 0; !stop; i++) {
      const h = newHash()
      hashes.push(h)
      crowd[i % crowd.length].post(T, nextCount(T), h)
      await sleep(120)
    }
  })()
  for (const where of route) {
    await sleep(2500)
    details.push(`move to ${where}: ${await moveTo(crowd[0], T, where)} ms`)
  }
  await sleep(route.length ? 2000 : 15_000)
  stop = true
  await posting
  const settled = await settle(crowd, 30_000)
  const r = await audit(T, hashes, false)
  const check = swapsOutsideSwitches(T, r.rows, hashes, crowd)
  details.push(`settled ${settled}; ${check.switches} switches; ${check.swaps} swaps, ${check.outside} of them away from a switch`)
  details.push(describe(r))
  const whole = r.missing.length === 0 && r.duplicated.length === 0
  return { name, pass: whole && settled && check.outside === 0 && (route.length > 0 || check.swaps === 0), details }
}

async function watchThread(thread: string) {
  if (viewers.has(thread)) return
  const v = io(SOCKET_URL, { transports: ['websocket'], extraHeaders: { Origin: ORIGIN }, reconnection: true, reconnectionDelay: 300 })
  viewers.set(thread, v)
  seen.set(thread, [])
  v.on('connect', () => v.emit('watch', thread))
  v.on('post', (m: { post?: { post_hash?: string; isValidCount?: boolean; rawCount?: string; rawText?: string; uuid?: string } }) => {
    const p = m?.post
    if (!p?.post_hash) return
    seen.get(thread)!.push({ hash: p.post_hash, valid: !!p.isValidCount, raw: p.rawCount ?? '', text: p.rawText ?? '', uuid: p.uuid ?? '', at: Date.now() })
  })
  v.on('thread_owner', (o: Owner) => {
    if (o.thread_name !== thread) return
    owners.set(thread, o)
    const log = moves.get(thread) ?? []
    moves.set(thread, log)
    const open = log.at(-1)?.end === undefined ? log.at(-1) : undefined
    if (o.moving && !open) log.push({ start: Date.now() })
    if (!o.moving && open) open.end = Date.now()
  })
  v.on('updateReaction', (m: { post_uuid: string; reactions: object }) => reactionUpdates.push(m))
  v.on('deleteComment', (m: { uuid: string; rawText: string }) => commentDeletes.push(m))
  await new Promise(r => v.once('connect', r))
  await sleep(300)
}

async function lastValid(thread: string): Promise<bigint> {
  const [[row]] = (await db.query(
    'SELECT p.rawCount FROM post p JOIN thread t ON t.id = p.threadId WHERE t.name = ? AND p.isValidCount = 1 ORDER BY p.id DESC LIMIT 1',
    [thread],
  )) as [{ rawCount: string }[]]
  return BigInt(row?.rawCount ?? '0')
}

const next = new Map<string, bigint>()
async function resync(thread: string) {
  next.set(thread, await lastValid(thread))
}
function nextCount(thread: string): string {
  const n = (next.get(thread) ?? 0n) + 1n
  next.set(thread, n)
  return String(n)
}

let hashSeq = 0
const runId = Date.now().toString(36)
const newHash = () => `rs-${runId}-${++hashSeq}`

function owner(thread: string) {
  return owners.get(thread)
}

async function waitOwner(thread: string, ok: (o: Owner) => boolean, ms = 30_000): Promise<Owner | null> {
  const end = Date.now() + ms
  while (Date.now() < end) {
    const o = owner(thread)
    if (o && ok(o)) return o
    await sleep(50)
  }
  return null
}

async function moveTo(user: User, thread: string, region: Region | 'vps', ms = 30_000): Promise<number | null> {
  const want = region === 'vps' ? null : region
  if (owner(thread) && owner(thread)!.region === want && !owner(thread)!.moving) return 0
  const started = Date.now()
  user.sender.emit('thread_region', { thread_name: thread, region })
  const o = await waitOwner(thread, x => !x.moving && x.region === want, ms)
  return o ? Date.now() - started : null
}

async function settle(users: User[], ms = 60_000): Promise<boolean> {
  const end = Date.now() + ms
  while (Date.now() < end) {
    if (users.every(u => u.lib.pendingCount() === 0 && u.lib.heldCount() === 0)) return true
    await sleep(200)
  }
  return false
}

async function audit(thread: string, hashes: string[], expectValid = true) {
  await sleep(2000)
  const [rows] = (await db.query(
    'SELECT id, rawText, rawCount, isValidCount, authorUUID FROM post WHERE threadId = (SELECT id FROM thread WHERE name = ?) AND id > ? ORDER BY id',
    [thread, startIds.get(thread) ?? 0],
  )) as [{ id: number; rawText: string; rawCount: string | null; isValidCount: number; authorUUID: string }[]]
  const waiting = new Map<string, string[]>()
  for (const h of hashes) {
    const key = `${authorOf.get(h)}|${textOf.get(h)}`
    if (!waiting.has(key)) waiting.set(key, [])
    waiting.get(key)!.push(h)
  }
  const matched = new Set<string>()
  const mine: { hash: string; valid: boolean; raw: string; text: string }[] = []
  for (const r of rows) {
    const queue = waiting.get(`${r.authorUUID}|${r.rawText}`)
    if (!queue) continue
    const hash = queue.find(h => !matched.has(h)) ?? queue[0]
    matched.add(hash)
    mine.push({ hash, valid: !!r.isValidCount, raw: r.rawCount ?? '', text: r.rawText })
  }
  const counts = new Map<string, number>()
  for (const r of mine) counts.set(r.hash, (counts.get(r.hash) ?? 0) + 1)
  const missing = hashes.filter(h => !counts.has(h))
  const duplicated = [...counts].filter(([, c]) => c > 1).map(([h]) => h)
  const order = new Map(hashes.map((h, i) => [h, i]))
  let outOfOrder = 0
  for (let i = 1; i < mine.length; i++) if ((order.get(mine[i].hash) ?? 0) < (order.get(mine[i - 1].hash) ?? 0)) outOfOrder++
  const invalid = expectValid ? mine.filter(r => !r.valid).map(r => r.raw || r.text) : []
  return { sent: hashes.length, seen: counts.size, missing, duplicated, outOfOrder, invalid, rows: mine }
}

type Audit = Awaited<ReturnType<typeof audit>>
const clean = (a: Audit) => a.missing.length === 0 && a.duplicated.length === 0 && a.outOfOrder === 0 && a.invalid.length === 0
const describe = (a: Audit) =>
  `sent ${a.sent}, seen ${a.seen}, missing ${a.missing.length}, duplicated ${a.duplicated.length}, out of order ${a.outOfOrder}, invalid ${a.invalid.length}${a.invalid.length ? ` (${a.invalid.slice(0, 4).join(', ')})` : ''}`

function poster(user: User, thread: string, perSecond: number) {
  const hashes: string[] = []
  const timer = setInterval(() => {
    const h = newHash()
    hashes.push(h)
    user.post(thread, nextCount(thread), h)
  }, 1000 / perSecond)
  return { hashes, stop: () => clearInterval(timer) }
}

function nest(cmd: string, data: object): Promise<void> {
  return new Promise((resolve, reject) => {
    const msg = JSON.stringify({ pattern: JSON.stringify({ cmd }), data })
    const s = net.connect(NEST_PORT, '127.0.0.1', () => s.end(`${msg.length}#${msg}`))
    s.on('close', () => resolve())
    s.on('error', reject)
  })
}

async function setLocked(thread: string, locked: boolean) {
  await db.query('UPDATE thread SET locked = ? WHERE name = ?', [locked ? 1 : 0, thread])
  const [[row]] = (await db.query('SELECT * FROM thread WHERE name = ?', [thread])) as [Record<string, unknown>[]]
  await nest('updateThread', { thread: { ...row, locked } })
  await sleep(500)
}

async function setBanned(uuid: string, banned: boolean) {
  const [[row]] = (await db.query('SELECT roles FROM counters WHERE uuid = ?', [uuid])) as [{ roles: string[] | string }[]]
  const stored = typeof row.roles === 'string' ? (JSON.parse(row.roles) as string[]) : row.roles
  const roles = stored.filter(r => r !== 'banned')
  if (banned) roles.push('banned')
  await db.query('UPDATE counters SET roles = ? WHERE uuid = ?', [JSON.stringify(roles), uuid])
  await nest('updateUser', { uuid })
  await sleep(500)
}

async function fault(thread: string, mode: 'broken' | 'slow' | 'cut' | 'clear', ms = 0, region?: Region) {
  const o = owner(thread)!
  const body = JSON.stringify({ thread, region: region ?? o.region, epoch: o.epoch, mode, ms })
  const ts = String(Date.now())
  const sig = createHmac('sha256', SHARED).update(`${ts}.${body}`).digest('hex')
  const res = await fetch(`${WORKER}/admin/fault`, {
    method: 'POST',
    headers: { 'content-type': 'application/json', 'x-cgg-ts': ts, 'x-cgg-sig': sig },
    body,
  })
  if (!res.ok) throw new Error(`fault ${mode}: ${res.status}`)
}

async function control(path: string, body: object = {}) {
  const res = await fetch(`${config.control}${path}`, {
    method: 'POST',
    headers: { 'content-type': 'application/json' },
    body: JSON.stringify(body),
  })
  if (!res.ok) throw new Error(`${path}: ${await res.text()}`)
}

const pm2 = (action: 'restart' | 'stop' | 'start') => control(`/socket/${action}`)

async function socketUp(ms = 30_000): Promise<boolean> {
  const end = Date.now() + ms
  while (Date.now() < end) {
    if ([...viewers.values()].every(v => v.connected)) return true
    await sleep(200)
  }
  return false
}

const restartWithEnv = (key: string, value: string | null) => control('/socket/env', { [key]: value })

const EU = { vps: 95, weur: 15, enam: 85, wnam: 140, apac: 250 }
const ASIA = { vps: 220, weur: 300, enam: 230, wnam: 160, apac: 10 }

function shareLatency(u: User, rtts: Record<string, number>) {
  for (const [region, ms] of Object.entries(rtts)) {
    u.sender.emit('regional_sample', { kind: region === 'vps' ? 'ping' : 'probe', region, ms })
  }
}

async function autoFromVps(u: User, thread = T) {
  if (owner(thread)?.region) await moveTo(u, thread, 'vps')
  u.sender.emit('thread_region', { thread_name: thread, region: 'auto' })
  await waitOwner(thread, o => o.auto === true && !o.moving, 10_000)
}

interface Result {
  name: string
  pass: boolean
  details: string[]
}

const users: Record<string, User> = {}
async function user(key: string, account: number, profile: Profile = {}, shortTokens = false): Promise<User> {
  if (!users[key]) users[key] = await new User(key, profile, shortTokens).start(account)
  return users[key]
}

const T = config.threads.main
const NODOUBLES = config.threads.noDoubles

const scenarios: Record<string, () => Promise<Result>> = {
  async lock() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const before: string[] = []
    for (let i = 0; i < 5; i++) {
      const h = newHash()
      before.push(h)
      a.post(T, nextCount(T), h)
      await sleep(150)
    }
    await settle([a], 15_000)
    await setLocked(T, true)
    const rejectedBefore = a.rejected.length
    const during: string[] = []
    for (let i = 0; i < 5; i++) {
      const h = newHash()
      during.push(h)
      a.post(T, nextCount(T), h)
      await sleep(150)
    }
    const settledLocked = await settle([a], 15_000)
    const leaked = (await audit(T, during, false)).seen
    const rejected = a.rejected.slice(rejectedBefore)
    await setLocked(T, false)
    await resync(T)
    const after: string[] = []
    for (let i = 0; i < 5; i++) {
      const h = newHash()
      after.push(h)
      a.post(T, nextCount(T), h)
      await sleep(150)
    }
    await settle([a], 15_000)
    await moveTo(a, T, 'vps')
    const b = await audit(T, before)
    const c = await audit(T, after)
    details.push(`before lock: ${describe(b)}`)
    details.push(`while locked: ${leaked} of 5 got through; rejections ${rejected.length} (${[...new Set(rejected.map(r => r.reason))].join(', ')}); pending cleared ${settledLocked}`)
    details.push(`after unlock: ${describe(c)}`)
    return { name: 'lock', pass: clean(b) && clean(c) && leaked === 0 && rejected.length === 5 && settledLocked, details }
  },

  async ban() {
    const a = await user('a', 0)
    const b = await user('b', 1)
    a.watch(T)
    b.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    await setBanned(b.uuid, true)
    const aHashes: string[] = []
    const bHashes: string[] = []
    const rejectedBefore = b.rejected.length
    for (let i = 0; i < 6; i++) {
      const h = newHash()
      aHashes.push(h)
      a.post(T, nextCount(T), h)
      await sleep(150)
      const hb = newHash()
      bHashes.push(hb)
      b.post(T, 'banned hello', hb)
      await sleep(150)
    }
    const settled = await settle([a, b], 15_000)
    await setBanned(b.uuid, false)
    await moveTo(a, T, 'vps')
    const aa = await audit(T, aHashes)
    const leaked = (await audit(T, bHashes, false)).seen
    const rejected = b.rejected.slice(rejectedBefore)
    details.push(`allowed counter: ${describe(aa)}`)
    details.push(`banned counter: ${leaked} of 6 got through; rejections ${rejected.length} (${[...new Set(rejected.map(r => r.reason))].join(', ')}); pending cleared ${settled}`)
    return { name: 'ban', pass: clean(aa) && leaked === 0 && rejected.length === 6 && settled, details }
  },

  async race() {
    const a = await user('a', 0)
    const b = await user('b', 1)
    await watchThread(NODOUBLES)
    a.watch(NODOUBLES)
    b.watch(NODOUBLES)
    await resync(NODOUBLES)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, NODOUBLES, 'enam')} ms`)
    const n = nextCount(NODOUBLES)
    const ha = newHash()
    const hb = newHash()
    a.post(NODOUBLES, n, ha)
    b.post(NODOUBLES, n, hb)
    await settle([a, b], 15_000)
    const both = await audit(NODOUBLES, [ha, hb], false)
    const winners = both.rows.filter(r => r.valid)
    details.push(`same number from two counters: ${both.seen} arrived, ${winners.length} valid`)
    const winner = winners[0]?.hash === ha ? a : b
    const loser = winner === a ? b : a
    const dh = newHash()
    winner.post(NODOUBLES, nextCount(NODOUBLES), dh)
    await settle([winner], 15_000)
    const double = (await audit(NODOUBLES, [dh], false)).rows[0]
    details.push(`winner counting twice in a row: ${double ? (double.valid ? 'VALID (wrong)' : 'invalid (right)') : 'never arrived'}`)
    await resync(NODOUBLES)
    const alternating: string[] = []
    for (let i = 0; i < 10; i++) {
      const h = newHash()
      alternating.push(h)
      ;(i % 2 === 0 ? loser : winner).post(NODOUBLES, nextCount(NODOUBLES), h)
      await sleep(400)
    }
    await settle([a, b], 15_000)
    await moveTo(a, NODOUBLES, 'vps')
    const alt = await audit(NODOUBLES, alternating)
    details.push(`alternating after the race: ${describe(alt)}`)
    return {
      name: 'race',
      pass: both.seen === 2 && winners.length === 1 && !!double && !double.valid && clean(alt),
      details,
    }
  },

  async 'restart-remote'() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 8)
    await sleep(3000)
    log('restarting the socket')
    await pm2('restart')
    details.push(`socket back: ${await socketUp()}`)
    await sleep(8000)
    p.stop()
    const settled = await settle([a], 60_000)
    const o = owner(T)
    await moveTo(a, T, 'vps')
    const r = await audit(T, p.hashes)
    details.push(`owner after restart: ${o?.region ?? 'vps'} epoch ${o?.epoch}; settled ${settled}`)
    details.push(describe(r))
    return { name: 'restart-remote', pass: clean(r) && settled, details }
  },

  async 'restart-mid-move'() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 8)
    await sleep(3000)
    a.sender.emit('thread_region', { thread_name: T, region: 'weur' })
    await sleep(40)
    log('restarting the socket mid-move')
    const restartedAt = Date.now()
    await pm2('restart')
    await socketUp()
    const recovered = await waitOwner(T, o => !o.moving, 90_000)
    const recoveredMs = Date.now() - restartedAt
    await sleep(6000)
    p.stop()
    const settled = await settle([a], 60_000)
    await moveTo(a, T, 'vps')
    const r = await audit(T, p.hashes, false)
    const reported = new Set(a.rejected.map(x => x.hash))
    const unexplained = r.missing.filter(h => !reported.has(h))
    details.push(`settled on ${recovered ? recovered.region ?? 'vps' : 'nothing'} ${recoveredMs} ms after the restart; pending cleared ${settled}; lost and reported ${r.missing.length - unexplained.length}, lost silently ${unexplained.length}`)
    details.push(describe(r))
    return {
      name: 'restart-mid-move',
      pass: r.outOfOrder === 0 && r.duplicated.length === 0 && unexplained.length === 0 && settled && !!recovered,
      details,
    }
  },

  async 'graceful-mid-move'() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 8)
    await sleep(3000)
    a.sender.emit('thread_region', { thread_name: T, region: 'weur' })
    await sleep(40)
    log('stopping the socket gracefully mid-move')
    const stopping = Date.now()
    await control('/socket/restart', { graceful: true })
    const restartedIn = Date.now() - stopping
    details.push(`stopped and restarted in ${restartedIn} ms`)
    await socketUp()
    const recovered = await waitOwner(T, o => !o.moving, 90_000)
    await sleep(6000)
    p.stop()
    const settled = await settle([a], 60_000)
    await moveTo(a, T, 'vps')
    const r = await audit(T, p.hashes)
    details.push(`settled on ${recovered ? recovered.region ?? 'vps' : 'nothing'}; pending cleared ${settled}`)
    details.push(describe(r))
    return { name: 'graceful-mid-move', pass: clean(r) && settled && !!recovered && restartedIn < 25_000, details }
  },

  async 'vps-down'() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 8)
    await sleep(3000)
    log('stopping the socket for 15 s')
    await pm2('stop')
    await sleep(15_000)
    await pm2('start')
    details.push(`socket back: ${await socketUp()}`)
    await sleep(5000)
    p.stop()
    const settled = await settle([a], 90_000)
    const o = owner(T)
    await moveTo(a, T, 'vps')
    const r = await audit(T, p.hashes)
    details.push(`owner afterwards: ${o?.region ?? 'vps'}; settled ${settled}`)
    details.push(describe(r))
    return { name: 'vps-down', pass: clean(r) && settled, details }
  },

  async 'slow-release'() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 8)
    await sleep(2000)
    await fault(T, 'slow', 7000)
    log('enam now answers 7 s late; moving to weur')
    const moved = await moveTo(a, T, 'weur', 60_000)
    await sleep(6000)
    p.stop()
    await fault(T, 'clear', 0, 'enam')
    const settled = await settle([a], 60_000)
    await moveTo(a, T, 'vps')
    const r = await audit(T, p.hashes)
    details.push(`move enam -> weur with a slow enam: ${moved === null ? 'never settled' : `${moved} ms`}; settled ${settled}`)
    details.push(describe(r))
    return { name: 'slow-release', pass: clean(r) && settled && moved !== null, details }
  },

  async redeploy() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 8)
    await sleep(2000)
    log('redeploying the worker')
    const deployed = await control('/worker/restart').then(
      () => true,
      () => false,
    )
    log(`deploy finished: ${deployed}`)
    await sleep(6000)
    p.stop()
    const settled = await settle([a], 60_000)
    await moveTo(a, T, 'vps')
    const r = await audit(T, p.hashes)
    details.push(`deploy ok ${deployed}; settled ${settled}`)
    details.push(describe(r))
    return { name: 'redeploy', pass: clean(r) && settled && deployed, details }
  },

  async 'redeploy-unforwarded'() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 8)
    await sleep(2000)
    await fault(T, 'slow', 5000)
    await sleep(2000)
    log('killing the worker with accepted posts not yet forwarded')
    const deployed = await control('/worker/restart').then(
      () => true,
      () => false,
    )
    await sleep(6000)
    p.stop()
    const settled = await settle([a], 60_000)
    await moveTo(a, T, 'vps')
    const r = await audit(T, p.hashes)
    const reasons = a.rejected.reduce<Record<string, number>>((m, x) => ({ ...m, [x.reason]: (m[x.reason] ?? 0) + 1 }), {})
    details.push(`restarted ${deployed}; settled ${settled}; rejections ${JSON.stringify(reasons)}`)
    details.push(describe(r))
    return { name: 'redeploy-unforwarded', pass: clean(r) && settled && deployed, details }
  },

  async 'return-path-cut'() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 8)
    await sleep(2000)
    log('cutting the way back from enam to the VPS')
    const cutAt = Date.now()
    await fault(T, 'cut')
    const reclaimed = await waitOwner(T, o => !o.moving && o.region === null, 45_000)
    details.push(`taken back after ${reclaimed ? Date.now() - cutAt : '-'} ms`)
    await sleep(3000)
    p.stop()
    const settled = await settle([a], 60_000)
    await fault(T, 'clear', 0, 'enam')
    const r = await audit(T, p.hashes)
    details.push(`settled ${settled}; ${describe(r)}`)
    await fault(T, 'cut', 0, 'weur')
    const refusalsBefore = a.refusals.length
    a.sender.emit('thread_region', { thread_name: T, region: 'weur' })
    await waitOwner(T, o => !o.moving, 20_000)
    await sleep(2000)
    const stayed = owner(T)?.region === null
    await fault(T, 'clear', 0, 'weur')
    details.push(`move to a region that cannot reach the VPS: stayed on the VPS ${stayed}; refused with ${a.refusals.slice(refusalsBefore).join(', ') || 'nothing'}`)
    return { name: 'return-path-cut', pass: !!reclaimed && settled && clean(r) && stayed, details }
  },

  async 'client-drop'() {
    const a = await user('a', 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 8)
    await sleep(3000)
    a.sender.emit('thread_region', { thread_name: T, region: 'weur' })
    await sleep(80)
    log('dropping every connection of the counter mid-move')
    a.dropConnections()
    await sleep(5000)
    a.dropConnections()
    await sleep(6000)
    p.stop()
    const settled = await settle([a], 60_000)
    await moveTo(a, T, 'vps')
    const r = await audit(T, p.hashes)
    details.push(`settled ${settled}`)
    details.push(describe(r))
    return { name: 'client-drop', pass: clean(r) && settled, details }
  },

  async 'token-expiry'() {
    const c = await user('c', 2, {}, true)
    c.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(c, T, 'enam')} ms`)
    const p = poster(c, T, 2)
    for (let i = 0; i < 4; i++) {
      await sleep(11_000)
      log('closing the region connection after its token expired')
      c.dropConnections()
    }
    await sleep(3000)
    p.stop()
    const settled = await settle([c], 60_000)
    await moveTo(c, T, 'vps')
    const r = await audit(T, p.hashes)
    details.push(`tokens used ${c.tokens.length}; settled ${settled}`)
    details.push(describe(r))
    return { name: 'token-expiry', pass: clean(r) && settled && c.tokens.length >= 3, details }
  },

  async 'two-tabs'() {
    const a1 = await user('a', 0)
    const a2 = await user('a2', 0)
    a1.watch(T)
    a2.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a1, T, 'enam')} ms`)
    const hashes: string[] = []
    for (let i = 0; i < 20; i++) {
      const h = newHash()
      hashes.push(h)
      ;(i % 2 ? a2 : a1).post(T, nextCount(T), h)
      if (i === 10) a2.sender.emit('thread_region', { thread_name: T, region: 'weur' })
      const end = Date.now() + 10_000
      while (Date.now() < end && !(seen.get(T) ?? []).some(r => r.hash === h)) await sleep(20)
    }
    const settled = await settle([a1, a2], 30_000)
    await moveTo(a1, T, 'vps')
    const r = await audit(T, hashes)
    details.push(`settled ${settled}`)
    details.push(describe(r))
    return { name: 'two-tabs', pass: clean(r) && settled, details }
  },

  async parity() {
    const a = await user('a', 0)
    const b = await user('b', 1)
    a.watch(T)
    b.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const cases: [string, string, object][] = [
      ['comment', `${nextCount(T)} hello there`, {}],
      ['long', `${nextCount(T)} ${'x'.repeat(3000)}`, {}],
      ['emoji', `${nextCount(T)} 🎉✨ héllo`, {}],
      ['macro', nextCount(T), { macroMetadata: { macroPresetId: 1, macroPresetVersionId: 1 } }],
    ]
    const hashes: string[] = []
    for (const [, text, extra] of cases) {
      const h = newHash()
      hashes.push(h)
      a.post(T, text, h, extra)
      await sleep(300)
    }
    await settle([a], 15_000)
    const rows = seen.get(T) ?? []
    for (let i = 0; i < cases.length; i++) {
      const r = rows.find(x => x.hash === hashes[i])
      const intact = !!r && r.text === cases[i][1].trim()
      details.push(`${cases[i][0]}: ${r ? (r.valid ? 'valid' : 'INVALID') : 'MISSING'}, text ${intact ? 'intact' : 'CHANGED'}`)
    }
    const [[macroRow]] = (await db.query(
      'SELECT macroPresetId FROM post WHERE threadId = (SELECT id FROM thread WHERE name = ?) AND rawText = ? ORDER BY id DESC LIMIT 1',
      [T, cases[3][1]],
    )) as [{ macroPresetId: number | null }[]]
    details.push(`macro preset stored: ${macroRow?.macroPresetId ?? 'none'}`)
    const target = rows.find(x => x.hash === hashes[0])
    const before = reactionUpdates.length
    if (target) b.sender.emit('updateReactions', { post_uuid: target.uuid, id: 'grinning' })
    await sleep(2000)
    const reacted = reactionUpdates.slice(before).some(u => u.post_uuid === target?.uuid)
    details.push(`reaction on a post that came through the region: ${reacted ? 'broadcast' : 'NOT broadcast'}`)
    await moveTo(a, T, 'vps')
    const r = await audit(T, hashes)
    details.push(describe(r))
    const textsIntact = details.filter(d => d.includes('CHANGED') || d.includes('MISSING')).length === 0
    return { name: 'parity', pass: clean(r) && reacted && textsIntact && macroRow?.macroPresetId === 1, details }
  },

  async delete() {
    const a = await user('a', 0)
    const admin = await user('admin', config.moderator)
    a.watch(T)
    admin.watch(T)
    await resync(T)
    const details: string[] = []
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const h = newHash()
    const count = nextCount(T)
    a.post(T, `${count} delete this comment`, h)
    await settle([a], 15_000)
    const row = (seen.get(T) ?? []).find(r => r.hash === h)
    const before = commentDeletes.length
    if (row) admin.sender.emit('deleteComment', { uuid: row.uuid })
    await sleep(2500)
    const deleted = commentDeletes.slice(before).find(d => d.uuid === row?.uuid)
    const [[stored]] = (await db.query('SELECT rawText, isCommentDeleted FROM post WHERE uuid = ?', [row?.uuid ?? ''])) as [
      { rawText: string; isCommentDeleted: number }[],
    ]
    await moveTo(a, T, 'vps')
    details.push(`post through the region: ${row ? (row.valid ? 'valid' : 'INVALID') : 'MISSING'}`)
    details.push(`comment delete broadcast: ${deleted ? `yes, text now "${deleted.rawText}"` : 'NO'}`)
    details.push(`stored: ${stored ? `"${stored.rawText}", comment deleted ${!!stored.isCommentDeleted}` : 'not found yet'}`)
    return { name: 'delete', pass: !!row?.valid && !!deleted && deleted.rawText === count, details }
  },

  async budget() {
    const a = await user('a', 0)
    a.watch(T)
    const today = Math.floor(Date.now() / 86_400_000) * 86_400_000
    const [[{ n }]] = (await db.query(
      "SELECT COUNT(*) n FROM cgg_regional_sample WHERE kind = 'ingest' AND at >= ?",
      [today],
    )) as [{ n: number }[]]
    const limit = Number(n) + 20
    await restartWithEnv('DO_DAILY_POST_LIMIT', String(limit))
    await socketUp()
    await sleep(2000)
    await resync(T)
    const details: string[] = [`limit set to ${limit} (${n} used today)`]
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    const p = poster(a, T, 5)
    const reclaimed = await waitOwner(T, o => !o.moving && o.region === null, 60_000)
    await sleep(2000)
    p.stop()
    const settled = await settle([a], 60_000)
    const refusalsBefore = a.refusals.length
    a.sender.emit('thread_region', { thread_name: T, region: 'weur' })
    await sleep(2000)
    const refused = a.refusals.slice(refusalsBefore)
    const closed = owner(T)?.regionsOpen === false
    await restartWithEnv('DO_DAILY_POST_LIMIT', null)
    await socketUp()
    await sleep(2000)
    const r = await audit(T, p.hashes)
    details.push(`taken back at the limit: ${!!reclaimed}; regions shown closed: ${closed}; next move refused with: ${refused.join(', ') || 'nothing'}`)
    details.push(`settled ${settled}; ${describe(r)}`)
    return { name: 'budget', pass: !!reclaimed && refused.includes('budget') && closed && clean(r) && settled, details }
  },

  async 'auto-solo'() {
    const eu = await user('auto-eu', 0)
    eu.watch(T)
    await autoFromVps(eu)
    shareLatency(eu, EU)
    await resync(T)
    const details: string[] = []
    const p = poster(eu, T, 8)
    const started = Date.now()
    const announced = await waitOwner(T, o => !!o.pending, 15_000)
    details.push(`announced ${announced?.pending?.target ?? 'nothing'} after ${Date.now() - started} ms, countdown ${announced?.pending?.inMs ?? '-'} ms`)
    const moved = await waitOwner(T, o => !o.moving && o.region === 'weur', 20_000)
    details.push(`on weur after ${Date.now() - started} ms`)
    await sleep(2000)
    p.stop()
    const settled = await settle([eu], 30_000)
    const r = await audit(T, p.hashes)
    details.push(describe(r))
    return { name: 'auto-solo', pass: announced?.pending?.target === 'weur' && !!moved && settled && clean(r), details }
  },

  async 'auto-one-offs'() {
    const eu = await user('auto-eu', 0)
    const eu2 = await user('auto-eu2', 1)
    await watchThread(NODOUBLES)
    eu.watch(NODOUBLES)
    eu2.watch(NODOUBLES)
    await autoFromVps(eu, NODOUBLES)
    shareLatency(eu, EU)
    shareLatency(eu2, EU)
    await resync(NODOUBLES)
    const hashes: string[] = []
    const seen: string[] = []
    for (const who of [eu, eu2, eu]) {
      const h = newHash()
      hashes.push(h)
      who.post(NODOUBLES, nextCount(NODOUBLES), h)
      await sleep(2500)
      const pending = owner(NODOUBLES)?.pending
      if (pending) seen.push(pending.target)
    }
    await sleep(5000)
    const o = owner(NODOUBLES)
    const stayed = o?.region === null && !o?.pending && seen.length === 0
    const settled = await settle([eu, eu2], 30_000)
    const r = await audit(NODOUBLES, hashes, false)
    return {
      name: 'auto-one-offs',
      pass: stayed && settled && clean(r),
      details: [`announced ${seen.join(', ') || 'nothing'}; stayed on the VPS ${stayed}`, describe(r)],
    }
  },

  async 'auto-show'() {
    const eu = await user('auto-eu', 0)
    eu.watch(T)
    await autoFromVps(eu)
    shareLatency(eu, EU)
    await resync(T)
    const p = poster(eu, T, 2)
    await sleep(Number(process.env.SHOW_MS ?? 45_000))
    p.stop()
    await settle([eu], 30_000)
    return { name: 'auto-show', pass: true, details: [`ended on ${owner(T)?.region ?? 'vps'}`] }
  },

  async 'auto-cancel'() {
    const eu = await user('auto-eu', 0)
    eu.watch(T)
    await autoFromVps(eu)
    shareLatency(eu, EU)
    await resync(T)
    const details: string[] = []
    const p = poster(eu, T, 8)
    const announced = await waitOwner(T, o => !!o.pending, 15_000)
    eu.sender.emit('thread_region_cancel', { thread_name: T })
    const paused = await waitOwner(T, o => !o.pending && o.auto === false, 5_000)
    await sleep(5000)
    const stayed = owner(T)?.region === null
    details.push(`announced ${announced?.pending?.target ?? 'nothing'}; cancelled ${!!paused}; paused until +${Math.round(((paused?.pausedUntil ?? 0) - Date.now()) / 1000)} s; stayed on the VPS ${stayed}`)
    p.stop()
    const settled = await settle([eu], 30_000)
    const r = await audit(T, p.hashes)
    details.push(describe(r))
    return { name: 'auto-cancel', pass: !!announced && !!paused && stayed && settled && clean(r), details }
  },

  async 'auto-dropper'() {
    const eu = await user('auto-eu', 0)
    const asia = await user('auto-asia', 2)
    eu.watch(T)
    asia.watch(T)
    await autoFromVps(eu)
    shareLatency(eu, EU)
    shareLatency(asia, ASIA)
    await resync(T)
    const details: string[] = []
    const hashes: string[] = []
    let stop = false
    const seen: string[] = []
    const posting = (async () => {
      for (let i = 0; !stop; i++) {
        const who = i % 30 === 15 ? asia : eu
        const h = newHash()
        hashes.push(h)
        who.post(T, nextCount(T), h)
        const o = owner(T)
        if (o?.pending) seen.push(o.pending.target)
        await sleep(150)
      }
    })()
    const moved = await waitOwner(T, o => !o.moving && o.region === 'weur', 20_000)
    await sleep(8000)
    stop = true
    await posting
    const settled = await settle([eu, asia], 30_000)
    const r = await audit(T, hashes)
    const targets = [...new Set(seen)]
    details.push(`announced ${targets.join(', ') || 'nothing'}; ended on ${owner(T)?.region ?? 'vps'}`)
    details.push(describe(r))
    return { name: 'auto-dropper', pass: !!moved && !targets.includes('apac') && owner(T)?.region === 'weur' && settled && clean(r), details }
  },

  async 'own-order'() {
    const a = await user(`own-${Date.now()}`, 0)
    a.watch(T)
    await resync(T)
    const details: string[] = []
    const p = poster(a, T, 8)
    details.push(`move to enam: ${await moveTo(a, T, 'enam')} ms`)
    await sleep(1500)
    a.sender.emit('thread_region', { thread_name: T, region: 'weur' })
    await sleep(60)
    await pm2('restart')
    await socketUp()
    await waitOwner(T, o => !o.moving, 90_000)
    await sleep(1500)
    a.dropConnections()
    await sleep(2000)
    const broken = owner(T)?.region ?? null
    if (broken) await fault(T, 'broken')
    await waitOwner(T, o => !o.moving && o.region === null, 30_000)
    if (broken) await fault(T, 'clear', 0, broken)
    details.push(`move to apac: ${await moveTo(a, T, 'apac')} ms`)
    await sleep(1500)
    p.stop()
    const settled = await settle([a], 60_000)
    await moveTo(a, T, 'vps')
    const r = await audit(T, p.hashes, false)
    const reported = new Set(a.rejected.map(x => x.hash))
    const unexplained = r.missing.filter(h => !reported.has(h))
    const sentOrder = new Map(p.hashes.map((h, i) => [h, i]))
    const swaps = r.rows.flatMap((row, i) => (i > 0 && (sentOrder.get(row.hash) ?? 0) < (sentOrder.get(r.rows[i - 1].hash) ?? 0) ? [`${r.rows[i - 1].text}>${row.text}`] : []))
    if (swaps.length) {
      const [, later] = swaps[0].split('>')
      const at = traced.findIndex(l => l.includes(`typed`) && l.endsWith(` ${later}`))
      for (const line of traced.slice(Math.max(0, at - 14), at + 14)) console.log(line)
    }
    if (swaps.length) details.push(`swapped: ${swaps.join(' ')}; dropped: ${a.rejected.map(x => textOf.get(x.hash)).join(' ')}; lost: ${r.missing.map(h => textOf.get(h)).join(' ')}`)
    details.push(`settled ${settled}; reported back ${a.rejected.length} (${[...new Set(a.rejected.map(x => x.reason))].join(', ')}); missing without a report ${unexplained.length}`)
    details.push(describe(r))
    return { name: 'own-order', pass: r.outOfOrder === 0 && r.duplicated.length === 0 && unexplained.length === 0 && settled, details }
  },

  'blind-crowd': () => blindCrowd('blind-crowd', ['enam', 'weur', 'vps', 'apac', 'enam', 'vps']),

  'blind-steady': () => blindCrowd('blind-steady', []),

  async 'multi-region'() {
    const eu = await user('eu', 0, { vps: 45, weur: 5, enam: 40, wnam: 70 })
    const us = await user('us', 1, { vps: 8, enam: 5, weur: 40, wnam: 30 })
    const asia = await user('asia', 2, { vps: 95, enam: 95, weur: 85, wnam: 70, apac: 10 })
    const all = [eu, us, asia]
    all.forEach(u => u.watch(T))
    await resync(T)
    const details: string[] = []
    const hashes: string[] = []
    const byOwner: Record<string, Record<string, number[]>> = {}
    for (const where of ['vps', 'weur', 'enam', 'apac', 'vps'] as const) {
      details.push(`move to ${where}: ${await moveTo(us, T, where)} ms`)
      const phase: { u: User; h: string }[] = []
      for (let i = 0; i < 12; i++) {
        const u = all[i % 3]
        const h = newHash()
        hashes.push(h)
        phase.push({ u, h })
        u.post(T, nextCount(T), h)
        await sleep(450)
      }
      await settle(all, 20_000)
      for (const { u, h } of phase) {
        const ms = (u.confirmedAt.get(h) ?? NaN) - (u.sentAt.get(h) ?? NaN)
        ;((byOwner[where] ??= {})[u.name] ??= []).push(ms)
      }
    }
    const median = (v: number[]) => {
      const s = v.filter(Number.isFinite).sort((x, y) => x - y)
      return s.length ? Math.round(s[Math.floor(s.length / 2)]) : NaN
    }
    for (const [where, per] of Object.entries(byOwner)) {
      details.push(`${where.padEnd(5)} confirm ms  eu ${median(per.eu)}  us ${median(per.us)}  asia ${median(per.asia)}`)
    }
    const [rows] = (await db.query(
      'SELECT id, CAST(timestamp AS DECIMAL(20,3)) ts FROM post WHERE threadId = (SELECT id FROM thread WHERE name = ?) ORDER BY id DESC LIMIT ?',
      [T, hashes.length + 20],
    )) as [{ id: number; ts: string }[]]
    const asc = rows.reverse()
    let backwards = 0
    for (let i = 1; i < asc.length; i++) if (Number(asc[i].ts) <= Number(asc[i - 1].ts)) backwards++
    const r = await audit(T, hashes)
    details.push(`timestamps not increasing: ${backwards}`)
    details.push(describe(r))
    return { name: 'multi-region', pass: clean(r) && backwards === 0, details }
  },
}

const MANUAL = new Set(['auto-show'])
const wanted = (process.argv[2] ?? Object.keys(scenarios).filter(n => !MANUAL.has(n)).join(',')).split(',')
await watchThread(T)
const results: Result[] = []
for (const name of wanted) {
  const run = scenarios[name]
  if (!run) {
    console.log(`unknown scenario ${name}; known: ${Object.keys(scenarios).join(', ')}`)
    continue
  }
  log(`=== ${name}`)
  for (const thread of [T, NODOUBLES]) {
    const [[row]] = (await db.query('SELECT MAX(id) id FROM post WHERE threadId = (SELECT id FROM thread WHERE name = ?)', [
      thread,
    ])) as [{ id: number }[]]
    startIds.set(thread, Number(row?.id ?? 0))
  }
  try {
    const r = await run()
    results.push(r)
    for (const d of r.details) log(`  ${d}`)
    log(`  ${r.pass ? 'PASS' : 'FAIL'}`)
  } catch (e) {
    results.push({ name, pass: false, details: [String(e)] })
    log(`  ERROR ${(e as Error).stack ?? e}`)
  }
  await sleep(2500)
}
console.log('')
for (const r of results) console.log(`${r.pass ? 'PASS' : 'FAIL'}  ${r.name}`)
const superseded = Object.values(users).flatMap(u => u.rejected.filter(x => x.reason === 'superseded'))
console.log(`superseded drops across the run: ${superseded.length}`)
for (const u of Object.values(users)) u.close()
for (const v of viewers.values()) v.close()
await db.end()
process.exit(results.every(r => r.pass) ? 0 : 1)
