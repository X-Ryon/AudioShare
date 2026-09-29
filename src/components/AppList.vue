<script setup lang="ts">
/**
 * 应用列表：直接显示所有有音频会话的程序，每行含状态/图标/名称/电平/音量/共享开关
 * 替代原 AppDropdown + SharedPanel 组合
 */
import { computed, reactive, onMounted, watch } from 'vue'
import { store, toggleShare, setVolume } from '../stores/app'
import { invoke } from '@tauri-apps/api/core'
import MarqueeText from './MarqueeText.vue'

const rows = computed(() => store.apps)

// 图标缓存：pid -> base64 data URL
const icons = reactive<Record<number, string>>({})

/** 延迟加载应用图标（仅首次请求时调用后端提取） */
async function loadIcon(pid: number, exe: string) {
  if (icons[pid]) return // 已加载
  try {
    const icon = await invoke<string | null>('get_app_icon', { pid })
    if (icon) icons[pid] = icon
  } catch (e) {
    console.warn('Failed to load icon for', exe, e)
  }
}

onMounted(() => {
  // 初始加载所有应用图标
  for (const app of store.apps) {
    loadIcon(app.pid, app.exe)
  }
})

// 监听应用列表变化，自动加载新应用图标
watch(() => store.apps, (apps) => {
  for (const app of apps) {
    if (!icons[app.pid]) loadIcon(app.pid, app.exe)
  }
}, { immediate: true, deep: true })

/** RMS(0-1) → 电平条宽度百分比（平方根曲线放大低值） */
function levelPct(pid: number): string {
  const rms = store.levels[pid] ?? 0
  return Math.min(100, Math.round(Math.sqrt(rms) * 200)) + '%'
}
</script>

<template>
  <div class="app-list">
    <div v-if="!rows.length" class="empty">暂无可用应用…</div>
    <div v-for="app in rows" :key="app.pid" class="row">
      <!-- 左列：状态点 + 图标 + 名称 + 电平条 -->
      <div class="left">
        <span class="dot" :class="{ playing: app.playing }" :title="app.playing ? '正在播放' : '未播放'" />
        <img v-if="icons[app.pid]" :src="icons[app.pid]" class="app-icon" :alt="app.name" />
        <div v-else class="app-icon placeholder" />
        <div class="info">
          <MarqueeText class="name" :text="app.name" />
          <div class="meter"><i :style="{ width: levelPct(app.pid) }" /></div>
        </div>
      </div>
      <!-- 右列：音量滑块 + 共享开关 -->
      <div class="right">
        <div class="vol">
          <input type="range" min="0" max="100" :value="store.volumes[app.pid] ?? 80"
            :aria-label="`${app.name}转发音量`"
            @input="(e) => setVolume(app.pid, Number((e.target as HTMLInputElement).value))" />
          <span class="pct">{{ store.volumes[app.pid] ?? 80 }}%</span>
        </div>
        <button class="toggle" role="switch" :aria-checked="store.selected.has(app.pid)"
          :aria-label="`共享 ${app.name}`"
          @click="toggleShare(app, !store.selected.has(app.pid))" />
      </div>
    </div>
  </div>
</template>

<style scoped>
.app-list { max-height: 160px; overflow-y: auto; overflow-x: hidden; }
.app-list::-webkit-scrollbar { width: 8px; }
.app-list::-webkit-scrollbar-thumb { background: #cfcfcf; border-radius: 4px; border: 2px solid #fbfbfb; }
.app-list::-webkit-scrollbar-thumb:hover { background: #b5b5b5; }
.app-list::-webkit-scrollbar-track { background: transparent; }
.empty { padding: 16px 2px; font-size: 12px; color: #8a8a8a; text-align: center; }
.row { display: flex; align-items: center; gap: 10px; padding: 8px 10px 8px 2px; border-bottom: 1px solid #e5e5e5; }
.row:last-child { border-bottom: none; }
.left { display: flex; align-items: center; gap: 8px; flex: 1; min-width: 0; }
.dot { width: 8px; height: 8px; border-radius: 50%; background: #d32f2f; flex-shrink: 0; }
.dot.playing { background: #2e7d32; }
.app-icon { width: 18px; height: 18px; border-radius: 3px; flex-shrink: 0; object-fit: contain; }
.app-icon.placeholder { background: #e0e0e0; }
.info { flex: 1; min-width: 0; }
.name { font-size: 12.5px; font-weight: 500; line-height: 1.3; }
.meter { width: 50%; height: 4px; border-radius: 2px; background: #ececec; overflow: hidden; margin-top: 4px; }
.meter i { display: block; height: 100%; width: 0; background: #0067c0; transition: width .15s; }
.right { display: flex; align-items: center; gap: 8px; flex-shrink: 0; }
.vol { display: flex; align-items: center; gap: 5px; }
.vol input[type=range] { width: 90px; accent-color: #0067c0; height: 3px; cursor: pointer; }
.vol .pct { font-size: 10px; color: #5d5d5d; width: 28px; text-align: right; }
.toggle { position: relative; width: 34px; height: 18px; border-radius: 9px; background: transparent; border: 1px solid #d6d6d6; cursor: pointer; transition: .15s; flex-shrink: 0; padding: 0; }
.toggle::after { content: ""; position: absolute; top: 2px; left: 2px; width: 12px; height: 12px; border-radius: 50%; background: #5d5d5d; transition: .15s; }
.toggle[aria-checked="true"] { background: #0067c0; border-color: #0067c0; }
.toggle[aria-checked="true"]::after { left: 18px; background: #fff; }
.toggle:hover { border-color: #5d5d5d; }
.toggle[aria-checked="true"]:hover { background: #1975c5; }
.toggle:focus-visible { outline: 2px solid #0067c0; outline-offset: 2px; }
</style>
