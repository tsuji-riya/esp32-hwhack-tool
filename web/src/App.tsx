import {Heading, Text, UnderlineNav} from '@primer/react'
import {CpuIcon, TerminalIcon} from '@primer/octicons-react'
import {useState} from 'react'
import './app.css'
import {FlashPanel} from './FlashPanel'
import {UartPanel} from './UartPanel'

type Tab = 'uart' | 'flash'

export function App() {
  const [tab, setTab] = useState<Tab>('uart')
  return (
    <div className="page">
      <Heading as="h1" style={{fontSize: 'var(--text-title-size-large)'}}>
        ESP32 HW Hack Tool
      </Heading>
      <Text style={{color: 'var(--fgColor-muted)'}}>Target UART terminal and SPI flash programmer</Text>
      <div style={{marginTop: 16}}>
      <UnderlineNav aria-label="Tools">
        <UnderlineNav.Item icon={TerminalIcon} aria-current={tab === 'uart' ? 'page' : undefined} onSelect={e => (e.preventDefault(), setTab('uart'))}>
          UART
        </UnderlineNav.Item>
        <UnderlineNav.Item icon={CpuIcon} aria-current={tab === 'flash' ? 'page' : undefined} onSelect={e => (e.preventDefault(), setTab('flash'))}>
          SPI Flash
        </UnderlineNav.Item>
      </UnderlineNav>
      </div>
      {/* Both panels stay mounted so a UART session survives tab switches. */}
      <div hidden={tab !== 'uart'}>
        <UartPanel />
      </div>
      <div hidden={tab !== 'flash'}>
        <FlashPanel />
      </div>
    </div>
  )
}
