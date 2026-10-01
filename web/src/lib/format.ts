/** Decimal or `0x` hexadecimal integer; `undefined` when invalid. */
export function parseNumber(text: string): number | undefined {
  const s = text.trim().toLowerCase().replace(/_/g, '')
  if (s === '') return undefined
  const m = /^(0x[0-9a-f]+|\d+)(k|m)?$/.exec(s)
  if (!m) return undefined
  const base = Number(m[1])
  const mult = m[2] === 'k' ? 1024 : m[2] === 'm' ? 1024 * 1024 : 1
  const value = base * mult
  return Number.isSafeInteger(value) ? value : undefined
}

export function formatBytes(n: number): string {
  if (n >= 1024 * 1024 && n % (1024 * 1024) === 0) return `${n / (1024 * 1024)} MiB`
  if (n >= 1024 && n % 1024 === 0) return `${n / 1024} KiB`
  return `${n} B`
}

export const hex = (n: number, width = 2): string => n.toString(16).toUpperCase().padStart(width, '0')

/** `xxd`-style dump: `00000000  de ad be ef ...  |....|` */
export function hexdump(data: Uint8Array, baseAddress = 0): string {
  const lines: string[] = []
  for (let off = 0; off < data.length; off += 16) {
    const row = data.subarray(off, off + 16)
    const bytes = Array.from(row, b => hex(b).toLowerCase()).join(' ').padEnd(47, ' ')
    const ascii = Array.from(row, b => (b >= 0x20 && b < 0x7f ? String.fromCharCode(b) : '.')).join('')
    lines.push(`${hex(baseAddress + off, 8).toLowerCase()}  ${bytes}  |${ascii}|`)
  }
  return lines.join('\n')
}

/** Parses `de ad BE EF`, `0xde,0xad`, `deadbeef`. Returns `undefined` on invalid input. */
export function parseHexBytes(text: string): Uint8Array<ArrayBuffer> | undefined {
  const cleaned = text.replace(/0x/gi, '').replace(/[\s,:-]/g, '')
  if (cleaned.length === 0 || cleaned.length % 2 !== 0 || /[^0-9a-f]/i.test(cleaned)) return undefined
  const out = new Uint8Array(cleaned.length / 2)
  for (let i = 0; i < out.length; i++) out[i] = parseInt(cleaned.slice(i * 2, i * 2 + 2), 16)
  return out
}

/** Offset of the first differing byte, or -1 when equal. */
export function firstMismatch(a: Uint8Array, b: Uint8Array): number {
  const n = Math.min(a.length, b.length)
  for (let i = 0; i < n; i++) if (a[i] !== b[i]) return i
  return a.length === b.length ? -1 : n
}

export function downloadBlob(data: BlobPart, filename: string, type = 'application/octet-stream') {
  const url = URL.createObjectURL(new Blob([data], {type}))
  const a = document.createElement('a')
  a.href = url
  a.download = filename
  a.click()
  setTimeout(() => URL.revokeObjectURL(url), 1000)
}

export function wsUrl(path: string): string {
  const proto = location.protocol === 'https:' ? 'wss' : 'ws'
  return `${proto}://${location.host}${path}`
}
