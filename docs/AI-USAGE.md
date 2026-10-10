# ssh-cli — AI Agent Usage Manual

A focused reference for AI agents invoking `ssh-cli`. Read this once and you'll
have everything you need.

> **TL;DR**
> ```bash
> ssh-cli exec ALIAS -- 'cmd'         # run, capture stdout, propagate exit
> ssh-cli exec -d //tmp/x.log ALIAS -- 'cmd'   # fire-and-forget, returns PID in ~1s
> ssh-cli exec -t 600 ALIAS -- 'cmd'  # bounded exec, exit 124 if timeout
> ssh-cli cp alias:/path /local      # pull file from remote
> ssh-cli cp /local alias:/path      # push file to remote
> ssh-cli list                       # show all hosts (--json for structured)
> ssh-cli show ALIAS --json          # print resolved config + exact ssh argv
> ```

---

## 1. What ssh-cli is (and isn't)

**Is**: A small Rust CLI that wraps the system `ssh`/`scp` binaries. It
manages host aliases in a TOML file and translates `ssh-cli exec my-box --
uptime` into `ssh -p 2222 -i ~/.ssh/foo user@1.2.3.4 uptime`.

**Isn't**: It does **not** reimplement the SSH protocol. So you keep every
feature of system OpenSSH for free: `ssh-agent`, `ProxyJump`, `ControlMaster`,
host key trust (`known_hosts`), `~/.ssh/config`, port forwarding via `-L`/`-R`,
keychain integration, etc.

**Doesn't support**:
- Password auth (uses `BatchMode=yes` — no way to type a password)
- Interactive prompts of any kind
- tmux/screen persistence (use the `-d` flag for fire-and-forget)

If you need any of those, drop down to raw `ssh` or `ssh-copy-id`.

---

## 2. Install & quick start

```bash
# Already installed in this env at ~/.local/bin/ssh-cli.exe (version 0.1.2+).
# Verify:
ssh-cli --version

# Three commands to be productive:
ssh-cli init                                                      # create empty config
ssh-cli add my-box --host 1.2.3.4 --user alice --identity ~/.ssh/id_ed25519
ssh-cli exec my-box -- 'uname -a && uptime'
```

Config file lives at:
- Linux/macOS/WSL: `~/.config/ssh-cli/config.toml`
- Windows:         `%APPDATA%\ssh-cli\config.toml`
- Override (tests/CI): `export SSH_CLI_CONFIG=/path/to/cfg.toml`

---

## 3. Command reference

### `ssh-cli init`
Create empty config at the platform-default path. No-op if it already exists.

### `ssh-cli config-path`
Print the resolved config file path. Use this when you don't know where the
config lives (e.g. in a new shell).

### `ssh-cli doctor [--json]`
Check: config exists & parses, `ssh`/`scp` on PATH, defaults sensible. Returns
0 always; issues are listed in the report. Read the table or `--json` output.

### `ssh-cli list [--json]`
Table view of all hosts. Columns: ALIAS, HOST, USER, PORT, IDENTITY, TAGS, DESC.
`--json` gives an array of objects (one per host) for structured parsing.

### `ssh-cli show ALIAS [--json]`
Human view: destination, port, identity (resolved), desc, tags, extra opts.
`--json` includes a `ssh_argv` array — the EXACT argv that would be passed to
`ssh`. **Use this to debug "why isn't key auth working?"** by comparing the
argv to your `~/.ssh/config`.

### `ssh-cli add ALIAS [--host H] [--user U] [--port P] [--identity FILE] [--desc "..."] [--tags t1,t2] [--force]`
Required: `--host`. Everything else falls back to `[defaults]` or prompts
interactively if missing.

### `ssh-cli remove ALIAS [-y]`
Interactive confirmation unless `-y`.

### `ssh-cli rename OLD NEW`
Atomically changes the alias key. Fails if NEW already exists.

