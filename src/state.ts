import { invoke, isTauri } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { create } from 'zustand'
import type { Language } from './i18n'

export type Application = { pid: number; name: string; path: string; started: number }
export type Channel = { enabled: boolean; volume: number }
export type Theme = 'system' | 'light' | 'dark'
export type Accent = 'lime' | 'blue' | 'violet' | 'coral' | 'amber'
export type Settings = {
  language: Language
  theme: Theme
  accent: Accent
  deviceName: string
  discovery: boolean
  reconnect: boolean
  source: 'system' | 'application'
  application: Application | null
  microphone: string
  computerOutput: string
  microphoneOutput: string
  channels: Channel[]
  computerBitrate: number
  microphoneBitrate: number
  jitterMs: number
  playbackMs: number
}
export type Peer = { id: string; name: string; ip: string }
export type Metrics = {
  rttMs: number | null
  lossPercent: number | null
  jitterMs: number | null
  bufferMs: number | null
  sent: number
  received: number
}
export type Connection = {
  phase: 'idle' | 'connecting' | 'pairing' | 'connected' | 'reconnecting'
  peer: Peer | null
  code: string | null
  incoming: boolean
  approved: boolean
  message: string | null
  metrics: Metrics
  levels: number[]
  channelMessages: (string | null)[]
  channelDevices: ({ configuredId: string; name: string } | null)[]
}
type Snapshot = { settings: Settings; connection: Connection; peers: Peer[]; addresses: string[]; ready: boolean }
type Endpoint = { id: string; name: string }
export type AudioDevices = {
  microphones: Endpoint[]
  outputs: Endpoint[]
  defaultMicrophone: string | null
  defaultOutput: string | null
  applications: Application[]
}
export type Section = 'mixer' | 'settings'
export type UpdateStatus = 'idle' | 'checking' | 'current' | 'available' | 'installing' | 'checkError' | 'installError'

const defaultSettings: Settings = {
  language: 'ru',
  theme: 'dark',
  accent: 'lime',
  deviceName: 'StreamSoundCast',
  discovery: true,
  reconnect: true,
  source: 'system',
  application: null,
  microphone: '',
  computerOutput: '',
  microphoneOutput: '',
  channels: [true, false, true, true].map(enabled => ({ enabled, volume: 100 })),
  computerBitrate: 160,
  microphoneBitrate: 64,
  jitterMs: 20,
  playbackMs: 20,
}
const idle: Connection = {
  phase: 'idle', peer: null, code: null, incoming: false, approved: false, message: null,
  metrics: { rttMs: null, lossPercent: null, jitterMs: null, bufferMs: null, sent: 0, received: 0 },
  levels: [0, 0, 0, 0], channelMessages: [null, null, null, null], channelDevices: [null, null, null, null],
}
export const native = isTauri()
let saveQueue = Promise.resolve()
let saves = 0
let deviceQueue = Promise.resolve()

type Store = Snapshot & {
  section: Section
  devices: AudioDevices
  devicesLoading: boolean
  devicesError: string | null
  error: string | null
  updateStatus: UpdateStatus
  updateVersion: string | null
  appVersion: string | null
  navigate: (section: Section) => void
  apply: (snapshot: Snapshot) => void
  refreshDevices: (includeApplications?: boolean) => Promise<void>
  updateSettings: (update: Partial<Settings> | ((settings: Settings) => Settings)) => Promise<boolean>
  clearError: () => void
  connect: (ip: string) => Promise<boolean>
  disconnect: () => Promise<boolean>
  approve: (code: string, approved: boolean) => Promise<boolean>
  scan: () => Promise<boolean>
  checkUpdate: () => Promise<void>
  installUpdate: () => Promise<void>
}

function errorText(error: unknown) {
  return typeof error === 'string' ? error : error instanceof Error ? error.message : 'i18n:errorGeneric'
}

