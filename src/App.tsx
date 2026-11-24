import { useEffect, useId, useLayoutEffect, useRef, useState, type FormEvent, type ReactNode } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import {
  ArrowDownLeft, ArrowRight, ArrowUpRight, AudioLines, ChevronDown, ChevronRight, Download,
  Check, CircleAlert, EthernetPort, Gauge, Headphones, Info, Laptop, Mic, Monitor, Moon, Palette, Plus, RefreshCw,
  Router, ShieldCheck, SlidersHorizontal, Speaker, Sun, Volume2, VolumeX, X,
} from 'lucide-react'
import { native, useWave, type Connection, type Peer, type Settings } from './state'
import { languages, translate, localizeMessage, type Language, type TextKey } from './i18n'
import mark from './mark.svg'

const navigation = ['mixer', 'settings'] as const

const themes = [
  { id: 'system', icon: Monitor },
  { id: 'light', icon: Sun },
  { id: 'dark', icon: Moon },
] as const

const accents = ['mint', 'lime', 'blue', 'violet', 'coral', 'amber'] as const

function useText() {
  const language = useWave(state => state.settings.language)
  return {
    language,
    t: (key: TextKey, values?: Record<string, string>) => translate(language, key, values),
    message: (value: string) => localizeMessage(language, value),
  }
}

export function App() {
  const { t, language, message } = useText()
  const updateVersion = useWave(state => state.updateVersion)
  const section = useWave(state => state.section)
  const navigate = useWave(state => state.navigate)
  const connection = useWave(state => state.connection)
  const theme = useWave(state => state.settings.theme)
  const accent = useWave(state => state.settings.accent)
  const error = useWave(state => state.error)
  const clearError = useWave(state => state.clearError)
  const [manual, openManual] = useState(false)

  useLayoutEffect(() => { document.documentElement.lang = language }, [language])

  useLayoutEffect(() => {
    const preference = window.matchMedia('(prefers-color-scheme: dark)')
    const applyTheme = () => {
      document.documentElement.dataset.theme = theme === 'system' ? (preference.matches ? 'dark' : 'light') : theme
    }
    applyTheme()
    document.documentElement.dataset.accent = accent
    preference.addEventListener('change', applyTheme)
    return () => preference.removeEventListener('change', applyTheme)
  }, [theme, accent])

  useEffect(() => {
    if (native) void getCurrentWindow().setTheme(theme === 'system' ? null : theme).catch(console.error)
  }, [theme])

  useEffect(() => {
    if (!error) return
    const timer = window.setTimeout(clearError, 6500)
    return () => window.clearTimeout(timer)
  }, [error, clearError])

  useEffect(() => {
    if (connection.phase === 'pairing') openManual(false)
  }, [connection.phase])

  return (
    <div className="app-shell">
      <header className="app-header">
        <button className="brand" onClick={() => navigate('mixer')} aria-label={`StreamSoundCast · ${t('mixer')}`}>
          <img className="brand-icon" src={mark} alt="" width={30} height={30} />
          <span className="wordmark">StreamSoundCast</span>
        </button>
        <nav className="nav" aria-label={t('sections')}>
          {navigation.map(id => (
            <button key={id} aria-current={section === id ? 'page' : undefined} className="nav-link"
              onClick={() => navigate(id)}>
              {t(id)}
              {id === 'settings' && updateVersion && <span className="update-dot" role="img"
                aria-label={t('updateAvailable', { version: updateVersion })} title={t('updateAvailable', { version: updateVersion })} />}
            </button>
          ))}
        </nav>
        <ConnectionMenu key={section} openManual={() => openManual(true)} />
      </header>
      <main className={section === 'mixer' ? 'mixer-page' : 'settings-page'} aria-label={t(section)}>
        {connection.message && <div className="notice" role="status"><CircleAlert size={17} />{message(connection.message)}</div>}
        {section === 'mixer' && <Mixer />}
        {section === 'settings' && <SettingsPage />}
      </main>
      {manual && connection.phase !== 'pairing' && <ManualDialog close={() => openManual(false)} />}
      {connection.phase === 'pairing' && connection.code && <PairingDialog connection={connection} />}
      {error && <div className="toast" role="alert"><CircleAlert size={20} /><span>{message(error)}</span><button className="icon-button" onClick={clearError} aria-label={t('close')}><X size={16} /></button></div>}
    </div>
  )
}

