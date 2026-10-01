// Fake ESP32 for UI development without hardware:
//   npm run mock            (terminal 1)
//   DEVICE=localhost:8787 npm run dev   (terminal 2)
// Speaks the same WebSocket protocol as src/flash_ws.rs and src/uart_bridge.rs.
import {WebSocketServer} from 'ws'

const PORT = Number(process.env.PORT ?? 8787)
const CAPACITY = 4 * 1024 * 1024
const mem = new Uint8Array(CAPACITY).fill(0xff)
for (let i = 0; i < 64 * 1024; i++) mem[i] = (i * 31) & 0xff
let baud = 115200

const wss = new WebSocketServer({port: PORT})
wss.on('connection', (ws, req) => {
  const send = v => ws.send(JSON.stringify(v))
  if (req.url === '/ws/uart') {
    ws.send(Buffer.from('ESP32 mock target ready\r\n'))
    const t = setInterval(() => ws.send(Buffer.from(`tick ${Date.now()}\r\n`)), 2000)
    ws.on('close', () => clearInterval(t))
    ws.on('message', (data, isBinary) => {
      if (isBinary) ws.send(data)
      else {
        const m = /^baud (\d+)/.exec(data.toString())
        if (m) baud = Number(m[1])
        ws.send('ok')
      }
    })
    return
  }
  let write = null
  // The real device handles one message at a time; keep that ordering here too.
  let chain = Promise.resolve()
  ws.on('message', (data, isBinary) => {
    chain = chain.then(() => handle(data, isBinary))
  })
  const handle = async (data, isBinary) => {
    if (isBinary) {
      if (!write) return
      for (let i = 0; i < data.length; i++) mem[write.addr + write.done + i] &= data[i]
      write.done += data.length
      await new Promise(r => setTimeout(r, 5))
      send({type: 'progress', op: 'write', done: write.done, total: write.len})
      if (write.done === write.len) {
        write = null
        send({type: 'done', op: 'write'})
      }
      return
    }
    const [cmd, , a, l] = data.toString().split(' ')
    const addr = Number(a), len = Number(l)
    switch (cmd) {
      case 'hello':
        return send({type: 'hello', devices: 3, maxHz: 40e6, defaultHz: 4e6, chunk: 4096})
      case 'probe':
      case 'set':
        return send({type: 'probe', jedec: 'EF4016', vendor: 'Winbond', capacity: CAPACITY, addrBytes: 3, pageSize: 256, source: 'sfdp', sr: 0,
          erase: [{op: 32, size: 4096}, {op: 82, size: 32768}, {op: 216, size: 65536}]})
      case 'status':
        return send({type: 'status', sr: 0})
      case 'unprotect':
        return send({type: 'done', op: 'unprotect'})
      case 'read':
        for (let off = 0; off < len; off += 4096) ws.send(mem.slice(addr + off, addr + Math.min(off + 4096, len)))
        return send({type: 'done', op: 'read'})
      case 'erase': {
        for (let done = 0; done < len; done += 65536) {
          mem.fill(0xff, addr + done, addr + Math.min(done + 65536, len))
          await new Promise(r => setTimeout(r, 20))
          send({type: 'progress', op: 'erase', done: Math.min(done + 65536, len), total: len})
        }
        return send({type: 'done', op: 'erase'})
      }
      case 'write':
        write = {addr, len, done: 0}
        return send({type: 'ready'})
      default:
        return send({type: 'error', msg: 'bad command'})
    }
  }
})
console.log(`mock device on ws://localhost:${PORT}`)
