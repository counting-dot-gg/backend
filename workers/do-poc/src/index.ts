import type { Env } from './env.ts'
import { asRegion, hintFor, type Region } from './regions.ts'
import { verifiedBody } from './signing.ts'
import { verify } from './token.ts'

export { ProbeDO } from './probe.ts'
export { ThreadDO } from './thread.ts'
import type { PassedPost } from './thread.ts'

interface AdminRequest {
  thread?: string
  region?: string
  epoch?: number
  meta?: unknown
  book?: unknown
  hashes?: string[]
  limits?: { burstPerSec: number; on: boolean }
  counter?: unknown
  posts?: PassedPost[]
  mode?: string
  ms?: number
}

function refuse(status: number, reason: string): Response {
  return new Response(reason, { status })
}

function threadObject(env: Env, thread: string, region: Region) {
  return env.THREAD.get(env.THREAD.idFromName(`${thread}@${region}`), { locationHint: hintFor(region) })
}

function probe(env: Env, request: Request, name: string): Promise<Response> | Response {
  const region = asRegion(name)
  if (!region) return refuse(404, 'unknown region')
  const stub = env.PROBE.get(env.PROBE.idFromName(region), { locationHint: hintFor(region) })
  return stub.fetch(request)
}

function thread(env: Env, request: Request, name: string, uuid: string): Promise<Response> | Response {
  const params = new URL(request.url).searchParams
  const region = asRegion(params.get('region') ?? params.get('hint'))
  if (!name || !region) return refuse(404, 'no thread or region')
  const forwarded = new Request(request)
  forwarded.headers.set('x-cgg-uuid', uuid)
  return threadObject(env, name, region).fetch(forwarded)
}

async function admin(env: Env, request: Request, action: string): Promise<Response> {
  const body = await verifiedBody(request, env.DO_SHARED_SECRET)
  if (body === null) return refuse(401, 'bad signature')
  const m = JSON.parse(body) as AdminRequest
  const region = asRegion(m.region)
  if (!m.thread || !region || !Number.isInteger(m.epoch)) return refuse(400, 'bad request')
  const stub = threadObject(env, m.thread, region)
  if (action === 'adopt') return Response.json(await stub.adopt(m.thread, m.epoch!, { meta: m.meta, book: m.book, hashes: m.hashes }, m.limits))
  try {
    if (action === 'meta') return Response.json(await stub.meta(m.epoch!, m.meta))
    if (action === 'counter') return Response.json(await stub.counter(m.epoch!, m.counter))
    if (action === 'posts') return Response.json(await stub.posts(m.epoch!, m.posts ?? []))
    if (action === 'release') return Response.json(await stub.release(m.epoch!))
    if (action === 'status') return Response.json(await stub.status(m.epoch!))
    if (action === 'unforwarded') return Response.json(await stub.unforwarded(m.epoch!))
    if (action === 'fault') return Response.json(await stub.fault(m.mode ?? 'clear', m.ms ?? 0))
  } catch {
    return refuse(503, 'object unavailable')
  }
  return refuse(404, 'unknown action')
}

async function ingestStub(request: Request): Promise<Response> {
  const body = (await request.json()) as { posts?: { seq: number }[] }
  return Response.json({ ok: true, lastSeq: Math.max(0, ...(body.posts ?? []).map(p => p.seq)) })
}

export default {
  async fetch(request: Request, env: Env): Promise<Response> {
    const url = new URL(request.url)
    const [, kind = '', name = ''] = url.pathname.split('/')
    if (kind === 'health') return Response.json({ ok: true, enabled: env.ENABLED !== 'off' })
    if (request.method === 'POST' && kind === 'ingest') return ingestStub(request)
    if (request.method === 'POST' && kind === 'admin') return admin(env, request, name)
    if (env.ENABLED === 'off') return refuse(503, 'disabled')
    if (request.headers.get('Upgrade') !== 'websocket') return refuse(426, 'websocket only')
    const claims = await verify(url.searchParams.get('t') ?? '', env.PROBE_SECRET, Date.now())
    if (!claims) return refuse(401, 'bad token')
    if (kind === 'probe') return probe(env, request, name)
    if (kind === 'thread') return thread(env, request, decodeURIComponent(name), claims.uuid)
    return refuse(404, 'not found')
  },
} satisfies ExportedHandler<Env>