function ConnectionMenu({ openManual }: { openManual: () => void }) {
  const { t } = useText()
  const { phase, peer } = useWave(state => state.connection)
  const [open, setOpen] = useState(false)
  const root = useRef<HTMLDivElement>(null)
  const trigger = useRef<HTMLButtonElement>(null)
  const panelId = useId()
  const label = phase === 'connected' && peer ? peer.name : t(phase === 'idle' ? 'connect' : phase)

  useEffect(() => {
    if (!open) return
    const closeOutside = (event: PointerEvent) => {
      if (event.target instanceof Node && !root.current?.contains(event.target)) setOpen(false)
    }
    const escape = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      setOpen(false)
      trigger.current?.focus()
    }
    document.addEventListener('pointerdown', closeOutside)
    document.addEventListener('keydown', escape)
    return () => {
      document.removeEventListener('pointerdown', closeOutside)
      document.removeEventListener('keydown', escape)
    }
  }, [open])

  const pairing = phase === 'pairing'
  useEffect(() => {
    if (pairing) setOpen(false)
  }, [pairing])

  return <div className="connection-menu" ref={root} onBlur={event => {
    if (event.relatedTarget && !event.currentTarget.contains(event.relatedTarget)) setOpen(false)
  }}>
    <button type="button" className="connection-trigger" ref={trigger} aria-expanded={open && !pairing}
      aria-controls={open && !pairing ? panelId : undefined}
      title={phase === 'connected' ? `${label} · ${t(phase)}` : label}
      disabled={pairing} onClick={() => setOpen(!open)}>
      <span className={`status-dot ${phase === 'connected' ? 'green' : phase !== 'idle' ? 'gold' : ''}`} />
      <span>{label}</span><ChevronDown size={14} aria-hidden="true" />
    </button>
    {open && !pairing && <PeerPanel id={panelId} openManual={() => {
      setOpen(false)
      trigger.current?.focus()
      openManual()
    }} />}
  </div>
}

function Mixer() {
  const { t } = useText()
  return (
    <div className="mixer">
      <div className="panel console">
        <section className="console-group" aria-labelledby="console-send">
          <h2 className="group-heading" id="console-send"><ArrowUpRight size={16} />{t('sendGroup')}</h2>
          <div className="channel-grid"><ChannelCard index={0} /><ChannelCard index={1} /></div>
        </section>
        <section className="console-group" aria-labelledby="console-receive">
          <h2 className="group-heading" id="console-receive"><ArrowDownLeft size={16} />{t('receiveGroup')}</h2>
          <div className="channel-grid"><ChannelCard index={2} /><ChannelCard index={3} /></div>
        </section>
      </div>
      <LinkQuality />
    </div>
  )
}

const lanes = [
  { title: 'computerAudio', icon: Monitor, incoming: false },
  { title: 'microphone', icon: Mic, incoming: false },
  { title: 'computerAudio', icon: Speaker, incoming: true },
  { title: 'microphone', icon: Headphones, incoming: true },
] as const

