import { DurableObject } from 'cloudflare:workers'
import { coloOf } from './colo.ts'
import type { Env } from './env.ts'
import { closeBack } from './sockets.ts'

export class ProbeDO extends DurableObject<Env> {
  constructor(ctx: DurableObjectState, env: Env) {
    super(ctx, env)
    ctx.setWebSocketAutoResponse(new WebSocketRequestResponsePair('ping', 'pong'))
  }

  async fetch(): Promise<Response> {
    const [client, server] = Object.values(new WebSocketPair())
    this.ctx.acceptWebSocket(server)
    return new Response(null, { status: 101, webSocket: client })
  }

  async webSocketMessage(ws: WebSocket, message: string | ArrayBuffer): Promise<void> {
    if (message === 'where') ws.send(JSON.stringify({ t: 'where', colo: await coloOf(this.ctx.storage) }))
  }

  async webSocketClose(ws: WebSocket, code: number): Promise<void> {
    closeBack(ws, code)
  }
}
