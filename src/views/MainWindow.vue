<script setup lang="ts">
/**
 * 主窗口：组合全部组件（对应原型 v6）
 */
import { onMounted, ref } from 'vue'
import { listen } from '@tauri-apps/api/event'
import { check } from '@tauri-apps/plugin-updater'
import { store, initStore, toggleMaster, quitApp } from '../stores/app'
import AppList from '../components/AppList.vue'
import MicSelect from '../components/MicSelect.vue'
import Onboarding from '../components/Onboarding.vue'

// 更新状态
const updateStatus = ref<'idle' | 'checking' | 'available' | 'downloading' | 'installing' | 'latest' | 'error'>('idle')
const updateInfo = ref({ version: '', body: '' })
const updateProgress = ref(0)
const updateError = ref('')

/** 检查并安装更新 */
async function doCheckUpdate() {
  updateStatus.value = 'checking'
  updateError.value = ''
  try {
    const update = await check()
    if (!update) {
      updateStatus.value = 'latest'
      return
    }
    updateInfo.value = { version: update.version, body: update.body || '' }
    updateStatus.value = 'available'
    // 自动下载并安装
    updateStatus.value = 'downloading'
    await update.downloadAndInstall((event) => {
      if (event.event === 'Progress') {
        // Progress 事件只有 chunkLength，无总大小，用累计字节数近似显示
        updateProgress.value = Math.min(99, updateProgress.value + 5)
      } else if (event.event === 'Finished') {
        updateStatus.value = 'installing'
      }
    })
    // 安装完成后应用会重启
  } catch (e: any) {
    updateStatus.value = 'error'
    updateError.value = e?.message || String(e)
  }
}

onMounted(async () => {
  await initStore()
  store.showOnboarding = !store.onboardingDone || !store.cableInstalled

  // 监听托盘“检查更新”事件
  await listen('tray-check-update', () => { doCheckUpdate() })
})
</script>

<template>
  <div class="app">
    <div class="titlebar">
      <button class="wizard-btn" @click="store.showOnboarding = true">配置向导</button>
      <span class="state">
        <span class="dot" :class="store.cableInstalled ? 'ok' : 'err'" />
        <span>{{ store.cableInstalled ? '虚拟麦克风就绪' : '虚拟麦克风不可用' }}</span>
      </span>
    </div>

    <!-- 异常横幅（PRD·异常场景） -->
    <div v-if="store.micUnavailable" class="banner err" role="alert">
      麦克风不可用
    </div>
    <div v-if="!store.cableInstalled" class="banner err" role="alert">
      未检测到虚拟声卡
      <button class="btn-sm" @click="store.showOnboarding = true">开始安装</button>
    </div>

    <!-- 共享总开关（AC-03）：文案 开启共享/关闭共享 -->
    <div class="card master">
      <div class="lab"><b>{{ store.masterOn ? '开启共享' : '关闭共享' }}</b></div>
      <button class="switch" role="switch" :aria-checked="store.masterOn"
        aria-label="共享总开关" @click="toggleMaster(!store.masterOn)" />
    </div>

    <!-- 应用列表（含所有音频会话程序） -->
    <div class="card">
      <label class="fieldlab">应用列表</label>
      <AppList />
    </div>

    <!-- 物理麦克风 -->
    <div class="card">
      <MicSelect />
    </div>


    <!-- 关闭=隐藏到托盘（lib.rs 已处理），托盘退出走 quit_app -->
    <div class="footnote quit">
      <a href="#" @click.prevent="quitApp">退出 AudioShare</a>
      <span v-if="store.version" class="ver">v{{ store.version }}</span>
    </div>

    <Onboarding v-if="store.showOnboarding" @close="store.showOnboarding = false" />

    <!-- 更新状态提示 -->
    <div v-if="updateStatus !== 'idle'" class="update-toast">
      <template v-if="updateStatus === 'checking'">🔍 正在检查更新…</template>
      <template v-else-if="updateStatus === 'latest'">✅ 已是最新版本</template>
      <template v-else-if="updateStatus === 'available'">📦 发现新版本 v{{ updateInfo.version }}，正在下载…</template>
      <template v-else-if="updateStatus === 'downloading'">⬇️ 下载中 {{ updateProgress }}%</template>
      <template v-else-if="updateStatus === 'installing'">⚙️ 安装中，即将重启…</template>
      <template v-else-if="updateStatus === 'error'">❌ 更新失败：{{ updateError }}</template>
    </div>
  </div>