function ChannelCard({ index }: { index: number }) {
  const { t, message: translateMessage } = useText()
  const { title: titleKey, icon: Icon, incoming } = lanes[index]
  const title = t(titleKey)
  const channel = useWave(state => state.settings.channels[index])
  const source = useWave(state => state.settings.source)
  const application = useWave(state => state.settings.application)
  const devices = useWave(state => state.devices)
  const configuredId = useWave(state => [null, state.settings.microphone, state.settings.computerOutput, state.settings.microphoneOutput][index])
  const activeDevice = useWave(state => state.connection.channelDevices[index])
  const enabledLevel = useWave(state => state.connection.levels[index])
  const message = useWave(state => state.connection.channelMessages[index])
  const updateSettings = useWave(state => state.updateSettings)
  const navigate = useWave(state => state.navigate)
  const [value, setValue] = useState(channel.volume)
  useEffect(() => { setValue(channel.volume) }, [channel.volume])
  const update = (patch: Partial<typeof channel>) => {
    void updateSettings(settings => ({ ...settings, channels: settings.channels.map((item, i) => i === index ? { ...item, ...patch } : item) }))
  }
  const commit = () => { if (value !== channel.volume) update({ volume: value }) }
  const level = channel.enabled && value > 0 ? enabledLevel : 0
  const lit = Math.max(0, Math.min(18, Math.ceil((20 * Math.log10(level) + 36) / 2)))
  const route = t(incoming ? 'incoming' : 'outgoing')
  const endpoints = index === 1 ? devices.microphones : devices.outputs
  const defaultId = index === 1 ? devices.defaultMicrophone : devices.defaultOutput
  const available = endpoints.find(device => device.id === (configuredId || defaultId))
  const activeName = activeDevice?.configuredId === configuredId ? activeDevice?.name : undefined
  const channelOptions = index === 0
    ? source === 'system' ? t('systemAudio') : application?.name ?? t('selectApp')
    : activeName ?? available?.name ?? (configuredId ? t('unavailableDevice') : t('windowsDefault'))

  return (
    <article className={channel.enabled ? 'channel' : 'channel muted'}>
      <div className="channel-heading">
        <span className="channel-icon"><Icon size={17} strokeWidth={1.8} aria-hidden="true" /></span>
        <div>
          <h3>{title}</h3>
          <button className="channel-source" onClick={() => navigate('settings')} title={channelOptions}>
            <span>{channelOptions}</span><ChevronRight size={12} aria-hidden="true" />
          </button>
        </div>
      </div>
      <div className="volume">
        <input type="range" min="0" max="100" value={value} aria-label={`${t('volume')} · ${route} · ${title}`}
          onChange={event => setValue(event.target.valueAsNumber)} onPointerUp={commit} onKeyUp={commit} onBlur={commit} />
        <div className="meter" aria-hidden="true">
          {Array.from({ length: 18 }, (_, i) => <span key={i} className={i < lit ? 'lit' : undefined} />)}
        </div>
        <span className="meter-scale" aria-hidden="true"><span>0</span><span>−18</span><span>−36</span></span>
        <output className="volume-value" aria-live="off">{value}%</output>
      </div>
      <div className="channel-controls">
        <button className="icon-button speaker-button" aria-label={`${value > 0 ? t('muteVolume') : t('restoreVolume')} · ${route} · ${title}`}
          onClick={() => update({ volume: value > 0 ? 0 : 100 })}>
          {value === 0 ? <VolumeX size={16} /> : <Volume2 size={16} />}
        </button>
        <Toggle enabled={channel.enabled} onChange={enabled => update({ enabled })} label={`${route} · ${title}`}
          text={t(channel.enabled ? 'channelOn' : 'channelOff')} />
      </div>
      {message && <p className="channel-message"><CircleAlert size={13} /> {translateMessage(message)}</p>}
    </article>
  )
}

function Toggle({ enabled, onChange, label, text }: { enabled: boolean; onChange: (on: boolean) => void; label: string; text?: string }) {
  return <button type="button" className="switch" role="switch" aria-checked={enabled} aria-label={label} onClick={() => onChange(!enabled)}>
    <span className="switch-track" aria-hidden="true" />{text && <span aria-hidden="true">{text}</span>}
  </button>
}

