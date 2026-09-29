<script setup lang="ts">
/**
 * 物理麦克风自定义下拉（TC-008）+ 实时电平指示
 * 不使用原生 select：其弹出列表宽度由最长选项决定，会溢出 400px 窗口；
 * 自定义面板宽度锁定在容器内，长文本由 MarqueeText 悬停跑马灯展示。
 */
import { ref, computed, onMounted, onBeforeUnmount } from 'vue'
import { store, setMic, setRemember } from '../stores/app'
import MarqueeText from './MarqueeText.vue'

const open = ref(false)

const currentName = computed(() => {
  // 不提供“系统默认”选项：默认麦克风必须是 CABLE，采它会回路自激
  const m = store.mics.find((x) => x.id === store.micId)
  return m ? m.name + (m.is_default ? '（默认）' : '') : '未选择麦克风'
})

function toggle(e: Event) {
  e.stopPropagation()
  open.value = !open.value
}
function onDocClick() { open.value = false }
onMounted(() => document.addEventListener('click', onDocClick))
onBeforeUnmount(() => document.removeEventListener('click', onDocClick))

function pick(id: string) {
  open.value = false
  if (id !== store.micId) setMic(id)
}

/** RMS(0-1) → 电平条宽度百分比（对数感放大） */
function micLevelPct(): string {
  return Math.min(100, Math.round(store.micLevel * 160)) + '%'
}
</script>

<template>
  <label class="fieldlab">麦克风</label>
  <div class="dd" :class="{ open }">
    <button class="dd-btn" type="button" aria-haspopup="listbox" :aria-expanded="open"
      aria-label="选择物理麦克风设备" @click="toggle">
      <MarqueeText class="txt" :text="currentName" />
      <svg class="chev" viewBox="0 0 12 12"><path d="M2 4l4 4 4-4" stroke="#5d5d5d" stroke-width="1.4" fill="none" stroke-linecap="round"/></svg>
    </button>
    <div class="dd-panel" role="listbox" aria-label="选择物理麦克风设备" @click.stop>
      <div v-for="m in store.mics" :key="m.id" class="dd-item" :class="{ sel: store.micId === m.id }"
        role="option" :aria-selected="store.micId === m.id" @click.stop="pick(m.id)">
        <MarqueeText class="optname" :text="m.name + (m.is_default ? '（默认）' : '')" />
      </div>
    </div>
  </div>
  <div class="mic-meter-row">
    <span class="mic-meter-label">实时输入音量</span>
    <div class="mic-meter"><i :style="{ width: micLevelPct() }" /></div>
  </div>
  <div class="mic-foot">
    <label class="remember-lab">
      <input type="checkbox" :checked="store.rememberOn"
        aria-label="记住选择，下次启动自动恢复" 
        @change="(e) => setRemember((e.target as HTMLInputElement).checked)" />
      记住选择
    </label>
  </div>
</template>

<style scoped>
.fieldlab { font-size: 11px; color: #5d5d5d; margin-bottom: 5px; display: block; }
.dd { position: relative; background: #fff; border: 1px solid #d6d6d6; border-radius: 4px; font-size: 13px; }
.dd:hover { background: #f9f9f9; }
.dd-btn { width: 100%; display: flex; align-items: center; gap: 8px; padding: 7px 10px; background: transparent; border: none; cursor: pointer; font: inherit; color: #1b1b1b; text-align: left; }
.dd-btn:focus-visible { outline: 2px solid #0067c0; outline-offset: 1px; }
.dd-btn .txt { flex: 1; min-width: 0; }
.chev { width: 10px; height: 10px; flex-shrink: 0; }
.dd-panel { display: none; position: absolute; left: -1px; right: -1px; top: calc(100% + 4px); z-index: 5; background: #fff; border: 1px solid #d6d6d6; border-radius: 4px; box-shadow: 0 4px 14px rgba(0,0,0,.12); max-height: 180px; overflow-y: auto; }
.dd.open .dd-panel { display: block; }
.dd-item { display: flex; align-items: center; gap: 8px; padding: 6px 10px; border-bottom: 1px solid #e5e5e5; cursor: pointer; }
.dd-item:last-child { border-bottom: none; }
.dd-item:hover { background: #e5f1fb; }
.dd-item.sel { background: #e5f1fb; }
.dd-item.sel .optname { font-weight: 600; }
.optname { flex: 1; min-width: 0; font-size: 12.5px; }
.mic-meter-row { display: flex; align-items: center; gap: 8px; margin-top: 8px; }
.mic-meter-label { font-size: 11px; color: #5d5d5d; flex-shrink: 0; }
.mic-meter { flex: 1; height: 5px; border-radius: 2px; background: #ececec; overflow: hidden; }
.mic-meter i { display: block; height: 100%; width: 0; background: #0f7b0f; transition: width .15s; }
.mic-foot { display: flex; justify-content: flex-end; margin-top: 6px; }
.remember-lab { display: flex; align-items: center; gap: 5px; font-size: 11px; color: #5d5d5d; cursor: pointer; user-select: none; }
.remember-lab input { accent-color: #0067c0; width: 13px; height: 13px; cursor: pointer; margin: 0; }
</style>
