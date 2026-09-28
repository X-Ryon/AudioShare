/**
 * 全局状态（组合式 store）
 * 对接 commands.rs 全部命令与 level 事件
 * 追溯：TC-001~TC-017
 */
import { reactive } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'

export interface AudioApp { pid: number; exe: string; name: string; playing: boolean }
export interface MicDevice { id: string; name: string; is_default: boolean }
export interface SharedInfo { pid: number; exe: string; volume: number }
export interface AppStatus {
  cable_installed: boolean
  master_on: boolean
  onboarding_done: boolean
  mic_id: string
  shared: SharedInfo[]
}

export const store = reactive({
  apps: [] as AudioApp[],
  mics: [] as MicDevice[],
  status: null as AppStatus | null,
  selected: new Set<number>(),      // 已勾选 pid
  volumes: {} as Record<number, number>, // pid -> 0-100
  levels: {} as Record<number, number>,  // pid -> rms 0-1
  masterOn: true,
  cableInstalled: false,
  onboardingDone: false,
  micId: '',
  micUnavailable: false,
  showOnboarding: false,
  version: '',
})

async function refreshApps() {
  store.apps = await invoke<AudioApp[]>('get_apps')
  // 移除已不存在的勾选（应用退出，TC-007）
  const pids = new Set(store.apps.map(a => a.pid))
  for (const pid of [...store.selected]) {
    if (!pids.has(pid)) {
      store.selected.delete(pid)
      delete store.volumes[pid]
      delete store.levels[pid]
    }
  }
}

async function refreshStatus() {
  const s = await invoke<AppStatus>('get_status')
  store.status = s
  store.masterOn = s.master_on
  store.cableInstalled = s.cable_installed
  store.onboardingDone = s.onboarding_done
  store.micId = s.mic_id
  // 恢复已共享源
  for (const sh of s.shared) {
    store.selected.add(sh.pid)
    store.volumes[sh.pid] = sh.volume
  }
  if (!s.onboarding_done || !s.cable_installed) {
    store.showOnboarding = true
  }
}

export async function initStore() {
  await Promise.all([refreshApps(), refreshStatus()])
  await invoke('get_mics').then((m) => { store.mics = m as MicDevice[] })
  await invoke<string>('get_version').then((v) => { store.version = v })

  // 应用列表每3秒刷新（PRD·应用音源枚举）
  setInterval(refreshApps, 3000)

  // 电平事件（30fps 由 Rust 侧节流）
  await listen<[number, number][]>('level', (e) => {
    for (const [pid, rms] of e.payload) {
      store.levels[pid] = rms
    }
  })
}

export async function toggleShare(app: AudioApp, enable: boolean) {
  if (enable) store.selected.add(app.pid)
  else store.selected.delete(app.pid)
  try {
    await invoke('toggle_share', { pid: app.pid, exe: app.exe, enable })
    if (enable && !(app.pid in store.volumes)) store.volumes[app.pid] = 80
  } catch (err) {
    // 失败回滚
    if (enable) store.selected.delete(app.pid)
    else store.selected.add(app.pid)
    throw err
  }
}

export async function clearAllShared() {
  const apps = store.apps.filter(a => store.selected.has(a.pid))
  for (const app of apps) {
    await toggleShare(app, false)
  }
}

export async function setVolume(pid: number, volume: number) {
  store.volumes[pid] = volume
  await invoke('set_volume', { pid, volume })
}

export async function setMic(id: string) {
  store.micId = id
  store.micUnavailable = false
  try {
    await invoke('set_mic', { deviceId: id })
  } catch {
    store.micUnavailable = true // TC-012 横幅
  }
}

export async function toggleMaster(on: boolean) {
  store.masterOn = on
  await invoke('toggle_master', { on })
}

export async function finishOnboarding() {
  store.showOnboarding = false
  store.onboardingDone = true
  await invoke('finish_onboarding')
  await refreshStatus()
}

export async function quitApp() {
  await invoke('quit_app')
}
