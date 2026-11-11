export async function coloOf(storage: DurableObjectStorage): Promise<string> {
  const cached = await storage.get<string>('colo')
  if (cached) return cached
  let colo = 'unknown'
  try {
    const trace = await (await fetch('https://www.cloudflare.com/cdn-cgi/trace')).text()
    colo = /^colo=(.+)$/m.exec(trace)?.[1] ?? colo
  } catch {
    return colo
  }
  await storage.put('colo', colo)
  return colo
}
