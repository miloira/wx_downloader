# wx_downloader

微信 Windows 各版本安装包下载器，基于 [gpui-kit](https://gpui-kit.com) 构建。

覆盖微信 **2.x / 3.x / 4.x** 全部历史版本，下拉框选择版本，点击按钮即可下载官方安装包。
版本来源：

| 版本系列 | 来源 |
| --- | --- |
| 微信 4.x | 实时拉取 [cscnk52/wechat-windows-versions](https://github.com/cscnk52/wechat-windows-versions) |
| 微信 2.x / 3.x | 内置固定数据，来源 [tom-snow/wechat-windows-versions](https://github.com/tom-snow/wechat-windows-versions) |

微信 2.x / 3.x 已停止更新（来源仓库最后发布于 2025-09），所以数据直接内置在
`src/legacy.rs`，不再请求网络——既省一次 API 配额，也让离线时这两组仍然完整。

## 功能

- **浏览全部版本**：4.x 首次拉取后本地缓存（6 小时内不再请求网络），2.x / 3.x 直接
  读取内置数据，按下拉框分组展示（微信 4.x / 3.x / 2.x），组内按版本号从新到旧；
  最新的 4.x 版本带绿色「最新」标签（2.x / 3.x 为旧版本线，不标注）。
- **下拉框选择**：支持输入版本号搜索过滤，分组标题也可命中。
- **一键下载**：下载到 `%USERPROFILE%\Downloads`（可在界面中更改）。
- **手动刷新**：界面上的「刷新」按钮可跳过缓存，重新拉取 4.x 列表。
- **进度反馈**：显示已下载大小 / 总大小与实时速度，可随时取消。
- **版本详情**：显示版本号、所属系列、文件名、发布日期与安装包大小。
- **文字可复制**：界面上的说明文字（版本号、系列、文件名、路径、状态行、底部项目链接等）
  都可以拖选后用 `Ctrl+C` 复制，方便粘贴版本号、安装包名或仓库地址。
- **路径过长自动省略**：保存路径过长时以 `…` 截断，鼠标悬浮会弹出完整路径。
- **项目链接**：底部显示仓库地址（`https://github.com/miloira/wxhook`），可拖选复制，
  地址过长时以 `…` 截断、悬浮显示完整地址。
- **微信绿主题**：主按钮、输入框 / 下拉框的激活边框、焦点环、进度条与悬停高亮
  都采用微信品牌绿 `#07C160`。
- **自定义标题栏与图标**：不用系统标题栏，自绘标题栏带 Logo 与窗口按钮；
  Logo 为微信绿圆形底 + 白色气泡 + 右下角下载标识，同时用作任务栏 / Alt-Tab
  图标（编译期打包成 `.ico` 资源嵌入 exe）。

## 使用

```bash
cargo run            # 调试运行（保留控制台，便于看日志/panic）
cargo run --release  # 发布运行（无控制台窗口，单文件约 10.5 MB）
```

发布版做了体积优化：`strip` + `lto` + `codegen-units = 1` + `opt-level = "s"`，
并且因为唯一的 panic 点在启动开窗、GPUI 里那处 `catch_unwind` 也是立刻重新抛出，
所以可以安全地开 `panic = "abort"`。相比默认配置的 22.8 MB **减小到 10.5 MB（约 -54%）**。
注意这会明显增加 release 编译时间（LTO + 单 codegen unit）。

首次启动会自动获取版本列表（需要联网）。若某个仓库不可用，会退回该仓库的
GitHub 标签接口并在界面上给出对应提示；两个仓库都取不到时才使用内置版本。

首次启动会自动获取 4.x 列表；2.x / 3.x 使用内置数据。4.x 获取成功后会缓存到
`%LOCALAPPDATA%\wx_downloader\versions.json`，之后 6 小时内再次启动**直接读缓存、
不请求网络**，因此不会反复消耗配额。想立即拉取最新列表点界面上的「刷新」按钮
（会跳过缓存重新请求）。

### 版本列表缓存

因为未登录访问 GitHub API **每小时只有 60 次**（按 IP 计算），共享 IP 下很容易用尽，
所以 4.x 列表会缓存到本地，优先级如下：

1. **缓存未超过 6 小时** → 直接用缓存，不发请求。
2. **缓存过期** → 请求 GitHub；成功则刷新缓存。
3. **请求失败**（限流/断网）→ 退回**过期缓存**，并在界面上说明原因。
4. 连缓存都没有 → 使用内置的单条 4.x 版本。

缓存文件损坏或读不出时当作"没有缓存"处理，不会影响启动。界面上会标明当前列表来自
哪个来源：`共 N 个版本可用 · 缓存（3 小时前）`，或离线时 `· 离线`。点「刷新」可强制
重新拉取。

### 如果仍然触发限流

点「刷新」会真实请求 API，频繁刷新可能用完配额。此时界面会提示：

> GitHub API 访问次数已达上限（未登录每小时 60 次），预计约 N 分钟 后恢复。
> 可设置环境变量 GITHUB_TOKEN 提高上限（已使用缓存版本列表）

这时程序仍会正常显示缓存里的版本列表，不会卡住。想彻底避免可以设置一个 GitHub
Token（不需要任何权限，公开仓库只读即可）：

```bash
GITHUB_TOKEN=ghp_xxx cargo run --release
# 或 Windows PowerShell
$env:GITHUB_TOKEN="ghp_xxx"; cargo run --release
```

带上 Token 后上限提升到每小时 5000 次，基本不会再触发。

## 实现说明

| 文件 | 作用 |
| --- | --- |
| `src/main.rs` | 程序入口：初始化 gpui-kit、应用主题、注册资源、打开窗口。 |
| `src/app.rs` | 界面视图：自定义标题栏、下拉框、详情、下载按钮与进度条。 |
| `src/theme.rs` | 微信绿主题：覆盖 gpui-kit 的语义色 token。 |
| `src/logo.rs` / `assets/logo.svg` | 应用图标（微信绿圆 + 白色气泡 + 下载标识）。 |
| `build.rs` | 编译期把 `logo.svg` 光栅化成 `logo.png`，并生成 `.ico` 资源嵌入 exe。 |
| `src/assets.rs` | 资源：默认图标包 + 额外图标 + 生成的 logo。 |
| `src/versions.rs` | 版本发现：实时 4.x（Releases API → Tags API）+ 内置 2.x/3.x。 |
| `src/legacy.rs` | 内置的微信 2.x / 3.x 固定版本数据。 |
| `src/cache.rs` | 4.x 版本列表的本地缓存（读写 + 过期判断）。 |
| `src/download.rs` | 后台下载线程，通过 channel 上报进度。 |
| `src/http.rs` | 统一 HTTP 客户端配置。 |

几个要点：

- **系列分组**：版本号的首段决定所属系列（2/3/4）。下拉框用
  `SearchableVec<SearchableGroup<_>>` 按系列分成三段（4.x / 3.x / 2.x）。
  无法解析的版本（例如来源仓库里那个只有 `v` 的发布）会被过滤掉。
- **2.x / 3.x 内置**：这两个版本线已冻结，数据放在 `src/legacy.rs`。为了不把 59 条
  数据手抄出错，只记录版本号、发布日期和大小——安装包名与下载地址都能由版本号推导，
  单元测试会逐条校验。需要刷新时运行
  `cargo test -- --ignored --nocapture show_legacy_refresh` 打印最新数据。
- **缓存写盘要原子**：`src/cache.rs` 先写 `versions.json.tmp` 再 rename，避免写入中途
  被打断后留下半个文件——那种文件下次读取会解析失败，等于白白丢缓存。缓存里的字段用
  独立的 `CachedVersion` 类型，这样以后改内存里的 `VersionInfo` 不会破坏旧缓存文件。
- **列表要翻页**：GitHub 的 `per_page` 上限是 100，单次请求**最多返回 100 条**，
  超出的部分会被静默丢弃、界面也不会报错。所以 `src/versions.rs` 会顺着响应里的
  `Link: rel="next"` 头一直取到没有下一页为止，releases 与 tags 两条路径都如此。
  （微信 4.x 目前 76 个版本，还没到 100，但按每月 4~5 个的速度终会超过。）
- **HTTP 客户端**：`ureq` 使用系统 TLS（Windows 上为 Schannel），因此不需要 C 工具链；
  默认的 rustls 后端需要 C 编译器，故显式指定 `native-tls`。
- **错误信息要能看懂**：GitHub 把失败原因（例如限流说明）放在**响应体**里，而 ureq
  默认把 4xx/5xx 直接变成 `Err(StatusCode(403))`，只剩一个状态码。因此 API 请求用
  `http_status_as_error(false)`，拿到响应后读取 `message` 字段再展示。两个仓库若因同一
  原因失败（限流是共享的），原因只打印一次，避免重复。
- **下载不阻塞界面**：`ureq` 是阻塞式的，下载运行在独立线程，通过 `async_channel`
  上报进度，前台任务负责刷新界面。
- **写入安全**：先写入 `.part` 临时文件，完成后再重命名；取消或失败时会清理临时文件，
  不会留下看似完整的残缺文件。
- **微信绿主题**：通过覆盖主题配置里的语义色 token 实现，而不是逐个组件改色——
  这样所有读取 token 的控件（按钮、输入框、下拉框、进度条）会一起生效。
  关键是 `ring`（焦点边框 / 焦点环）和 `progress_bar`，因为它们的默认值不是
  `primary`，必须单独指定。
- **图标资源**：默认图标包（`gpui_kit::assets::Assets`）只内置组件自身要用的图标，
  `Download` / `X` 并不在其中，直接引用会渲染成空白。`src/assets.rs` 用
  `icon_assets!` 只额外打包这两个图标，其余路径仍回退到默认包，避免把整份图标库
  （约 2.8 MB）都塞进二进制。
- **自定义标题栏**：`appears_transparent: true` 隐藏系统标题栏，由应用自己绘制
  （`TitleBar` 组件，带 Logo、标题和最小化/最大化/关闭按钮，并负责拖动与双击最大化）。
  注意仍要设置 `title`——Windows 用它作为任务栏与 Alt-Tab 的名称。
- **标题栏按下不能触发文字选区**：`TitleBar` 自己处理「按下拖动窗口」，但不会阻止
  同一次按下也启动窗口级文字选区。拖动窗口走的是原生 modal 循环，mouse-up 收不到，
  于是选区手势一直挂着（`is_selecting` 为真），下一次在别处点击就会从标题栏画出
  一整段选区。所以给标题栏包了一层，左键按下时调用
  `gpui_base::GlobalState::suppress_text_selection`（官方 `Button` 也是这么防的）；
  只抑制选区，不 `stop_propagation`，拖动/双击最大化照常。
- **缩放窗口同样会留下选区手势**：窗口边框的拖拽缩放由 `Root` 自带的 `window_border`
  处理，它在应用视图之外、包不到，而且缩放进的是同一个原生 modal 循环。因此额外用
  `cx.observe_window_bounds` 监听窗口尺寸/位置变化，一旦 bounds 改变就
  `TextSelection::clear`，把这次手势清掉——窗口移动和缩放两种情况一并覆盖。
- **文字选择与复制**：`Root` 已经画好窗口级的选区层，并绑定了 `Ctrl+C` 的复制动作，
  所以界面文字用 `gpui_base::SelectableText`（`copyable()` 辅助函数）即可拖选复制；
  GPUI 的普通文本是纯绘制、不可选中的。`SelectableText` 会继承外层 `div` 的文字样式，
  因此调用处照常对包裹的 `div` 设置字号与颜色。
- **拖选高亮要自己触发重绘**：GPUI 只在 **OS 级拖拽**（`has_active_drag`）时才在鼠标移动
  时重绘窗口；`SelectableText` 的拖选不是 OS 拖拽，所以选区模型在更新、画面却不刷新，
  表现为「鼠标划选时看不到高亮，松手后才（或干脆不）出现」。因此 `WxApp` 在根节点上挂了
  `on_mouse_move`，只要左键按着就 `window.refresh()`，高亮才能跟着指针实时扩展。
- **复制需要焦点在派发路径上**：`Ctrl+C` 绑定在 `Root` 的 `"Root"` 键上下文下，而按键只
  派发到「聚焦节点 → 根节点」这条路径；窗口里没有任何控件持有焦点时，路径不含该上下文，
  复制会静默失效。所以 `WxApp` 创建时主动 `window.focus()` 自己并 `track_focus`，让
  `"Root"`（祖先）始终在路径上。`src/app.rs` 的 `selection_tests` 用 GPUI 的无头测试环境
  模拟「框选 → Ctrl+C」把这条链路钉死（Windows 上合成系统输入时序不稳，不适合做断言）。
- **长路径截断 + 悬浮提示**：`div().truncate()` 在宽度不足时以 `…` 收尾，再挂
  `div().tooltip(...)` 在悬浮时弹出完整路径；弹层用 `Tooltip::element` 包一层
  `max_w`，让超长路径在弹层里换行而不是撑到屏幕外。
- **Logo 为什么要光栅化**：GPUI 的 SVG 渲染只取**单色 alpha 蒙版**再乘一个颜色，
  会丢掉 logo 里的微信绿和白色。所以 `assets/logo.svg` 作为唯一来源，
  由 `build.rs` 在编译期用 resvg 渲染成真彩色 `logo.png` 再交给图像管线显示。
  这样素材不会与提交的二进制脱节，也不会手工维护两份。
- **任务栏图标走 exe 资源**：GPUI 的 `WindowOptions::icon` 只在 X11 生效，Windows
  上会被忽略，任务栏 / Alt-Tab / 资源管理器取的是可执行文件自身的图标。所以
  `build.rs` 把同一份 `logo.svg` 渲染成多尺寸（16–256）`.ico`，写成资源脚本后调用
  Windows SDK 的 `rc.exe` 编成 `.res`，再用 `cargo:rustc-link-arg-bins` 链接进 exe。
  没找到 `rc.exe` 时只发一条 warning、不影响构建（可通过 `RC` 环境变量指定路径）。
- **发布版无控制台**：`windows_subsystem = "windows"` 只在 release 下启用；
  debug 保留控制台，方便查看日志与 panic。

## 测试

```bash
cargo test                              # 单元测试
cargo test -- --ignored --nocapture     # 需要联网的集成测试
```

联网测试会真实访问两个仓库的 GitHub API，并验证 4.x / 3.x / 2.x 的官方安装包 URL
都能正常开始流式下载与取消。
