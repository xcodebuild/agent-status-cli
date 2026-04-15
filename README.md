# agent-status-cli

[中文说明](README_CN.md)

`agent-status-cli` wraps an interactive coding CLI in a PTY and mirrors its current state into your terminal tab title and, in iTerm2, the tab color. It is designed for running multiple agent sessions side by side without losing track of which tab is busy, ready, starting, or broken.

## Quick Start

Install the latest release:

```bash
curl -fsSL https://gh-proxy.com/https://raw.githubusercontent.com/xcodebuild/agent-status-cli/master/install.sh | sh
```

Quick examples:

```bash
asc-codex
asc-claude
```

## Commands

The project ships three executables:

- `agent-status-cli`: generic wrapper, choose the tool with `--asc-tool`.
- `asc-codex`: fast path for `codex`; all following arguments are passed through.
- `asc-claude`: fast path for `claude`; all following arguments are passed through.

Examples:

```bash
agent-status-cli --asc-tool codex
agent-status-cli --asc-tool claude resume --continue

asc-codex --model gpt-5
asc-codex exec "fix the failing test"
asc-codex --asc-title-map ready=✅

asc-claude
asc-claude resume --continue
```

Wrapper options are now the arguments prefixed with `--asc-`. Everything else is passed through to `codex` or `claude` unchanged. If you want to stop wrapper parsing explicitly, use `--`:

```bash
agent-status-cli --asc-tool codex --asc-title-map ready=✅ --model gpt-5
agent-status-cli --asc-tool codex -- --help
```

## Behavior

- Status is inferred from the wrapped CLI screen output.
- Tab titles are updated through OSC title sequences.
- Tab colors are updated only when running inside iTerm2.
- `--asc-keep-alt-screen` is kept as a compatibility no-op; the wrapper currently preserves the wrapped CLI screen behavior as-is.

Default state mappings:

- `starting` -> `⏳`
- `busy` -> `⚙️`
- `ready` -> `🟢`
- `error` -> `🔴`

## Release Artifacts

GitHub Actions builds zip artifacts for:

- `x86_64-unknown-linux-gnu`
- `x86_64-apple-darwin`
- `aarch64-apple-darwin`

Pushes to `main` or `master` update a rolling `latest` GitHub Release with those zip files, which is what `install.sh` downloads by default.

Tagged pushes like `v0.1.0` also publish versioned GitHub Releases so you can install a pinned build:

```bash
curl -fsSL https://gh-proxy.com/https://raw.githubusercontent.com/xcodebuild/agent-status-cli/master/install.sh | sh -s -- v0.1.0
```

## Development

Run tests:

```bash
cargo test
```

Show help:

```bash
cargo run -- --asc-help
```
