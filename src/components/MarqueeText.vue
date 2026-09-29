<script setup lang="ts">
/**
 * 单行文本容器：溢出时省略号截断；光标悬停且确实溢出时，
 * 触发慢速循环横向滚动（滚动横幅效果），移开光标即复位。
 */
import { ref, watch, nextTick, onMounted } from 'vue'

const props = defineProps<{ text: string }>()

const inner = ref<HTMLElement | null>(null)
const run = ref(false)   // 悬停且溢出 → 滚动中
const dist = ref(0)      // 溢出像素数
const dur = ref(4)       // 一轮循环时长（秒）

const SPEED = 26 // 滚动速度 px/s（慢速）

function measure() {
  const i = inner.value
  if (!i) { dist.value = 0; return }
  const d = i.scrollWidth - i.clientWidth
  dist.value = d > 1 ? d : 0
  // 往返两段位移占整轮 70%，其余为两端停顿
  dur.value = Math.max(3, ((dist.value / SPEED) * 2) / 0.7)
}

function enter() {
  measure()
  if (dist.value > 0) run.value = true
}
function leave() { run.value = false }

onMounted(measure)
watch(() => props.text, () => nextTick(measure))
</script>

<template>
  <span class="mq" :class="{ run }" @mouseenter="enter" @mouseleave="leave">
    <span ref="inner" class="mq-t"
      :style="run ? { '--mq-d': `-${dist}px`, '--mq-dur': `${dur}s` } : undefined">{{ text }}</span>
  </span>
</template>

<style scoped>
.mq { display: block; overflow: hidden; min-width: 0; }
.mq-t { display: block; white-space: nowrap; overflow: hidden; text-overflow: ellipsis; }
.mq.run .mq-t { overflow: visible; text-overflow: clip; animation: mq-scroll-loop var(--mq-dur, 4s) linear infinite; }
@keyframes mq-scroll-loop {
  0%, 10%   { transform: translateX(0); }
  45%, 55%  { transform: translateX(var(--mq-d, 0px)); }
  90%, 100% { transform: translateX(0); }
}
</style>
