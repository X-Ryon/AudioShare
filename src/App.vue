<script setup lang="ts">
/**
 * 窗口分发：后端以 label="float" 创建悬浮窗，同一前端包按窗口 label 渲染不同页面（第3期）
 */
import { getCurrentWindow } from '@tauri-apps/api/window'
import MainWindow from './views/MainWindow.vue'
import FloatWindow from './views/FloatWindow.vue'

const isFloat = getCurrentWindow().label === 'float'
// 悬浮窗窗口：inline 透明背景（优先级高于 MainWindow 全局浅色背景，避免白色矩形）
if (isFloat) {
  document.documentElement.style.background = 'transparent'
  document.body.style.background = 'transparent'
}
</script>

<template>
  <FloatWindow v-if="isFloat" />
  <MainWindow v-else />
</template>
