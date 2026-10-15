import {Banner, Button, FormControl, Select, TextInput} from '@primer/react'
import {FitAddon} from '@xterm/addon-fit'
import {Terminal} from '@xterm/xterm'
import '@xterm/xterm/css/xterm.css'
import {useCallback, useEffect, useRef, useState} from 'react'
import {downloadBlob, wsUrl} from './lib/format'

const BAUDS = [1200, 2400, 4800, 9600, 19200, 38400, 57600, 115200, 230400, 460800, 921600, 1500000]
const FORMATS = ['8N1', '8E1', '8O1', '8N2', '7E1', '7O1']
/** What the Enter key sends (xterm.js produces CR). */
const ENTER: Record<string, string> = {cr: '\r', lf: '\n', crlf: '\r\n'}
const MAX_LOG = 1024 * 1024
/** The device accepts WebSocket messages of at most 1 KiB, so long pastes are split. */
const SEND_CHUNK = 512
/** Keep-alive period; the device drops sessions idle for 20 s. */
const PING_MS = 5000
/** No message from the device (it answers every ping) for this long means it is gone. */
const DEAD_MS = 15000

const THEMES = {
  dark: {background: '#0d1117', foreground: '#e6edf3', cursor: '#e6edf3', selectionBackground: '#264f78'},
  light: {background: '#ffffff', foreground: '#1f2328', cursor: '#1f2328', selectionBackground: '#b6d6fd'},
}
const currentTheme = () => (window.matchMedia('(prefers-color-scheme: dark)').matches ? THEMES.dark : THEMES.light)

