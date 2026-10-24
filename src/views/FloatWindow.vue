<script setup lang="ts">
/**
 * 悬浮窗（第3期，label="float" 窗口渲染）：
 * 共享开关 / 共享应用图标（悬停名称）/ 一键闭麦 / ⏯ 暂停 / 🖥 主界面 / ✕ 隐藏 / ⠿ 拖动吸附
 * 追溯：TC-045/047/048/049/050/051/052/053/054/055/059/060
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { PhysicalPosition, PhysicalSize } from '@tauri-apps/api/dpi'
import { store, initStore, toggleMaster, toggleOutputMute, getAppIcon, logFrontend, setFloatEnabled } from '../stores/app'

const appWindow = getCurrentWindow()
const vertical = ref(false)      // 当前布局（吸附后切换）
const settling = ref(false)      // 吸附执行中：抑制 onMoved 自激
const icons = ref(new Map<number, string>()) // pid -> base64 data URL
let moveTimer: number | undefined
let unmounted = false

const pids = computed(() => [...store.selected])
// 垂直布局高度有限（232px），图标区最多显示 3 个，避免溢出裁切
const visiblePids = computed(() => (vertical.value ? pids.value.slice(0, 3) : pids.value))

function nameOf(pid: number): string {
  return store.apps.find(a => a.pid === pid)?.name ?? '未知应用'
}

async function loadIcons() {
  for (const pid of pids.value) {
    if (icons.value.has(pid)) continue
    const icon = await getAppIcon(pid)
    if (icon) icons.value.set(pid, icon)
  }
  // 清理已取消共享的图标缓存
  for (const pid of [...icons.value.keys()]) {
    if (!store.selected.has(pid)) icons.value.delete(pid)
  }
}

watch(pids, () => { void loadIcons() })

function pauseLocal() {
  invoke('media_play_pause').catch((e) => logFrontend('warn', `media_play_pause 失败: ${e}`))
}

function openMain() {
  invoke('show_main_window').catch((e) => logFrontend('warn', `show_main_window 失败: ${e}`))
}

/// ✕ 关闭悬浮窗：走 set_float_enabled(false)（落盘 + 通知主界面开关同步）
function closeFloat() {
  setFloatEnabled(false).catch((e) => logFrontend('warn', `set_float_enabled(false) 失败: ${e}`))
}

onMounted(async () => {
  await initStore()

  // 拖动（原生 drag-region）结束后 200ms 防抖吸附：后端纯函数计算，前端执行位置/尺寸切换
  await appWindow.onMoved(async () => {
    if (settling.value || unmounted) return
    clearTimeout(moveTimer)
    moveTimer = window.setTimeout(async () => {
      try {
        const pos = await appWindow.outerPosition()
        const size = await appWindow.outerSize()
        const r = await invoke<{ x: number; y: number; orientation: string }>('snap_float', {
          x: pos.x, y: pos.y, w: size.width, h: size.height,
        })
        settling.value = true
        const isVert = r.orientation === 'vertical'
        vertical.value = isVert
        const [w, h] = isVert ? [44, 232] : [232, 44]
        // 丝滑变换（验收反馈）：位置/尺寸同步插值 6 步约 90ms，
        // 目标位置已含后端以 ⠿ 滑块为锚点的补偿，避免切换时僵硬和窗口错位
        const steps = 6
        const [cx, cy] = [pos.x, pos.y]
        const [cw, ch] = [size.width, size.height]
        for (let i = 1; i <= steps; i++) {
          const t = i / steps
          await appWindow.setSize(new PhysicalSize(Math.round(cw + (w - cw) * t), Math.round(ch + (h - ch) * t)))
          await appWindow.setPosition(new PhysicalPosition(Math.round(cx + (r.x - cx) * t), Math.round(cy + (r.y - cy) * t)))
          await new Promise((res) => setTimeout(res, 15))
        }
        settling.value = false
        await invoke('save_float_position', { x: r.x, y: r.y, orientation: r.orientation })
      } catch (e) {
        settling.value = false
        logFrontend('warn', `边缘吸附失败: ${e}`)
      }
    }, 200)
  })

  void loadIcons()
})

onBeforeUnmount(() => {
  unmounted = true
  clearTimeout(moveTimer)
})
</script>

<template>
  <div class="float" :class="{ vertical }">
    <button class="fw-btn drag-handle" data-tauri-drag-region title="拖动移动（释放时边缘吸附）">⠿</button>
    <button class="switch" :class="{ on: store.masterOn }" role="switch" :aria-checked="store.masterOn"
      aria-label="共享开关" @click="toggleMaster(!store.masterOn)" />
    <span class="fw-sep"></span>
    <div class="icons">
      <span v-for="pid in visiblePids" :key="pid" class="app-ico">
        <img v-if="icons.get(pid)" :src="icons.get(pid)" alt="" />
        <span v-else class="ph">{{ nameOf(pid).slice(0, 1) }}</span>
        <span class="tip">{{ nameOf(pid) }}</span>
      </span>
    </div>
    <span class="fw-sep"></span>
    <button class="fw-btn" :class="store.outputMuted ? 'mic-off' : 'mic-on'"
      :title="store.outputMuted ? '已闭麦（输出全静音），点击恢复' : '一键闭麦/恢复'"
      aria-label="一键闭麦" @click="toggleOutputMute(!store.outputMuted)">{{ store.outputMuted ? '🔇' : '🎤' }}</button>
    <button class="fw-btn" title="暂停/播放本地音频" aria-label="暂停/播放" @click="pauseLocal">⏯</button>
    <button class="fw-btn" title="打开主界面" aria-label="打开主界面" @click="openMain">🖥</button>
    <button class="fw-btn fw-close" title="关闭悬浮窗" aria-label="关闭悬浮窗" @click="closeFloat">✕</button>
  </div>
</template>

<style>
/* 悬浮窗窗口透明背景（覆盖主窗口全局浅色背景，配合 transparent 窗口） */
html, body, #app { background: transparent; }
</style>

