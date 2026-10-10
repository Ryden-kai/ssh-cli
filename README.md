# ssh-cli

A focused SSH host manager that wraps the system `ssh`/`scp` binaries. Designed
for both interactive shell use and AI-agent invocation (think `ssh-cli exec
arch-prod -- pacman -Syu` from a script or MCP tool).

The point: stop editing `~/.ssh/config` by hand and stop re-typing
`ssh -p 2222 -i ~/.ssh/foo user@10.0.0.5`. Register hosts once, refer to them
by alias forever.

[![License: MIT](https://img.shields.io/badge/License-MIT-yellow.svg)](LICENSE-MIT)
[![License: Apache 2.0](https://img.shields.io/badge/License-Apache%202.0-blue.svg)](LICENSE-APACHE)

## Why this exists

Other "ssh-cli" / "ssh-manager" tools on GitHub are either:

- 0-star and abandoned for years
- Trying to reimplement the SSH protocol in pure Rust (losing `ssh-agent`, `ProxyJump`, `ControlMaster`, `known_hosts` trust, etc.)
- Bundled into a much larger AI CLI workspace

ssh-cli deliberately does NOT reimplement SSH — it wraps the system OpenSSH
client you already have, so you keep `ssh-agent`, `ProxyJump`, `ControlMaster`,
host-key trust (`known_hosts`), `~/.ssh/config`, port forwarding via `-L`/`-R`,
and every other OpenSSH feature for free.

## Features

- **Alias system** — register hosts once as `web1`, `db-prod`, `arch-atom`, etc.
- **TOML config** with `[defaults]` and per-host overrides
- **`exec` with full output capture** — stdout/stderr separate, exit code propagates
- **Long-task fire-and-forget** — `-d LOG` returns PID in ~1s, perfect for AI agents with timeouts
- **Bounded exec** — `-t N` kills after N seconds, exit 124
- **Local stdin forwarding** — heredocs and pipes work out of the box
- **File transfer** — `ssh-cli cp alias:/path /local` (pull) or `/local alias:/path` (push)
- **Port forwarding** — `tunnel local/remote/dynamic` with persistent `--detach` (PID-tracked)
- **Bulk operations** — `for h in $(ssh-cli list --json | jq -r ...); do ... done`
- **`--json` everywhere it matters** — for structured access from agents
- **24+ unit tests** + every flag manually verified on a real box

## 📖 AI Agent Manual

If you're an AI agent reading this repo, see **[docs/AI-USAGE.md](docs/AI-USAGE.md)**
for the complete usage manual — command reference, patterns, gotchas, recipes,
and a one-page cheat sheet at the bottom.

## 📦 Worked example

See **[examples/deepseek-harness-build.sh](examples/deepseek-harness-build.sh)** for
a real-world wrapper that fires off a long pnpm build via SSH tunnel and
returns the PID in ~1 second — illustrating Pattern E (fire-and-forget) from
the manual.

## Install

```bash
# If you have Rust toolchain:
git clone https://github.com/<owner>/ssh-cli.git
cd ssh-cli
cargo install --path .

# Or use the bundled install.sh:
./install.sh                # build + install to ~/.local/bin
./install.sh --no-build     # just copy existing release binary
./install.sh --mirror-bin  # also copy to ~/bin if it's on PATH
```

The binary is `ssh-cli`. Add `~/.local/bin` (or `~/.cargo/bin`) to your `PATH`
if it isn't already.

## Quick start

```bash
ssh-cli init
ssh-cli add my-box --host 1.2.3.4 --user alice --identity ~/.ssh/id_ed25519
ssh-cli exec my-box -- 'uname -a && uptime'
```

```bash
# Long-running task, returns in ~1s with a PID:
ssh-cli exec -d //tmp/build.log my-box -- 'cd /app && make -j8'

# File transfer:
ssh-cli cp my-box:/etc/nginx/nginx.conf ./nginx.conf
ssh-cli cp ./nginx.conf my-box:/etc/nginx/nginx.conf

# Port forwarding (e.g. access a remote web UI via 127.0.0.1):
ssh-cli tunnel local 8080:127.0.0.1:8080 my-box --detach

# Bulk operations:
for h in $(ssh-cli list --json | jq -r '.[].alias'); do
    ssh-cli exec "$h" -- 'hostname'
done
```

See **[docs/AI-USAGE.md](docs/AI-USAGE.md)** for the full manual.

## License

Licensed under either of:

- Apache License, Version 2.0 ([LICENSE-APACHE](LICENSE-APACHE))
- MIT license ([LICENSE-MIT](LICENSE-MIT))

at your option.

## Contributing

Issues and PRs welcome. For substantial changes, please open an issue first
to discuss the design. The manual at [docs/AI-USAGE.md](docs/AI-USAGE.md) has
a "Gotchas" section that documents real failure modes — adding tests that cover
those before fixing is appreciated.