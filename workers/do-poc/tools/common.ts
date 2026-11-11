import { sign } from '../src/token.ts'

export function requireSecret(): string {
  const secret = process.env.PROBE_SECRET
  if (!secret) {
    console.error('Set PROBE_SECRET to the same value the Worker has (.dev.vars locally).')
    process.exit(2)
  }
  return secret
}

export function tokenFor(uuid: string, minutes = 60): Promise<string> {
  return sign({ uuid, exp: Date.now() + minutes * 60_000 }, requireSecret())
}

export function percentile(values: number[], p: number): number {
  if (values.length === 0) return NaN
  const sorted = [...values].sort((a, b) => a - b)
  return sorted[Math.min(sorted.length - 1, Math.floor(p * sorted.length))]
}

export function summary(values: number[]): string {
  const f = (v: number) => (Number.isNaN(v) ? '-' : v.toFixed(1))
  return `n=${values.length} min=${f(Math.min(...values))} p50=${f(percentile(values, 0.5))} p90=${f(percentile(values, 0.9))} p99=${f(percentile(values, 0.99))} ms`
}

export function arg(name: string, fallback: string): string {
  const i = process.argv.indexOf(`--${name}`)
  return i >= 0 && process.argv[i + 1] ? process.argv[i + 1] : fallback
}

export function open(url: string): Promise<WebSocket> {
  return new Promise((resolve, reject) => {
    const ws = new WebSocket(url)
    ws.addEventListener('open', () => resolve(ws), { once: true })
    ws.addEventListener('error', () => reject(new Error(`could not connect to ${url.split('?')[0]}`)), { once: true })
  })
}

export function nextMessage(ws: WebSocket, wanted: (data: string) => boolean, timeoutMs = 5000): Promise<string> {
  return new Promise((resolve, reject) => {
    const timer = setTimeout(() => {
      ws.removeEventListener('message', onMessage)
      reject(new Error('timed out'))
    }, timeoutMs)
    function onMessage(event: MessageEvent) {
      const data = String(event.data)
      if (!wanted(data)) return
      clearTimeout(timer)
      ws.removeEventListener('message', onMessage)
      resolve(data)
    }
    ws.addEventListener('message', onMessage)
  })
}
