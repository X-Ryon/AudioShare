<script setup lang="ts">
/**
 * 已共享应用栏位：应用名 + 实时电平 + 音量滑杆，限高3行溢出滚动（TC-009/TC-010）
 */
import { computed } from 'vue'
import { store, setVolume } from '../stores/app'

const rows = computed(() =>
  store.apps.filter(a => store.selected.has(a.pid))
)

/** RMS(0-1) → 电平条宽度百分比（对数感放大） */
function levelPct(pid: number): string {
  const rms = store.levels[pid] ?? 0
  return Math.min(100, Math.round(rms * 160)) + '%'
}
</script>

<template>
  <div id="selPanel" aria-label="已共享应用">
    <div v-if="!rows.length" class="selempty">勾选上方应用开始共享</div>
    <div v-for="app in rows" :key="app.pid" class="selrow">
      <div class="name">{{ app.name }}<span>{{ app.exe }}</span></div>
      <div class="srcmeter"><i :style="{ width: store.masterOn ? levelPct(app.pid) : '0%' }" /></div>
      <div class="volwrap">
        <input type="range" min="0" max="100" :value="store.volumes[app.pid] ?? 80"
          :aria-label="`${app.name}共享音量`" @input="(e) => setVolume(app.pid, Number((e.target as HTMLInputElement).value))" />
        <span class="pct">{{ store.volumes[app.pid] ?? 80 }}%</span>
      </div>
    </div>
  </div>
</template>

<style scoped>
#selPanel { max-height: 136px; overflow-y: auto; margin-top: 8px; }
#selPanel::-webkit-scrollbar { width: 8px; }
#selPanel::-webkit-scrollbar-thumb { background: #cfcfcf; border-radius: 4px; border: 2px solid #fbfbfb; }
#selPanel::-webkit-scrollbar-thumb:hover { background: #b5b5b5; }
#selPanel::-webkit-scrollbar-track { background: transparent; }
.selempty { padding: 10px 2px; font-size: 11.5px; color: #8a8a8a; text-align: center; }
.selrow { display: flex; align-items: center; gap: 10px; padding: 6px 2px; border-bottom: 1px solid #e5e5e5; }
.selrow:last-child { border-bottom: none; }
.selrow .name { width: 96px; flex-shrink: 0; font-size: 12.5px; font-weight: 600; line-height: 1.25; }
.selrow .name span { display: block; font-size: 10px; color: #8a8a8a; font-weight: 400; }
.srcmeter { width: 56px; height: 5px; border-radius: 2px; background: #ececec; overflow: hidden; flex-shrink: 0; }
.srcmeter i { display: block; height: 100%; width: 0; background: #0067c0; transition: width .15s; }
.volwrap { flex: 1; display: flex; align-items: center; gap: 7px; min-width: 0; }
.volwrap input[type=range] { flex: 1; accent-color: #0067c0; height: 3px; cursor: pointer; }
.volwrap .pct { font-size: 10.5px; color: #5d5d5d; width: 31px; text-align: right; flex-shrink: 0; }
</style>
