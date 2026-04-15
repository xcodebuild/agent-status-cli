# agent-status-cli

[English README](README.md)

`agent-status-cli` 会把 Claude Code 和 Codex 的状态同步到终端标签页标题和颜色上。

---

## 快速开始

安装最新 release：

```bash
curl -fsSL https://gh-proxy.com/https://raw.githubusercontent.com/xcodebuild/agent-status-cli/master/install.sh | sh
```

快速示例：

```bash
asc-codex
asc-claude
```

预览：

下图是在 iTerm2 中将 `Tab bar location` 设为 `Left` 时的效果。

![agent-status-cli preview](https://img.cdn1.vip/i/69df1fd6627b6_1776230358.webp)

下图是在 iTerm2 中将 `Tab bar location` 设为 `Top` 时的效果。

![agent-status-cli preview top](https://img.cdn1.vip/i/69def996cb76d_1776220566.webp)

## 命令

项目会提供三个可执行文件：

- `agent-status-cli`：通用包装器，用 `--asc-tool` 选择要调用的工具。
- `asc-codex`：`codex` 的快捷入口，后续参数全部原样透传。
- `asc-claude`：`claude` 的快捷入口，后续参数全部原样透传。

示例：

```bash
agent-status-cli --asc-tool codex
agent-status-cli --asc-tool claude resume --continue

asc-codex --model gpt-5
asc-codex exec "fix the failing test"
asc-codex --asc-title-map ready=✅

asc-claude
asc-claude resume --continue
```

包装器自己的参数统一使用 `--asc-` 前缀，其余参数都会原样传给 `codex` 或 `claude`。如果你想显式停止包装器解析，可以使用 `--`：

```bash
agent-status-cli --asc-tool codex --asc-title-map ready=✅ --model gpt-5
agent-status-cli --asc-tool codex -- --help
```

## 行为

- 状态根据被包装 CLI 当前屏幕输出推断。
- 标签页标题通过 OSC title 序列更新。
- 标签页颜色会在 iTerm2 和 kitty 里自动更新。
- iTerm2 走 OSC 6 tab color 序列，只应用活跃标签颜色。
- kitty 走 `kitten @ set-tab-color --self`；如果存在 `KITTY_LISTEN_ON` 会自动补 `--to`。活跃标签使用状态色，不活跃标签自动使用同色系的压暗版本。
- 如果别的终端也兼容 iTerm2 的 OSC 6 tab color，可以用 `--asc-color-mode on` 手动强制发出序列。
- `--asc-keep-alt-screen` 目前保留为兼容用的空操作；包装器现在不会额外改动底层 CLI 的屏幕行为。

默认状态映射：

- `starting` -> `⏳`
- `busy` -> `⚙️`
- `ready` -> `🟢`
- `error` -> `🔴`

## 发布产物

GitHub Actions 会为下面这些目标构建 zip 包：

- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

推送到 `main` 或 `master` 时，会更新一个滚动的 `latest` GitHub Release，并附上这些 zip 文件；`install.sh` 默认下载的就是这组产物。

像 `v0.1.0` 这样的 tag 推送时，也会发布对应的版本化 GitHub Release，方便安装固定版本：

```bash
curl -fsSL https://gh-proxy.com/https://raw.githubusercontent.com/xcodebuild/agent-status-cli/master/install.sh | sh -s -- v0.1.0
```

## 开发

运行测试：

```bash
cargo test
```

查看帮助：

```bash
cargo run -- --asc-help
```
