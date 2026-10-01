import {describe, expect, it} from 'vitest'
import {firstMismatch, formatBytes, hexdump, parseHexBytes, parseNumber} from './format'

describe('format', () => {
  it('parseNumber', () => {
    expect(parseNumber('0x1000')).toBe(4096)
    expect(parseNumber('64k')).toBe(65536)
    expect(parseNumber('4M')).toBe(4 * 1024 * 1024)
    expect(parseNumber('12')).toBe(12)
    expect(parseNumber('abc')).toBeUndefined()
    expect(parseNumber('')).toBeUndefined()
  })
  it('formatBytes', () => {
    expect(formatBytes(4 * 1024 * 1024)).toBe('4 MiB')
    expect(formatBytes(4096)).toBe('4 KiB')
    expect(formatBytes(100)).toBe('100 B')
  })
  it('hexdump', () => {
    expect(hexdump(Uint8Array.from([0x41, 0x00, 0xff]), 0x10)).toBe(`00000010  41 00 ff${' '.repeat(47 - 8)}  |A..|`)
  })
  it('parseHexBytes', () => {
    expect(parseHexBytes('de AD,0xbe ef')).toEqual(Uint8Array.from([0xde, 0xad, 0xbe, 0xef]))
    expect(parseHexBytes('abc')).toBeUndefined()
    expect(parseHexBytes('zz')).toBeUndefined()
  })
  it('firstMismatch', () => {
    expect(firstMismatch(Uint8Array.of(1, 2, 3), Uint8Array.of(1, 2, 3))).toBe(-1)
    expect(firstMismatch(Uint8Array.of(1, 2, 3), Uint8Array.of(1, 9, 3))).toBe(1)
    expect(firstMismatch(Uint8Array.of(1, 2), Uint8Array.of(1, 2, 3))).toBe(2)
  })
})
