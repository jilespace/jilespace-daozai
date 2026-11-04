# 岛仔剪贴板历史 · 设计文档

- 日期：2026-11-04
- 状态：已确认，待实现
- 范围：v1（纯文本）

## 1. 背景与目标

岛仔已经常驻屏幕顶部，承担媒体、待办、邮件、日程和 AI 额度的集中展示。剪贴板是用户每天最高频的信息通道之一，但 Windows 自带的 `Win+V` 只提供基础列表，缺少置顶、快速搜索和与桌面工具的整合。

本功能在岛仔内提供：

- `Ctrl+Alt+V` 全局唤起文本剪贴板历史
- 搜索、置顶、删除、选择后写回剪贴板
- 历史使用 Windows DPAPI 加密保存在本机
- 密码管理器等标记为「不要记录」的内容自动跳过

## 2. 非目标（v1 不做）

- 图片、文件、富文本/HTML
- 自动模拟 `Ctrl+V` 粘贴
- 云同步、跨设备
- 用户自定义热键组合
- 历史内容预览中的语法高亮

## 3. 用户流程

1. 用户在任意应用中按 `Ctrl+Alt+V`
2. 弹窗出现在光标所在显示器顶部居中位置，搜索框自动聚焦
3. 输入关键词过滤；`↑/↓` 选择，`Enter` 复制，`Esc` 关闭
4. 选中后弹窗关闭，焦点回到原应用，用户按 `Ctrl+V` 粘贴
5. 岛上短暂显示「已复制」提示，不抢焦点

首次使用（功能默认关闭）：按热键后弹窗显示「剪贴板历史未开启」和一键开启按钮；开启后立即开始记录。

## 4. 架构与数据流

沿用项目现有的「后台 Service + channel + 每帧 poll」模式，剪贴板读写全部在后台线程完成。

```
[后台 clipboard 线程]
  recv_timeout(100ms) 处理 UI 命令
  每 500ms GetClipboardSequenceNumber
    → 变化时读取 CF_UNICODETEXT
    → 排除格式 / owner / 长度 / 空内容过滤
    → History 去重、置顶、容量与过期整理
    → DPAPI 加密 + atomic_write
    → Event::Changed(snapshot)

[主线程 Shell]
  WM_HOTKEY（native 子类过程）→ 原子标记 → 打开/切换弹窗
  clipboard_ui::Popup::show()
    → 搜索/选择/删除/置顶
    → Cmd::Copy(id) / Cmd::Delete(id) / ...
    → Event::Copied(id) → 关闭弹窗 + 恢复前台窗口
```

### 组件边界

| 组件 | 职责 | 依赖 |
| --- | --- | --- |
| `clipboard::History` | 纯数据逻辑：去重、排序、淘汰、过期、搜索、序列化 | 无 |
| `clipboard::Service` | 后台线程：轮询、Win32 读写、DPAPI、持久化 | Win32 |
| `clipboard_ui::Popup` | 弹窗状态与渲染、键鼠交互 | egui |
| `native` | 热键注册、`WM_HOTKEY`、前台窗口捕获/恢复、光标显示器工作区 | Win32 |
| `main::Shell` | 服务接线、事件分发、弹窗生命周期 | 以上 |

## 5. 数据模型与存储

```rust
pub struct Item {
    pub id: String,      // SHA-256(text) 前 16 位十六进制
    pub text: String,
    pub copied_at: i64,  // Unix 秒
    pub pinned: bool,
}

pub struct History {
    items: Vec<Item>,    // 显示顺序：置顶优先，其余按 copied_at 倒序
}
```

### 文件

- 路径：`model::config_dir().join("clipboard.bin")`，遵循 `DAOZAI_DATA_DIR`
- 格式：`b"DPAPI"` + `CryptProtectData(JSON)`
- JSON：

```json
{
  "version": 1,
  "items": [
    { "id": "9f2c...", "text": "复制的文本", "copied_at": 1762250000, "pinned": false }
  ]
}
```

