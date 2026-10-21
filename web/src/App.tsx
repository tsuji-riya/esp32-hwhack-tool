import {Heading, Text} from '@primer/react'
import './app.css'
import {UartPanel} from './UartPanel'

export function App() {
  return (
    <div className="page">
      <Heading as="h1" style={{fontSize: 'var(--text-title-size-large)'}}>
        ESP32 HW Hack Tool
      </Heading>
      <Text style={{color: 'var(--fgColor-muted)'}}>Target UART terminal</Text>
      <div style={{marginTop: 16}}>
        <UartPanel />
      </div>
    </div>
  )
}
