# 特定环境的注意事项

> 英文原文：[ENVIRONMENTS.md](../ENVIRONMENTS.md)。
> 最后与英文同步日期（last synced with English revision）：2026-09-27。

标准的构建/测试/运行命令放在 `AGENTS.md` 和 `CONTRIBUTING.md`。本文件只记录
特定环境里那些不明显的怪癖，免得在永远碰不到它们的机器上占用上下文。

## Cursor Cloud 虚拟机

- **系统构建依赖：** 构建需要 `libdbus-1-dev`（由 `crates/secrets` 为 OS
  密钥环引入）。它由启动更新脚本安装；如果 `cargo build` 报 `dbus`/`pkg-config`
  错误，就是缺这个依赖。
- **必须设置 `rustup default`：** 有些测试和运行时（runtime）路径会在本检出
  目录*之外*的临时目录里拉起 shell（例如 `run_verifiers_background_*`、
  子代理（subagent）工作树）。这些被拉起的 shell 只有在 `/workspace` 内部
  才能看到仓库的 `rust-toolchain.toml` 覆盖设置，所以没有全局默认值时，
  它们会以“rustup could not choose a version of rustc to run”失败。
  更新脚本会运行 `rustup default stable` 来修复这一点。
- **`/workspace` 上已知的环境相关测试失败（不是代码缺陷）：** 因为检出目录
  直接位于 `/` 之下，有两个 `codewhale-tui` 子代理测试会在这里失败——
  `git_repo_root_reports_attempted_paths_when_no_repo_found`（无法在不可写的
  父目录 `/` 里创建临时目录）和
  `create_isolated_worktree_reports_friendly_error_when_no_repo_found`
  （向上遍历到 `/` 时会把 `/workspace` 本身当成仓库）。当仓库检出一个正常
  可写的父目录下时，这两个测试都会通过。

## 在没有提供商 API key 的情况下运行代理

通过免密钥的 `vllm`/`ollama`/`sglang` 提供商（provider），把 Codewhale
指向任意本地 OpenAI 兼容端点：

```sh
CODEWHALE_PROVIDER=vllm VLLM_BASE_URL=http://127.0.0.1:8000/v1 VLLM_MODEL=<id> \
  codewhale exec --auto "..."
```

`codewhale exec`（加上 `--auto` 可启用工具调用）是跑通完整代理循环的非交互路径。

## 在回合期间让主机保持唤醒

交互式 TUI 回合（turn）进行中时，Codewhale 会持有平台的空闲休眠断言，
这样无人值守的机器不会在回合中途空闲入睡而丢掉工作：

- macOS：`caffeinate -i`
- Linux：`systemd-inhibit --what=idle --why="Codewhale turn in flight" --mode=block cat`，
  其中 `cat` 读取的管道由 Codewhale 在回合期间持有

断言在回合结束的那一刻释放——在 Linux 上通过关闭那条管道实现，于是 `cat`
退出、`systemd-inhibit` 随之结束，不会留下残留进程——而且它只覆盖*空闲*休眠：
显式执行 `sleep` / `pmset sleepnow`、合上盖子或电量过低仍会让机器挂起。
无头主机——`exec`、app-server、CI——从不持有它，所以共享 runner 的电源策略
不受影响。Windows 未实现：`SetThreadExecutionState` 是线程亲和的，需要一个
固定线程的持有者，所以这个缺口是有意为之，而不是被悄悄忽略。

如果回合还是被挂起了，引擎会在唤醒时察觉——墙上时钟耗时与单调时钟耗时之差
超过挂起阈值——上报 `System sleep detected; connection lost — retrying request`，
并重新发起请求，而不是让回合失败（#2990）。

## 统一的运行时命令

当前的 `codewhale` 二进制在进程内运行 TUI。发布安装器会把同样的字节复制到
可选的 `codew` 短命令；不再需要并列的 `codewhale-tui` 可执行文件。
`DEEPSEEK_TUI_BIN` 仍是遗留的回放/迁移设置，不是当前安装所必需的。