function PeerPanel({ id, openManual }: { id: string, openManual: () => void }) {
  const { t } = useText()
  const { peers, connection } = useWave()
  const scan = useWave(state => state.scan)
  const discovery = useWave(state => state.settings.discovery)
  const [scanning, setScanning] = useState(false)
  const timer = useRef<number | null>(null)
  useEffect(() => () => { if (timer.current !== null) window.clearTimeout(timer.current) }, [])
  const refresh = () => {
    setScanning(true)
    void scan()
    timer.current = window.setTimeout(() => setScanning(false), 1000)
  }
  const current = connection.peer
  const others = peers.filter(peer => peer.id !== current?.id && peer.ip !== current?.ip)
  return (
    <section className="panel peers-panel" id={id} aria-labelledby="peers-title">
      <div className="panel-heading">
        <span className="panel-icon"><Router size={19} /></span>
        <h2 id="peers-title">{t('devices')}</h2>
        <button className={'icon-button ' + (scanning ? 'spinning' : '')} title={t('refresh')} aria-label={t('refresh')}
          disabled={scanning || !discovery} onClick={refresh}><RefreshCw size={17} /></button>
      </div>
      {(current || others.length > 0) ? (
        <div className="peer-list">
          {current && <PeerRow peer={current} current />}
          {others.map(peer => <PeerRow key={peer.id} peer={peer} />)}
        </div>
      ) : <div className="peer-empty">
        <span className="empty-icon"><Laptop size={24} strokeWidth={1.55} /></span>
        <strong>{scanning ? t('scanning') : discovery ? t('noDevices') : t('discoveryOff')}</strong>
        <p>{discovery ? t('openOther') : t('discoveryHint')}</p>
      </div>}
      <button className="button secondary manual-button" onClick={openManual}><Plus size={17} /> {t('connectIP')}</button>
    </section>
  )
}

function PeerRow({ peer, current = false }: { peer: Peer; current?: boolean }) {
  const { t } = useText()
  const phase = useWave(state => state.connection.phase)
  const connect = useWave(state => state.connect)
  const disconnect = useWave(state => state.disconnect)
  return <div className="peer-row">
    <span className="small-icon"><Laptop size={18} /></span>
    <div className="peer-name"><strong title={peer.name}>{peer.name}</strong><small>{peer.ip}{current ? ' · ' + t(phase) : ''}</small></div>
    {current
      ? <button className="icon-button" title={t('disconnect')} aria-label={`${t('disconnect')} ${peer.name}`} onClick={() => void disconnect()}><X size={17} /></button>
      : <button className="icon-button peer-connect" disabled={phase !== 'idle'} title={t('connect')} aria-label={`${t('connect')} ${peer.name}`} onClick={() => void connect(peer.ip)}><ArrowRight size={19} /></button>}
  </div>
}

function LinkQuality() {
  const { t } = useText()
  const metrics = useWave(state => state.connection.metrics)
  return <footer className="link-quality" aria-label={t('quality')}>
    <Metric label="RTT" value={metrics.rttMs} unit={t('ms')} />
    <Metric label={t('loss')} value={metrics.lossPercent} unit="%" />
    <Metric label={t('jitter')} value={metrics.jitterMs} unit={t('ms')} />
    <span className="codec">Opus · 48 {t('khz')} · 10 {t('ms')}</span>
  </footer>
}

function Metric({ label, value, unit }: { label: string; value: number | null; unit: string }) {
  const { language } = useText()
  return <span className="metric">{label} <strong>{value === null ? '—' : value.toLocaleString(language, { maximumFractionDigits: 1 }) + (unit === '%' ? unit : ` ${unit}`)}</strong></span>
}