- 写入复用 `model::atomic_write`，在后台线程同步执行；剪贴板事件频率低，不需要防抖
- 加密沿用 `gemini_quota.rs` / `native_auth.rs` 的 DPAPI 模式，不新增依赖

### 容量与淘汰

| 规则 | 值 |
| --- | --- |
| 最大条数 | 200（可在设置中调整 50–500） |
| 最大文本总量 | 1 MiB |
| 自动过期 | 7 天（可在设置中调整 1–30 天） |
| 单条上限 | 32 KiB，超出跳过 |
| 置顶项 | 不参与条数、字节和过期淘汰 |

- 完全相同的文本再次复制：移除旧条目，插入到顶部，保留置顶状态并更新时间
- `id` 由文本 SHA-256 派生，天然去重；哈希碰撞概率可忽略
- 启动时和每小时后执行一次过期清理

### 解密失败

原文件重命名为 `clipboard.bin.unreadable-<Unix 秒>`，以空历史启动；设置页显示一次性状态提示。之后的新历史写入新的 `clipboard.bin`，不覆盖备份。

## 6. 剪贴板捕获与隐私

### 捕获

- 后台线程每 500ms 调用 `GetClipboardSequenceNumber`
- 序号变化才读取，避免无谓的 `OpenClipboard`
- `OpenClipboard` 失败重试 5 次、每次间隔 10ms；仍失败则跳过，等下一次变化
- 只读取 `CF_UNICODETEXT`；UTF-16 解码后去掉末尾 NUL
- `trim()` 为空的内容跳过
- 超过 32 KiB 的文本跳过

### 隐私过滤（强制，不可关闭）

复制前检查以下标准剪贴板格式：

| 格式 | 行为 |
| --- | --- |
| `ExcludeClipboardContentFromMonitorProcessing` | 存在即跳过 |
| `CanIncludeInClipboardHistory` | 值为 0 时跳过 |
| `CanUploadToCloudClipboard` | 值为 0 时跳过 |

这些格式由 KeePass、1Password、Bitwarden 等密码管理器以及 Windows 自身设置，是系统级的「不要记录」约定。

### 自身写入

- 通过 `GetClipboardOwner` + `GetWindowThreadProcessId` 判断 owner 是否为本进程
- 从历史复制回剪贴板后，更新 `last_seen_sequence`，避免同一条内容被重新记录或重排

### 暂停与总开关

- 设置中的「暂停记录」只停止读取和写盘，保留已有历史
- 关闭「启用剪贴板历史」等同暂停：Service 线程仍启动并处理 UI 命令，但不轮询剪贴板、不写盘
- 暂停/关闭状态在弹窗和设置页可见

### 日志

剪贴板正文不得写入任何日志、诊断信息或错误消息。错误消息只描述操作失败，不包含内容。

## 7. 热键、窗口与焦点

### 热键

- 无论功能是否开启，都注册热键：关闭状态下按热键会打开「一键开启」提示
- 在主岛 HWND 上注册：`RegisterHotKey(hwnd, 0xDA0A, MOD_CONTROL | MOD_ALT | MOD_NOREPEAT, VK_V)`
- `native.rs` 子类过程处理 `WM_HOTKEY`，置位原子标记并返回 0
- 暴露 `native::take_clipboard_hotkey() -> bool` 供主循环消费
- 注册失败（被其他程序占用）：
  - 设置页显示「热键被其他程序占用」
  - 托盘菜单「剪贴板历史」仍可用
- 进程退出时 `UnregisterHotKey`；即使遗漏，系统也会在进程结束时清理

### 弹窗窗口

- 使用已有的 `ctx.show_viewport_immediate`（与设置窗口、邮件全文窗口一致）
- `ViewportId::from_hash_of("daozai-clipboard")`
- 属性：
  - 420×440，最小 360×300
  - 无边框、透明、置顶、不占任务栏
  - `theme::app_icon()`
  - 初始 `with_visible(false)`，首帧交换缓冲后再 `Visible(true)` + `Focus`，避免白色空窗口闪烁（沿用设置窗口的做法）