</template>

<style>
/* Win11 全局风格：400x550 固定窗口（1080p@100%），2K/4K 由 Windows DPI 缩放自动等比放大 */
* { box-sizing: border-box; margin: 0; padding: 0; }
html, body, #app {
  font-family: "Segoe UI Variable", "Segoe UI", "Microsoft YaHei UI", "Microsoft YaHei", system-ui, sans-serif;
  background: #f3f3f3; color: #1b1b1b; font-size: 13px;
}
html, body, #app { height: 100%; }
/* 仅"已勾选应用"列表允许滚动，其余容器禁止滚动 */
body { overflow: hidden; }
.app { width: 100%; max-width: 400px; margin: 0 auto; display: flex; flex-direction: column; height: 100%; padding: 12px 14px; }
.footnote.quit { margin-top: auto; padding-bottom: 0; display: flex; justify-content: space-between; align-items: center; }
.footnote .ver { color: #b3b3b3; font-size: 10px; }
.card { background: #fbfbfb; border: 1px solid #e5e5e5; border-radius: 8px; padding: 10px 12px; margin-bottom: 6px; flex-shrink: 0; }
.titlebar { display: flex; align-items: center; gap: 8px; padding: 0 2px 8px; flex-shrink: 0; }
.wizard-btn { border: 1px solid #0067c0; border-radius: 4px; padding: 4px 12px; font-size: 12px; cursor: pointer; background: #0067c0; color: #fff; font-family: inherit; }
.wizard-btn:hover { background: #1975c5; }
.titlebar .state { margin-left: auto; font-size: 12px; color: #5d5d5d; display: flex; align-items: center; gap: 6px; }
.dot { width: 7px; height: 7px; border-radius: 50%; }
.dot.ok { background: #0f7b0f; }
.dot.err { background: #c42b1c; }
.master { display: flex; align-items: center; gap: 12px; }
.master .lab { flex: 1; }
.master .lab b { font-size: 13px; font-weight: 600; }
.switch { position: relative; width: 40px; height: 20px; border-radius: 10px; background: transparent; border: 1px solid #d6d6d6; cursor: pointer; transition: .15s; flex-shrink: 0; }
.switch::after { content: ""; position: absolute; top: 3px; left: 3px; width: 12px; height: 12px; border-radius: 50%; background: #5d5d5d; transition: .15s; }
.switch[aria-checked="true"] { background: #0067c0; border-color: #0067c0; }
.switch[aria-checked="true"]::after { left: 23px; background: #fff; }
.switch:hover { border-color: #5d5d5d; }
.switch[aria-checked="true"]:hover { background: #1975c5; }
.switch:focus-visible { outline: 2px solid #0067c0; outline-offset: 2px; }
.banner { border-radius: 6px; padding: 6px 10px; font-size: 12px; margin-bottom: 6px; display: flex; align-items: center; gap: 8px; border: 1px solid transparent; flex-shrink: 0; }
.banner.err { background: #fdf3f2; color: #c42b1c; border-color: #f0c7c3; }
.banner .btn-sm { margin-left: auto; }
.btn-sm { border: 1px solid #0067c0; border-radius: 4px; padding: 4px 12px; font-size: 11.5px; cursor: pointer; background: #0067c0; color: #fff; font-family: inherit; }
.btn-sm:hover { background: #1975c5; }
.footnote { font-size: 10.5px; color: #8a8a8a; line-height: 1.6; padding: 1px 2px; flex-shrink: 0; }
.footnote.quit { margin-top: auto; }
.footnote a { color: #8a8a8a; }
.fieldlab { font-size: 11px; color: #5d5d5d; margin-bottom: 5px; display: block; }
.update-toast { position: fixed; bottom: 40px; left: 50%; transform: translateX(-50%); background: #1b1b1b; color: #fff; padding: 8px 16px; border-radius: 6px; font-size: 12px; white-space: nowrap; z-index: 100; box-shadow: 0 2px 8px rgba(0,0,0,.2); }
</style>
