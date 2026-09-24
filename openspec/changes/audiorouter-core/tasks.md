# Tasks: AudioRouter 核心音频路由

每项可独立验证；完成即勾选并更新本文档。

- [ ] 1. 脚手架：`npm create tauri-app`（Vue3+TS）初始化工程，tauri-plugin-store/tray 注册，`npm run tauri dev` 可启动空窗口（验证：窗口出现）
- [ ] 2. mixer.rs 纯函数核心 + 单测：增益乘法、饱和求和、断流剔除（验证：`cargo test` 通过，追溯 TC-002/003）
- [ ] 3. device_enum.rs：枚举采集设备/音频进程、检测 CABLE（验证：单测 mock + 本机 `get_apps` 返回真实列表，追溯 TC-016）
- [ ] 4. capture.rs + mic_capture.rs：Process Loopback 与麦克风捕获线程（验证：本机捕获网易云播放并打印 RMS>0，追溯 TC-001）
- [ ] 5. virtual_dev.rs：写出 VB-CABLE + 安装引导提权（验证：录音机录"AudioRouter 虚拟麦"有人声+音乐，追溯 TC-001/011）
- [ ] 6. commands.rs + 事件桥：全部 #[command] 与 level/apps_changed 事件（验证：前端调用返回数据，追溯 TC-006）
- [ ] 7. 前端组件：AppDropdown / SharedPanel（限高滚动）/ MicSelect / MainWindow（验证：Vitest 组件测试通过，追溯 TC-009/010/017）
- [ ] 8. Onboarding 三步引导 + onboarding_done 持久化（验证：引导级联点亮逻辑测试，追溯 TC-004）
- [ ] 9. 托盘常驻 + 总开关文案"开启共享/关闭共享" + 配置持久化（验证：最小化到托盘、重启恢复状态，追溯 TC-003）
- [ ] 10. 稳定性：设备热插拔重连、睡眠恢复重连、单源故障隔离（验证：模拟插拔耳机共享不断流，追溯 TC-013/015）
- [ ] 11. `tauri build` 产出安装包 + 冒烟（验证：全新安装走完引导并可共享）
