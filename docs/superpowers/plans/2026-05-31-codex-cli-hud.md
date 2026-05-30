# Codex CLI HUD Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** 让用户在终端输入 `codex` 时，默认进入同一个交互式会话，并在同一终端底部显示 inline HUD 状态栏。

**Architecture:** 这不是 Codex 原生插件注入，而是一个 PATH 级 wrapper + PTY host。核心 crate `codex-hud` 负责 app-server 连接、状态归并和 HUD 渲染；`src/bin/codex.rs` 负责接管交互式启动、发现真实 `codex`、选择传输路径、托管子 PTY，并把上方的 Codex 交互区和底部 HUD 组合到同一个终端里。官方文档保证 `codex` 交互 TUI、`codex app-server` 的 `unix://` / `ws://` 传输和 `codex --remote ws://...` 路径；当前安装的 CLI help 还额外显示 `--remote unix://` / `unix://PATH`，所以实现里要先做 capability probe：能直连 unix 就直连，不能就退回到文档化的 loopback `ws://` bridge。

**Tech Stack:** Rust, `tokio`, `tokio-tungstenite`, `crossterm`, `ratatui`, `serde`, `serde_json`, `clap`, `tracing`, `notify`, `tempfile`.

---

**Reality check:** 目前仓库只有设计文档和这份计划，没有可执行 Rust 源码树。计划顺序按“先把启动器接管入口，再接协议，再做 HUD，再做 PTY 宿主与安装”排布，避免把未确认的实现细节提前写死。

### Task 1: Bootstrap the crate and shared smoke test

**Files:**
- Create: `Cargo.toml`
- Create: `src/lib.rs`
- Create: `src/bin/codex.rs`
- Create: `tests/smoke.rs`

- [ ] **Step 1: Define the crate surface**

建立最小 crate：包名 `codex-hud`，导出一个稳定的 `app_name()` 或同等标识函数，作为后续 wrapper、HUD 和测试的公共入口。

- [ ] **Step 2: Add the smoke test**

`tests/smoke.rs` 只做两件事：确认 crate 能被测试框架加载，以及 `app_name()` 返回 `codex-hud`。这个测试的目的是先锁住包名和二进制入口，不碰任何 Codex 协议逻辑。

- [ ] **Step 3: Run the smoke test**

Run: `cargo test --test smoke -v`

Expected: 通过；如果连 crate 都无法加载，先把脚手架补齐再进入下一步。

- [ ] **Step 4: Commit**

```bash
git add Cargo.toml src/lib.rs src/bin/codex.rs tests/smoke.rs
git commit -m "feat: bootstrap codex hud crate"
```

### Task 2: Implement wrapper routing and real binary discovery

**Files:**
- Create: `src/wrapper.rs`
- Modify: `src/lib.rs`
- Modify: `src/bin/codex.rs`
- Create: `tests/wrapper_args.rs`

- [ ] **Step 1: Lock down command classification**

`tests/wrapper_args.rs` 要覆盖三类行为：
1. 需要接管的交互式入口：空参数、裸 prompt、`resume`、`fork`、带模型提示的交互启动。
2. 必须透传的非交互命令：`exec`、`review`、`login`、`logout`、`mcp`、`plugin`、`mcp-server`、`app-server`、`remote-control`、`completion`、`update`、`doctor`、`sandbox`、`debug`、`apply`、`cloud`、`exec-server`、`help`，以及 `-h`、`--help`、`-V`、`--version`。
3. 参数保持顺序不变，只在需要接管时前置 remote 相关参数。

- [ ] **Step 2: Lock down real binary discovery**

测试真实 `codex` 的查找逻辑必须跳过 wrapper 自己，保持 PATH 顺序可预测，并且在 PATH 里同时存在多个 `codex` 时返回“后一个真实二进制”而不是当前 wrapper。

- [ ] **Step 3: Run the wrapper tests**

Run: `cargo test --test wrapper_args -v`

Expected: 通过。

- [ ] **Step 4: Commit**

