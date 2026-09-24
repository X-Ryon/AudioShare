<script setup lang="ts">
/**
 * 首次配置引导：三步级联点亮（TC-004）
 * 第1步引导安装 VB-CABLE（打开官方下载页），第2步选择输入设备，第3步完成
 */
import { ref, computed } from 'vue'
import { openUrl } from '@tauri-apps/plugin-opener'
import { store, finishOnboarding } from '../stores/app'

const step = computed(() => {
  if (!store.cableInstalled) return 1
  return 2
})
const done2 = ref(false)
const emit = defineEmits<{ close: [] }>()

async function install() {
  // 引导到 VB-CABLE 官方下载页（个人免费）
  try {
    await openUrl('https://vb-audio.com/Cable/')
  } catch { /* 打开失败也允许继续 */ }
  // 提示：安装完成后重启本应用
  alert('请在打开的网页下载并安装 VB-CABLE（约1分钟，需要管理员权限）。\n安装完成后重启 AudioRouter，会自动检测到虚拟声卡。')
}
</script>

<template>
  <div class="overlay show" role="dialog" aria-modal="true" aria-label="首次配置引导">
    <div class="modal">
      <h2>欢迎使用 AudioRouter</h2>
      <p class="sub">三步完成配置，之后每天一键共享（目标：5 分钟内搞定）</p>

      <div class="step" :class="{ done: step > 1, current: step === 1 }">
        <div class="num">{{ step > 1 ? '✓' : '1' }}</div>
        <div class="c">
          <b>安装虚拟声卡</b>
          <p>约 1 分钟，需要管理员权限（UAC 弹窗请点"是"）。装完重启本应用，自动检测。{{ store.cableInstalled ? '' : '' }}</p>
        </div>
        <button v-if="step === 1" class="btn" @click="install">下载安装</button>
        <span v-else class="status done">已安装 ✓</span>
      </div>

      <div class="step" :class="{ done: done2, current: step === 2 && !done2, dim: step < 2 }">
        <div class="num">{{ done2 ? '✓' : '2' }}</div>
        <div class="c">
          <b>聊天软件里选输入设备</b>
          <p>在微信 / QQ / 游戏语音的麦克风设置里，把输入设备选为「CABLE Input (VB-Audio Virtual Cable)」。选一次，以后不用再改。</p>
        </div>
        <button v-if="step === 2 && !done2" class="btn" @click="done2 = true">我已选好</button>
        <span v-else-if="done2" class="status done">已选择 ✓</span>
        <span v-else class="status todo">等待第 1 步</span>
      </div>

      <div class="step" :class="{ current: done2, dim: !done2 }">
        <div class="num">3</div>
        <div class="c">
          <b>完成</b>
          <p>回到主界面，勾选想共享的应用，好友就能同时听到你说话和音乐了。</p>
        </div>
        <span v-if="!done2" class="status todo">等待第 2 步</span>
        <span v-else class="status done">可以开始了 ✓</span>
      </div>

      <div class="modal-foot">
        <button class="btn ghost" @click="emit('close')">稍后再说</button>
        <button class="btn" :disabled="!done2" @click="finishOnboarding()">开始使用</button>
      </div>
    </div>
  </div>
</template>

<style scoped>
.overlay { position: fixed; inset: 0; background: rgba(0,0,0,.32); display: flex; align-items: center; justify-content: center; padding: 20px; z-index: 20; }
.modal { background: #fff; border-radius: 8px; max-width: 460px; width: 100%; padding: 22px 24px; box-shadow: 0 16px 44px rgba(0,0,0,.22); }
h2 { font-size: 15px; font-weight: 600; margin-bottom: 2px; }
.sub { font-size: 11.5px; color: #5d5d5d; margin-bottom: 14px; }
.step { display: flex; gap: 12px; padding: 11px 0; border-top: 1px solid #e5e5e5; }
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
</style>
