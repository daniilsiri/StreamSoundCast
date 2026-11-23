import { useEffect, useLayoutEffect, useRef, useState, type FormEvent, type ReactNode } from 'react'
import { getCurrentWindow } from '@tauri-apps/api/window'
import {
  Activity, ArrowDownLeft, ArrowRight, ArrowUpRight, AudioLines, ChevronDown, ChevronRight, Download,
  Check, CircleAlert, EthernetPort, Gauge, Headphones, Laptop, Mic, Monitor, Moon, Palette, Plus, RefreshCw,
  Router, Settings2, ShieldCheck, SlidersHorizontal, Speaker, Sun, Volume2, VolumeX, X,
} from 'lucide-react'
import { native, useWave, type Connection, type Peer, type Settings } from './state'
import { languages, translate, localizeMessage, type Language, type TextKey } from './i18n'
import mark from './mark.svg'

const navigation = [
  { id: 'mixer', icon: SlidersHorizontal },
  { id: 'settings', icon: Settings2 },
] as const

const themes = [
  { id: 'dark', icon: Moon },
  { id: 'light', icon: Sun },
  { id: 'system', icon: Monitor },
] as const

const accents = ['lime', 'blue', 'violet', 'coral', 'amber'] as const

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
  const deviceName = useWave(state => state.settings.deviceName)
  const theme = useWave(state => state.settings.theme)
  const accent = useWave(state => state.settings.accent)
  const addresses = useWave(state => state.addresses)
  const ready = useWave(state => state.ready)
  const error = useWave(state => state.error)
  const clearError = useWave(state => state.clearError)
  const [manual, openManual] = useState(false)
  const title = t(section === 'mixer' ? 'mixerTitle' : 'settings')
  const description = t(section === 'mixer' ? 'mixerDescription' : 'settingsDescription')
  const localIp = addresses[0]

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
      <aside className="sidebar">
        <button className="brand" onClick={() => navigate('mixer')} aria-label={`StreamSoundCast · ${t('mixer')}`}>
          <img className="brand-icon" src={mark} alt="" width={34} height={34} />
          <span className="wordmark">StreamSoundCast</span>
        </button>
        <nav className="nav" aria-label={t('sections')}>
          {navigation.map(({ id, icon: Icon }) => (
            <button key={id} aria-current={section === id ? 'page' : undefined} className={section === id ? 'nav-link active' : 'nav-link'}
              onClick={() => navigate(id)} title={t(id)} aria-label={t(id)}>
              <Icon size={19} strokeWidth={1.8} /><span>{t(id)}</span>
              {id === 'settings' && updateVersion && <i className="update-dot" title={t('updateAvailable', { version: updateVersion })} />}
            </button>
          ))}
        </nav>
        <div className="sidebar-bottom">
          <div className="identity">
            <span className="small-icon"><Monitor size={18} /></span>
            <div><strong title={deviceName}>{deviceName}</strong><small title={addresses.join(', ')}>{localIp ?? t('localDevice')}</small></div>
          </div>
          <div className="sidebar-network"><span className={ready ? 'status-dot green' : 'status-dot'} /> {t('localNetwork')}</div>
        </div>
      </aside>
      <main className="main-content">
        <header className="page-heading">
          <div><h1>{title}</h1><p>{description}</p></div>
          <span className={'connection-pill ' + (connection.phase === 'connected' ? 'connected' : '')}>
            <span className={'status-dot ' + (connection.phase === 'connected' ? 'green' : connection.phase !== 'idle' ? 'gold' : '')} />
            {t(connection.phase)}
          </span>
        </header>
        {connection.message && <div className="notice" role="status"><CircleAlert size={17} />{message(connection.message)}</div>}
        {section === 'mixer' && <Mixer openManual={() => openManual(true)} />}
        {section === 'settings' && <SettingsPage />}
      </main>
      {manual && connection.phase !== 'pairing' && <ManualDialog close={() => openManual(false)} />}
      {connection.phase === 'pairing' && connection.code && <PairingDialog connection={connection} />}
      {error && <div className="toast" role="alert"><CircleAlert size={20} /><span>{message(error)}</span><button className="icon-button" onClick={clearError} aria-label={t('close')}><X size={16} /></button></div>}
    </div>
  )
}

