const encoder = new TextEncoder()
const MAX_CLOCK_SKEW_MS = 60_000

function hex(bytes: ArrayBuffer): string {
  return [...new Uint8Array(bytes)].map(b => b.toString(16).padStart(2, '0')).join('')
}

async function hmacHex(secret: string, timestamp: string, body: string): Promise<string> {
  const key = await crypto.subtle.importKey('raw', encoder.encode(secret), { name: 'HMAC', hash: 'SHA-256' }, false, [
    'sign',
  ])
  return hex(await crypto.subtle.sign('HMAC', key, encoder.encode(`${timestamp}.${body}`)))
}

function same(a: string, b: string): boolean {
  if (a.length !== b.length) return false
  let diff = 0
  for (let i = 0; i < a.length; i++) diff |= a.charCodeAt(i) ^ b.charCodeAt(i)
  return diff === 0
}

export async function signedHeaders(secret: string, body: string): Promise<Record<string, string>> {
  const timestamp = String(Date.now())
  return {
    'content-type': 'application/json',
    'x-cgg-ts': timestamp,
    'x-cgg-sig': await hmacHex(secret, timestamp, body),
  }
}

export async function verifiedBody(request: Request, secret: string): Promise<string | null> {
  const timestamp = request.headers.get('x-cgg-ts') ?? ''
  const signature = request.headers.get('x-cgg-sig') ?? ''
  if (!secret || Math.abs(Date.now() - Number(timestamp)) > MAX_CLOCK_SKEW_MS) return null
  const body = await request.text()
  return same(await hmacHex(secret, timestamp, body), signature) ? body : null
}
