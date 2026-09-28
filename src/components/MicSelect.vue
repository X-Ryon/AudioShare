<script setup lang="ts">
/**
 * 物理麦克风下拉（TC-008）
 */
import { store, setMic } from '../stores/app'
</script>

<template>
  <label class="fieldlab" for="micSelect">麦克风</label>
  <select id="micSelect" class="mic" aria-label="选择物理麦克风设备"
    :value="store.micId" @change="(e) => setMic((e.target as HTMLSelectElement).value)">
    <option value="">系统默认麦克风</option>
    <option v-for="m in store.mics" :key="m.id" :value="m.id">
      {{ m.name }}{{ m.is_default ? '（默认）' : '' }}
    </option>
  </select>
</template>

<style scoped>
.fieldlab { font-size: 11px; color: #5d5d5d; margin-bottom: 5px; display: block; }
.mic { width: 100%; padding: 7px 10px; font: inherit; font-size: 13px; color: #1b1b1b; background: #fff; border: 1px solid #d6d6d6; border-radius: 4px; cursor: pointer; }
.mic:hover { background: #f9f9f9; }
.mic:focus-visible { outline: 2px solid #0067c0; outline-offset: 1px; }
</style>