### `ssh-cli connect ALIAS`
Spawns `ssh` with full TTY passthrough. For interactive shells. Exit code
propagates from remote.

### `ssh-cli exec ALIAS [--no-stdin] [--timeout SECS] [--detach LOG] -- CMD...`
The workhorse. Everything after `--` is the remote command (passed verbatim).
Captures stdout/stderr separately, propagates exit code.

| Flag | Behavior |
|---|---|
| (default) | Forward local stdin to remote (heredoc / pipes work) |
| `-n`, `--no-stdin` | Close local stdin (mirrors `ssh -n`) |
| `-t`, `--timeout SECS` | Kill after N seconds; exit 124 like coreutils `timeout` |
| `-d`, `--detach LOG` | Run via `nohup bash -c '...' </dev/null >>LOG 2>&1 & disown`; print `started on ALIAS: pid=N (log: LOG)` and return in ~1s |
| `--json` | Output machine-readable JSON where supported |

### `ssh-cli tunnel <local|remote|dynamic> <SPEC> <ALIAS> [--bind ADDR] [--detach] [--exit-on-failure] [--idle-exit SECS]`
Open an SSH port-forwarding tunnel via system `ssh`. Three modes:

| Mode | Effect | SPEC format |
|---|---|---|
| `local` | `ssh -L`: listen locally, forward to remote target | `[bind:]port:host:hostport` |
| `remote` | `ssh -R`: listen on remote, forward to local target | `[bind:]port:host:hostport` |
| `dynamic` | `ssh -D`: SOCKS5 proxy on a local port | just `port` |

Flags:
- `--bind ADDR` (default `127.0.0.1`) — listening interface
- `--detach` — run in background; ssh-cli spawns ssh with detached process flags so it survives ssh-cli exit; PID is written to `<config_dir>/tunnels/<alias>-bind-port.pid` for later cleanup
- `--exit-on-failure` — fail fast if forward can't be set up
- `--idle-exit SECS` — terminate after N seconds with no keepalive

> ⚠️ The `-L/-R/-D` listeners bind on the LOCAL ssh (where ssh-cli runs), not the
> remote. **Detach does NOT use `ssh -f`** — on Windows, ssh -f's forked child dies
> when ssh-cli exits (Job Object cleanup). ssh-cli instead spawns ssh with
> `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` so it survives.

### `ssh-cli tunnel list`
List tracked detached tunnels by reading PID files under
`<config_dir>/tunnels/`. Shows PID, alive status (via `tasklist`/`kill -0`), and
the PID file path.

### `ssh-cli tunnel kill [-y] [ALIAS]`
Kill detached ssh tunnel processes that `ssh-cli tunnel ... --detach` started.
- No args: kill ALL tracked tunnels
- With `ALIAS`: kill only tunnels for that alias (matches `<alias>-*.pid`)
- Without `-y`: dry-run (print what would be killed)

Stale PID files (process already gone) are removed automatically.

### `ssh-cli cp SRC DST`
File transfer via `scp`. Accepted forms for either side:
- Local path: `/tmp/foo`, `./relative.txt`, `C:/Users/me/file.txt`
- Alias form: `alias:/remote/path` (alias resolved from config)
- Native form: `user@host:/path`

Examples:
```bash
ssh-cli cp my-box:/etc/nginx/nginx.conf ./nginx.conf        # pull
ssh-cli cp ./nginx.conf my-box:/etc/nginx/nginx.conf        # push
ssh-cli cp alice@1.2.3.4:/tmp/x /tmp/x                     # native form
```

### `ssh-cli ping ALIAS`
Quick reachability check: `ssh ALIAS true`. Exits 0 if reachable+auth OK,
otherwise non-zero with ssh's stderr surfaced.

---

## 4. AI agent patterns (the meat)

