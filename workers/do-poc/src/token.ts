const encoder = new TextEncoder()

export interface Claims {
  uuid: string
  exp: number
}

function toBase64Url(bytes: ArrayBuffer): string {
  let binary = ''
  for (const b of new Uint8Array(bytes)) binary += String.fromCharCode(b)
  return btoa(binary).replace(/\+/g, '-').replace(/\//g, '_').replace(/=+$/, '')
}

function fromBase64Url(text: string): Uint8Array {
  const padded = text.replace(/-/g, '+').replace(/_/g, '/') + '==='.slice((text.length + 3) % 4)
  return Uint8Array.from(atob(padded), c => c.charCodeAt(0))
}

function hmacKey(secret: string): Promise<CryptoKey> {
  return crypto.subtle.importKey('raw', encoder.encode(secret), { name: 'HMAC', hash: 'SHA-256' }, false, [
    'sign',
    'verify',
  ])
}

export async function sign(claims: Claims, secret: string): Promise<string> {
  const body = `${claims.uuid}.${claims.exp}`
  const signature = await crypto.subtle.sign('HMAC', await hmacKey(secret), encoder.encode(body))
  return `${body}.${toBase64Url(signature)}`
}

export async function verify(token: string, secret: string, nowMs: number): Promise<Claims | null> {
  const parts = token.split('.')
  if (parts.length !== 3 || !secret) return null
  const [uuid, expText, signature] = parts
  const exp = Number(expText)
  if (!uuid || !Number.isFinite(exp) || exp < nowMs) return null
  let signatureBytes: Uint8Array
  try {
    signatureBytes = fromBase64Url(signature)
  } catch {
    return null
  }
  const valid = await crypto.subtle.verify(
    'HMAC',
    await hmacKey(secret),
    signatureBytes,
    encoder.encode(`${uuid}.${exp}`),
  )
  return valid ? { uuid, exp } : null
}
