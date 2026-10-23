import { afterEach, describe, expect, it, vi } from 'vitest'
import { createHash, webcrypto } from 'node:crypto'
import { downloadFixedVersion, sha256Hex } from './workspace/plugin-center/registry-client'

afterEach(() => vi.unstubAllGlobals())
const bytes = text => new TextEncoder().encode(text)
const independent = input => createHash('sha256').update(input).digest('hex')

describe('SHA-256 on LAN HTTP and secure contexts', () => {
  it('hashes standard vectors without WebCrypto, including multi-block input', async () => {
    vi.stubGlobal('crypto', {})
    vi.stubGlobal('isSecureContext', false)
    for (const input of [bytes(''), bytes('abc'), new Uint8Array(1_000_000).fill(97)]) {
      expect(await sha256Hex(input)).toBe(independent(input))
    }
  })

  it('hashes only the selected view, preserving offsets and original bytes', async () => {
    vi.stubGlobal('crypto', undefined)
    const backing = new Uint8Array([9, 97, 98, 99, 8])
    expect(await sha256Hex(backing.subarray(1, 4))).toBe(independent(bytes('abc')))
    expect([...backing]).toEqual([9, 97, 98, 99, 8])
    expect(await sha256Hex(bytes('abc').buffer)).toBe(independent(bytes('abc')))
  })

  it('retains native hashing where available and does not conceal native failures', async () => {
    const digest = vi.fn((algorithm, input) => webcrypto.subtle.digest(algorithm, input))
    vi.stubGlobal('crypto', { subtle: { digest } })
    expect(await sha256Hex(bytes('abc'))).toBe(independent(bytes('abc')))
    expect(digest).toHaveBeenCalledWith('SHA-256', expect.any(ArrayBuffer))
    digest.mockRejectedValueOnce(new Error('native digest failed'))
    await expect(sha256Hex(bytes('abc'))).rejects.toThrow('native digest failed')
  })

  it('still rejects a mismatched official archive without WebCrypto', async () => {
    vi.stubGlobal('crypto', {})
    const input = bytes('not the expected archive')
    const fetchImpl = vi.fn(async () => ({ ok: true, headers: { get: () => null }, arrayBuffer: async () => input.buffer }))
    await expect(downloadFixedVersion({ id: 'demo', version: '1.0.0', source: 'official', download_url: '/archive', sha256: '0'.repeat(64) }, { fetchImpl }))
      .rejects.toMatchObject({ code: 'hash_mismatch' })
  })
})
