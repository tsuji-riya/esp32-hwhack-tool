// Fake ESP32 for UI development without hardware:
//   npm run mock            (terminal 1)
//   DEVICE=localhost:8787 npm run dev   (terminal 2)
// Speaks the same WebSocket protocol as src/uart_bridge.rs.
import {WebSocketServer} from 'ws'

const PORT = Number(process.env.PORT ?? 8787)
let baud = 115200

const wss = new WebSocketServer({port: PORT})
wss.on('connection', (ws, req) => {
  if (req.url === '/ws/uart') {
    ws.send(Buffer.from('ESP32 mock target ready\r\n'))
    const t = setInterval(() => ws.send(Buffer.from(`tick ${Date.now()}\r\n`)), 2000)
    ws.on('close', () => clearInterval(t))
    ws.on('message', (data, isBinary) => {
      if (isBinary) ws.send(data)
      else {
        if (data.toString() === 'ping') return ws.send('pong')
        const m = /^baud (\d+)/.exec(data.toString())
        if (m) baud = Number(m[1])
        ws.send('ok')
      }
    })
    return
  }
  ws.close()
})
console.log(`mock device on ws://localhost:${PORT}`)