```bash
git add src/wrapper.rs src/lib.rs src/bin/codex.rs tests/wrapper_args.rs
git commit -m "feat: add codex wrapper routing"
```

### Task 3: Implement app-server protocol client and local transport selection

**Files:**
- Create: `src/protocol.rs`
- Create: `src/bridge.rs`
- Modify: `src/wrapper.rs`
- Modify: `src/lib.rs`
- Create: `tests/link_unix.rs`
- Create: `tests/bridge_roundtrip.rs`

- [ ] **Step 1: Nail the unix-socket handshake**

`tests/link_unix.rs` 要验证最小握手顺序：连接 `unix://` app-server，发起 `initialize`，读取初始化结果，再发送 `initialized`。如果后续要读取 thread 或 account 状态，这个测试还要继续验证能顺利发出 `thread/read`、`account/rateLimits/read` 之类的请求并拿到 JSON 结果。

- [ ] **Step 2: Nail the local WebSocket bridge**

`tests/bridge_roundtrip.rs` 要验证本地 `ws://127.0.0.1:PORT` 代理能把客户端 JSON-RPC 原样转发到 unix backend，并把 backend 的响应和事件继续转回来。这个测试只关心双向 relay，不关心 UI。

- [ ] **Step 3: Add runtime capability probing**

实现一个一次性探测：当前安装的 `codex --help` 是否已经接受 `--remote unix://...`。如果接受，就允许 launcher 直接连 unix remote；如果不接受，就强制走官方文档保证的 loopback `ws://` bridge。这个探测结果要缓存，避免每次启动都重复调用 help。

- [ ] **Step 4: Run the transport tests**

Run: `cargo test --test link_unix --test bridge_roundtrip -v`

Expected: 通过。

- [ ] **Step 5: Commit**

```bash
git add src/protocol.rs src/bridge.rs src/wrapper.rs src/lib.rs tests/link_unix.rs tests/bridge_roundtrip.rs
git commit -m "feat: add app-server transport layer"
```

### Task 4: Build the HUD snapshot and compact renderer

**Files:**
- Create: `src/hud.rs`
- Create: `src/surface.rs`
- Modify: `src/lib.rs`
- Create: `tests/hud_render.rs`

- [ ] **Step 1: Define the snapshot contract**

`HudSnapshot` 需要能装下当前窗口的最小真相：`threadId`、当前线程名称、模型、turn 状态、token usage、rate limit、MCP / tool 摘要，以及本地补充信息如 cwd、git branch、git dirty 状态。不要把完整 transcript 当成 HUD 输入。

- [ ] **Step 2: Define compact rendering**

`tests/hud_render.rs` 要验证 80 列左右的宽度下，HUD 仍能稳定输出一到两行，并且至少包含模型、上下文使用率、线程名和项目/分支摘要。宽度不足时要截断而不是撑爆布局。

- [ ] **Step 3: Define expanded rendering**

expanded 视图要补充 plan、goal、MCP、tool 汇总和 account / rate limit 摘要，但仍然保持终端内可读，不要演变成独立大面板。

- [ ] **Step 4: Run the renderer test**

Run: `cargo test --test hud_render -v`

Expected: 通过。

- [ ] **Step 5: Commit**

```bash
git add src/hud.rs src/surface.rs src/lib.rs tests/hud_render.rs
git commit -m "feat: render compact HUD surface"
```

### Task 5: Implement the PTY host and inline bottom bar composition

**Files:**
- Create: `src/pty.rs`
- Modify: `src/bin/codex.rs`
- Modify: `src/lib.rs`
- Create: `tests/pty_layout.rs`
- Create: `tests/launcher_flow.rs`

- [ ] **Step 1: Reserve the bottom rows**

`tests/pty_layout.rs` 要验证底栏行数预留不会 underflow；例如终端只有 2 行而状态栏要 4 行时，也必须至少保住 1 行给子进程，避免把终端压成不可用状态。

- [ ] **Step 2: Verify fallback selection**

