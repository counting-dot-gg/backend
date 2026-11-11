import { signedHeaders } from '../src/signing.ts'
import { arg } from './common.ts'

const action = process.argv[2]
const secret = process.env.DO_SHARED_SECRET
if (!secret || (action !== 'adopt' && action !== 'release')) {
  console.error('usage: DO_SHARED_SECRET=… npm run admin -- adopt|release --thread T --region R --epoch N [--last N] [--base URL]')
  process.exit(2)
}
const base = arg('base', 'http://127.0.0.1:8787').replace(/^ws/, 'http')
const body = JSON.stringify({
  thread: arg('thread', ''),
  region: arg('region', ''),
  epoch: Number(arg('epoch', '1')),
  last: Number(arg('last', '0')),
})
const response = await fetch(`${base}/admin/${action}`, {
  method: 'POST',
  headers: await signedHeaders(secret, body),
  body,
  signal: AbortSignal.timeout(15_000),
})
console.log(response.status, await response.text())
