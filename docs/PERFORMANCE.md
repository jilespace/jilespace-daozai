# 性能基线

> 最近更新：2026-11-10。目标是让这个常驻桌面顶部的应用「几乎不占资源」：
> 空闲时低频刷新、展开时顺滑、长时间运行不涨内存。本文记录当前基线、
> 测量方法与已建立的回归护栏，方便后续迭代对比。

## 测量方法

- 构建：`cargo build --locked --release --bin daozai`。
- 启动耗时：`Start-Process` 后轮询 `MainWindowHandle` 首次非零的时间
  （冷启动包含字体加载，第二次热启动更快）。
- 空闲 CPU / 内存：窗口可见后静置 2 秒，再采样 5 秒进程 `TotalProcessorTime`
  与 `WorkingSet64`；CPU 百分比 = 进程 CPU 时间 ÷ (墙钟时间 × 逻辑核心数)。
- 测试数据目录使用临时 `DAOZAI_DATA_DIR`，不触碰真实用户数据；测量结束即结束进程并清理。
- 环境：Windows 11 x64，Rust stable 1.98，release（LTO + codegen-units=1）。

可复用的 PowerShell 片段（在本仓库根目录执行）：

```powershell
$exe = ".\target\release\daozai.exe"
$dir = Join-Path $env:TEMP "daozai-bench"; New-Item -ItemType Directory $dir -Force | Out-Null
$env:DAOZAI_DATA_DIR = $dir
$p = Start-Process $exe -PassThru
Start-Sleep -Seconds 2
$cpu0 = $p.TotalProcessorTime
Start-Sleep -Seconds 5
$p.TotalProcessorTime - $cpu0   # 除以 5 秒 × 核心数得到总 CPU 占比
Stop-Process $p.Id -Force
```

## 当前基线（2026-11-10）

| 场景 | 启动到可见 | 空闲 CPU | 工作集内存 | 句柄数 |
| --- | --- | --- | --- | --- |
| 默认（吉祥物呼吸/眨眼，约 10 fps） | 144 ms（冷启动） | 0.23% | 81.8 MB | 511 |
| 静态（关闭吉祥物，静态界面 1 fps 核对） | 72 ms（热启动） | 0.16% | 76.9 MB | 509 |

参考：同一台机器上普通 Electron 桌面应用的空闲内存通常为数百 MB；岛仔当前
80 MB 量级主要来自字体图集、图像加载器与邮件/额度等后台线程预算。

## 刷新策略

`App::frame` 按状态返回 `repaint_after`，主窗口据此调度重绘：

- 交互形变（展开/收起、页面切换）：60 fps。
- 常驻动画（播放中的歌词条、专注计时、系统页曲线、计时器）：30 fps。
- 吉祥物活动：20 fps；吉祥物睡眠：10 fps。
- 其它静态界面：每秒一次，仅用于核对提醒/跨天。

「设置 → 岛体外观 → 动效 → 关闭」会把弹簧全部收成直接到位，并让页面切换、
导航圆点与常驻组件立即完成，避免无意义的逐帧重绘。浮层（设置、剪贴板、
命令面板、框选遮罩）打开时按 16 ms 刷新，关闭后回到上述节奏。

## 已建立的回归护栏

- `ui_tests::idle_repaint_policy_follows_animation_state`：静态收起/展开应 ≥ 900 ms
  刷新，媒体播放应 ≤ 40 ms，关闭动效后不能残留逐帧重绘。
- `ocr::tests::random_or_truncated_dib_bytes_never_panic`：随机/截断 DIB 不 panic。
- `region_capture::tests::random_crop_windows_stay_in_bounds_without_panicking`：
  任意选区不越界、不 panic。
- `ui_tests::corrupt_settings_values_fall_back_without_panicking`：损坏配置文件
  回落到默认值并继续渲染。
- 编译警告清零（`cargo check` 只剩上游依赖的未来兼容提示）。

## 后续目标

- 空闲内存压到 60 MB 以内：评估字体子集化、图像加载器与后台服务按需启动。
- 冷启动 < 100 ms：把字体/图标加载与文件读取移到首帧之后。
- 长时运行（24h）内存曲线与句柄数不增长；系统页采样线程与剪贴板轮询的低功耗策略。
- 多显示器/混合 DPI 下的重绘与命中区开销。