- 位置：光标所在显示器工作区顶部居中，`y = work.top + 60`；超出工作区时钳制
- 跟随 `settings_theme`（跟随系统/浅色/深色），与设置窗口一致

### 焦点

- 打开前记录 `GetForegroundWindow()`；若前台是本进程窗口则不记录
- 关闭时若原窗口仍有效（`IsWindow`），调用 `SetForegroundWindow` 恢复
- 管理员权限窗口可能因 UIPI 拒绝恢复：静默失败，用户手动点回即可
- 打开后首次获得焦点前不触发「失焦关闭」；获得焦点后失焦超过 100ms 才关闭，避免首帧误关
- `Esc`、再次按热键、点击条目复制、点击窗口外都会关闭

## 8. UI 设计（严格复用现有设计语言）

> 原则：不引入新的视觉体系。颜色、字体、圆角、间距全部复用 `theme.rs`、`settings_theme.rs` 和设置窗口已有的令牌；图标沿用 `paint.rs` 的矢量线条风格。

### 8.1 颜色令牌

所有颜色通过 `settings_theme::color(ctx, dark, light)` 选择，与设置窗口完全一致：

| 用途 | 深色 | 浅色 | 现有来源 |
| --- | --- | --- | --- |
| 弹窗卡片背景 | `35,35,38` | `255,255,255` | `settings_card` |
| 内嵌/列表背景 | `25,25,27` | `246,247,249` | 设置 CentralPanel |
| 主文字 | `theme::WHITE` | `30,34,41` | 设置正文 |
| 次要文字 | `gray(195)` | `gray(60)` | 设置说明 |
| 弱化文字 | `gray(150)` | `gray(105)` | 设置侧栏标题 |
| 分隔线/描边 | `42,42,45` | `216,219,225` | 设置侧栏描边 |
| 行悬停 | `43,43,46` | `214,221,232` | 设置选中项 |
| 强调/选中 | `theme::BLUE` `62,172,248` | 同左 | 岛体强调色 |
| 置顶星标 | `theme::YELLOW` `255,196,61` | 同左 | 现有配色 |
| 删除悬停 | `theme::RED` `242,48,70` | 同左 | 现有配色 |
| 滚动条轨道 | `theme::TRACK` `36,36,39` | 设置浅色背景 | 现有配色 |

选中行不使用整块高饱和填充：沿用岛体的做法，使用 `theme::alpha_blue(0.16)` 淡蓝底 + 左侧 2px `theme::BLUE` 竖条，文字颜色不变。

### 8.2 字体与字号

全部使用 `theme::font(size, W)`，不新增字体族：

| 元素 | 字号/字重 |
| --- | --- |
| 搜索框 | 13 / Regular |
| 条目正文 | 13 / Regular，最多 2 行 |
| 条目元信息（时间） | 11 / Regular |
| 底部提示 | 10 / Regular |
| 空状态/暂停提示 | 12 / Regular |

中文继续使用现有的 Segoe UI + 微软雅黑 UI 回退。

### 8.3 布局

```
┌────────────────────────────────────┐  420×440，圆角 14，内边距 14
│  [搜索图标] 搜索剪贴板…      12 条 │  搜索行高 36
│ ────────────────────────────────── │  1px 分隔线
│  复制的第一行文字                  │  行高 56
│  第二行文字…                  ★ ×   │
│ ────────────────────────────────── │
│  …                                 │
│                                    │
│ ↑↓ 选择 · Enter 复制 · Esc 关闭    │  底部提示 10px
└────────────────────────────────────┘
```

- 行间距 4，列表使用 `egui::ScrollArea`
- 星标常驻显示（已置顶为黄色，未置顶为弱化灰，悬停才明显）
- 删除 `×` 仅在悬停/选中行显示
- 空状态：居中显示「还没有记录 / 复制文字后会自动出现在这里」
- 暂停状态：列表顶部显示一条浅色提示条「已暂停记录 · 点击恢复」
- 搜索无结果：显示「没有匹配的内容」

