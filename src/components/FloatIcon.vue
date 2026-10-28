<script setup lang="ts">
/**
 * 悬浮窗内联 SVG 图标（Win11 Fluent 线性风格，16px 网格）
 *
 * 为什么不用 emoji：emoji 在不同 Windows 版本 / 缩放比下渲染各异（有的彩色、有的变方块），
 * 且与深色亚克力底不协调（FR-020）。改为受控的线性 SVG，形状与颜色完全可控。
 *
 * 麦克风两态统一（FR-018）：开麦/闭麦使用【同一个麦克风图形】，闭麦仅改变颜色并叠加斜杠，
 * 用户无需重新识别语义（此前开麦是麦克风、闭麦变喇叭）。
 *
 * 追溯：PRD FR-018 / FR-020
 */
withDefaults(defineProps<{
  /** 图标名 */
  name: 'grip' | 'rotate' | 'mic' | 'pause' | 'monitor' | 'close'
  /** 麦克风闭麦态：叠加斜杠（仅 name === 'mic' 生效） */
  muted?: boolean
}>(), { muted: false })
</script>

<template>
  <svg
    class="fico"
    viewBox="0 0 16 16"
    fill="none"
    stroke="currentColor"
    stroke-width="1.5"
    stroke-linecap="round"
    stroke-linejoin="round"
    aria-hidden="true"
    focusable="false"
  >
    <!-- 拖动手柄：6 点 grip -->
    <g v-if="name === 'grip'" fill="currentColor" stroke="none">
      <circle cx="5" cy="4" r="1.25" />
      <circle cx="11" cy="4" r="1.25" />
      <circle cx="5" cy="8" r="1.25" />
      <circle cx="11" cy="8" r="1.25" />
      <circle cx="5" cy="12" r="1.25" />
      <circle cx="11" cy="12" r="1.25" />
    </g>

    <!-- 旋转：环形箭头 ⟳ -->
    <g v-else-if="name === 'rotate'">
      <path d="M13 8A5 5 0 1 1 8 3" />
      <polyline points="8,0.7 8,3 10.3,3" />
    </g>

    <!-- 麦克风：开麦/闭麦同一图形，闭麦叠加斜杠 -->
    <g v-else-if="name === 'mic'">
      <rect x="6" y="1.6" width="4" height="7.6" rx="2" />
      <path d="M3.6 7.2v1.2a4.4 4.4 0 0 0 8.8 0V7.2" />
      <line x1="8" y1="12.8" x2="8" y2="14.4" />
      <line x1="5.6" y1="14.4" x2="10.4" y2="14.4" />
      <line v-if="muted" x1="2.6" y1="13.4" x2="13.4" y2="2.6" />
    </g>

    <!-- 暂停（媒体键透传） -->
    <g v-else-if="name === 'pause'" fill="currentColor" stroke="none">
      <rect x="4.4" y="3.4" width="2.5" height="9.2" rx="0.9" />
      <rect x="9.1" y="3.4" width="2.5" height="9.2" rx="0.9" />
    </g>

    <!-- 显示器（打开主界面） -->
    <g v-else-if="name === 'monitor'">
      <rect x="1.6" y="2.6" width="12.8" height="8.8" rx="1.6" />
      <line x1="5.6" y1="14.4" x2="10.4" y2="14.4" />
      <line x1="8" y1="11.4" x2="8" y2="14.4" />
    </g>

    <!-- 关闭（✕） -->
    <g v-else-if="name === 'close'" stroke-width="1.7">
      <path d="M4.4 4.4l7.2 7.2M11.6 4.4l-7.2 7.2" />
    </g>
  </svg>
</template>

<style scoped>
/* pointer-events:none —— 让鼠标事件落在宿主按钮上，
   既保证按钮的原生 title 提示正常弹出，也避免 SVG 吞掉 hover 态 */
.fico {
  width: 16px;
  height: 16px;
  display: block;
  pointer-events: none;
}
</style>
