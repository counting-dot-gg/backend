import decideWasm from './decide.wasm'

interface Exports {
  memory: WebAssembly.Memory
  input(len: number): number
  output(): number
  call(ptr: number, len: number): number
}

export interface Snapshot {
  meta: unknown
  book: unknown
  counters?: unknown[]
}

export interface Judged {
  error?: string
  rejected?: string
  decision?: unknown
  message?: Record<string, unknown>
  threadUpdate?: Record<string, unknown> | null
}

export interface PostAsk {
  author: string
  text: string
  hash: string
  macroPresetId?: number
  macroPresetVersionId?: number
}

const encoder = new TextEncoder()
const decoder = new TextDecoder()

function instantiate(): Exports {
  return new WebAssembly.Instance(decideWasm, {}).exports as unknown as Exports
}

export class Engine {
  private wasm = instantiate()
  loaded = false

  private call<T>(ask: object): T {
    try {
      const bytes = encoder.encode(JSON.stringify(ask))
      const ptr = this.wasm.input(bytes.length)
      new Uint8Array(this.wasm.memory.buffer, ptr, bytes.length).set(bytes)
      const len = this.wasm.call(ptr, bytes.length)
      return JSON.parse(decoder.decode(new Uint8Array(this.wasm.memory.buffer, this.wasm.output(), len))) as T
    } catch (e) {
      this.wasm = instantiate()
      this.loaded = false
      throw e
    }
  }

  load(snapshot: Snapshot): boolean {
    this.loaded = this.call<{ ok?: boolean }>({ op: 'load', ...snapshot }).ok === true
    return this.loaded
  }

  setMeta(meta: unknown): boolean {
    return this.call<{ ok?: boolean }>({ op: 'meta', meta }).ok === true
  }

  addCounter(counter: unknown): boolean {
    return this.call<{ ok?: boolean }>({ op: 'counter', counter }).ok === true
  }

  knows(uuid: string): boolean {
    return this.call<{ known?: boolean }>({ op: 'knows', uuid }).known === true
  }

  snapshot(): Snapshot {
    return this.call<Snapshot>({ op: 'book' })
  }

  judge(post: PostAsk): Judged {
    const now = Date.now()
    return this.call<Judged>({
      op: 'post',
      ...post,
      ns: now * 1_000_000,
      nowMs: now,
      random: [Math.random(), Math.random(), Math.random(), Math.random()],
    })
  }
}
