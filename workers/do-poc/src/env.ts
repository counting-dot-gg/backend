import type { ProbeDO } from './probe.ts'
import type { ThreadDO } from './thread.ts'

export interface Env {
  PROBE: DurableObjectNamespace<ProbeDO>
  THREAD: DurableObjectNamespace<ThreadDO>
  PROBE_SECRET: string
  DO_SHARED_SECRET: string
  ENABLED: string
  FLUSH_MS: string
  INGEST_URL: string
}