function AppearanceSettings() {
  const { t, language } = useText()
  const theme = useWave(state => state.settings.theme)
  const accent = useWave(state => state.settings.accent)
  const updateSettings = useWave(state => state.updateSettings)
  return <section className="settings-section" aria-labelledby="settings-appearance">
    <div className="settings-heading"><h1 id="settings-appearance">{t('appearance')}</h1></div>
    <div className="panel appearance">
      <Select label={t('language')} value={language} items={languages.map(({ id, label }) => ({ value: id, label }))}
        onChange={value => void updateSettings({ language: value as Language })} />
      <div className="field">
        <span id="theme-label">{t('theme')}</span>
        <div className="segmented" role="group" aria-labelledby="theme-label">
          {themes.map(({ id, icon: Icon }) => <button key={id} type="button" aria-pressed={theme === id}
            onClick={() => void updateSettings({ theme: id })}><Icon size={15} />{t(id)}</button>)}
        </div>
      </div>
      <div className="field">
        <span id="accent-label">{t('accent')}</span>
        <div className="swatches" role="group" aria-labelledby="accent-label">
          {accents.map(id => <button className="swatch" data-accent={id} key={id} type="button" aria-label={t(id)} title={t(id)}
            aria-pressed={accent === id} onClick={() => void updateSettings({ accent: id })}>
            <span>{accent === id && <Check size={15} strokeWidth={3} />}</span>
          </button>)}
        </div>
      </div>
    </div>
  </section>
}

function UpdatePanel() {
  const { t } = useText()
  const { updateStatus, updateVersion, appVersion, checkUpdate, installUpdate } = useWave()
  const active = useWave(state => state.connection.phase !== 'idle')
  const statusKeys = {
    idle: null, checking: 'updateChecking', current: 'updateCurrent', available: 'updateAvailable',
    installing: 'updateDownloading', checkError: 'updateError', installError: 'installError',
  } as const
  const status = statusKeys[updateStatus]
  const installing = updateStatus === 'installing'
  return <section className="settings-section" aria-labelledby="updates-title">
    <div className="settings-heading"><h1 id="updates-title">{t('updates')}</h1></div>
    <div className="panel settings-list">
      {appVersion && <div className="settings-row"><span>{t('installedVersion')}</span><strong className="mono">{appVersion}</strong></div>}
      <div className="settings-row"><span>{t('updateSource')}</span><span>GitHub Releases</span></div>
      <div className="update-row">
        <p aria-live="polite">{active && updateVersion ? t('updateBusy')
          : status ? t(status, { version: updateVersion ?? '' }) : t('updateAtStartup')}</p>
        <button className="button secondary" disabled={!native || updateStatus === 'checking' || installing}
          onClick={() => void checkUpdate()}><RefreshCw size={15} />{t('updateCheck')}</button>
        {updateVersion && <button className="button primary" disabled={active || installing}
          onClick={() => void installUpdate()}><Download size={15} />{t('updateInstall')}</button>}
      </div>
    </div>
  </section>
}

const settingsPages = [
  { id: 'audio', icon: SlidersHorizontal, Panel: AudioSettings },
  { id: 'appearance', icon: Palette, Panel: AppearanceSettings },
  { id: 'localNetwork', icon: Router, Panel: NetworkSettings },
  { id: 'updates', icon: Download, Panel: UpdatePanel },
] as const

function SettingsPage() {
  const { t } = useText()
  const [page, setPage] = useState<typeof settingsPages[number]['id']>('audio')
  const Panel = settingsPages.find(item => item.id === page)!.Panel
  return <div className="settings-layout">
    <nav className="settings-nav" aria-label={t('settingsSections')}>
      {settingsPages.map(({ id, icon: Icon }) => <button key={id} className="nav-link" type="button"
        aria-current={page === id ? 'page' : undefined} onClick={() => setPage(id)}>
        <Icon size={16} aria-hidden="true" />{t(id)}
      </button>)}
    </nav>
    <Panel />
  </div>
}

