# agent-status-cli

[English README](README.md)

`agent-status-cli` 会把 Codex 或 Claude 这类交互式 CLI 包在一个 PTY 里运行，然后把当前状态同步到终端标签页标题；如果你用的是 iTerm2，还会顺手改标签页颜色。多开几个 agent 时，哪个还在跑、哪个已经空闲，一眼就能看出来。

## 快速安装

安装最新 release：

```bash
curl -fsSL https://gh-proxy.com/https://raw.githubusercontent.com/xcodebuild/agent-status-cli/master/install.sh | sh
```

简单示例：

```bash
asc-codex
asc-claude
```

预览：

下图为 iTerm2 将 `Tab bar location` 设为 `Left` 时的效果。

![agent-status-cli preview](https://img.cdn1.vip/i/69def8deb0077_1776220382.webp)

下图为 iTerm2 将 `Tab bar location` 设为 `Top` 时的效果。

![agent-status-cli preview top](https://img.cdn1.vip/i/69def996cb76d_1776220566.webp)

## 可执行文件

项目会安装三个命令：

- `agent-status-cli`：通用入口，用 `--asc-tool` 选择要包裹的 CLI。
- `asc-codex`：直接走 `codex`，后面的参数原样透传。
- `asc-claude`：直接走 `claude`，后面的参数原样透传。

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

现在包装器自己的参数统一使用 `--asc-` 前缀，其他参数都会原样透传给 `codex` 或 `claude`。如果你想显式停止包装器解析，可以加 `--`：

```bash
agent-status-cli --asc-tool codex --asc-title-map ready=✅ --model gpt-5
agent-status-cli --asc-tool codex -- --help
```

## 行为说明

- 状态来自被包装 CLI 当前屏幕上的可见内容。
- 标题通过 OSC title 序列更新。
- 颜色只会在 iTerm2 里生效。
- `--asc-keep-alt-screen` 目前只保留为兼容参数，不会额外改动底层 CLI 的屏幕行为。

默认状态映射：

- `starting` -> `⏳`
- `busy` -> `⚙️`
- `ready` -> `🟢`
- `error` -> `🔴`

## 发布产物

GitHub Actions 会构建下面三个 zip：

- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

推送到 `main` 或 `master` 时，workflow 会更新一个滚动的 `latest` GitHub Release；`install.sh` 默认就是从这里下载。

推送 `v0.1.0` 这种 tag 时，workflow 还会额外发布一个带版本号的 GitHub Release，方便安装固定版本：

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
