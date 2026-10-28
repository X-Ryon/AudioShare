<script setup lang="ts">
/**
 * 悬浮窗（label="float" 窗口渲染）· 第3期变更轮次 1
 *
 * 结构：⠿ 拖动吸附 · ⟳ 手动旋转 · 共享开关 · 应用图标区 · 功能组（闭麦 / 暂停 / 主界面 / 关闭）
 * 变更点（FR-016~FR-020）：
 *   - 新增 ⟳ 旋转按钮：主动切换 水平 ⇄ 垂直 布局（后端 rotate_float 计算目标位置/尺寸）
 *   - 应用图标区禁用右键（根元素 + 图标区双层 contextmenu 拦截）
 *   - 应用图标悬停名称改用浏览器原生 title 提示：由浏览器渲染在应用窗口之外，
 *     不再被悬浮窗 44px 的边界裁剪（原自绘 .tip 被 overflow:hidden 吃掉）
 *   - 麦克风开/闭统一为同一图形，仅颜色 + 斜杠区分
 *   - 整体视觉重设计为 Win11 Fluent 亚克力风格（渐变 + 圆角 + 描边 + 内高光）
 *
 * 追溯：TC-045~048/050~055/059~064（PRD FR-001/009~011/013~020）
 */
import { computed, onBeforeUnmount, onMounted, ref, watch } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { listen } from '@tauri-apps/api/event'
import { getCurrentWindow } from '@tauri-apps/api/window'
import { PhysicalPosition, PhysicalSize } from '@tauri-apps/api/dpi'
import { store, initStore, toggleMaster, toggleOutputMute, getAppIcon, logFrontend, setFloatEnabled } from '../stores/app'
import FloatIcon from '../components/FloatIcon.vue'

/** 后端返回的几何结果（物理像素） */
interface FloatGeometry {
  x: number
  y: number
  orientation: 'horizontal' | 'vertical'
  /** 目标宽（物理像素）：前端 setSize 直接使用，避免前后端两处硬编码尺寸漂移 */
  w: number
  /** 目标高（物理像素） */
  h: number
}

/** 过渡动画步数（6 步 × 15ms ≈ 90ms） */
const TRANSITION_STEPS = 6
/** 图标区最多显示的应用数（水平/垂直一致，保证窗口尺寸恒定） */
const MAX_VISIBLE_ICONS = 3

const appWindow = getCurrentWindow()
const vertical = ref(false)      // 当前布局方向
const settling = ref(false)      // 位置/尺寸过渡中：抑制 onMoved 自激
const movedOnce = ref(false)     // 已收到首个 onMoved（窗口显示/创建期的移动噪声需忽略）
const spinning = ref(false)      // 旋转按钮自转动效
const busy = ref(false)          // 旋转/吸附互斥，避免并发改窗口尺寸
const icons = ref(new Map<number, string>()) // pid -> base64 data URL
let moveTimer: number | undefined
let unmounted = false
let ignoreMoveUntil = 0   // 窗口重显/旋转后的噪声抑制截止时间戳

const pids = computed(() => [...store.selected])
// 图标区固定宽度（3×20 + 2×4 = 68），超出部分裁切，避免撑破窗口
const visiblePids = computed(() => pids.value.slice(0, MAX_VISIBLE_ICONS))

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

/**
 * 位置/尺寸 6 步插值（约 90ms）：吸附切换与手动旋转共用，消除尺寸跳变
 * 目标位置已含后端以 ⠿ 手柄为锚点的补偿，避免切换时僵硬和窗口错位
 */
async function animateTo(from: FloatGeometry, to: FloatGeometry) {
  for (let i = 1; i <= TRANSITION_STEPS; i++) {
    const t = i / TRANSITION_STEPS
    await appWindow.setSize(new PhysicalSize(
      Math.round(from.w + (to.w - from.w) * t),
      Math.round(from.h + (to.h - from.h) * t),
    ))
    await appWindow.setPosition(new PhysicalPosition(
      Math.round(from.x + (to.x - from.x) * t),
      Math.round(from.y + (to.y - from.y) * t),
    ))
    await new Promise((res) => setTimeout(res, 15))
  }
}

