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
- **微信绿主题**：主按钮、输入框 / 下拉框的激活边框、焦点环、进度条与悬停高亮
  都采用微信品牌绿 `#07C160`。
- **自定义标题栏与图标**：不用系统标题栏，自绘标题栏带 Logo 与窗口按钮；
  Logo 直接取自微信官方图标（从 `Weixin.exe` 提取后矢量化）并在右下角加下载标识。

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
| `src/logo.rs` / `assets/logo.svg` | 应用图标（官方微信图标矢量化 + 下载标识）。 |
| `tools/trace_icon.py` | 从 `Weixin.exe` 提取官方图标并生成 `logo.svg`。 |
| `build.rs` | 编译期把 `logo.svg` 光栅化成 `logo.png`。 |
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
- **Logo 是描摹来的，不是手画的**：`tools/trace_icon.py` 用 Win32
  `PrivateExtractIconsW` 从 `Weixin.exe` 取出官方图标，再用 OpenCV 把白色气泡
  轮廓描摹成贝塞尔路径写进 `assets/logo.svg`，最后叠加右下角的下载标识。
  这样形状、双气泡之间的绿色分隔缝、「眼睛」位置和圆角都与官方一致。
  想更新重跑脚本即可：`python tools/trace_icon.py`（需要 Pillow 与 OpenCV，仅为开发依赖）。
- **Logo 为什么要光栅化**：GPUI 的 SVG 渲染只取**单色 alpha 蒙版**再乘一个颜色，
  会丢掉 logo 里的微信绿和白色。所以 `assets/logo.svg` 作为唯一来源，
  由 `build.rs` 在编译期用 resvg 渲染成真彩色 `logo.png` 再交给图像管线显示。
  这样素材不会与提交的二进制脱节，也不会手工维护两份。
- **发布版无控制台**：`windows_subsystem = "windows"` 只在 release 下启用；
  debug 保留控制台，方便查看日志与 panic。

## 测试

```bash
cargo test                              # 单元测试
cargo test -- --ignored --nocapture     # 需要联网的集成测试
```

联网测试会真实访问两个仓库的 GitHub API，并验证 4.x / 3.x / 2.x 的官方安装包 URL
都能正常开始流式下载与取消。