function AudioSettings() {
  const { t, message } = useText()
  const { settings, devices, devicesLoading, devicesError, refreshDevices, updateSettings } = useWave()
  const sourceLabelId = useId()
  const sourceHintId = useId()
  const option = (update: Partial<Settings>) => { void updateSettings(update) }
  const endpoints = (entries: { id: string; name: string }[], current: string, defaultId: string | null) => [
    { value: '', label: [t('windowsDefault'), entries.find(entry => entry.id === defaultId)?.name].filter(Boolean).join(' · ') },
    ...entries.map(entry => ({ value: entry.id, label: entry.name })),
    ...(current && !entries.some(entry => entry.id === current) ? [{ value: current, label: t('unavailableDevice') }] : []),
  ]
  return <section className="settings-section" aria-labelledby="audio-title">
      <div className="settings-heading">
        <h1 id="audio-title">{t('audio')}</h1>
        <button className={`button secondary ${devicesLoading ? 'spinning' : ''}`} disabled={devicesLoading || !native}
          onClick={() => void refreshDevices(true)}><RefreshCw size={15} aria-hidden="true" />{t('refreshAudio')}</button>
      </div>
      {devicesError && <div className="notice" role="status"><CircleAlert size={17} />{message(devicesError)}</div>}
      <div className="panel audio-routes">
        <section className="audio-group" aria-labelledby="audio-send">
          <h2 className="group-heading" id="audio-send"><ArrowUpRight size={16} />{t('sendGroup')}</h2>
          <div className="audio-fields">
            <div className="field">
              <FieldHint id={sourceHintId} label={t('computerSource')} text={t('computerSourceHint')}>
                <span id={sourceLabelId}>{t('computerSource')}</span>
              </FieldHint>
              <div className="segmented" role="group" aria-labelledby={sourceLabelId} aria-describedby={sourceHintId}>
                <button type="button" aria-pressed={settings.source === 'system'} onClick={() => option({ source: 'system' })}>{t('systemAudio')}</button>
                <button type="button" aria-pressed={settings.source === 'application'} onClick={() => option({ source: 'application' })}>{t('application')}</button>
              </div>
            </div>
            {settings.source === 'application' && <Select label={t('application')} hint={t('applicationHint')} value={settings.application?.pid.toString() ?? ''}
              items={[
                { value: '', label: devicesLoading ? t('findingApps') : t('selectApp') },
                ...(settings.application && !devices.applications.some(app => app.pid === settings.application?.pid)
                  ? [{ value: String(settings.application.pid), label: settings.application.name + ' · ' + t('waitingApp') }] : []),
                ...devices.applications.map(app => ({ value: String(app.pid), label: `${app.name} · PID ${app.pid}` })),
              ]}
              onChange={pid => option({ application: devices.applications.find(app => String(app.pid) === pid) ?? null })} />}
            <Select label={t('microphone')} hint={t('microphoneHint')} value={settings.microphone}
              items={endpoints(devices.microphones, settings.microphone, devices.defaultMicrophone)} onChange={microphone => option({ microphone })} />
          </div>
        </section>
        <section className="audio-group" aria-labelledby="audio-receive">
          <h2 className="group-heading" id="audio-receive"><ArrowDownLeft size={16} />{t('receiveGroup')}</h2>
          <div className="audio-fields">
            <Select label={t('computerOutput')} hint={t('computerOutputHint')} value={settings.computerOutput}
              items={endpoints(devices.outputs, settings.computerOutput, devices.defaultOutput)} onChange={computerOutput => option({ computerOutput })} />
            <Select label={t('micOutput')} hint={t('micOutputHint')} value={settings.microphoneOutput}
              items={endpoints(devices.outputs, settings.microphoneOutput, devices.defaultOutput)} onChange={microphoneOutput => option({ microphoneOutput })} />
          </div>
        </section>
      </div>
      <details className="panel audio-quality">
        <summary><Gauge size={17} aria-hidden="true" /><span>{t('audioQuality')}</span><small>Opus · 48 {t('khz')}</small><ChevronDown size={15} aria-hidden="true" /></summary>
        <div className="fields">
          <NumberSelect label={t('computerBitrate')} value={settings.computerBitrate} values={[64, 96, 128, 160, 192, 224, 256]} unit={t('kbps')} onChange={computerBitrate => option({ computerBitrate })} />
          <NumberSelect label={t('micBitrate')} value={settings.microphoneBitrate} values={[16, 24, 32, 48, 64, 80, 96]} unit={t('kbps')} onChange={microphoneBitrate => option({ microphoneBitrate })} />
          <NumberSelect label={t('networkBuffer')} value={settings.jitterMs} values={[20, 30, 40, 50, 60, 70, 80]} unit={t('ms')} onChange={jitterMs => option({ jitterMs })} />
          <NumberSelect label={t('playbackBuffer')} value={settings.playbackMs} values={[10, 20, 30, 40]} unit={t('ms')} onChange={playbackMs => option({ playbackMs })} />
        </div>
      </details>
    </section>
}

