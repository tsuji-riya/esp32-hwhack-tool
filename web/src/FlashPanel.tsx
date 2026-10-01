import {Banner, Button, Checkbox, FormControl, Label, ProgressBar, Select, TextInput} from '@primer/react'
import {useEffect, useRef, useState} from 'react'
import {FlashClient, type ProbeInfo} from './lib/flashClient'
import {downloadBlob, firstMismatch, formatBytes, hex, parseNumber, wsUrl} from './lib/format'

const SPEEDS = [1, 2, 4, 8, 10, 20, 40].map(m => m * 1_000_000)

interface Job {
  label: string
  done: number
  total: number
}

export function FlashPanel() {
  const clientRef = useRef<FlashClient | null>(null)
  const [connected, setConnected] = useState(false)
  const [devices, setDevices] = useState(1)
  const [dev, setDev] = useState(0)
  const [hz, setHz] = useState(4_000_000)
  const [info, setInfo] = useState<ProbeInfo | null>(null)
  const [job, setJob] = useState<Job | null>(null)
  const [error, setError] = useState<string | null>(null)
  const [log, setLog] = useState<string[]>([])

  const [addr, setAddr] = useState('0')
  const [len, setLen] = useState('')
  const [file, setFile] = useState<File | null>(null)
  const [eraseFirst, setEraseFirst] = useState(true)
  const [verify, setVerify] = useState(true)
  const [manualCap, setManualCap] = useState('')
  const [manualAddr, setManualAddr] = useState('3')

  const note = (s: string) => setLog(l => [...l.slice(-199), `${new Date().toLocaleTimeString()}  ${s}`])
  useEffect(() => () => clientRef.current?.close(), [])

  const connect = async () => {
    setError(null)
    try {
      const c = await FlashClient.connect(() => new WebSocket(wsUrl('/ws/flash')))
      clientRef.current = c
      setDevices(c.hello.devices)
      setHz(c.hello.defaultHz)
      setConnected(true)
      note('connected')
    } catch (e) {
      setError(String(e))
    }
  }

  const disconnect = () => {
    clientRef.current?.close()
    clientRef.current = null
    setConnected(false)
    setInfo(null)
  }

  /** Runs a device operation; reports errors and clears the progress bar. */
  const run = async (label: string, fn: (c: FlashClient) => Promise<void>) => {
    const c = clientRef.current
    if (!c || job) return
    setError(null)
    setJob({label, done: 0, total: 1})
    const t0 = performance.now()
    try {
      await fn(c)
      note(`${label}: ok (${((performance.now() - t0) / 1000).toFixed(1)} s)`)
    } catch (e) {
      const msg = e instanceof Error ? e.message : String(e)
      setError(`${label}: ${msg}`)
      note(`${label}: FAILED - ${msg}`)
      if (msg === 'connection closed') disconnect()
    } finally {
      setJob(null)
    }
  }
  const progress = (label: string) => (done: number, total: number) => setJob({label, done, total})

  const probe = () =>
    run('Probe', async c => {
      const i = await c.probe(dev, hz)
      setInfo(i)
      setLen(String(i.capacity))
      note(`${i.vendor} ${i.jedec}, ${formatBytes(i.capacity)}, ${i.source}`)
    })

  const applyManual = () =>
    run('Set parameters', async c => {
      const cap = parseNumber(manualCap)
      if (!cap) throw new Error('capacity is not a number')
      const i = await c.setInfo(dev, cap, Number(manualAddr))
      setInfo(i)
      setLen(String(i.capacity))
    })

  const range = () => {
    const a = parseNumber(addr)
    const l = parseNumber(len)
    if (a === undefined || l === undefined || l <= 0) {
      setError('Address / length are not valid numbers')
      return null
    }
    return {a, l}
  }

  const read = () => {
    const r = range()
    if (!r) return
    void run('Read', async c => {
      const data = await c.read(dev, r.a, r.l, progress('Reading'))
      downloadBlob(data, `flash-${info?.jedec ?? 'dump'}-${hex(r.a, 8)}-${r.l}.bin`)
    })
  }

  const minErase = info ? Math.min(...info.erase.map(e => e.size)) : 4096
  const align = (a: number, l: number) => {
    const start = Math.floor(a / minErase) * minErase
    const end = Math.ceil((a + l) / minErase) * minErase
    return {start, size: end - start}
  }

  const erase = (whole: boolean) => {
    if (!info) return
    const r = whole ? {a: 0, l: info.capacity} : range()
    if (!r) return
    const e = align(r.a, r.l)
    if (!confirm(`Erase ${formatBytes(e.size)} at 0x${hex(e.start, 8)}? This cannot be undone.`)) return
    void run('Erase', async c => c.erase(dev, e.start, e.size, progress('Erasing')))
  }

  const program = () => {
    const a = parseNumber(addr)
    if (!info || !file || a === undefined) return setError('Choose a file and a valid address')
    if (a + file.size > info.capacity) return setError(`File does not fit: ${formatBytes(file.size)} at 0x${hex(a, 8)} exceeds ${formatBytes(info.capacity)}`)
    const e = align(a, file.size)
    if (!confirm(`Write ${file.name} (${formatBytes(file.size)}) at 0x${hex(a, 8)}?${eraseFirst ? `\nSectors 0x${hex(e.start, 8)}..0x${hex(e.start + e.size, 8)} are erased first.` : ''}`)) return
    void run('Program', async c => {
      const data = new Uint8Array(await file.arrayBuffer())
      if (eraseFirst) await c.erase(dev, e.start, e.size, progress('Erasing'))
      await c.write(dev, a, data, progress('Writing'))
      if (verify) {
        const back = await c.read(dev, a, data.length, progress('Verifying'))
        const bad = firstMismatch(data, back)
        if (bad >= 0) throw new Error(`verify failed at 0x${hex(a + bad, 8)}`)
        note('verify: ok')
      }
    })
  }

  const busy = job !== null
  const ready = connected && !busy
  const pct = job ? Math.round((job.done / Math.max(job.total, 1)) * 100) : 0

  return (
    <div className="panel">
      {error && <Banner title="SPI flash" description={error} variant="critical" onDismiss={() => setError(null)} />}

      <div className="card">
        <h3>Connection &amp; chip</h3>
        <div className="row">
          <Button variant={connected ? 'danger' : 'primary'} onClick={connected ? disconnect : connect} disabled={busy}>
            {connected ? 'Disconnect' : 'Connect'}
          </Button>
          <FormControl>
            <FormControl.Label>Chip select</FormControl.Label>
            <Select value={String(dev)} onChange={e => setDev(Number(e.target.value))} disabled={!ready}>
              {Array.from({length: devices}, (_, i) => (
                <Select.Option key={i} value={String(i)}>
                  CS{i}
                </Select.Option>
              ))}
            </Select>
          </FormControl>
          <FormControl>
            <FormControl.Label>SPI clock</FormControl.Label>
            <Select value={String(hz)} onChange={e => setHz(Number(e.target.value))} disabled={!ready}>
              {SPEEDS.map(s => (
                <Select.Option key={s} value={String(s)}>
                  {s / 1_000_000} MHz
                </Select.Option>
              ))}
            </Select>
          </FormControl>
          <Button variant="primary" disabled={!ready} onClick={probe}>
            Detect chip
          </Button>
        </div>

        {info && (
          <dl className="kv" style={{marginTop: 16}}>
            <dt>JEDEC ID</dt>
            <dd>
              {info.jedec} ({info.vendor})
            </dd>
            <dt>Capacity</dt>
            <dd>{formatBytes(info.capacity)}</dd>
            <dt>Address bytes</dt>
            <dd>{info.addrBytes}</dd>
            <dt>Page size</dt>
            <dd>{info.pageSize} B</dd>
            <dt>Erase sizes</dt>
            <dd>{info.erase.map(e => `${formatBytes(e.size)} (0x${hex(e.op)})`).join(', ')}</dd>
            <dt>Parameters from</dt>
            <dd>
              <Label variant={info.source === 'sfdp' ? 'success' : 'attention'}>{info.source === 'sfdp' ? 'SFDP (auto)' : info.source}</Label>
            </dd>
            <dt>Status register</dt>
            <dd>0x{hex(info.sr)}</dd>
          </dl>
        )}

        <details style={{marginTop: 16}}>
          <summary>Manual parameters (chips without SFDP)</summary>
          <div className="row" style={{marginTop: 8}}>
            <FormControl>
              <FormControl.Label>Capacity (bytes, 0x…, 4M)</FormControl.Label>
              <TextInput value={manualCap} onChange={e => setManualCap(e.target.value)} placeholder="4M" />
            </FormControl>
            <FormControl>
              <FormControl.Label>Address bytes</FormControl.Label>
              <Select value={manualAddr} onChange={e => setManualAddr(e.target.value)}>
                <Select.Option value="3">3</Select.Option>
                <Select.Option value="4">4</Select.Option>
              </Select>
            </FormControl>
            <Button disabled={!ready} onClick={applyManual}>
              Apply
            </Button>
            <Button disabled={!ready} onClick={() => run('Clear block protect', c => c.unprotect(dev))}>
              Clear block protect
            </Button>
          </div>
        </details>
      </div>

      <div className="card">
        <h3>Range</h3>
        <div className="row">
          <FormControl>
            <FormControl.Label>Address</FormControl.Label>
            <TextInput monospace value={addr} onChange={e => setAddr(e.target.value)} />
          </FormControl>
          <FormControl>
            <FormControl.Label>Length</FormControl.Label>
            <TextInput monospace value={len} onChange={e => setLen(e.target.value)} />
          </FormControl>
          <Button disabled={!ready || !info} onClick={read}>
            Read → .bin
          </Button>
          <Button variant="danger" disabled={!ready || !info} onClick={() => erase(false)}>
            Erase range
          </Button>
          <Button variant="danger" disabled={!ready || !info} onClick={() => erase(true)}>
            Erase whole chip
          </Button>
        </div>
      </div>

      <div className="card">
        <h3>Program</h3>
        <div className="row">
          <FormControl>
            <FormControl.Label>Image file</FormControl.Label>
            <input type="file" onChange={e => setFile(e.target.files?.[0] ?? null)} disabled={busy} />
            {file && <FormControl.Caption>{formatBytes(file.size)}</FormControl.Caption>}
          </FormControl>
          <FormControl>
            <Checkbox checked={eraseFirst} onChange={e => setEraseFirst(e.target.checked)} />
            <FormControl.Label>Erase first</FormControl.Label>
          </FormControl>
          <FormControl>
            <Checkbox checked={verify} onChange={e => setVerify(e.target.checked)} />
            <FormControl.Label>Verify</FormControl.Label>
          </FormControl>
          <Button variant="primary" disabled={!ready || !info || !file} onClick={program}>
            Program at address above
          </Button>
        </div>
      </div>

      {job && (
        <div role="status" aria-live="polite">
          <div style={{marginBottom: 4}}>
            {job.label}… {job.total > 1 ? `${pct}% (${formatBytes(job.done)} / ${formatBytes(job.total)})` : ''}
          </div>
          <ProgressBar progress={pct} aria-label={job.label} />
        </div>
      )}

      <pre className="log" aria-label="Log">
        {log.join('\n')}
      </pre>
    </div>
  )
}
