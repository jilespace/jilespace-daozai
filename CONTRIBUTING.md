# 参与开发

欢迎反馈兼容性问题、修复 bug 或完善使用说明。提交较大的功能前，先开 Issue 说明具体场景与预期行为。

## 本地开发

安装 Rust stable、Visual Studio C++ 构建工具和 Windows SDK，克隆仓库后运行：

```powershell
cargo build --locked --bin daozai
cargo test --locked --all-targets --features record -- --test-threads=1
```

为开发实例指定独立目录，避免覆盖日常使用数据：

```powershell
$env:DAOZAI_DATA_DIR = Join-Path $env:TEMP 'daozai-development'
cargo run --locked --bin daozai
```

常规测试使用合成数据。带 `#[ignore]` 的真实账号测试或截图导出测试须按需要显式运行；不要在公共 CI 中连接私人账号。

## 代码导航

| 目录／文件 | 内容 |
| --- | --- |
| `src/main.rs` | Windows 外壳、系统状态和后台服务衔接 |
| `src/app.rs`、`src/pages.rs` | 状态与主界面（含系统状态页） |
| `src/calendar.rs`、`src/calendar_ui.rs` | CalDAV、重复日程、月历与列表 |
| `src/mail.rs`、`src/mail_reader.rs` | IMAP、MIME、验证码与邮件阅读 |
| `src/quota*.rs` 及 provider 模块 | AI 额度与连接 |
| `src/clipboard.rs`、`src/clipboard_ui.rs` | 剪贴板历史服务与快捷面板 |
| `src/command_palette.rs`、`src/panel.rs` | 命令面板、应用检索、计算/换算与浮层视觉令牌 |
| `src/perf.rs` | CPU／内存／网络／磁盘采样与格式化 |
| `src/world_clock.rs` | 命令面板世界时钟与时区格式化 |
| `src/weather.rs` | Open-Meteo 天气解析、缓存与后台刷新服务 |
| `src/audio.rs` | WASAPI 回环抓音、FFT 与频段映射 |
| `src/notifications.rs`、`src/notifications_ui.rs` | 通知历史存储与通知面板 |
| `src/ocr.rs` | 剪贴板图片读取、DIB 解析与 Windows 本地 OCR |
| `src/region_capture.rs`、`src/capture_ui.rs` | 屏幕截取、框选遮罩与选区换算 |
| `src/native.rs`、`src/system.rs` | 原生窗口、输入、拖放和系统事件 |
| `src/ui_tests.rs` | 无窗口交互与布局回归测试 |
| `src/bin/daozai-record` | 合成数据演示与软件渲染器 |
| `docs/ROADMAP.md`、`docs/PROGRESS.md` | 对标、迭代顺序与阶段记录 |
| `docs/PERFORMANCE.md` | 启动/空闲 CPU/内存基线、刷新策略与回归护栏 |
| `vendor/` | 少量必要的上游补丁，见依赖说明 |
| `installer/`、`scripts/` | 安装、卸载与打包 |

## 提交要求

修 bug 时提供能复现的测试；界面变更附不含私人信息的截图。保持改动聚焦，不把个人配置、授权码、Cookie、编译产物或真实账号数据提交到仓库。

源码贡献按仓库 MIT 许可提交；第三方素材须同时提交准确的来源与许可声明。不要修改或移除 vendored 依赖的原始版权声明。