<style scoped>
.float { display: flex; align-items: center; gap: 2px; width: 100%; height: 100%;
  background: #1b1b1b; border: 1px solid #444; border-radius: 8px; padding: 4px 6px;
  opacity: .65; transition: opacity .15s; user-select: none; overflow: hidden; }
.float:hover { opacity: 1; }
.float.vertical { flex-direction: column; padding: 6px 4px; }
.fw-btn { border: none; background: none; cursor: pointer; color: #c8c8c8; font-size: 15px;
  width: 28px; height: 28px; border-radius: 6px; display: flex; align-items: center; justify-content: center; flex-shrink: 0; }
.fw-btn:hover { background: #333; color: #fff; }
.drag-handle { cursor: move; color: #6b6b6b; font-size: 14px; letter-spacing: 2px; }
.drag-handle:hover { background: #333; }
.fw-sep { width: 1px; height: 20px; background: #3a3a3a; flex-shrink: 0; }
.float.vertical .fw-sep { width: 20px; height: 1px; }
/* 共享开关（悬浮窗紧凑版，aria-checked 驱动） */
.switch { position: relative; width: 38px; height: 20px; border-radius: 10px; background: transparent;
  border: 1px solid #555; cursor: pointer; transition: .15s; flex-shrink: 0; margin: 0 4px; }
.switch::after { content: ""; position: absolute; top: 3px; left: 3px; width: 12px; height: 12px;
  border-radius: 50%; background: #8a8a8a; transition: .15s; }
.switch[aria-checked="true"] { background: #0067c0; border-color: #0067c0; }
.switch[aria-checked="true"]::after { left: 21px; background: #fff; }
.float.vertical .switch { margin: 2px 0; }
/* 应用图标区 */
.icons { display: flex; align-items: center; gap: 4px; min-width: 0; }
.float.vertical .icons { flex-direction: column; }
.app-ico { position: relative; width: 24px; height: 24px; border-radius: 5px;
  display: flex; align-items: center; justify-content: center; color: #fff; font-size: 11px;
  font-weight: 600; flex-shrink: 0; background: #3a3a3a; }
.app-ico img { width: 24px; height: 24px; border-radius: 5px; object-fit: contain; }
.app-ico .tip { position: absolute; top: 110%; left: 50%; transform: translateX(-50%);
  background: #1b1b1b; border: 1px solid #444; color: #fff; font-size: 11px; padding: 3px 8px;
  border-radius: 4px; white-space: nowrap; display: none; z-index: 20; }
.float.vertical .app-ico .tip { top: 50%; left: 115%; transform: translateY(-50%); }
.app-ico:hover .tip { display: block; }
.mic-on { color: #4da3ff; }
.mic-off { color: #ff5252; }
.fw-close:hover { background: #c42b1c; }
</style>