function NetworkSettings() {
  const { t } = useText()
  const { settings, addresses, ready, updateSettings } = useWave()
  const [name, setName] = useState(settings.deviceName)
  useEffect(() => setName(settings.deviceName), [settings.deviceName])
  const saveName = (event: FormEvent) => {
    event.preventDefault()
    if (name.trim()) void updateSettings({ deviceName: name.trim() })
  }
  return <section className="settings-section" aria-labelledby="network-title">
    <div className="settings-heading"><h1 id="network-title">{t('localNetwork')}</h1></div>
    <div className="panel settings-list">
      <form className="settings-row name-field" onSubmit={saveName}>
        <label htmlFor="name">{t('thisComputerName')}</label>
        <div className="inline-form">
          <input id="name" value={name} onChange={event => setName(event.target.value)} required maxLength={48} />
          <button className="button secondary" disabled={!name.trim() || name.trim() === settings.deviceName}>{t('save')}</button>
        </div>
      </form>
      <div className="settings-row"><span>{t('ip')}</span><span className="local-address">
        <span className={ready ? 'status-dot green' : 'status-dot'} aria-hidden="true" />
        <span className="mono">{addresses.join(', ') || '—'}</span>
      </span></div>
      <div className="settings-row"><span>{t('autoDiscovery')}</span><Toggle enabled={settings.discovery} label={t('autoDiscovery')}
        onChange={discovery => void updateSettings({ discovery })} /></div>
      <div className="settings-row"><span>{t('autoReconnect')}</span><Toggle enabled={settings.reconnect} label={t('autoReconnect')}
        onChange={reconnect => void updateSettings({ reconnect })} /></div>
    </div>
  </section>
}

function NumberSelect({ label, value, values, unit, onChange }: { label: string; value: number; values: number[]; unit: string; onChange: (value: number) => void }) {
  const items = [...new Set([...values, value])].sort((a, b) => a - b).map(number => ({ value: String(number), label: `${number} ${unit}` }))
  return <Select label={label} value={String(value)} items={items} onChange={value => onChange(Number(value))} />
}

function Select({ label, hint, value, items, onChange }: {
  label: string; hint?: string; value: string; items: { value: string; label: string }[]; onChange: (value: string) => void
}) {
  const id = useId()
  const hintId = `${id}-hint`
  const caption = <label htmlFor={id}>{label}</label>
  return <div className="field">
    {hint ? <FieldHint id={hintId} label={label} text={hint}>{caption}</FieldHint> : caption}
    <span className="select-box">
      <select id={id} aria-describedby={hint ? hintId : undefined} value={value}
        onChange={event => onChange(event.target.value)}>
        {items.map(item => <option key={item.value} value={item.value}>{item.label}</option>)}
      </select>
      <ChevronDown size={16} aria-hidden="true" />
    </span>
  </div>
}

function FieldHint({ id, label, text, children }: { id: string; label: string; text: string; children: ReactNode }) {
  const { t } = useText()
  const [hovered, setHovered] = useState(false)
  const [focused, setFocused] = useState(false)
  const [dismissed, setDismissed] = useState(false)
  const visible = !dismissed && (hovered || focused)
  useEffect(() => {
    if (!visible) return
    const dismiss = (event: KeyboardEvent) => {
      if (event.key === 'Escape') setDismissed(true)
    }
    document.addEventListener('keydown', dismiss)
    return () => document.removeEventListener('keydown', dismiss)
  }, [visible])
  return <span className="field-caption"
    onPointerEnter={event => {
      if (event.pointerType !== 'touch') {
        setHovered(true)
        setDismissed(false)
      }
    }}
    onPointerLeave={() => setHovered(false)}>
    {children}
    <button className="hint-trigger" type="button" aria-label={t('fieldHelp', { field: label })} aria-describedby={id}
      onFocus={() => { setFocused(true); setDismissed(false) }}
      onBlur={() => setFocused(false)}
      onClick={() => { setFocused(true); setDismissed(false) }}><Info size={14} aria-hidden="true" /></button>
    <span id={id} role="tooltip" className="field-tooltip" hidden={!visible}>{text}</span>
  </span>
}