同一组测试要确认 `dumb` / 不支持 inline 布局的终端会落到 split 或 plain TUI fallback，而支持正常终端布局的环境继续走 inline status bar。

- [ ] **Step 3: Verify launcher flow**

`tests/launcher_flow.rs` 要确认 `src/bin/codex.rs` 只接管交互式启动，并且能把真实 Codex 的退出码原样返回。`codex exec ...`、`codex plugin ...` 这些非交互路径必须完全透传。

- [ ] **Step 4: Run the PTY tests**

Run: `cargo test --test pty_layout --test launcher_flow -v`

Expected: 通过。

- [ ] **Step 5: Commit**

```bash
git add src/pty.rs src/bin/codex.rs src/lib.rs tests/pty_layout.rs tests/launcher_flow.rs
git commit -m "feat: add inline PTY host primitives"
```

### Task 6: Add config loading, install flow, and runtime defaults

**Files:**
- Create: `src/config.rs`
- Modify: `src/lib.rs`
- Modify: `install.sh`
- Create: `tests/config.rs`
- Create: `tests/launcher_e2e.rs`

- [ ] **Step 1: Lock down config discovery**

`tests/config.rs` 要验证默认配置路径是 `~/.config/codex-hud/config.toml`，缺失文件会回落到内置默认值，默认值至少要覆盖 launcher surface、bridge listen、status rows、expanded rows 和 auto-show 行为。

- [ ] **Step 2: Lock down install flow**

`install.sh` 要把 wrapper 放进 PATH 的前面，同时保持真实 `codex` 仍然可被 wrapper 找到，不允许安装脚本把系统里原来的 Codex 覆盖掉。

- [ ] **Step 3: Add an end-to-end launcher smoke test**

`tests/launcher_e2e.rs` 要在临时 PATH 下模拟一个真实 `codex`，再运行当前 wrapper 二进制，确认交互式启动会被接管，而 `exec` / `plugin` 仍然透传。

- [ ] **Step 4: Run the config and launcher tests**

Run: `cargo test --test config --test launcher_e2e -v`

Expected: 通过。

- [ ] **Step 5: Commit**

```bash
git add src/config.rs src/lib.rs install.sh tests/config.rs tests/launcher_e2e.rs
git commit -m "feat: finish launcher config and e2e coverage"
```

### Task 7: Final integration pass and manual verification against the official Codex flow

**Files:**
- Modify: `src/bin/codex.rs`
- Modify: `src/lib.rs`
- Modify: `codex-cli-hud-design.md` only if runtime probing or fallback text needs another round of校正

- [ ] **Step 1: Wire the final launcher**

把 wrapper、transport selector、bridge、PTY host、HUD renderer 和 config 一次性连起来：交互式 `codex` 自动进入 inline HUD；非交互命令完全透传；当 HUD 或 bridge 失败时，直接降级到普通 Codex 交互会话，并保留原始退出码。

- [ ] **Step 2: Run the full verification set**

Run: `cargo test && cargo fmt --check && cargo clippy --all-targets -- -D warnings`

Expected: 全部通过，没有 warning。

- [ ] **Step 3: Manual smoke check**

手工验证三条路径：
1. `codex` 进入同一终端的交互界面并显示底部 HUD。
2. `codex exec ...`、`codex plugin ...`、`codex --help` 仍然透传。
3. 断开 app-server 或 bridge 时，launcher 会退回到普通 Codex，而不是卡死在 wrapper。

- [ ] **Step 4: Commit**

```bash
git add src/bin/codex.rs src/lib.rs
git commit -m "feat: wire codex inline hud end to end"
```

## Coverage notes

- 设计目标 1-4：Task 1、Task 4、Task 6、Task 7
- 共享 unix app-server 和传输兼容层：Task 3 和 Task 7
- 一窗一 thread 的事件过滤：Task 3、Task 4、Task 7
- inline 底栏和 PTY 处理：Task 5 和 Task 7
- 配置和安装：Task 6
- 测试、fmt、clippy：每个 Task 的验证步骤和最终集成检查
