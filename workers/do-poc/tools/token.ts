import { arg, tokenFor } from './common.ts'

const uuid = process.argv[2]
if (!uuid || uuid.startsWith('--')) {
  console.error('usage: npm run token -- <uuid> [--minutes 60]')
  process.exit(2)
}
console.log(await tokenFor(uuid, Number(arg('minutes', '60'))))