export const useWave = create<Store>((set, get) => {
  const command = async (name: string, args?: Record<string, unknown>) => {
    if (!native) {
      set({ error: 'i18n:desktopOnly' })
      return false
    }
    try {
      await invoke(name, args)
      await refreshSnapshot()
      return true
    } catch (error) {
      set({ error: errorText(error) })
      return false
    }
  }
  return {
    settings: defaultSettings, connection: idle, peers: [], addresses: [], ready: false,
    section: 'mixer', devices: { microphones: [], outputs: [], defaultMicrophone: null, defaultOutput: null, applications: [] },
    devicesLoading: false, devicesError: null, error: null,
    updateStatus: 'idle', updateVersion: null, appVersion: null,
    navigate: section => {
      set({ section })
      if (section === 'settings') void get().refreshDevices(true)
    },
    apply: snapshot => set(previous => ({
      ...snapshot,
      settings: saves ? previous.settings : snapshot.settings,
    })),
    refreshDevices: (includeApplications = false) => {
      if (!native || (!includeApplications && get().devicesLoading)) return Promise.resolve()
      set({ devicesLoading: true })
      deviceQueue = deviceQueue.then(async () => {
        set({ devicesLoading: true })
        try {
          const devices = await invoke<Omit<AudioDevices, 'applications'> & { applications: Application[] | null }>('audio_devices', { includeApplications })
          set(previous => ({
            devices: { ...devices, applications: devices.applications ?? previous.devices.applications },
            devicesError: null,
          }))
        } catch (error) {
          set({ devicesError: errorText(error) })
        } finally {
          set({ devicesLoading: false })
        }
      })
      return deviceQueue
    },
    updateSettings: update => {
      const previous = get().settings
      const settings = typeof update === 'function' ? update(previous) : { ...previous, ...update }
      set({ settings })
      if (!native) return Promise.resolve(true)
      saves += 1
      const task = saveQueue.then(async () => {
        try {
          const saved = await invoke<Settings>('save_settings', { settings })
          if (saves === 1) set({ settings: saved })
          return true
        } catch (error) {
          set({ error: errorText(error) })
          if (saves === 1) set({ settings: previous })
          return false
        } finally {
          saves -= 1
        }
      })
      saveQueue = task.then(() => undefined)
      return task
    },
    clearError: () => set({ error: null }),
    connect: ip => command('connect', { ip }),
    disconnect: () => command('disconnect'),
    approve: (code, approved) => command('approve_pairing', { code, approved }),
    scan: () => native ? command('scan') : Promise.resolve(true),
    checkUpdate: async () => {
      if (!native || ['checking', 'installing'].includes(get().updateStatus)) return
      set({ updateStatus: 'checking' })
      try {
        const result = await invoke<{ current: string; available: string | null }>('check_update')
        set({
          appVersion: result.current,
          updateVersion: result.available,
          updateStatus: result.available === null ? 'current' : 'available',
        })
      } catch (error) {
        console.error(error)
        set({ updateVersion: null, updateStatus: 'checkError' })
      }
    },
    installUpdate: async () => {
      const { updateVersion, updateStatus, connection } = get()
      if (!native || !updateVersion || !['available', 'installError'].includes(updateStatus) || connection.phase !== 'idle') return
      set({ updateStatus: 'installing' })
      try {
        await invoke('install_update', { version: updateVersion })
      } catch (error) {
        set({ error: errorText(error), updateStatus: 'installError' })
      }
    },
  }
})

async function refreshSnapshot() {
  useWave.getState().apply(await invoke<Snapshot>('snapshot'))
}

export async function initialize() {
  if (!native) return
  try {
    await listen<Snapshot>('wavecast-state', event => useWave.getState().apply(event.payload))
    await refreshSnapshot()
    void useWave.getState().checkUpdate()
    void useWave.getState().refreshDevices()
    window.addEventListener('focus', () => void useWave.getState().refreshDevices())
    window.setInterval(() => {
      const state = useWave.getState()
      if (state.section === 'mixer' && document.visibilityState === 'visible') void state.refreshDevices()
    }, 4000)
  } catch (error) {
    useWave.setState({ error: errorText(error) })
  }
}