export function UartPanel() {
  const frameRef = useRef<HTMLDivElement>(null)
  const hostRef = useRef<HTMLDivElement>(null)
  const termRef = useRef<Terminal | null>(null)
  const wsRef = useRef<WebSocket | null>(null)
  const logRef = useRef<Uint8Array[]>([])
  const logSize = useRef(0)
  const enterRef = useRef('\r')

  const [connected, setConnected] = useState(false)
  const [error, setError] = useState<string | null>(null)
  const [baud, setBaud] = useState('115200')
  const [format, setFormat] = useState('8N1')
  const [enter, setEnter] = useState('cr')
  const [localEcho, setLocalEcho] = useState(false)
  const echoRef = useRef(false)
  useEffect(() => {
    enterRef.current = ENTER[enter]
  }, [enter])
  useEffect(() => {
    echoRef.current = localEcho
  }, [localEcho])

  // Terminal lifetime = component lifetime (the panel stays mounted across tab switches).
  useEffect(() => {
    const term = new Terminal({
      cursorBlink: true,
      convertEol: false,
      scrollback: 10000,
      fontFamily: 'ui-monospace, SFMono-Regular, Menlo, Consolas, monospace',
      fontSize: 13,
      theme: currentTheme(),
    })
    const fit = new FitAddon()
    term.loadAddon(fit)
    const paint = () => {
      const t = currentTheme()
      term.options.theme = t
      if (frameRef.current) frameRef.current.style.backgroundColor = t.background
    }
    paint()
    term.open(hostRef.current!)
    fit.fit()
    termRef.current = term

    const ro = new ResizeObserver(() => hostRef.current?.offsetParent && fit.fit())
    ro.observe(hostRef.current!)
    const mq = window.matchMedia('(prefers-color-scheme: dark)')
    mq.addEventListener('change', paint)

    // Keystrokes go straight to the target, like a serial console.
    const enc = new TextEncoder()
    term.onData(data => {
      const ws = wsRef.current
      if (!ws || ws.readyState !== WebSocket.OPEN) return
      const out = enc.encode(data.replace(/\r/g, enterRef.current))
      for (let i = 0; i < out.length; i += SEND_CHUNK) ws.send(out.subarray(i, i + SEND_CHUNK))
      if (echoRef.current) term.write(data === '\r' ? '\r\n' : data)
    })

    return () => {
      ro.disconnect()
      mq.removeEventListener('change', paint)
      wsRef.current?.close()
      term.dispose()
    }
  }, [])

  const connect = useCallback(() => {
    if (wsRef.current) return // a previous click is still connecting
    setError(null)
    const term = termRef.current!
    const ws = new WebSocket(wsUrl('/ws/uart'))
    ws.binaryType = 'arraybuffer'
    let lastRx = Date.now()
    let heartbeat: ReturnType<typeof setInterval> | undefined
    const closed = () => {
      clearInterval(heartbeat)
      setConnected(false)
      wsRef.current = null
      term.write('\r\n\x1b[90m--- disconnected ---\x1b[0m\r\n')
    }
    ws.onopen = () => {
      setConnected(true)
      ws.send(`baud ${baud} ${format}`)
      term.write(`\x1b[90m--- connected (${baud} ${format}) ---\x1b[0m\r\n`)
      term.focus()
      lastRx = Date.now()
      heartbeat = setInterval(() => {
        if (Date.now() - lastRx < DEAD_MS) return ws.send('ping')
        // A dead TCP peer can take minutes to report onclose, so give up right away.
        setError('Device not responding (rebooted or Wi-Fi lost?)')
        ws.onclose = null
        ws.close()
        closed()
      }, PING_MS)
    }
    ws.onmessage = ev => {
      lastRx = Date.now()
      if (typeof ev.data === 'string') {
        if (ev.data.startsWith('error') || ev.data.startsWith('busy')) setError(ev.data)
        else if (ev.data.startsWith('warn')) term.write(`\r\n\x1b[33m--- ${ev.data} ---\x1b[0m\r\n`)
        return
      }
      const data = new Uint8Array(ev.data as ArrayBuffer)
      term.write(data) // xterm decodes UTF-8 and ANSI escapes itself
      logRef.current.push(data)
      logSize.current += data.length
      while (logSize.current > MAX_LOG && logRef.current.length > 1) logSize.current -= logRef.current.shift()!.length
    }
    ws.onclose = closed
    ws.onerror = () => setError('WebSocket error - is the UART bridge already in use?')
    wsRef.current = ws
  }, [baud, format])

  const applyConfig = (b: string, f: string) => {
    wsRef.current?.send(`baud ${b} ${f}`)
    termRef.current?.write(`\x1b[90m--- ${b} ${f} ---\x1b[0m\r\n`)
  }

  return (
    <div className="panel">
      {error && <Banner title="UART" description={error} variant="critical" onDismiss={() => setError(null)} />}
      <div className="card">
        <div className="row">
          <FormControl>
            <FormControl.Label>Baud rate</FormControl.Label>
            <TextInput
              value={baud}
              list="bauds"
              inputMode="numeric"
              onChange={e => setBaud(e.target.value)}
              onBlur={() => connected && applyConfig(baud, format)}
            />
            <datalist id="bauds">
              {BAUDS.map(b => (
                <option key={b} value={b} />
              ))}
            </datalist>
          </FormControl>
          <FormControl>
            <FormControl.Label>Format</FormControl.Label>
            <Select
              value={format}
              onChange={e => {
                setFormat(e.target.value)
                if (connected) applyConfig(baud, e.target.value)
              }}
            >
              {FORMATS.map(f => (
                <Select.Option key={f} value={f}>
                  {f}
                </Select.Option>
              ))}
            </Select>
          </FormControl>
          <FormControl>
            <FormControl.Label>Enter sends</FormControl.Label>
            <Select value={enter} onChange={e => setEnter(e.target.value)}>
              <Select.Option value="cr">CR</Select.Option>
              <Select.Option value="lf">LF</Select.Option>
              <Select.Option value="crlf">CR+LF</Select.Option>
            </Select>
          </FormControl>
          <Button aria-pressed={localEcho} onClick={() => setLocalEcho(v => !v)}>
            Local echo: {localEcho ? 'on' : 'off'}
          </Button>
          <Button variant={connected ? 'danger' : 'primary'} onClick={() => (connected ? wsRef.current?.close() : connect())}>
            {connected ? 'Disconnect' : 'Connect'}
          </Button>
          <Button onClick={() => termRef.current?.clear()}>Clear</Button>
          <Button onClick={() => downloadBlob(new Blob(logRef.current as BlobPart[]), 'uart-log.bin')}>Save log</Button>
        </div>
      </div>
      <div className="console" ref={frameRef}>
        <div className="console-host" ref={hostRef} aria-label="UART console" />
      </div>
    </div>
  )
}