### Pattern A — run a command, parse the output
```bash
out=$(ssh-cli exec my-box -- 'cat /etc/os-release')
echo "$out"
```
Stdout is the command's stdout. Stderr goes to your stderr. Exit code propagates.

### Pattern B — fail-fast on non-zero exit
```bash
if ! ssh-cli exec my-box -- 'systemctl is-active nginx'; then
    echo "nginx is down"
fi
# or short-circuit:
ssh-cli exec my-box -- 'test -f /etc/app.conf' || { echo "missing"; exit 1; }
```

### Pattern C — pipe local stdin into the remote command
```bash
echo "config content" | ssh-cli exec my-box -- 'cat > /etc/app.conf'
```
Local stdin is forwarded by default. Use `-n` to close it.

### Pattern D — heredoc into a remote script
```bash
ssh-cli exec my-box -- bash <<'REMOTE'
set -e
cd /opt/app
./deploy.sh
REMOTE
```
The quoted `'REMOTE'` prevents local variable expansion — the heredoc body
goes to remote bash literally.

### Pattern E — long-running task, fire-and-forget (CRITICAL)
**This is the pattern for builds, deploys, batch jobs that take minutes.**

```bash
# Start detached. Returns ~1 second later with:
#   started on my-box: pid=2474 (log: //tmp/build.log)
ssh-cli exec -d //tmp/build.log my-box -- 'cd /opt/app && pnpm run build'

# In a SEPARATE call (use local run_in_background=true to not block):
ssh-cli exec my-box -- 'tail -f //tmp/build.log'
```

**DO NOT** roll your own `setsid bash -c "..." & disown; sleep N; pgrep ...`
wrapper. The ssh-cli `-d` flag does it correctly in 1s; a manual wrapper
adds 5-10s of overhead and your framework's bash tool may SIGTERM you before
you see the PID.

> 📦 **Concrete example**: `examples/deepseek-harness-build.sh` in this repo
> wraps this exact pattern for a real pnpm/tsc -b build on arch-atom,
> complete with `--watch` mode and `pkill` cleanup hints. Read it as a
> worked example.

### Pattern F — bounded execution with timeout
```bash
ssh-cli exec -t 600 my-box -- ./long-job.sh    # max 10 min, exit 124 if exceeded
```

### Pattern G — combine detach + soft timeout
Wait N seconds for the detached task to make progress; return whether it
finished. (Implementation note: in 0.1.2 the wrapper returns immediately
even with `-t`, so this is best implemented as: start with `-d`, then poll
the log with a separate `-t` `tail` call.)

### Pattern H — inspect what ssh will actually run
```bash
ssh-cli show my-box --json | python -c "import json,sys; print(json.load(sys.stdin)['ssh_argv'])"
# → ['-p', '22', '-o', 'BatchMode=yes', '-i', 'C:/Users/.../id_ed25519', 'alice@1.2.3.4']
```
Compare this against your `~/.ssh/config` overrides when debugging.

### Pattern I — structured host discovery
```bash
ssh-cli list --json | jq '.[] | select(.tags | contains(["prod"])) | .alias'
# → "web1" "web2" ...
```

### Pattern J — bulk operations
```bash
for h in $(ssh-cli list --json | jq -r '.[].alias'); do
    ssh-cli exec "$h" -- 'hostname' && echo "$h OK" || echo "$h FAIL"
done
```

---

## 5. Gotchas — read this section before debugging

