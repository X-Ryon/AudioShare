<script setup lang="ts">
/**
 * 首次配置引导：三步级联点亮（TC-004）
 * 第1步：引导用户安装 VB-CABLE（打开安装程序，用户手动完成）
 */
import { ref, computed } from 'vue'
import { invoke } from '@tauri-apps/api/core'
import { openUrl } from '@tauri-apps/plugin-opener'
import { store, finishOnboarding } from '../stores/app'

interface InstallResult { installed: boolean; already: boolean; message: string }
interface AppStatus { cable_installed: boolean; master_on: boolean; onboarding_done: boolean; mic_id: string }

const VB_CABLE_URL = 'https://vb-audio.com/Cable/'

const step = computed(() => {
  if (!store.cableInstalled) return 1
  return 2
})
const done2 = ref(false)
const installing = ref(false)
const installErr = ref('')
const showManualModal = ref(false)
const emit = defineEmits<{ close: [] }>()

async function install() {
  installing.value = true
  installErr.value = ''
  try {
    const r = await invoke<InstallResult>('install_cable')
    if (r.installed) {
      store.cableInstalled = true
      // 检查是否需要重启
      if (r.message.includes('重启')) {
        installErr.value = r.message
        // 不自动 finish_onboarding，等用户重启后重新检测
      } else {
        // 安装成功，立即拉起音频管线
        await invoke('finish_onboarding')
      }
    }
  } catch (e) {
    installErr.value = String(e)
    // 自动安装失败：弹窗提供手动安装途径（官网链接）
    showManualModal.value = true
  } finally {
    installing.value = false
  }
}

async function openOfficial() {
  await openUrl(VB_CABLE_URL)
}

async function recheck() {
  // 用户手动安装后重新检测虚拟声卡
  const s = await invoke<AppStatus>('get_status')
  store.cableInstalled = s.cable_installed
  if (store.cableInstalled) {
    installErr.value = ''
    showManualModal.value = false
    await invoke('finish_onboarding')
  }
}
</script>

<template>
  <div class="overlay show" role="dialog" aria-modal="true" aria-label="首次配置引导">
    <div class="modal">
      <h2>欢迎使用 AudioShare</h2>
      <p class="sub">三步完成配置</p>

      <div class="step" :class="{ done: step > 1, current: step === 1 }">
        <div class="num">{{ step > 1 ? '✓' : '1' }}</div>
        <div class="c">
          <b>安装虚拟声卡</b>
          <p>点击"安装"会打开 VB-CABLE 安装程序，请在弹出的 UAC 窗口点"是"，然后在安装器界面点"Install Driver"。安装完成后可能需要重启系统。</p>
          <p v-if="installErr" class="err">{{ installErr }}</p>
        </div>
        <button v-if="step === 1" class="btn" :disabled="installing" @click="install">
          {{ installing ? '安装中…' : '安装' }}
        </button>
        <span v-else class="status done">已安装 ✓</span>
      </div>

      <div class="step" :class="{ done: done2, current: step === 2 && !done2, dim: step < 2 }">
        <div class="num">{{ done2 ? '✓' : '2' }}</div>
        <div class="c">
          <b>聊天软件里选输入设备</b>
          <p>在微信 / QQ / 游戏语音的麦克风设置里，把输入设备选为「CABLE Input (VB-Audio Virtual Cable)」。</p>
        </div>
        <button v-if="step === 2 && !done2" class="btn" @click="done2 = true">我已选好</button>
        <span v-else-if="done2" class="status done">已选择 ✓</span>
        <span v-else class="status todo">等待第 1 步</span>
      </div>

      <div class="step" :class="{ current: done2, dim: !done2 }">
        <div class="num">3</div>
        <div class="c">
          <b>完成</b>
        </div>
        <span v-if="!done2" class="status todo">等待第 2 步</span>
        <span v-else class="status done">可以开始了 ✓</span>
      </div>

      <div class="modal-foot">
        <button class="btn ghost" @click="emit('close')">稍后再说</button>
        <button class="btn" @click="finishOnboarding()">开始使用</button>
      </div>
    </div>

    <!-- 自动安装失败弹窗：提供手动安装与官网链接 -->
    <div v-if="showManualModal" class="overlay manual" role="dialog" aria-modal="true" aria-label="自动安装失败">
      <div class="modal small">
        <h3>自动安装失败</h3>
        <p class="mmsg">虚拟声卡未能自动安装（可能被 UAC 拒绝或安装器异常）。您可以前往官网手动下载安装，安装完成后回来检测。</p>
        <div class="mactions">
          <button class="btn" @click="openOfficial">前往 VB-Audio 官网</button>
          <button class="btn ghost" @click="recheck">我已手动安装，重新检测</button>
          <button class="btn ghost" @click="showManualModal = false">关闭</button>
        </div>
      </div>
    </div>
  </div>
</template>

<style scoped>
.overlay { position: fixed; inset: 0; background: rgba(0,0,0,.32); display: flex; align-items: center; justify-content: center; padding: 16px; z-index: 20; overflow-y: auto; }
.modal { background: #fff; border-radius: 8px; max-width: 420px; width: 100%; padding: 18px 20px; box-shadow: 0 16px 44px rgba(0,0,0,.22); }
h2 { font-size: 15px; font-weight: 600; margin-bottom: 2px; }
.sub { font-size: 11.5px; color: #5d5d5d; margin-bottom: 14px; }
.step { display: flex; gap: 12px; padding: 9px 0; border-top: 1px solid #e5e5e5; }
.step.dim { opacity: .45; }
.step .num { width: 22px; height: 22px; border-radius: 50%; background: #ececec; color: #5d5d5d; display: flex; align-items: center; justify-content: center; font-size: 11.5px; font-weight: 600; flex-shrink: 0; }
.step.done .num { background: #0f7b0f; color: #fff; }
.step.current .num { background: #0067c0; color: #fff; }
.step .c { flex: 1; }
.step .c b { font-size: 13px; display: block; }
.step .c p { font-size: 11.5px; color: #5d5d5d; margin-top: 3px; line-height: 1.55; }
.step .btn { align-self: center; }
.status { font-size: 11.5px; align-self: center; }
.status.done { color: #0f7b0f; }
.status.todo { color: #8a8a8a; }
.modal-foot { display: flex; justify-content: flex-end; gap: 8px; margin-top: 16px; }
.btn { border: 1px solid #0067c0; border-radius: 4px; padding: 5px 14px; font-size: 11.5px; cursor: pointer; background: #0067c0; color: #fff; font-family: inherit; }
.btn:hover { background: #1975c5; }
.btn:disabled { opacity: .45; cursor: default; }
.btn.ghost { background: transparent; color: #0067c0; }
.btn.ghost:hover { background: #e5f1fb; }
/* 手动安装弹窗（覆盖在引导之上） */
.overlay.manual { z-index: 30; background: rgba(0,0,0,.45); }
.modal.small { max-width: 340px; padding: 16px 18px; }
h3 { font-size: 14px; font-weight: 600; }
.mmsg { font-size: 12px; color: #5d5d5d; margin: 8px 0 14px; line-height: 1.6; }
.mactions { display: flex; flex-direction: column; gap: 8px; }
.mactions .btn { width: 100%; text-align: center; }
</style>
<style scoped>
.err { color: #c42b1c !important; }
</style>
