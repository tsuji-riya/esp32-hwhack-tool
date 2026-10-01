import {describe, expect, it} from 'vitest'
import {FlashClient, type SocketLike} from './flashClient'

/** In-memory device speaking the same protocol as the firmware. */
class FakeDevice implements SocketLike {
  binaryType = 'blob'
  readyState = 1
  onmessage: SocketLike['onmessage'] = null
  onclose: SocketLike['onclose'] = null
  onerror: SocketLike['onerror'] = null
  mem = new Uint8Array(64 * 1024).fill(0xff)
  private writing: {addr: number; len: number; done: number} | null = null

  private out(v: unknown) {
    queueMicrotask(() => this.onmessage?.({data: JSON.stringify(v)} as MessageEvent))
  }
  close() {
    this.readyState = 3
  }
  send(data: string | Uint8Array<ArrayBuffer>) {
    if (typeof data !== 'string') {
      const d = data as Uint8Array
      const w = this.writing!
      for (let i = 0; i < d.length; i++) this.mem[w.addr + w.done + i] &= d[i]
      w.done += d.length
      this.out({type: 'progress', op: 'write', done: w.done, total: w.len})
      if (w.done === w.len) {
        this.writing = null
        this.out({type: 'done', op: 'write'})
      }
      return
    }
    const [cmd, ...a] = data.split(' ')
    const n = a.map(Number)
    switch (cmd) {
      case 'hello':
        return this.out({type: 'hello', devices: 2, maxHz: 40e6, defaultHz: 4e6, chunk: 1024})
      case 'probe':
        return this.out({type: 'probe', jedec: 'EF4016', vendor: 'Winbond', capacity: this.mem.length, addrBytes: 3, pageSize: 256, source: 'sfdp', sr: 0, erase: [{op: 32, size: 4096}]})
      case 'read':
        for (let off = 0; off < n[2]; off += 1000) {
          const part = this.mem.slice(n[1] + off, n[1] + Math.min(off + 1000, n[2]))
          queueMicrotask(() => this.onmessage?.({data: part.buffer} as MessageEvent))
        }
        return this.out({type: 'done', op: 'read'})
      case 'erase':
        this.mem.fill(0xff, n[1], n[1] + n[2])
        this.out({type: 'progress', op: 'erase', done: n[2], total: n[2]})
        return this.out({type: 'done', op: 'erase'})
      case 'write':
        this.writing = {addr: n[1], len: n[2], done: 0}
        return this.out({type: 'ready'})
      default:
        return this.out({type: 'error', msg: 'bad command'})
    }
  }
}

const connect = async () => {
  const dev = new FakeDevice()
  return {dev, client: await FlashClient.connect(() => dev)}
}

describe('FlashClient', () => {
  it('says hello and probes', async () => {
    const {client} = await connect()
    expect(client.hello.devices).toBe(2)
    expect((await client.probe(0, 4_000_000)).jedec).toBe('EF4016')
  })

  it('erase -> write -> read round trip with chunking and windowing', async () => {
    const {client, dev} = await connect()
    dev.mem.fill(0)
    await client.erase(0, 0, 8192)
    const data = Uint8Array.from({length: 5000}, (_, i) => (i * 13) & 0xff)
    const progress: number[] = []
    await client.write(0, 100, data, d => progress.push(d))
    expect(progress.at(-1)).toBe(5000)
    const back = await client.read(0, 100, 5000)
    expect(back).toEqual(data)
  })

  it('reports device errors', async () => {
    const {client, dev} = await connect()
    const orig = dev.send.bind(dev)
    dev.send = d => {
      if (typeof d === 'string' && d.startsWith('status')) queueMicrotask(() => dev.onmessage?.({data: '{"type":"error","msg":"nope"}'} as MessageEvent))
      else orig(d)
    }
    await expect(client.status(0)).rejects.toThrow('nope')
  })
})