function Mixer({ openManual }: { openManual: () => void }) {
  const { t } = useText()
  return (
    <div className="dashboard-grid">
      <div className="mixer-column">
        <div className="section-heading">
          <h2>{t('channels')}</h2>
          <span className="format-pill"><AudioLines size={15} /> 48 {t('khz')}</span>
        </div>
        <div className="channel-grid">
          {lanes.map((_, index) => <ChannelCard key={index} index={index} />)}
        </div>
      </div>
      <div className="dashboard-side">
        <PeerPanel openManual={openManual} />
        <QualityCard />
      </div>
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
  const level = channel.enabled ? enabledLevel : 0
  const lit = Math.ceil(Math.min(1, Math.pow(level, 0.52)) * 18)
  const route = t(incoming ? 'incoming' : 'outgoing')
  const direction = incoming ? ArrowDownLeft : ArrowUpRight
  const Direction = direction
  const endpoints = index === 1 ? devices.microphones : devices.outputs
  const defaultId = index === 1 ? devices.defaultMicrophone : devices.defaultOutput
  const available = endpoints.find(device => device.id === (configuredId || defaultId))
  const activeName = activeDevice?.configuredId === configuredId ? activeDevice?.name : undefined
  const channelOptions = index === 0
    ? source === 'system' ? t('systemAudio') : application?.name ?? t('selectApp')
    : activeName ?? available?.name ?? (configuredId ? t('unavailableDevice') : t('windowsDefault'))

  return (
    <article className={'panel channel ' + (incoming ? 'incoming' : 'outgoing') + (!channel.enabled ? ' muted' : '')}>
      <div className="channel-header">
        <span className="channel-icon"><Icon size={21} strokeWidth={1.75} /></span>
        <span className="route"><Direction size={13} />{route}</span>
        <Toggle enabled={channel.enabled} onChange={enabled => update({ enabled })} label={`${route} · ${title}`} />
      </div>
      <h3>{title}</h3>
      <button className="channel-source" onClick={() => navigate('settings')} title={channelOptions}><span>{channelOptions}</span><ChevronRight size={13} /></button>
      <div className="meter" aria-hidden="true">
        {Array.from({ length: 18 }, (_, i) => <span key={i} className={i < lit ? 'lit' : ''} />)}
      </div>
      <div className="volume">
        <button className="speaker-button" aria-label={value > 0 ? t('muteVolume') : t('restoreVolume')} onClick={() => update({ volume: value > 0 ? 0 : 100 })}>
          {value === 0 ? <VolumeX size={16} /> : <Volume2 size={16} />}
        </button>
        <input type="range" min="0" max="100" value={value} aria-label={`${t('volume')} · ${route} · ${title}`}
          style={{ backgroundSize: value + '% 100%' }}
          onChange={event => setValue(event.target.valueAsNumber)}
          onPointerUp={commit} onKeyUp={commit} onBlur={commit} />
        <span className="volume-value">{value}%</span>
      </div>
      {message && <p className="channel-message"><CircleAlert size={13} /> {translateMessage(message)}</p>}
    </article>
  )
}

function Toggle({ enabled, onChange, label }: { enabled: boolean; onChange: (on: boolean) => void; label: string }) {
  return <button type="button" className="switch" role="switch" aria-checked={enabled} aria-label={label} onClick={() => onChange(!enabled)}><span /></button>
}

function PeerPanel({ openManual }: { openManual: () => void }) {
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
    <section className="panel peers-panel" aria-labelledby="peers-title">
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

function QualityCard() {
  const { t } = useText()
  const metrics = useWave(state => state.connection.metrics)
  return <section className="panel quality-panel" aria-labelledby="quality-title">
    <div className="panel-heading">
      <span className="panel-icon teal"><Activity size={18} /></span>
      <h2 id="quality-title">{t('quality')}</h2>
    </div>
    <div className="summary-metrics">
      <Metric label={t('rtt')} value={metrics.rttMs} unit={t('ms')} />
      <Metric label={t('loss')} value={metrics.lossPercent} unit="%" />
      <Metric label={t('jitter')} value={metrics.jitterMs} unit={t('ms')} />
    </div>
    <div className="codec-row"><span><span className="status-dot green" /> Opus</span><span>48 {t('khz')} <span className="separator">·</span> 10 {t('ms')}</span></div>
  </section>
}

function Metric({ label, value, unit }: { label: string; value: number | null; unit?: string }) {
  const { language } = useText()
  return <div className="metric"><small>{label}</small><div><strong>{value === null ? '—' : value.toLocaleString(language, { maximumFractionDigits: 1 })}</strong>{unit && <span>{unit}</span>}</div></div>
}

function AppearanceSettings() {
  const { t, language } = useText()
  const theme = useWave(state => state.settings.theme)
  const accent = useWave(state => state.settings.accent)
  const updateSettings = useWave(state => state.updateSettings)
  return <section className="panel subpage-card wide appearance">
    <div className="panel-heading"><span className="panel-icon"><Palette size={19} /></span><h2>{t('appearance')}</h2></div>
    <div className="fields appearance-fields">
      <Select label={t('language')} value={language} items={languages.map(({ id, label }) => ({ value: id, label }))}
        onChange={value => void updateSettings({ language: value as Language })} />
      <fieldset className="field">
        <legend>{t('theme')}</legend>
        <div className="segmented">
          {themes.map(({ id, icon: Icon }) => <button key={id} type="button" aria-pressed={theme === id}
            onClick={() => void updateSettings({ theme: id })}><Icon size={15} />{t(id)}</button>)}
        </div>
      </fieldset>
      <fieldset className="field">
        <legend>{t('accent')}</legend>
        <div className="swatches">
          {accents.map(id => <button className="swatch" data-accent={id} key={id} type="button" aria-label={t(id)} title={t(id)}
            aria-pressed={accent === id} onClick={() => void updateSettings({ accent: id })}>
            <span>{accent === id && <Check size={15} strokeWidth={3} />}</span>
          </button>)}
        </div>
      </fieldset>
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
  return <section className="panel subpage-card wide update-panel" aria-labelledby="updates-title">
    <div className="panel-heading">
      <span className="panel-icon"><Download size={18} /></span>
      <h2 id="updates-title">{t('updates')}</h2>
      {appVersion && <small className="mono">v{appVersion}</small>}
    </div>
    <div className="update-row">
      <p aria-live="polite">{active && updateVersion ? t('updateBusy')
        : status ? t(status, { version: updateVersion ?? '' }) : null}</p>
      <button className="button secondary" disabled={!native || updateStatus === 'checking' || installing}
        onClick={() => void checkUpdate()}><RefreshCw size={15} />{t('updateCheck')}</button>
      {updateVersion && <button className="button primary" disabled={active || installing}
        onClick={() => void installUpdate()}><Download size={15} />{t('updateInstall')}</button>}
    </div>
  </section>
}

function SettingsPage() {
  const { t, message } = useText()
  const { settings, devices, devicesLoading, devicesError, refreshDevices, updateSettings } = useWave()
  const [name, setName] = useState(settings.deviceName)
  useEffect(() => setName(settings.deviceName), [settings.deviceName])
  const option = (update: Partial<Settings>) => { void updateSettings(update) }
  const endpoints = (entries: { id: string; name: string }[], current: string, defaultId: string | null) => [
    { value: '', label: [t('windowsDefault'), entries.find(entry => entry.id === defaultId)?.name].filter(Boolean).join(' · ') },
    ...entries.map(entry => ({ value: entry.id, label: entry.name })),
    ...(current && !entries.some(entry => entry.id === current) ? [{ value: current, label: t('unavailableDevice') }] : []),
  ]
  const saveName = (event: FormEvent) => {
    event.preventDefault()
    if (name.trim()) option({ deviceName: name.trim() })
  }
  return <div className="subpage-grid settings-grid">
    <AppearanceSettings />
    <UpdatePanel />
    <section className="panel subpage-card wide">
      <div className="panel-heading">
        <span className="panel-icon"><Speaker size={19} /></span><h2>{t('audioRoutes')}</h2>
        <button className={'icon-button ' + (devicesLoading ? 'spinning' : '')} disabled={devicesLoading || !native}
          aria-label={t('refreshAudio')} onClick={() => void refreshDevices(true)}><RefreshCw size={18} /></button>
      </div>
      {devicesError && <div className="notice" role="status"><CircleAlert size={17} />{message(devicesError)}</div>}
      <div className="fields">
        <div className="field">
          <span>{t('computerSource')}</span>
          <div className="segmented" role="group" aria-label={t('computerSource')}>
            <button aria-pressed={settings.source === 'system'} onClick={() => option({ source: 'system' })}><Monitor size={16} /> {t('systemAudio')}</button>
            <button aria-pressed={settings.source === 'application'} onClick={() => option({ source: 'application' })}><AudioLines size={16} /> {t('application')}</button>
          </div>
        </div>
        {settings.source === 'application' && <Select label={t('application')} value={settings.application?.pid.toString() ?? ''}
          items={[
            { value: '', label: devicesLoading ? t('findingApps') : t('selectApp') },
            ...(settings.application && !devices.applications.some(app => app.pid === settings.application?.pid)
              ? [{ value: String(settings.application.pid), label: settings.application.name + ' · ' + t('waitingApp') }] : []),
            ...devices.applications.map(app => ({ value: String(app.pid), label: `${app.name} · PID ${app.pid}` })),
          ]}
          onChange={pid => option({ application: devices.applications.find(app => String(app.pid) === pid) ?? null })} />}
        <Select label={t('microphone')} value={settings.microphone} items={endpoints(devices.microphones, settings.microphone, devices.defaultMicrophone)} onChange={microphone => option({ microphone })} />
        <Select label={t('computerOutput')} value={settings.computerOutput} items={endpoints(devices.outputs, settings.computerOutput, devices.defaultOutput)} onChange={computerOutput => option({ computerOutput })} />
        <Select label={t('micOutput')} value={settings.microphoneOutput} items={endpoints(devices.outputs, settings.microphoneOutput, devices.defaultOutput)} onChange={microphoneOutput => option({ microphoneOutput })} />
      </div>
    </section>
    <section className="panel subpage-card">
      <div className="panel-heading"><span className="panel-icon"><Router size={19} /></span><h2>{t('localNetwork')}</h2></div>
      <form className="field name-field" onSubmit={saveName}>
        <label htmlFor="name">{t('thisComputerName')}</label>
        <div className="inline-form">
          <input id="name" value={name} onChange={event => setName(event.target.value)} required maxLength={48} />
          <button className="button secondary" disabled={!name.trim() || name.trim() === settings.deviceName}>{t('save')}</button>
        </div>
      </form>
      <div className="settings-row"><span>{t('autoDiscovery')}</span><Toggle enabled={settings.discovery} label={t('autoDiscovery')} onChange={discovery => option({ discovery })} /></div>
      <div className="settings-row"><span>{t('autoReconnect')}</span><Toggle enabled={settings.reconnect} label={t('autoReconnect')} onChange={reconnect => option({ reconnect })} /></div>
    </section>
    <section className="panel subpage-card">
      <div className="panel-heading"><span className="panel-icon teal"><Gauge size={19} /></span><h2>{t('audioQuality')}</h2></div>
      <div className="fields">
        <NumberSelect label={t('computerBitrate')} value={settings.computerBitrate} values={[64, 96, 128, 160, 192, 224, 256]} unit={t('kbps')} onChange={computerBitrate => option({ computerBitrate })} />
        <NumberSelect label={t('micBitrate')} value={settings.microphoneBitrate} values={[16, 24, 32, 48, 64, 80, 96]} unit={t('kbps')} onChange={microphoneBitrate => option({ microphoneBitrate })} />
        <NumberSelect label={t('networkBuffer')} value={settings.jitterMs} values={[20, 30, 40, 50, 60, 70, 80]} unit={t('ms')} onChange={jitterMs => option({ jitterMs })} />
        <NumberSelect label={t('playbackBuffer')} value={settings.playbackMs} values={[10, 20, 30, 40]} unit={t('ms')} onChange={playbackMs => option({ playbackMs })} />
      </div>
    </section>
  </div>
}

function NumberSelect({ label, value, values, unit, onChange }: { label: string; value: number; values: number[]; unit: string; onChange: (value: number) => void }) {
  const items = [...new Set([...values, value])].sort((a, b) => a - b).map(number => ({ value: String(number), label: `${number} ${unit}` }))
  return <Select label={label} value={String(value)} items={items} onChange={value => onChange(Number(value))} />
}

function Select({ label, value, items, onChange }: { label: string; value: string; items: { value: string; label: string }[]; onChange: (value: string) => void }) {
  return <label className="field"><span>{label}</span><span className="select-box">
    <select aria-label={label} value={value} onChange={event => onChange(event.target.value)}>{items.map(item => <option key={item.value} value={item.value}>{item.label}</option>)}</select>
    <ChevronDown size={16} aria-hidden="true" />
  </span></label>
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
