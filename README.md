# AudioShare

Windows 11 个人自用的桌面音频路由工具：把选定应用的播放声音与真实麦克风人声混合，送入一个虚拟麦克风设备，供聊天软件选用。本地播放完全不受影响（本工具只"旁路复制"一份声音）。

## 核心功能

- **按应用取声**：基于 Windows 11 WASAPI Process Loopback（进程环回），按 PID 精确捕获指定应用的声音（含进程树聚合，如 Chrome 的全部子进程），无需 hook、无需驱动
- **多源混音**：多个应用音源 + 真实麦克风，每源独立音量（0–100%），软件混音后写入虚拟麦克风
- **虚拟麦克风**：复用 VB-CABLE 虚拟声卡（成熟免费），应用负责自动检测、一键引导安装（UAC 提权）与状态校验
- **应用列表直选**：所有具备音频会话的程序直接平铺在列表中，每行含播放状态圆点、实时电平条、转发音量滑杆与独立共享开关，长名称悬停跑马灯滚动
- **监控与共享解耦**：应用播放状态、实时音量与麦克风电平作为基础能力持续监控，不随共享开关变化；关闭共享仅停止混音输出
- **记忆功能**：勾选"记住选择"后，退出时持久化共享总开关状态与所选麦克风，下次启动自动恢复（配置文件 `%APPDATA%\AudioShare\remember.json`）
- **首次配置引导**：两步引导（装虚拟声卡 → 系统默认输入设备改为 CABLE Input），提供"打开声音设置"一键跳转并每 10s 轮询检测，完成状态持久化，之后不再弹出
- **托盘常驻**：最小化到托盘，关闭隐藏，双击恢复
- **极简配置**：聊天软件侧唯一需要用户做的事，就是把输入设备切到虚拟麦（之后不再变）

## 技术架构

| 层 | 技术 |
|---|---|
| 前端 | Vue 3 + TypeScript + Vite（单窗口 400×550，组件化） |
| 后端 | Rust + Tauri v2 + windows-rs 0.62（WASAPI / COM） |
| 音频链路 | `[应用1 PCM ×音量A] + [应用2 PCM ×音量B] + … + [麦克风 PCM] → 混音 → VB-CABLE 虚拟麦` |
| 进程间通信 | Tauri command（`invoke`），前端仅 11 个调用点 |

### 音频管线（src-tauri/src/audio/）

| 模块 | 职责 |
|---|---|
| `capture.rs` | 按 PID 进程环回捕获：手工构造 VT_BLOB PROPVARIANT + `ActivateAudioInterfaceAsync` 异步激活，48k/16bit/2ch PCM，轮询采集。激活/初始化/采集全部在专用线程内完成（COM 同公寓） |
| `mic_capture.rs` | 物理麦克风采集：WASAPI 共享模式 + 事件驱动，两级格式协商（固定 48k/2ch/f32 失败时回退设备 mix format）+ 软件转换（位深解码/声道映射/线性重采样），兼容 8k 单声道等特殊设备 |
| `virtual_dev.rs` | 写入 VB-CABLE 虚拟麦：`IAudioRenderClient`，固定 48k/2ch/f32 |
| `mixer.rs` | 纯计算：多源混音、RMS 电平、增益表（无平台依赖） |
| `device_enum.rs` | 设备/进程枚举、CABLE 检测（STGM_READ 读属性存储） |
| `commands.rs` | Tauri 命令层：引擎编排、锁外创建捕获、UAC 提权装驱动 |

## 环境要求

- **Windows 11**（Process Loopback 为 Win11 原生能力；Win10 需 22H2+ 并可能受限）
- Node.js 20+（本项目在 Node 24 / npm 11 验证）
- Rust 1.85+（本项目在 1.98 验证）
- VB-CABLE 驱动（应用内一键安装，或手动运行 `VBCABLE_Driver_Pack45/VBCABLE_Setup_x64.exe`）

## 构建与运行

```bash
# 安装前端依赖
npm install

# 开发模式（Vite 端口 4300 + cargo run）
npm run tauri dev

# 仅构建前端类型检查
npm run build

# 打包发布版
npm run tauri build
```

> 若 4300 端口被上次崩溃残留的 Vite 进程占用，先结束该进程再启动。

## 使用说明

1. 首次启动：检测虚拟声卡 → 未安装则点击"一键安装"（UAC 授权，约 1 分钟）；向导第二步可点"打开声音设置"将系统默认输入设备改为 `CABLE Input`（每 10s 自动复查）
2. 在应用列表中打开要共享的应用开关（可多选），每源用滑杆调转发音量；列表实时显示每个应用的播放状态与电平
3. 底部选择人声来源麦克风（枚举时自动过滤 CABLE 设备防止自激）；勾选右下角"记住选择"可持久化总开关与麦克风
4. 聊天软件输入设备选 `CABLE Input`（仅此一次，之后不变）
5. 顶部"共享总开关"控制总输出（需与各应用独立开关同时开启才会共享）；关闭窗口后托盘常驻

## 目录结构

```
AudioShare/
├── src/                    # 前端（Vue 3）
│   ├── components/         # Onboarding / AppList / MicSelect / MarqueeText
│   ├── stores/app.ts       # 前端状态
│   └── views/MainWindow.vue
├── src-tauri/
│   ├── src/
│   │   ├── audio/          # 音频管线（capture / mic_capture / virtual_dev / mixer / device_enum）
│   │   ├── bin/            # 诊断二进制（见下）
│   │   ├── commands.rs     # Tauri 命令层
│   │   ├── logging.rs      # 文件日志（%APPDATA%\AudioShare\logs）
│   │   ├── lib.rs          # 窗口/托盘/关闭隐藏/数据目录
│   │   └── main.rs
│   ├── resources/          # 打包附带的 VB-CABLE 安装器
│   └── tauri.conf.json
├── VBCABLE_Driver_Pack45/  # VB-CABLE 驱动包（安装器 + inf/sys，release安装包内自带，源码运行需自行从官网下载，然后解压到项目根目录）
├── 产品库/迭代需求/…/文档/  # PRD / 技术设计 / 测试用例 / 目标
└── package.json
```