/** ⟳ 手动旋转布局（FR-016）：水平 ⇄ 垂直，锚点 = ⠿ 手柄中心 */
async function rotateLayout(e?: MouseEvent) {
  if (busy.value || settling.value) return
  busy.value = true
  spinning.value = true
  window.setTimeout(() => { spinning.value = false }, 280)
  logFrontend('info', `旋转点击 screen=(${e?.screenX ?? -1},${e?.screenY ?? -1})`)
  try {
    const pos = await appWindow.outerPosition()
    // 尺寸用【内层】尺寸（setSize 设置的就是内层尺寸）；位置用【外层】位置（setPosition 设置的就是外层位置）
    const size = await appWindow.innerSize()
    const r = await invoke<FloatGeometry>('rotate_float', {
      x: pos.x, y: pos.y, w: size.width, h: size.height,
    })
    settling.value = true
    vertical.value = r.orientation === 'vertical'
    await animateTo(
      { x: pos.x, y: pos.y, w: size.width, h: size.height, orientation: r.orientation },
      { x: r.x, y: r.y, w: r.w, h: r.h, orientation: r.orientation },
    )
    settling.value = false
    // 旋转动画本身会连续触发 onMoved，抑制一段时间，避免"旋转后立刻被吸附规则改回"
    ignoreMoveUntil = Date.now() + 800
    store.floatOrientation = r.orientation
    await invoke('save_float_position', { x: r.x, y: r.y, orientation: r.orientation })
    logFrontend('info', `悬浮窗旋转为 ${r.orientation}，位置 (${r.x}, ${r.y}) 尺寸 ${r.w}x${r.h}`)
  } catch (e) {
    settling.value = false
    logFrontend('warn', `旋转布局失败: ${e}`)
  } finally {
    busy.value = false
  }
}

