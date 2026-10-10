# ssh-cli

<p align="right">
  <a href="README.md">English</a>
  &nbsp;·&nbsp;
  <a href="README.zh.md"><b>中文</b></a>
</p>

一个聚焦的 SSH 主机管理器，包装系统 `ssh`/`scp` 二进制。同时为交互式 shell
使用和 AI 代理调用而设计（想想从脚本或 MCP 工具里跑
`ssh-cli exec arch-prod -- pacman -Syu`）。

目标是：不再手改 `~/.ssh/config`，不再反复敲
`ssh -p 2222 -i ~/.ssh/foo user@10.0.0.5`。注册一次主机名，用别名永久调用。

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE-APACHE)

## 为什么做这个

GitHub 上其他 "ssh-cli" / "ssh-manager" 工具的现状：

- 要么 0 star 多年没维护
- 要么试图用纯 Rust 重新实现 SSH 协议（失去 `ssh-agent`、`ProxyJump`、`ControlMaster`、`known_hosts` 信任链等）
- 要么塞在一个很大的 AI CLI workspace 里

ssh-cli 故意**不**重新实现 SSH —— 它包装系统自带的 OpenSSH 客户端，所以
`ssh-agent`、`ProxyJump`、`ControlMaster`、主机密钥信任（`known_hosts`）、
`~/.ssh/config`、`-L`/`-R` 端口转发等所有 OpenSSH 特性**白送**。

## 功能特性

- **别名系统** — 注册一次主机，后续用 `web1`、`db-prod`、`arch-atom` 等别名
- **TOML 配置** — `[defaults]` 默认段 + 每主机覆盖
- **`exec` 全输出捕获** — stdout / stderr 分开，退出码透传
- **长任务 fire-and-forget** — `-d LOG` 1 秒内返回 PID，专治 AI 代理超时
- **限时执行** — `-t N` 到点强杀，exit 124
- **本地 stdin 转发** — heredoc、管道默认就能用
- **文件传输** — `ssh-cli cp alias:/path /local`（拉）或 `/local alias:/path`（推）
- **端口转发** — `tunnel local/remote/dynamic` + 持久 `--detach`（PID 自动跟踪）
- **批量操作** — `for h in $(ssh-cli list --json | jq -r ...); do ... done`
- **`--json` 该有的地方都有** — 方便代理结构化解析
- **24+ 单元测试** + 每个 flag 都在真机上验证过

## 📖 AI 代理手册

如果你是 AI 代理在读这个仓库，看 **[docs/AI-USAGE.md](docs/AI-USAGE.md)**
拿完整使用手册——命令参考、模式、踩坑、recipes、底部有一页 cheat sheet。

## 📦 真实示例

**[examples/deepseek-harness-build.sh](examples/deepseek-harness-build.sh)** 是一个真实场景封装：
通过 SSH 隧道跑一个长 pnpm build，1 秒内返回 PID —— 演示手册里的 Pattern E
（fire-and-forget）。

## 安装

```bash
# 如果你有 Rust 工具链：
git clone https://github.com/Ryden-kai/ssh-cli.git
cd ssh-cli
cargo install --path .

# 或者用自带的 install.sh：
./install.sh                # build + 装到 ~/.local/bin
./install.sh --no-build     # 只拷现成的 release 二进制
./install.sh --mirror-bin  # 也拷一份到 ~/bin（如果在 PATH 上）
```

二进制叫 `ssh-cli`。把 `~/.local/bin`（或 `~/.cargo/bin`）加进 `PATH`（如果还没）。

## 快速上手

```bash
ssh-cli init
ssh-cli add my-box --host 1.2.3.4 --user alice --identity ~/.ssh/id_ed25519
ssh-cli exec my-box -- 'uname -a && uptime'
```

```bash
# 长任务，1 秒内返回 PID：
ssh-cli exec -d //tmp/build.log my-box -- 'cd /app && make -j8'

# 文件传输：
ssh-cli cp my-box:/etc/nginx/nginx.conf ./nginx.conf
ssh-cli cp ./nginx.conf my-box:/etc/nginx/nginx.conf

# 端口转发（比如通过 127.0.0.1 访问远端 web UI）：
ssh-cli tunnel local 8080:127.0.0.1:8080 my-box --detach

# 批量操作：
for h in $(ssh-cli list --json | jq -r '.[].alias'); do
    ssh-cli exec "$h" -- 'hostname'
done
```

完整手册看 **[docs/AI-USAGE.md](docs/AI-USAGE.md)**。

## 🤖 AI 代理：帮我装

把下面这段 prompt 复制给你的 AI 代理（Claude Code、Cursor、WorkBuddy、
Cody、Aider 等）：

````text
在这台机器上从 https://github.com/Ryden-kai/ssh-cli 安装 ssh-cli。
步骤：
1. 跑 cargo --version 确认 Rust 工具链已装。如果没装，去 https://rustup.rs/ 装（一行命令：curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh）。
2. 跑下面这条装 ssh-cli：
   cargo install --git https://github.com/Ryden-kai/ssh-cli.git --tag v0.1.7
   （这条会克隆、build、把 ssh-cli 二进制装到 ~/.cargo/bin/）
3. 验证：跑 ssh-cli --version，应该打印 ssh-cli 0.1.7（或更新）。如果 ssh-cli 不在 $PATH 里，把 ~/.cargo/bin 加进去（或拷到 ~/.local/bin/）。
4. 冒烟测试：跑 ssh-cli doctor，应该报告 ssh 和 scp 二进制都找到了。
5. （可选）跑 ssh-cli init 在平台默认位置建空 config（Linux/macOS：~/.config/ssh-cli/config.toml，Windows：%APPDATA%\ssh-cli\config.toml）。

不要加主机、改 config、或连任何真实 SSH 服务器 —— 只装 + 验证。报告最终 ssh-cli --version 输出、doctor 摘要、和任何报错。如果遇到 build 失败（比如网慢、缺系统依赖），原样报错误，不要瞎猜 workaround。
````

代理装完后你可以直接加主机：

```bash
ssh-cli add my-box --host 1.2.3.4 --user alice --identity ~/.ssh/id_ed25519
ssh-cli exec my-box -- 'uptime'
```

<details>
<summary>等价的手动安装命令（人类 / CI）</summary>

如果你想自己装或在 CI 里脚本化：

```bash
# 方案 A —— 直接从 git 装（不用先 clone）：
cargo install --git https://github.com/Ryden-kai/ssh-cli.git --tag v0.1.7

# 方案 B —— clone + 从源码装：
git clone https://github.com/Ryden-kai/ssh-cli.git
cd ssh-cli
cargo install --path .

# 方案 C —— 用自带的 install.sh（和方案 B 一样，但额外把二进制拷到
# ~/.local/bin，不用改 PATH）：
git clone https://github.com/Ryden-kai/ssh-cli.git
cd ssh-cli
./install.sh

# 验证：
ssh-cli --version
ssh-cli doctor
```

</details>

## 许可证

任选其一：

- Apache License, Version 2.0（[LICENSE-APACHE](LICENSE-APACHE)）
- MIT license（[LICENSE-MIT](LICENSE-MIT)）

## 贡献

欢迎 issue 和 PR。改动比较大前请先开个 issue 讨论设计。
[docs/AI-USAGE.md](docs/AI-USAGE.md) 里的"踩坑"章节记录了真实失败模式 —— 修之前先加测试覆盖，欢迎贡献。