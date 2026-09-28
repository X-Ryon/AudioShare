<script setup lang="ts">
/**
 * 应用下拉复选框：仅显示应用名（TC-016），收起时显示摘要（TC-017）
 */
import { ref, computed } from 'vue'
import { store, toggleShare, clearAllShared } from '../stores/app'

const open = ref(false)

const selectedNames = computed(() =>
  store.apps.filter(a => store.selected.has(a.pid)).map(a => a.name)
)
const summary = computed(() =>
  selectedNames.value.length ? selectedNames.value.join('、') : '选择要共享的应用'
)

function toggle(e: Event) {
  e.stopPropagation()
  open.value = !open.value
}
function onDocClick() { open.value = false }
document.addEventListener('click', onDocClick)
</script>

<template>
  <label class="fieldlab">共享音频</label>
  <div class="dd" :class="{ open }">
    <button class="dd-btn" type="button" aria-haspopup="listbox" :aria-expanded="open" @click="toggle">
      <span class="txt" :class="{ placeholder: !selectedNames.length }">{{ summary }}</span>
      <span v-if="selectedNames.length" class="clear-btn" role="button" tabindex="0" title="清空已选应用"
        @click.stop="clearAllShared" @keydown.enter.prevent="clearAllShared">
        <svg viewBox="0 0 12 12"><path d="M3 3l6 6M9 3l-6 6" stroke="currentColor" stroke-width="1.4" fill="none" stroke-linecap="round"/></svg>
      </span>
      <svg class="chev" viewBox="0 0 12 12"><path d="M2 4l4 4 4-4" stroke="#5d5d5d" stroke-width="1.4" fill="none" stroke-linecap="round"/></svg>
    </button>
    <div class="dd-panel" role="listbox" aria-label="具备扬声器权限的应用">
      <div v-for="app in store.apps" :key="app.pid" class="dd-item" @click="toggleShare(app, !store.selected.has(app.pid))">
        <button class="cb" type="button" role="checkbox"
          :aria-checked="store.selected.has(app.pid)" :aria-label="`共享 ${app.name}`" tabindex="-1" />
        <span class="appname">{{ app.name }}</span>
        <span v-if="app.playing" class="stat playing" title="正在播放">播放中</span>
        <span v-else class="stat" title="当前无声音输出">未播放</span>
      </div>
      <div v-if="!store.apps.length" class="dd-empty">正在枚举应用…</div>
    </div>
  </div>
</template>

<style scoped>
.fieldlab { font-size: 11px; color: #5d5d5d; margin-bottom: 5px; display: block; }
.dd { position: relative; background: #fff; border: 1px solid #d6d6d6; border-radius: 4px; font-size: 13px; }
.dd:hover { background: #f9f9f9; }
.dd-btn { width: 100%; display: flex; align-items: center; gap: 8px; padding: 7px 10px; background: transparent; border: none; cursor: pointer; font: inherit; color: #1b1b1b; text-align: left; }
.dd-btn .txt { flex: 1; overflow: hidden; text-overflow: ellipsis; white-space: nowrap; }
.dd-btn .txt.placeholder { color: #8a8a8a; }
.chev { width: 10px; height: 10px; flex-shrink: 0; }
.clear-btn { width: 20px; height: 20px; display: flex; align-items: center; justify-content: center; background: transparent; border: none; cursor: pointer; padding: 0; color: #8a8a8a; border-radius: 3px; flex-shrink: 0; }
.clear-btn:hover { color: #c42b1c; background: #fdf3f2; }
.clear-btn svg { width: 10px; height: 10px; }
.dd-panel { display: none; position: absolute; left: -1px; right: -1px; top: calc(100% + 4px); z-index: 5; background: #fff; border: 1px solid #d6d6d6; border-radius: 4px; box-shadow: 0 4px 14px rgba(0,0,0,.12); max-height: 260px; overflow-y: auto; }
.dd.open .dd-panel { display: block; }
.dd-item { display: flex; align-items: center; gap: 10px; padding: 6px 10px; border-bottom: 1px solid #e5e5e5; cursor: pointer; }
.dd-item:last-child { border-bottom: none; }
.dd-item:hover { background: #e5f1fb; }
.appname { flex: 1; font-size: 12.5px; }
.stat { flex-shrink: 0; font-size: 10.5px; color: #8a8a8a; }
.stat.playing { color: #107c10; }
.dd-empty { padding: 10px; font-size: 12px; color: #8a8a8a; text-align: center; }
.cb { width: 16px; height: 16px; border-radius: 3px; border: 1px solid #d6d6d6; flex-shrink: 0; position: relative; background: #fff; padding: 0; }
.cb[aria-checked="true"] { background: #0067c0; border-color: #0067c0; }
.cb[aria-checked="true"]::after { content: ""; position: absolute; left: 4.5px; top: 1.5px; width: 4px; height: 8px; border: solid #fff; border-width: 0 1.7px 1.7px 0; transform: rotate(40deg); }
</style>
