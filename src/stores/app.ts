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
// 记忆配置：总开关状态 + 麦克风设备 id
export interface RememberConfig { enabled: boolean; master_on: boolean; mic: string }
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
  micLevel: 0,                              // 麦克风实时 RMS 0-1
  masterOn: false,                          // 初始化为关闭
  cableInstalled: false,
  onboardingDone: false,
  micId: '',
  micUnavailable: false,
  showOnboarding: false,
  version: '',
  rememberOn: false,                  // 记住选择开关
})

/** 前端日志通道：异常经后端 write_log 落盘（与后端日志同文件） */
export function logFrontend(level: 'error' | 'warn' | 'info', message: string) {
  invoke('write_log', { level, message }).catch(() => {})
}

async function refreshApps() {
  let apps: AudioApp[]
  try {
    apps = await invoke<AudioApp[]>('get_apps')
  } catch (err) {
    logFrontend('warn', `get_apps 失败: ${err}`)
    return
  }
  store.apps = apps
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
  // 记住选择配置加载
  await invoke<RememberConfig>('get_remember')
    .then((rc) => {
      store.rememberOn = rc.enabled
    })
    .catch((err) => logFrontend('warn', `记住配置加载失败: ${err}`))
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

  // 麦克风实时电平
  await listen<number>('mic_level', (e) => {
    store.micLevel = e.payload
  })
}

export async function toggleShare(app: AudioApp, enable: boolean) {
  if (enable) store.selected.add(app.pid)
  else store.selected.delete(app.pid)
  try {
    await invoke('toggle_share', { pid: app.pid, exe: app.exe, enable })
    if (enable && !(app.pid in store.volumes)) store.volumes[app.pid] = 80
  } catch (err) {
    logFrontend('error', `toggle_share 失败 ${app.exe} enable=${enable}: ${err}`)
    // 失败回滚
    if (enable) store.selected.delete(app.pid)
    else store.selected.add(app.pid)
    throw err
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
  } catch (err) {
    logFrontend('error', `set_mic 失败: ${id}: ${err}`)
    store.micUnavailable = true // TC-012 横幅
  }
}

export async function setRemember(on: boolean) {
  store.rememberOn = on
  await invoke('set_remember', { on })
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