function Dialog({ title, icon, close, children }: { title: string; icon: ReactNode; close: () => void; children: ReactNode }) {
  const { t } = useText()
  const ref = useRef<HTMLDialogElement>(null)
  useEffect(() => {
    const dialog = ref.current
    dialog?.showModal()
    return () => dialog?.close()
  }, [])
  return <dialog className="dialog" ref={ref} aria-labelledby="dialog-title"
    onCancel={event => { event.preventDefault(); close() }}
    onClick={event => { if (event.target === ref.current) close() }}>
    <div className="dialog-body">
      <div className="panel-heading"><span className="panel-icon">{icon}</span><h2 id="dialog-title">{title}</h2><button className="icon-button" onClick={close} aria-label={t('close')}><X size={18} /></button></div>
      {children}
    </div>
  </dialog>
}

function ManualDialog({ close }: { close: () => void }) {
  const { t, message } = useText()
  const connect = useWave(state => state.connect)
  const [ip, setIp] = useState('')
  const [busy, setBusy] = useState(false)
  const [validation, setValidation] = useState('')
  const submit = async (event: FormEvent) => {
    event.preventDefault()
    const candidate = ip.trim()
    const octets = candidate.split('.')
    if (octets.length !== 4 || octets.some(part => !/^(0|[1-9]\d{0,2})$/.test(part) || Number(part) > 255)) {
      setValidation('i18n:invalidIP')
      return
    }
    if (!native) {
      setValidation('i18n:desktopOnly')
      return
    }
    setBusy(true)
    if (await connect(candidate)) close()
    else setValidation(useWave.getState().error ?? 'i18n:connectError')
    setBusy(false)
  }
  return <Dialog title={t('connectIP')} icon={<EthernetPort size={20} />} close={close}>
    <p>{t('enterIP')}</p>
    <form onSubmit={event => void submit(event)}>
      <label className="field">{t('ip')}<input autoFocus className="mono" type="text" inputMode="decimal" required maxLength={15} placeholder="192.168.1.32" value={ip} onChange={event => { setIp(event.target.value); setValidation('') }} /></label>
      {validation && <p className="form-error" role="alert">{message(validation)}</p>}
      <div className="dialog-actions"><button type="button" className="button secondary" onClick={close}>{t('cancel')}</button><button className="button primary" disabled={busy}>{t('connect')} <ArrowRight size={17} /></button></div>
    </form>
  </Dialog>
}

function PairingDialog({ connection }: { connection: Connection }) {
  const { t } = useText()
  const approve = useWave(state => state.approve)
  const disconnect = useWave(state => state.disconnect)
  const code = connection.code!
  const close = () => { if (connection.approved) void disconnect(); else void approve(code, false) }
  return <Dialog title={connection.incoming ? t('incomingCall') : t('pairDevices')} icon={<ShieldCheck size={20} />} close={close}>
    <p>{t('compareCode', { name: connection.peer?.name ?? '' })}</p>
    <div className="pair-code mono" aria-label={code.split('').join(' ')}>{code}</div>
    <div className="dialog-actions">
      <button className="button secondary" onClick={close}>{connection.approved ? t('cancel') : t('reject')}</button>
      <button className="button primary" disabled={connection.approved} onClick={() => void approve(code, true)}>
        {connection.approved ? t('awaitingApproval') : t('codeMatches')} <ShieldCheck size={17} />
      </button>
    </div>
  </Dialog>
}