### 8.4 动效

- 行悬停、选中使用 `ctx.animate_bool_with_time(..., 0.12)`，与设置窗口的动画时长一致
- 不新增弹簧动画；弹窗打开不做缩放/滑入，避免与岛体 60fps 动画系统耦合
- 关闭立即隐藏，不做退场动画

### 8.5 交互细节

- 搜索框自动聚焦，输入即时过滤（大小写不敏感的子串匹配）
- `ArrowUp/Down` 在 `TextEdit` 之前用 `input_mut().consume_key` 截获，避免被光标移动吃掉
- `Enter` 复制选中项；`Delete` 删除选中项；`Esc` 关闭
- 鼠标单击条目即复制；点击星标只切换置顶，不复制；点击 `×` 只删除
- 复制后关闭弹窗，并调用现有的 `App::say("已复制")` 在岛上显示短暂提示；提示不抢焦点
- 滚轮只滚动列表

### 8.6 设置页

新增「剪贴板」标签页，索引 12，放在「AI 额度」之后、「关于」之前；「关于」仍按现有 `start == 9` 规则固定在侧栏底部：

- 复用 `settings_card`、`setting_row`、`settings_label`、`setting_number`，不写新的控件样式
- 新增 `assets/settings-icons/system/clipboard.svg`，沿用现有 32×32 圆角底色 + 白色线条图标的构造；主色建议 `#6D65FF`（品牌紫），与现有图标区分
- 内容：

| 区块 | 控件 |
| --- | --- |
| 启用 | `setting_row`：启用剪贴板历史（默认关闭） |
| 状态 | 热键 `Ctrl+Alt+V`；注册失败时显示冲突提示 |
| 容量 | 保留条数 50–500；自动过期 1–30 天 |
| 隐私 | 暂停记录；清空历史（二次确认）；DPAPI 加密说明 |
| 数据 | 历史文件位置说明；解密失败时的一次性提示 |

- 需要同步更新两处 `titles` 数组、`SETTINGS_GROUPS`、图标 `match`、`settings_tab.min(...)` 上限，以及相关 UI 测试
- `settings_tab` 只是运行时状态，不入配置文件，索引变化不影响旧配置

### 8.7 托盘

- `tray::Event` 新增 `OpenClipboard`
- 菜单在「打开日程」之后新增「剪贴板历史」
- 热键冲突或弹窗异常时作为兜底入口

## 9. 错误处理

| 场景 | 行为 |
| --- | --- |
| `OpenClipboard` 忙 | 重试 5×10ms，仍失败跳过本次 |
| DPAPI 加密失败 | 内存历史继续可用；设置页显示状态；保留旧文件 |
| 解密/JSON 解析失败 | 备份为 `clipboard.bin.unreadable-<ts>`，空历史启动 |
| 热键注册失败 | 设置页提示；托盘入口可用 |
| 前台窗口恢复失败 | 静默忽略；用户手动点回 |
| 弹窗创建失败 | 不崩溃；托盘入口可重试 |
| 历史文件不可写 | 内存历史继续；设置页提示 |

## 10. 测试计划

### 纯逻辑（`clipboard.rs`）

- 新增条目置顶
- 重复文本去重并移到顶部，置顶状态保留
- 条数上限、字节上限淘汰最旧的未置顶项
- 置顶项不参与淘汰和过期
- 过期清理
- 搜索大小写不敏感、空查询返回全部
- JSON 序列化/反序列化往返
- 损坏 JSON 的处理

### Windows

- DPAPI 加解密往返
- 解密失败返回错误而不是 panic

### 设置

- 旧 `settings.txt` 缺少剪贴板字段时使用默认值
- 非法条数/天数回落到默认值
- 保存后重新加载一致
- 默认关闭

### UI 状态

