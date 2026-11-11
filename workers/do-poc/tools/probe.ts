import { REGIONS } from '../src/regions.ts'
import { arg, nextMessage, open, summary, tokenFor } from './common.ts'

const base = arg('base', 'ws://127.0.0.1:8787')
const pings = Number(arg('pings', '5'))
const regions = arg('regions', REGIONS.join(',')).split(',')
const token = await tokenFor('probe-cli')

async function measure(region: string): Promise<string> {
  const ws = await open(`${base}/probe/${region}?t=${encodeURIComponent(token)}`)
  try {
    ws.send('where')
    const { colo } = JSON.parse(await nextMessage(ws, d => d.startsWith('{"t":"where"'), 15000))
    const rtts: number[] = []
    for (let i = 0; i < pings; i++) {
      const sent = performance.now()
      ws.send('ping')
      await nextMessage(ws, d => d === 'pong')
      rtts.push(performance.now() - sent)
    }
    return `${region.padEnd(8)} colo=${String(colo).padEnd(8)} ${summary(rtts)}`
  } finally {
    ws.close(1000)
  }
}

for (const region of regions) {
  try {
    console.log(await measure(region))
  } catch (e) {
    console.log(`${region.padEnd(8)} failed: ${(e as Error).message}`)
  }
}
