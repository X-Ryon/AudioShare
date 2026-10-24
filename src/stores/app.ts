/**
 * 全局状态（组合式 store）
 * 对接 commands.rs 全部命令与 level 事件
 * 追溯：TC-001~TC-017
 */
import { reactive } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { HotkeyConfig, DEFAULT_HOTKEYS } from '../utils/hotkey'

export type { HotkeyConfig }
export { DEFAULT_HOTKEYS }

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
  output_muted: boolean
}

/** 悬浮窗配置（float_window.json，第3期） */
export interface FloatWindowConfig { enabled: boolean; x: number; y: number; orientation: string }

export const store = reactive({
  apps: [] as AudioApp[],
  mics: [] as MicDevice[],
  status: null as AppStatus | null,
  selected: new Set<number>(),      // 已勾选 pid
  favorites: new Set<string>(),    // 收藏自动共享 exe（小写，第2期 P2）
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
  hotkeys: { ...DEFAULT_HOTKEYS } as HotkeyConfig, // 热键配置（TC-020/023）
  showSettings: false,                 // 设置面板显隐
  outputMuted: false,                  // 输出闭麦（第3期 TC-053，不持久化）
  floatEnabled: false,                 // 悬浮窗设置开关（第3期 TC-046）
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
  store.outputMuted = s.output_muted
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
  // 收藏自动共享名单加载（第2期 P2，TC-035 重启保留）
  await invoke<string[]>('get_favorites')
    .then((list) => { store.favorites = new Set(list) })
    .catch((err) => logFrontend('warn', `收藏列表加载失败: ${err}`))

  // 悬浮窗配置加载（第3期 TC-046/056：enabled 同步到设置开关）
  await loadFloatConfig()

  // 应用列表每3秒刷新（PRD·应用音源枚举）
  setInterval(refreshApps, 3000)

  // 热键配置加载（TC-023 主界面提醒行数据源）
  await loadHotkeys()

  // 主开关状态同步（UI 开关/热键/托盘任一入口变更均触发，TC-018/022）
  await listen<boolean>('master_changed', (e) => {
    store.masterOn = e.payload
  })

  // 热键配置变更同步（TC-020 改键成功回显）
  await listen<HotkeyConfig>('hotkeys_changed', (e) => {
    store.hotkeys = e.payload
  })

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

  // 收藏名单变更同步（第2期 P2 星标回显）
  await listen<string[]>('favorites_changed', (e) => {
    store.favorites = new Set(e.payload)
  })

  // 自动共享开关同步（第2期 P2：后端自动开/关共享时同步勾选，TC-036/037）
  await listen<[number, boolean][]>('shared_changed', (e) => {
    for (const [pid, shared] of e.payload) {
      if (shared) store.selected.add(pid)
      else store.selected.delete(pid)
    }
  })

  // 输出闭麦状态同步（第3期 TC-053：悬浮窗 🎤 与后端真实状态一致）
  await listen<boolean>('output_mute_changed', (e) => {
    store.outputMuted = e.payload
  })

  // 悬浮窗开关状态同步（第3期：主界面开关与后端一致）
  await listen<boolean>('float_enabled_changed', (e) => {
    store.floatEnabled = e.payload
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

// ---------- 第3期：悬浮窗 ----------

/** 加载悬浮窗配置（enabled 同步到设置开关） */
export async function loadFloatConfig() {
  try {
    const cfg = await invoke<FloatWindowConfig>('get_float_config')
    store.floatEnabled = cfg.enabled
  } catch (err) {
    logFrontend('warn', `get_float_config 失败: ${err}`)
  }
}

/** 主界面设置开关：true→后端建窗显示；false→后端销毁（TC-046/056） */
export async function setFloatEnabled(on: boolean) {
  store.floatEnabled = on
  try {
    await invoke('set_float_enabled', { on })
  } catch (err) {
    logFrontend('error', `set_float_enabled 失败: ${err}`)
    store.floatEnabled = !on
    throw err
  }
}

/** 一键闭麦：仅静音输出，不改变共享状态（TC-053/058） */
export async function toggleOutputMute(on: boolean) {
  store.outputMuted = on
  try {
    await invoke('set_output_mute', { on })
  } catch (err) {
    logFrontend('error', `set_output_mute 失败: ${err}`)
    store.outputMuted = !on
    throw err
  }
}

/** 获取应用图标 base64 data URL（失败返回 null，占位符兑底） */
export async function getAppIcon(pid: number): Promise<string | null> {
  try {
    return await invoke<string | null>('get_app_icon', { pid })
  } catch (err) {
    logFrontend('warn', `get_app_icon 失败 pid=${pid}: ${err}`)
    return null
  }
}

// ---------- 热键配置（TC-020/021/023） ----------

export async function loadHotkeys() {
  try {
    store.hotkeys = await invoke<HotkeyConfig>('get_hotkeys')
  } catch (err) {
    logFrontend('warn', `get_hotkeys 失败: ${err}`)
  }
}

/** 保存热键配置；后端注册失败（冲突/非法）会抛错，由调用方回显旧值 */
export async function saveHotkeys(cfg: HotkeyConfig) {
  await invoke('set_hotkeys', { cfg })
  store.hotkeys = cfg
}

/** 录制模式开关：true 注销全部热键（否则 keydown 被系统拦截），false 恢复注册 */
export async function setHotkeysRecording(on: boolean) {
  await invoke('set_hotkeys_recording', { on })
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

// ---------- 收藏自动共享（第2期 P2，TC-035） ----------

/** 收藏/取消收藏：乐观更新 + 失败回滚 */
export async function toggleFavorite(exe: string) {
  const key = exe.toLowerCase()
  const wasFav = store.favorites.has(key)
  if (wasFav) store.favorites.delete(key)
  else store.favorites.add(key)
  try {
    await invoke('set_favorite', { exe, on: !wasFav })
  } catch (err) {
    logFrontend('error', `set_favorite 失败 ${exe}: ${err}`)
    if (wasFav) store.favorites.add(key)
    else store.favorites.delete(key)
    throw err
  }
}