onMounted(async () => {
  await initStore()

  // 启动时按落盘的 orientation 恢复布局方向（后端建窗已用对应尺寸）
  vertical.value = store.floatOrientation === 'vertical'

  // 后端重显悬浮窗时广播：此后短时间内的 onMoved 是位置噪声，忽略以免误吸附
  await listen('float_window_shown', () => {
    ignoreMoveUntil = Date.now() + 600
  })

  // 拖动（原生 drag-region）结束后 200ms 防抖吸附：后端纯函数计算，前端执行位置/尺寸切换
  await appWindow.onMoved(async () => {
    if (Date.now() < ignoreMoveUntil) return
    // 首次事件是窗口显示/创建期的移动噪声（此时并未拖动），
    // 若不忽略会触发一次误吸附，把悬浮窗吸到屏幕边缘并切换布局
    if (!movedOnce.value) {
      movedOnce.value = true
      return
    }
    if (settling.value || busy.value || unmounted) return
    clearTimeout(moveTimer)
    moveTimer = window.setTimeout(async () => {
      try {
        const pos = await appWindow.outerPosition()
        // 尺寸用【内层】尺寸（与 setSize 语义一致），位置用【外层】位置
        const size = await appWindow.innerSize()
        const r = await invoke<FloatGeometry>('snap_float', {
          x: pos.x, y: pos.y, w: size.width, h: size.height,
        })
        logFrontend('info', `吸附结果 orientation=${r.orientation} pos=(${r.x},${r.y}) size=${r.w}x${r.h}`)
        settling.value = true
        const isVert = r.orientation === 'vertical'
        vertical.value = isVert
        // 目标尺寸由后端随 SnapResult 返回（w/h），前端不再硬编码 [232,44]/[44,232]
        await animateTo(
          { x: pos.x, y: pos.y, w: size.width, h: size.height, orientation: r.orientation },
          { x: r.x, y: r.y, w: r.w, h: r.h, orientation: r.orientation },
        )
        settling.value = false
        ignoreMoveUntil = Date.now() + 400
        store.floatOrientation = r.orientation
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
  <!-- 根元素拦截右键：屏蔽 WebView 原生菜单（后退/刷新/检查） -->
  <div class="float" :class="{ vertical }" @contextmenu.prevent>
    <!-- 拖动手柄：原生 drag-region，释放时边缘吸附 -->
    <button class="grip" data-tauri-drag-region title="拖动移动（释放时边缘吸附）" aria-label="拖动移动">
      <FloatIcon name="grip" />
    </button>

    <!-- 旋转按钮：手动切换水平 / 垂直布局 -->
    <button class="ibtn rot" :class="{ spin: spinning }" title="旋转：切换水平 / 垂直布局"
      aria-label="旋转布局" @click="rotateLayout($event)">
      <FloatIcon name="rotate" />
    </button>

    <!-- 共享开关 -->
    <button class="switch" :class="{ on: store.masterOn }" role="switch" :aria-checked="store.masterOn"
      aria-label="共享开关" title="共享开关（开启/关闭）" @click="toggleMaster(!store.masterOn)" />

    <!-- 正在共享的应用：原生 title 提示渲染在窗口之外，不再被悬浮窗裁剪 -->
    <div class="icons" @contextmenu.prevent>
      <span v-for="pid in visiblePids" :key="pid" class="app-ico" :title="nameOf(pid)" :aria-label="nameOf(pid)"
        draggable="false">
        <img v-if="icons.get(pid)" :src="icons.get(pid)" alt="" draggable="false" />
        <span v-else class="ph">{{ nameOf(pid).slice(0, 1) }}</span>
      </span>
    </div>

    <!-- 功能按钮组 -->
    <div class="group">
      <button class="ibtn" :class="store.outputMuted ? 'mic-off' : 'mic-on'"
        :title="store.outputMuted ? '已闭麦（输出全静音），点击恢复' : '一键闭麦 / 恢复'"
        :aria-label="store.outputMuted ? '恢复开麦' : '一键闭麦'"
        @click="toggleOutputMute(!store.outputMuted)">
        <FloatIcon name="mic" :muted="store.outputMuted" />
      </button>
      <button class="ibtn" title="暂停 / 播放本地音频" aria-label="暂停/播放" @click="pauseLocal">
        <FloatIcon name="pause" />
      </button>
      <button class="ibtn" title="打开主界面" aria-label="打开主界面" @click="openMain">
        <FloatIcon name="monitor" />
      </button>
      <button class="ibtn fw-close" title="关闭悬浮窗" aria-label="关闭悬浮窗" @click="closeFloat">
        <FloatIcon name="close" />
      </button>
    </div>
  </div>
</template>

<style>
/* 悬浮窗窗口透明背景（覆盖主窗口全局浅色背景，配合 transparent 窗口） */
html, body, #app { background: transparent; }
</style>

<style scoped>
/* ===== 容器：Win11 Fluent 亚克力 =====
   窗口尺寸 == 工具条尺寸，外投影会被窗口边界裁剪，故以「1px 描边 + 内高光 + 内暗边」制造层次 */
.float {
  display: flex;
  align-items: center;
  gap: 4px;
  width: 100%;
  height: 100%;
  box-sizing: border-box;
  padding: 8px;
  background: linear-gradient(180deg, rgba(56, 58, 64, .92) 0%, rgba(28, 29, 34, .94) 100%);
  border: 1px solid rgba(255, 255, 255, .10);
  border-radius: 12px;
  box-shadow: inset 0 1px 0 rgba(255, 255, 255, .10), inset 0 -1px 0 rgba(0, 0, 0, .28);
  opacity: .78;
  transition: opacity .15s;
  user-select: none;
  -webkit-user-select: none;
  overflow: hidden;
}
.float:hover { opacity: 1; }
.float.vertical { flex-direction: column; align-items: center; }

/* ===== 拖动手柄（16x16） ===== */
.grip {
  width: 16px; height: 16px; flex-shrink: 0; padding: 0; border: none; background: none;
  cursor: move; color: #8a8a8a;
  display: flex; align-items: center; justify-content: center;
  transition: color .12s;
}
.grip:hover { color: #d6d6d6; }

/* ===== 通用图标按钮（24x24） ===== */
.ibtn {
  width: 24px; height: 24px; flex-shrink: 0; padding: 0; border: none; background: none;
  cursor: pointer; color: #d6d6d6; border-radius: 6px;
  display: flex; align-items: center; justify-content: center;
  transition: background .12s ease-out, color .12s ease-out, transform .12s ease-out;
}
.ibtn:hover { background: rgba(255, 255, 255, .10); color: #ffffff; }
.ibtn:active { transform: scale(.9); }

/* 旋转按钮：图标自转 180 度反馈"已切换方向" */
.rot { margin-right: 4px; }
.float.vertical .rot { margin-right: 0; margin-bottom: 4px; }
.rot :deep(svg) { transition: transform .25s cubic-bezier(.4, 0, .2, 1); }
.rot.spin :deep(svg) { transform: rotate(180deg); }

/* ===== 共享开关（30x18） ===== */
.switch {
  position: relative; width: 30px; height: 18px; border-radius: 9px; flex-shrink: 0;
  background: rgba(255, 255, 255, .14); border: none; padding: 0; cursor: pointer;
  transition: background .15s; margin-right: 6px;
}
.switch::after {
  content: ""; position: absolute; top: 2px; left: 2px; width: 14px; height: 14px;
  border-radius: 50%; background: #cfcfcf;
  transition: transform .15s cubic-bezier(.4, 0, .2, 1), background .15s;
}
.switch.on { background: #4cc2ff; }
.switch.on::after { transform: translateX(12px); background: #fff; }
.float.vertical .switch { margin-right: 0; margin-bottom: 6px; }

/* ===== 应用图标区（固定 3 个 = 68px） ===== */
.icons {
  display: flex; align-items: center; gap: 4px;
  width: 68px; flex-shrink: 0; overflow: hidden; margin-right: 6px;
}
.float.vertical .icons { flex-direction: column; width: 20px; height: 68px; margin-right: 0; margin-bottom: 6px; }
.app-ico {
  width: 20px; height: 20px; border-radius: 5px; flex-shrink: 0; overflow: hidden;
  display: flex; align-items: center; justify-content: center;
  color: #fff; font-size: 10px; font-weight: 700; background: #3a3a3a;
  box-shadow: inset 0 0 0 1px rgba(255, 255, 255, .10);
  transition: transform .12s ease-out, box-shadow .12s ease-out;
}
.app-ico img {
  width: 20px; height: 20px; border-radius: 5px; object-fit: contain;
  -webkit-user-drag: none; pointer-events: none;
}
.app-ico:hover { transform: scale(1.08); box-shadow: 0 0 0 2px #4cc2ff; }

/* ===== 功能按钮组（分隔线） ===== */
.group {
  display: flex; align-items: center; gap: 2px;
  border-left: 1px solid rgba(255, 255, 255, .10); padding-left: 8px;
}
.float.vertical .group {
  flex-direction: column; border-left: none;
  border-top: 1px solid rgba(255, 255, 255, .10); padding-left: 0; padding-top: 8px;
}

/* ===== 状态色 ===== */
.mic-on { color: #4cc2ff; }
.mic-off { color: #ff6b6b; }
/* hover 时保留语义色：.ibtn:hover 的 color:#fff 会盖掉 .mic-on/.mic-off，
   导致闭麦图标在悬停下变白、丢失“已静音”警示语义，故单独覆盖 */
.ibtn.mic-on:hover { background: rgba(76, 194, 255, .18); color: #4cc2ff; }
.ibtn.mic-off:hover { background: rgba(255, 107, 107, .18); color: #ff6b6b; }
.fw-close:hover { background: #ff6b6b; color: #fff; }
</style>
