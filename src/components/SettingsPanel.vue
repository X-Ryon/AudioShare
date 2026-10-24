<script setup lang="ts">
/**
 * 设置面板：4 条热键按键捕获录制改键（TC-020/021）
 * 打开面板时注销全部热键——已注册的全局热键会拦截 keydown，窗口收不到按键；
 * 关闭面板时按当前配置恢复注册（set_hotkeys_recording）。
 */
import { ref, onMounted, onBeforeUnmount } from 'vue'
import { store, saveHotkeys, setHotkeysRecording, logFrontend } from '../stores/app'
import { HotkeyConfig, DEFAULT_HOTKEYS, formatCombo, isModifierCode } from '../utils/hotkey'

const emit = defineEmits<{ close: [] }>()

type FieldKey = keyof HotkeyConfig
const FIELDS: { key: FieldKey; label: string }[] = [
  { key: 'toggle_share', label: '开/关共享' },
  { key: 'play_pause', label: '播放/暂停音乐' },
  { key: 'prev_track', label: '上一首' },
  { key: 'next_track', label: '下一首' },
]

/** 当前录制中的字段；null=非录制态 */
const recording = ref<FieldKey | null>(null)
/** 录制中已按下的修饰键预览（如 "Ctrl+Shift+"） */
const pendingMods = ref('')
const error = ref('')

onMounted(async () => {
  await setHotkeysRecording(true)
  window.addEventListener('keydown', onKeydown)
})
onBeforeUnmount(async () => {
  window.removeEventListener('keydown', onKeydown)
  await setHotkeysRecording(false)
})

function onKeydown(e: KeyboardEvent) {
  if (!recording.value) return
  e.preventDefault()
  e.stopPropagation()
  // Escape：取消录制，保持旧值
  if (e.code === 'Escape') {
    recording.value = null
    pendingMods.value = ''
    return
  }
  // Delete/Backspace：清空 = 禁用该条
  if (e.code === 'Delete' || e.code === 'Backspace') {
    commit('')
    return
  }
  // 修饰键：只累积预览，不结束录制
  if (isModifierCode(e.code)) {
    const parts: string[] = []
    if (e.ctrlKey) parts.push('Ctrl')
    if (e.shiftKey) parts.push('Shift')
    if (e.altKey) parts.push('Alt')
    if (e.metaKey) parts.push('Win')
    pendingMods.value = parts.length ? parts.join('+') + '+' : ''
    return
  }
  const combo = formatCombo(e)
  if (!combo) {
    error.value = '该按键不支持，或需至少包含一个修饰键（Ctrl/Shift/Alt/Win）'
    return
  }
  commit(combo)
}

/** 提交录制结果（空串=禁用） */
async function commit(combo: string) {
  const key = recording.value
  if (!key) return
  recording.value = null
  pendingMods.value = ''
  error.value = ''
  const next: HotkeyConfig = { ...store.hotkeys, [key]: combo }
  try {
    await saveHotkeys(next)
  } catch (err) {
    error.value = String(err)
    logFrontend('error', `set_hotkeys 失败: ${err}`)
  } finally {
    // 保存会触发全量注册（成功=新配置 / 失败=回滚旧配置），面板仍处于录制模式，
    // 必须立即重新注销，否则已注册组合会拦截后续录制的按键（TC-020）
    await setHotkeysRecording(true)
  }
}

function startRecord(key: FieldKey) {
  recording.value = key
  pendingMods.value = ''
  error.value = ''
}

async function restoreDefaults() {
  error.value = ''
  try {
    await saveHotkeys({ ...DEFAULT_HOTKEYS })
  } catch (err) {
    error.value = String(err)
    logFrontend('error', `恢复默认热键失败: ${err}`)
  } finally {
    await setHotkeysRecording(true)
  }
}
</script>

<template>
  <div class="overlay show" role="dialog" aria-modal="true" aria-label="设置">
    <div class="modal">
      <h2>设置</h2>
      <p class="sub">点击键位按钮后按下组合键即可改键（如 Ctrl+Alt+F9）</p>

      <div v-for="f in FIELDS" :key="f.key" class="row">
        <span class="lab">{{ f.label }}</span>
        <button class="keybtn" :class="{ recording: recording === f.key }"
          :aria-label="`修改${f.label}快捷键`"
          @click="startRecord(f.key)">
          <template v-if="recording === f.key">
            {{ pendingMods }}…
          </template>
          <template v-else>
            {{ store.hotkeys[f.key] || '未设置' }}
          </template>
        </button>
      </div>

      <p v-if="recording" class="tip">Esc 取消 · Delete 清除（禁用）· 至少一个修饰键 + 一个按键</p>
      <p v-if="error" class="err">{{ error }}</p>

      <div class="modal-foot">
        <button class="btn ghost" @click="restoreDefaults">恢复默认</button>
        <button class="btn" @click="emit('close')">完成</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.overlay { position: fixed; inset: 0; background: rgba(0,0,0,.32); display: flex; align-items: center; justify-content: center; padding: 16px; z-index: 20; }
.modal { background: #fff; border-radius: 8px; max-width: 360px; width: 100%; padding: 18px 20px; box-shadow: 0 16px 44px rgba(0,0,0,.22); }
h2 { font-size: 15px; font-weight: 600; margin-bottom: 2px; }
.sub { font-size: 11.5px; color: #5d5d5d; margin-bottom: 12px; }
.row { display: flex; align-items: center; justify-content: space-between; gap: 12px; padding: 7px 0; border-top: 1px solid #e5e5e5; }
.lab { font-size: 13px; }
.keybtn { min-width: 150px; text-align: center; border: 1px solid #d6d6d6; border-radius: 4px; padding: 5px 12px; font-size: 11.5px; cursor: pointer; background: #fbfbfb; color: #1b1b1b; font-family: inherit; }
.keybtn:hover { border-color: #0067c0; background: #e5f1fb; }
.keybtn.recording { border-color: #0067c0; background: #e5f1fb; color: #0067c0; }
.tip { font-size: 11px; color: #5d5d5d; margin-top: 8px; line-height: 1.5; }
.err { color: #c42b1c; font-size: 11.5px; margin-top: 8px; line-height: 1.5; }
.modal-foot { display: flex; justify-content: flex-end; gap: 8px; margin-top: 14px; }
.btn { border: 1px solid #0067c0; border-radius: 4px; padding: 5px 14px; font-size: 11.5px; cursor: pointer; background: #0067c0; color: #fff; font-family: inherit; }
.btn:hover { background: #1975c5; }
.btn.ghost { background: transparent; color: #0067c0; }
.btn.ghost:hover { background: #e5f1fb; }
</style>