- 热键打开/再次按下关闭
- 上下键选择边界不越界
- 搜索过滤后选择索引重置到有效范围
- Enter 发出正确的 `Cmd::Copy(id)`
- Delete 发出 `Cmd::Delete(id)`
- 失焦宽限期：首帧未聚焦不关闭
- 暂停状态显示
- 浅色/深色令牌选择正确

### 手动验收

- 记事本、VS Code、浏览器复制文本 → 1 秒内出现在弹窗
- 搜索、置顶、删除、清空
- KeePass/1Password 复制密码 → 不记录
- 重启应用 → 历史保留
- 热键冲突 → 设置提示 + 托盘可用
- 管理员窗口 → 焦点恢复失败但复制成功
- 多显示器 → 弹窗出现在光标所在屏幕
- 浅色/深色主题下与实际设置窗口并排对比

### 全量命令

```powershell
cargo test --locked --all-targets --features record -- --test-threads=1
```

## 11. 文件改动

| 文件 | 改动 |
| --- | --- |
| `src/clipboard.rs` | 新增：`Item`、`History`、`Service`、Win32 读写、DPAPI |
| `src/clipboard_ui.rs` | 新增：`Popup` 状态与渲染 |
| `src/lib.rs` | 注册两个模块 |
| `src/main.rs` | `Shell` 增加 service/popup 字段；热键、事件、窗口生命周期接线 |
| `src/native.rs` | `RegisterHotKey`/`UnregisterHotKey`、`WM_HOTKEY`、`take_clipboard_hotkey`、前台窗口/工作区辅助 |
| `src/model.rs` | 设置字段：`clipboard_enabled`、`clipboard_limit`、`clipboard_days` |
| `src/extras.rs` | 新增设置标签页与状态展示 |
| `src/tray.rs` | `Event::OpenClipboard`、菜单项与消息处理 |
| `src/paint.rs` | 新增 `search`、`pin` 矢量图标（沿用现有线条风格） |
| `assets/settings-icons/system/clipboard.svg` | 新增设置页图标 |
| `src/ui_tests.rs` | 新增状态与键盘交互测试 |

不新增 Rust 依赖。`windows` crate 现有 feature 已覆盖剪贴板、热键和消息处理；实现时确认 `GetMonitorInfoW` 等 GDI 符号的 feature 是否已在 `Cargo.toml` 中启用。

## 12. 风险与后续

| 风险 | 应对 |
| --- | --- |
| 透明视口在个别 GPU/远程桌面下闪烁 | 提供不透明白底卡片作为回退；沿用主窗口已验证的 glow 透明路径 |
| 管理员窗口无法恢复前台 | 文档说明；复制本身不受影响 |
| 500ms 轮询漏掉极快的连续复制 | v2 可切换为 `AddClipboardFormatListener` 事件驱动 |
| 与 Windows `Win+V` 竞争 | 差异化：搜索、置顶、本地加密、岛内整合 |
| DPAPI 换用户/换机器不可解密 | 按解密失败处理，保留备份，不崩溃 |
| 设置标签页索引变化影响测试 | 集中更新 `titles`、`SETTINGS_GROUPS` 和测试断言 |

后续可扩展：图片与文件、自定义热键、按应用排除、`AddClipboardFormatListener`。

## 13. 验收标准

1. `Ctrl+Alt+V` 在 150ms 内打开弹窗；`Esc`、失焦、再次按热键均可关闭
2. 在记事本复制文本后 1 秒内出现在列表；搜索可定位；`Enter` 后原窗口可用 `Ctrl+V` 粘贴
3. 密码管理器标记的内容不进入历史
4. 重启后历史保留；文件不可解密时以空历史启动并留下 `.unreadable` 备份
5. 热键冲突时设置页有明确提示，托盘入口可用
6. 弹窗在浅色/深色主题下与设置窗口配色、字体、圆角一致
7. 不新增 Rust 依赖
8. `cargo test --locked --all-targets --features record -- --test-threads=1` 全部通过
