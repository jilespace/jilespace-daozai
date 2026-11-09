# 性能基线

> 最近更新：2026-11-10（Phase N）。目标是让这个常驻桌面顶部的应用「几乎不占资源」：
> 空闲时低频刷新、展开时顺滑、长时间运行不涨内存。本文记录当前基线、
> 测量方法与已建立的回归护栏，方便后续迭代对比。

## 测量方法

- 构建：`cargo build --locked --release --bin daozai`。
- 启动耗时：`Start-Process` 后轮询 `MainWindowHandle` 首次非零的时间
  （冷启动包含字体加载，第二次热启动更快）。
- 空闲 CPU / 内存：窗口可见后静置 2 秒，再采样进程 `TotalProcessorTime`
  与 `WorkingSet64`；CPU = 进程 CPU 时间 ÷ 墙钟时间（同时记录「单核占用」与
  「整机占用」，避免不同核心数下数字不可比）。
- 长时运行：同一进程每 20 秒采样 RSS、线程数、句柄数与窗口 CPU，观察是否存在
  单调增长；测试数据目录使用临时 `DAOZAI_DATA_DIR`，不触碰真实用户数据。
- 环境：Windows 11 x64，Rust stable 1.98，release（LTO + codegen-units=1），12 逻辑核心。

可复用的 PowerShell 片段（在本仓库根目录执行）：

```powershell
$exe = ".\target\release\daozai.exe"
$dir = Join-Path $env:TEMP "daozai-bench"; New-Item -ItemType Directory $dir -Force | Out-Null
$env:DAOZAI_DATA_DIR = $dir
$p = Start-Process $exe -PassThru
Start-Sleep -Seconds 2
$p.Refresh(); $cpu0 = $p.TotalProcessorTime
Start-Sleep -Seconds 5
$p.Refresh(); $cpu1 = $p.TotalProcessorTime
($cpu1 - $cpu0).TotalMilliseconds / 5000   # 毫秒/秒 ≈ 单核占用比例
Stop-Process $p.Id -Force
```

## 当前基线（2026-11-10，Phase N）

| 场景 | 启动到可见 | 空闲 CPU（单核） | 工作集内存 | 句柄数 | 线程数 |
| --- | --- | --- | --- | --- | --- |
| 默认（吉祥物呼吸/眨眼，约 10 fps） | 143 ms（冷启动） | ≈ 0.5 %（≈ 0.04 % 整机） | 82–91 MB | 510–512 | 21–26 |
| 关闭吉祥物（静态界面 1 fps 核对） | 约 100 ms（热启动，早期基线 72 ms） | < 0.2 % | 77 MB 量级 | ≈ 509 | ≈ 20 |

说明：

- 内存前 1 分钟会从 82 MB 增长到 86–91 MB 后趋于平稳，主要来自字体图集与
  GL 纹理的首次填充；`preview_raster`/测试进程不计入。
- 80 MB 量级主要构成为：Windows 图形驱动与 eframe/Glow 上下文、系统字体 mmap、
  中日韩字形图集、SVG 图标缓存与后台服务。距离 60 MB 的目标仍有差距。
- 音频可视化（默认关闭）开启时实测额外 CPU 约 0.2 %、RSS +0～2 MB；
  仅在设置开启后抓取，关闭后线程立即退出。
- 字体图集探针（`theme::tests::font_atlas_probe`，默认忽略）实测典型 UI 字符在
  三档字重、13 种字号下约 52 万像素（≈2 MB GPU 纹理），合并字重对整体内存收益
  有限且会牺牲排版层次，因此暂不调整字体家族。

## 三分钟空闲长时运行（默认配置）

每 20 秒采样一次，共 9 次：

| 采样 | RSS | 句柄 | 线程 |
| --- | --- | --- | --- |
| 20 s | 86.2 MB | 511 | 26 |
| 60 s | 86.7 MB | 510 | 22 |
| 100 s | 90.5 MB | 512 | 22 |
| 140 s | 90.7 MB | 510 | 21 |
| 180 s | 90.6 MB | 511 | 21 |

结论：前 100 秒有一次约 4 MB 的纹理/字形图集增长，此后内存、句柄与线程数均保持
平稳，没有单调泄漏迹象；空闲窗口 CPU 平均约 0.5 % 单核（≈ 0.04 % 整机）。
（长时间真实使用场景下的 24 小时回归仍待后续补做。）

## 刷新策略

`App::frame` 按状态返回 `repaint_after`，主窗口据此调度重绘：

- 交互形变（展开/收起、页面切换）：60 fps。
- 常驻动画（播放中的歌词条、专注计时、系统页曲线、计时器、天气图标）：30 fps。
- 进程页展开时 30 fps，并由后台线程每 2 秒采样一次进程；收起页面立即结束采样线程。
- 吉祥物活动：20 fps；吉祥物睡眠：10 fps。
- 其它静态界面：每秒一次，仅用于核对提醒/跨天。

「设置 → 岛体外观 → 动效 → 关闭」会把弹簧全部收成直接到位，并让页面切换、
导航圆点与常驻组件立即完成，避免无意义的逐帧重绘。浮层（设置、剪贴板、
命令面板、通知历史、框选遮罩）打开时按 16 ms 刷新，关闭后回到上述节奏。

## 已建立的回归护栏

- `ui_tests::idle_repaint_policy_follows_animation_state`：静态收起/展开应 ≥ 900 ms
  刷新，媒体播放应 ≤ 40 ms，关闭动效后不能残留逐帧重绘。
- `ocr::tests::random_or_truncated_dib_bytes_never_panic`：随机/截断 DIB 不 panic。
- `region_capture::tests::random_crop_windows_stay_in_bounds_without_panicking`：
  任意选区不越界、不 panic。
- `ui_tests::corrupt_settings_values_fall_back_without_panicking`：损坏配置文件
  回落到默认值并继续渲染。
- 命令面板搜索剪贴板时只保留最近 30 条、每条最多 4000 字符的检索文本，
  避免大段剪贴板内容把面板拖慢。
- 编译警告清零（`cargo check` 只剩上游依赖的未来兼容提示）。

## 后续目标

- 空闲内存压到 60 MB 以内：评估字体子集化/按需加载体、图像加载器与后台服务按需启动。
- 冷启动 < 100 ms：把字体/图标加载与文件读取移到首帧之后。
- 24 小时真实使用回归（内存/句柄/线程不增长）与多显示器混合 DPI 重绘审计。
- 命令面板剪贴板搜索的增量索引（避免每次输入全量扫描）。
