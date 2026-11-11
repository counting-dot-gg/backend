const RESERVED_CLOSE_CODES = new Set([1005, 1006, 1015])

export function closeBack(ws: WebSocket, code: number): void {
  try {
    ws.close(RESERVED_CLOSE_CODES.has(code) ? 1000 : code, 'closed')
  } catch {
    return
  }
}
