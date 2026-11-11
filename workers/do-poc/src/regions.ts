export const REGIONS = ['wnam', 'enam', 'weur', 'eeur', 'apac', 'apac-ne', 'apac-se', 'oc'] as const

export type Region = (typeof REGIONS)[number]

export function asRegion(text: string | null | undefined): Region | null {
  return REGIONS.find(r => r === text) ?? null
}

export function hintFor(region: Region): DurableObjectLocationHint {
  return region as DurableObjectLocationHint
}
