/** Client for the firmware's `/ws/flash` protocol (see `src/flash_ws.rs`). */

export interface EraseType {
  op: number
  size: number
}

export interface ProbeInfo {
  jedec: string
  vendor: string
  capacity: number
  addrBytes: number
  pageSize: number
  source: 'sfdp' | 'default' | 'manual'
  sr: number
  erase: EraseType[]
}

export interface Hello {
  devices: number
  maxHz: number
  defaultHz: number
  chunk: number
}

export type Progress = (done: number, total: number) => void

type Incoming = {kind: 'json'; value: Record<string, unknown>} | {kind: 'bin'; data: Uint8Array<ArrayBuffer>}

/** The subset of `WebSocket` used here (lets tests inject a fake). */
export interface SocketLike {
  binaryType: string
  readyState: number
  send(data: string | Uint8Array<ArrayBuffer>): void
  close(): void
  onmessage: ((ev: MessageEvent) => void) | null
  onclose: ((ev: CloseEvent) => void) | null
  onerror: ((ev: Event) => void) | null
}

export class DeviceError extends Error {}

const WINDOW = 2 // binary frames in flight while writing

export class FlashClient {
  private queue: Incoming[] = []
  private waiter: ((m: Incoming | Error) => void) | null = null
  private failure: Error | null = null
  private chain: Promise<unknown> = Promise.resolve()
  hello: Hello = {devices: 1, maxHz: 20_000_000, defaultHz: 4_000_000, chunk: 4096}

  private constructor(private ws: SocketLike) {
    ws.binaryType = 'arraybuffer'
    ws.onmessage = ev => {
      const d = ev.data
      this.push(typeof d === 'string' ? {kind: 'json', value: JSON.parse(d)} : {kind: 'bin', data: new Uint8Array(d as ArrayBuffer)})
    }
    const fail = () => this.fail(new Error('connection closed'))
    ws.onclose = fail
    ws.onerror = fail
  }

  static async connect(makeSocket: () => SocketLike): Promise<FlashClient> {
    const ws = makeSocket()
    const c = new FlashClient(ws)
    await new Promise<void>((resolve, reject) => {
      const t = setInterval(() => {
        if (ws.readyState === 1) {
          clearInterval(t)
          resolve()
        } else if (ws.readyState > 1) {
          clearInterval(t)
          reject(new Error('could not open WebSocket'))
        }
      }, 10)
    })
    c.hello = (await c.request('hello', 'hello')) as unknown as Hello
    return c
  }

  close() {
    this.ws.close()
  }

  private push(m: Incoming) {
    if (this.waiter) {
      const w = this.waiter
      this.waiter = null
      w(m)
    } else this.queue.push(m)
  }

  private fail(e: Error) {
    this.failure ??= e
    if (this.waiter) {
      const w = this.waiter
      this.waiter = null
      w(e)
    }
  }

  private next(): Promise<Incoming> {
    const m = this.queue.shift()
    if (m) return Promise.resolve(m)
    if (this.failure) return Promise.reject(this.failure)
    return new Promise((resolve, reject) => {
      this.waiter = r => (r instanceof Error ? reject(r) : resolve(r))
    })
  }

  /** Next JSON message; throws `DeviceError` for `{"type":"error"}`. */
  private async nextJson(): Promise<Record<string, unknown>> {
    const m = await this.next()
    if (m.kind !== 'json') throw new Error('unexpected binary frame')
    if (m.value.type === 'error') throw new DeviceError(String(m.value.msg))
    return m.value
  }

  /** Serialises operations: the device handles one command at a time. */
  private exclusive<T>(fn: () => Promise<T>): Promise<T> {
    const run = this.chain.then(fn, fn)
    this.chain = run.catch(() => undefined)
    return run
  }

  private async request(cmd: string, expect: string): Promise<Record<string, unknown>> {
    return this.exclusive(async () => {
      this.ws.send(cmd)
      const r = await this.nextJson()
      if (r.type !== expect) throw new Error(`unexpected reply ${String(r.type)}`)
      return r
    })
  }

  async probe(dev: number, hz: number): Promise<ProbeInfo> {
    return (await this.request(`probe ${dev} ${hz}`, 'probe')) as unknown as ProbeInfo
  }

  async setInfo(dev: number, capacity: number, addrBytes: number): Promise<ProbeInfo> {
    return (await this.request(`set ${dev} ${capacity} ${addrBytes}`, 'probe')) as unknown as ProbeInfo
  }

  async status(dev: number): Promise<number> {
    return (await this.request(`status ${dev}`, 'status')).sr as number
  }

  async unprotect(dev: number): Promise<void> {
    await this.request(`unprotect ${dev}`, 'done')
  }

  read(dev: number, addr: number, len: number, onProgress?: Progress): Promise<Uint8Array<ArrayBuffer>> {
    return this.exclusive(async () => {
      const out = new Uint8Array(len)
      let got = 0
      this.ws.send(`read ${dev} ${addr} ${len}`)
      for (;;) {
        const m = await this.next()
        if (m.kind === 'bin') {
          if (got + m.data.length > len) throw new Error('device sent more data than requested')
          out.set(m.data, got)
          got += m.data.length
          onProgress?.(got, len)
        } else if (m.value.type === 'error') {
          throw new DeviceError(String(m.value.msg))
        } else if (m.value.type === 'done') {
          if (got !== len) throw new Error(`short read: ${got}/${len}`)
          return out
        }
      }
    })
  }

  erase(dev: number, addr: number, len: number, onProgress?: Progress): Promise<void> {
    return this.exclusive(async () => {
      this.ws.send(`erase ${dev} ${addr} ${len}`)
      for (;;) {
        const r = await this.nextJson()
        if (r.type === 'progress') onProgress?.(r.done as number, r.total as number)
        else if (r.type === 'done') return
      }
    })
  }

  write(dev: number, addr: number, data: Uint8Array<ArrayBuffer>, onProgress?: Progress): Promise<void> {
    return this.exclusive(async () => {
      const len = data.length
      this.ws.send(`write ${dev} ${addr} ${len}`)
      const ready = await this.nextJson()
      if (ready.type !== 'ready') throw new Error('device did not accept write')

      const chunk = this.hello.chunk
      let sent = 0
      let acked = 0
      let inFlight = 0
      while (acked < len) {
        while (sent < len && inFlight < WINDOW) {
          const part = data.subarray(sent, Math.min(sent + chunk, len))
          this.ws.send(part)
          sent += part.length
          inFlight++
        }
        const r = await this.nextJson()
        if (r.type === 'progress') {
          acked = r.done as number
          inFlight--
          onProgress?.(acked, len)
        } else if (r.type === 'done') {
          return
        }
      }
      // consume the final `done`
      const done = await this.nextJson()
      if (done.type !== 'done') throw new Error('missing completion message')
    })
  }
}