### G8. `cmd.exe` mangles argv when invoked via `cmd.exe /c "..."`
If the caller invokes ssh-cli through `cmd.exe /c "..."` (some Windows
automation tools do this), cmd.exe splits on spaces AND treats
`&&`/`||`/`|` as command-chain operators — single quotes are NOT
recognized as grouping. A command like:
```
ssh-cli exec box -- 'cd /opt && rm -rf x && echo done'
```
becomes (from cmd.exe's perspective) three separate commands:
```
cmd1: ssh-cli exec box -- 'cd /opt
cmd2: rm -rf x
cmd3: echo done
```
ssh-cli receives argv with an unmatched `'` in element 4, ssh joins the
parts and forwards to sshd, and the remote bash sees `... -- 'cd /opt`
and reports `unexpected EOF`.

This is NOT ssh-cli's bug — the breakage happened before ssh-cli saw
the argv. ssh-cli 0.1.7+ detects the symptom (odd number of `'` across
the cmd argv) and emits a one-line warning to stderr before the
remote-shell error so the cause is obvious.

Fix in the caller: invoke via `bash -c "..."` instead of `cmd.exe /c "..."`.

### G1. Git Bash converts `/tmp/foo` to a Windows path
On Windows + Git Bash, MSYS rewrites `/tmp/foo` to
`C:/Users/<you>/AppData/Local/Temp/foo` before it reaches ssh-cli's argv.
The remote Linux rejects this — `cat` or `bash` won't write to a Windows path.

**Fix** (pick one):
```bash
ssh-cli exec -d //tmp/build.log my-box -- cmd         # double-slash escapes MSYS
MSYS_NO_PATHCONV=1 ssh-cli exec -d /tmp/build.log my-box -- cmd   # one-shot override
```

This affects ONLY `ssh-cli exec -d LOG` (the log path). Remote command
arguments inside the `-- cmd` body are passed verbatim — they survive Git Bash
fine because they're already inside single quotes.

### G2. `~/.ssh/config` can shadow command-line `-i` flags
System ssh reads `~/.ssh/config` BEFORE command-line flags. A block like:
```
Host 1.2.3.4
    PubkeyAuthentication no
    PreferredAuthentications password
```
forces password auth and BLOCKS all key attempts, even if ssh-cli passes
`-i /path/to/key`. This is the #1 reason "ssh-cli says Permission denied
even though my key is in authorized_keys".

**Fix**: remove the override, or change `no` to `yes`. See §6 for the
complete battle story.

### G3. Host key changed after reinstall
After reinstalling an OS, sshd regenerates its host key. ssh rejects the
connection with "REMOTE HOST IDENTIFICATION HAS CHANGED".

**Fix**:
```bash
ssh-keygen -R HOSTNAME_OR_IP -f ~/.ssh/known_hosts
ssh-keyscan -t ed25519 HOSTNAME >> ~/.ssh/known_hosts
```
Or one-shot for a single connection: `ssh -o StrictHostKeyChecking=accept-new ...`
(not built into ssh-cli; use raw ssh for setup).

### G4. Commands with spaces or quotes
The remote command must be a single string argument after `--`:
```bash
ssh-cli exec my-box -- 'echo "hello world"'     # ✓ single string
ssh-cli exec my-box -- echo "hello world"       # ✗ splits; local shell sees 3 args
```
For complex commands, use a single-quoted string with `set -e` and
explicit paths.

### G5. Alias validation
Aliases must match `[A-Za-z0-9._-]{1,64}` and cannot start with `.` or `-`.
This is enforced on every command. Bad input fails fast — you'll see
`Error: InvalidAlias("...")`.

### G6. `identity_file` is local, NOT remote
The `identity_file` field in config is expanded on the LOCAL machine (`~/`
resolves to your home dir here, not the remote's). It must point to a key
whose **public** counterpart is in `~/.ssh/authorized_keys` on the remote.

### G7. No password auth
ssh-cli sets `BatchMode=yes` and `-o PubkeyAuthentication=yes` is implicit.
If a host has no key set up, ssh-cli can't help you log in — you'll need
`ssh-copy-id` (interactive password) once, or manually paste the public key
into the remote's `authorized_keys`.

### G8. `exec` exit code 124 means timeout
124 is the convention from coreutils `timeout`. Distinguish:
- exit 0: command succeeded
- exit 1-125: remote command's own exit code
- exit 126: command found but not executable
- exit 127: command not found
- exit 124: ssh-cli `--timeout` killed it
- exit 255: ssh itself failed (auth, network, host key)

---

## 6. Battle story (read if you hit a wall)

These are real failures we hit building ssh-cli. Skim the headings; deep-dive
only if relevant.

### 6.1 `Permission denied (publickey,password)` even with right key
→ see G2. Check `ssh-cli show ALIAS --json` for the argv, then check
`~/.ssh/config` for a `PubkeyAuthentication no` override for that host.

### 6.2 `REMOTE HOST IDENTIFICATION HAS CHANGED`
→ see G3. `ssh-keygen -R` then `ssh-keyscan` then retry.

### 6.3 `cp` says `InvalidScpTarget("alias:/path")`
Bug in pre-0.1.1. Update to ≥0.1.1. The parser now accepts alias-only form
without a fallback and defers resolution to `resolve_alias_in_target`.

### 6.4 `exec` loses heredoc / pipe input
Bug in pre-0.1.1. The remote command's stdin was being closed locally.
Update to ≥0.1.1. Default is now inherit; use `-n` to opt out.

### 6.5 Long command times out, framework sends SIGTERM, you can't see the PID
Use `-d` (Pattern E). Don't roll your own `setsid ... & disown` wrapper.
Manual wrappers add 5-10s of overhead before returning the PID; framework
timeouts (30-60s) often fire first. The `-d` flag returns in ~1s.

### 6.6 `pacman --version` outputs nothing via exec
Caused by G4 reversed: stdin was closed → pacman suppresses banner.
Fixed by the stdin-inherit change in 0.1.1.

---

## 7. Recipes

### Discover what's on a box
```bash
ssh-cli exec my-box -- 'uname -a; echo; cat /etc/os-release; echo; uptime; echo; df -h /'
```

### Update packages
```bash
# Arch
ssh-cli exec my-box -- 'pacman -Syu --noconfirm'
# Debian/Ubuntu
ssh-cli exec my-box -- 'apt update && apt upgrade -y'
```

### Deploy a single binary
```bash
ssh-cli cp ./build/myapp my-box:/usr/local/bin/myapp
ssh-cli exec my-box -- 'systemctl restart myapp.service && systemctl status myapp.service --no-pager'
```

### Tail a log
```bash
# One-shot, last 50 lines
ssh-cli exec my-box -- 'tail -50 /var/log/app.log'

# Streaming (call with local run_in_background=true; stdin is inherited so Ctrl+C is clean)
ssh-cli exec my-box -- 'tail -f /var/log/app.log'
```

### Watch a long build
```bash
# Start detached
ssh-cli exec -d //tmp/build.log my-box -- 'cd /app && pnpm run build'

# Liveness check
ssh-cli exec my-box -- 'pgrep -af "pnpm run build" || echo "process gone"'

# Recent log lines
ssh-cli exec my-box -- 'tail -30 //tmp/build.log'

# Force-kill if stuck
ssh-cli exec my-box -- 'pkill -f "pnpm run build"; echo killed'
```

### Copy a directory recursively
ssh-cli uses `scp` under the hood, which supports `-r` via extra args. For v0.1.x,
use the `extra_opts` field in config, or wrap with a tar pipe:
```bash
ssh-cli exec my-box -- 'tar czf - /etc/myapp' > snapshot.tar.gz
# restore:
ssh-cli exec my-box -- 'tar xzf - -C /etc/myapp' < snapshot.tar.gz
```

### Find a process by name across hosts
```bash
for h in $(ssh-cli list --json | jq -r '.[].alias'); do
    echo "=== $h ==="
    ssh-cli exec "$h" -- "pgrep -af nginx || echo none"
done
```

### Generate a per-host report (JSON)
```bash
for h in $(ssh-cli list --json | jq -r '.[].alias'); do
    ssh-cli exec "$h" -- 'uname -r; cat /etc/os-release; uptime' 2>/dev/null \
        | jq -R -s --arg h "$h" '{host:$h, raw:.}' \
        # ... aggregate as you like
done
```

---

## 8. Config reference

```toml
version = 1

[defaults]
# Any field set here applies to every host that doesn't override it.
user = "deploy"
port = 22
identity_file = "~/.ssh/id_ed25519"
extra_opts = ["-o", "ServerAliveInterval=60"]

[hosts.web1]
host = "10.0.0.10"
desc = "Frontend"
tags = ["prod", "web"]

[hosts.db1]
host = "10.0.0.20"
user = "postgres"
port = 2222
identity_file = "~/.ssh/db1_ed25519"
desc = "Primary database"
tags = ["prod", "db"]
extra_opts = ["-o", "ForwardAgent=yes"]   # appended after [defaults].extra_opts
```

Field precedence per host: `host`-level value → `[defaults]` value →
built-in fallback (port=22). `user` has no built-in fallback — set it
somewhere or `cmd_exec` returns `MissingUser`.

Aliases are stored as `BTreeMap<String, Host>` so `ssh-cli list` is
deterministically sorted.

---

## 9. When ssh-cli is NOT the right tool

| Situation | Use instead |
|---|---|
| Password auth (first-time setup) | raw `ssh user@host` + paste key into `~/.ssh/authorized_keys` |
| Port forwarding ergonomics | raw `ssh -L 8080:localhost:80 user@host` (or wait for v0.2's `tunnel` subcommand) |
| Multi-hop (jump hosts) | raw `ssh -J jump user@target` (works inside `extra_opts`) |
| `scp -r` recursive copy | tar pipe (see Recipes) |
| Run something on agent host THEN ssh | plain bash + raw ssh |
| `mosh` (mobile-friendly persistent shell) | raw `mosh` |

---

## 10. Versioning & breaking changes

- 0.1.0 — initial release
- 0.1.1 — fixed stdin forwarding (heredoc/pipes), `--no-stdin` flag
- 0.1.2 — added `--detach` and `--timeout` flags for async/long-running tasks

This document tracks 0.1.2+. If a feature isn't here, check
`D:/WorkBuddy/Arch Linux/ssh-cli/docs/` for a newer revision.

---

## 11. One-page cheat sheet

```bash
# Setup
ssh-cli init
ssh-cli add web1 --host 10.0.0.10 --user deploy --identity ~/.ssh/id_ed25519
ssh-cli add db1  --host 10.0.0.20 --user postgres --port 2222

# Discovery
ssh-cli list                          # table
ssh-cli list --json | jq              # structured
ssh-cli show web1                     # resolved config
ssh-cli show web1 --json | jq .ssh_argv   # exact ssh argv
ssh-cli ping web1                     # reachability
ssh-cli doctor                        # env check

# Run
ssh-cli exec web1 -- 'uptime'
ssh-cli exec -t 60 web1 -- ./slow.sh          # bounded
ssh-cli exec -d //tmp/x.log web1 -- make -j8  # fire-and-forget
echo data | ssh-cli exec web1 -- 'cat > /tmp/in'   # stdin forward
ssh-cli exec -n web1 -- ./readonly              # close stdin

# Transfer
ssh-cli cp web1:/etc/app.conf ./
ssh-cli cp ./app.conf web1:/etc/app.conf
ssh-cli cp alice@1.2.3.4:/x /tmp/x            # native form

# Bulk
for h in $(ssh-cli list --json | jq -r '.[].alias'); do
    ssh-cli exec "$h" -- 'hostname'
done

# Watch
ssh-cli exec web1 -- 'tail -f /var/log/app.log'   # with local run_in_background=true
ssh-cli exec web1 -- 'pgrep -af myapp || echo gone' # liveness

# Misc
ssh-cli rename web1 web1-prod
ssh-cli remove web1 -y
ssh-cli config-path                       # where is my config?
```
