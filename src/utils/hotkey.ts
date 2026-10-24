/**
 * 热键格式化工具：KeyboardEvent → "Ctrl+Shift+S"
 * 与后端 hotkeys.rs 的组合串规范对齐（修饰键固定顺序 Ctrl+Shift+Alt+Win，主键白名单一致）
 * 追溯：TC-020（自定义改键）、TC-023（主界面提醒）
 */

export interface HotkeyConfig {
  toggle_share: string
  play_pause: string
  prev_track: string
  next_track: string
}

/** 默认配置（与 Rust HotkeyConfig::default 保持一致，恢复默认用） */
export const DEFAULT_HOTKEYS: HotkeyConfig = {
  toggle_share: 'Ctrl+Shift+S',
  play_pause: 'Ctrl+Shift+P',
  prev_track: 'Ctrl+Shift+Left',
  next_track: 'Ctrl+Shift+Right',
}

const MODIFIER_CODES = new Set([
  'ControlLeft', 'ControlRight', 'ShiftLeft', 'ShiftRight',
  'AltLeft', 'AltRight', 'MetaLeft', 'MetaRight',
])

/** e.code → 主键显示名（与后端 KEY_NAMES 白名单对齐；非白名单返回 null） */
export function codeToKeyName(code: string): string | null {
  if (/^Key[A-Z]$/.test(code)) return code.slice(3)
  if (/^Digit[0-9]$/.test(code)) return code.slice(5)
  if (/^F([1-9]|1[0-2])$/.test(code)) return code
  const map: Record<string, string> = {
    ArrowLeft: 'Left', ArrowRight: 'Right', ArrowUp: 'Up', ArrowDown: 'Down',
    Space: 'Space', Enter: 'Enter', Escape: 'Escape', Tab: 'Tab',
    Backspace: 'Backspace', Delete: 'Delete', Insert: 'Insert',
    Home: 'Home', End: 'End', PageUp: 'PageUp', PageDown: 'PageDown',
    Comma: 'Comma', Period: 'Period',
  }
  return map[code] ?? null
}

/** 是否修饰键（录制时只累积不提交） */
export function isModifierCode(code: string): boolean {
  return MODIFIER_CODES.has(code)
}

/**
 * 组装组合串；主键不在白名单或无修饰键时返回 null
 * （后端同样拒绝无修饰键的单键热键，前端先行提示）
 */
export function formatCombo(e: {
  ctrlKey: boolean
  shiftKey: boolean
  altKey: boolean
  metaKey: boolean
  code: string
}): string | null {
  const key = codeToKeyName(e.code)
  if (!key) return null
  const parts: string[] = []
  if (e.ctrlKey) parts.push('Ctrl')
  if (e.shiftKey) parts.push('Shift')
  if (e.altKey) parts.push('Alt')
  if (e.metaKey) parts.push('Win')
  if (parts.length === 0) return null
  parts.push(key)
  return parts.join('+')
}
