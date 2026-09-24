# Design: AudioRouter 核心音频路由

技术设计（产品库/迭代需求/2026-09-24-Windows11系统音频路由软件/文档/技术设计.md）的执行细节展开。全部为新增代码。

## 工程结构
```
src-tauri/
  src/
    lib.rs                 # Tauri builder：注册命令、托盘、事件
    commands.rs            # #[command] 桥接层
    audio/
      mod.rs
      device_enum.rs       # MicDevice{ id, name }, AudioApp{ pid, exe, name, is_uwp }
      capture.rs           # ProcessCapture：WASAPI Process Loopback（按 exe 聚合多会话）
      mic_capture.rs       # MicCapture：物理麦克风采集（共享模式）
      mixer.rs             # MixerEngine：增益、饱和求和、断流剔除、RMS 电平
      virtual_dev.rs       # VirtualSink：写 VB-CABLE "CABLE Input"；ensure_cable_installed()
src/
  views/MainWindow.vue     # 组合组件 + 状态（Pinia store）
  components/
    AppDropdown.vue        # 仅应用名的下拉复选框 + 摘要
    SharedPanel.vue        # 已共享应用（电平+滑杆），max-height 136px 滚动
    MicSelect.vue          # 麦克风下拉
    Onboarding.vue         # 三步引导
  stores/app.ts            # Pinia：selectedApps, volumes, micId, masterOn
```

## 关键实现点
- 捕获激活：`ActivateAudioInterfaceAsync` + `AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK`（PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE），按可执行名聚合
- 混音周期 10ms；每源 rtrb 环形队列 100ms 容量，满则丢旧；输出 clamp [-1,1]
- 写出：VB-CABLE 端点共享模式渲染，mix format 48kHz f32 stereo，事件驱动
- 事件：`level`（每 tick RMS，前端节流 30fps）、`apps_changed`（3s 枚举刷新）、`device_changed`（IMMNotificationClient）
- 配置：tauri-plugin-store，键见技术设计§9

## 测试
- Rust：mixer 纯函数单测（增益/饱和/剔除）；device_enum 可注入 mock
- 前端：Vitest 组件测试（下拉多选/摘要/限高滚动/滑杆/引导级联）
- 追溯：每个测试标注对应 TC-xxx
