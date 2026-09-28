<script setup lang="ts">
/**
 * 主窗口：组合全部组件（对应原型 v5）
 */
import { onMounted } from 'vue'
import { store, initStore, toggleMaster, quitApp } from '../stores/app'
import AppDropdown from '../components/AppDropdown.vue'
import SharedPanel from '../components/SharedPanel.vue'
import MicSelect from '../components/MicSelect.vue'
import Onboarding from '../components/Onboarding.vue'

onMounted(async () => {
  await initStore()
  store.showOnboarding = !store.onboardingDone || !store.cableInstalled
})
</script>

<template>
  <div class="app">
    <div class="titlebar">
      <button class="wizard-btn" @click="store.showOnboarding = true">配置向导</button>
      <span class="state">
        <span class="dot" :class="store.cableInstalled ? 'ok' : 'err'" />
        <span>{{ store.cableInstalled ? '虚拟麦就绪' : '虚拟麦不可用' }}</span>
      </span>
    </div>

    <!-- 异常横幅（PRD·异常场景） -->
    <div v-if="store.micUnavailable" class="banner err" role="alert">
      麦克风不可用：好友将听不到你的说话声（音乐共享不受影响）
    </div>
    <div v-if="!store.cableInstalled" class="banner err" role="alert">
      未检测到虚拟声卡，共享已停止
      <button class="btn-sm" @click="store.showOnboarding = true">开始安装</button>
    </div>

    <!-- 共享总开关（AC-03）：文案 开启共享/关闭共享 -->
    <div class="card master">
      <div class="lab"><b>{{ store.masterOn ? '开启共享' : '关闭共享' }}</b></div>
      <button class="switch" role="switch" :aria-checked="store.masterOn"
        aria-label="共享总开关" @click="toggleMaster(!store.masterOn)" />
    </div>

    <!-- 应用下拉 + 已共享栏位 -->
    <div class="card">
      <AppDropdown />
      <SharedPanel />
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
body { display: flex; justify-content: center; padding: 12px 14px; overflow: hidden; }
.app { width: 100%; max-width: 400px; margin: 0 auto; display: flex; flex-direction: column; height: 100%; }
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
</style>
