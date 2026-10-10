# Changelog

All notable changes to ssh-cli are documented here. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/) and the project
adheres to [Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [Unreleased]

## [0.1.7] - 2026-10-04

### Added
- `tunnel list` and `tunnel kill [-y] [ALIAS]` subcommands to manage PID-tracked
  detached tunnels (kills ssh.exe via `taskkill /PID N /F` on Windows, `kill` on Unix)
- `is_pid_alive` now requires image name to be `ssh.exe` (was: only PID — broken by
  Windows PID recycling where a fresh `cmd.exe` got the recycled PID)
- Defensive warning when the remote command has an odd number of single quotes —
  a strong indicator that an outer shell (cmd.exe / `cmd /c`) split argv before
  ssh-cli saw it. Suggests using `bash -c` instead.
- `ServerAliveInterval=15` + `ServerAliveCountMax=3` automatically added to
  detached tunnels (silent network blips no longer kill the listener)

### Fixed
- Detached ssh tunnels actually persisting past ssh-cli exit (was: `ssh -f`'s
  forked child was killed by Windows Job Object when ssh-cli exited)
- Stdio deadlock on detached tunnels (was: `Stdio::piped()` on stderr without a
  reader → ssh.exe blocks when its 4 KiB warning buffer fills → ssh dies)

## [0.1.6] - 2026-09-29

### Added
- `tunnel local/remote/dynamic` subcommand (ssh -L / -R / -D) with
  `--detach`, `--exit-on-failure`, `--idle-exit`

## [0.1.5] - 2026-09-29

### Fixed
- Detached ssh tunnel now persists after ssh-cli exit (spawn with
  `CREATE_NEW_PROCESS_GROUP | DETACHED_PROCESS` instead of `ssh -f`)
- Tunnel PID file tracking under `<config_dir>/tunnels/`

## [0.1.4] - 2026-09-29

### Added
- `tunnel` subcommand (initial implementation; superseded by 0.1.5/0.1.6)

## [0.1.2] - 2026-09-29

### Added
- `--detach LOG` on `exec` for fire-and-forget long-running tasks (returns PID
  in ~1s)
- `--timeout SECS` on `exec` (kills after N seconds, exit 124)
- AI agent usage manual at `docs/AI-USAGE.md`

## [0.1.1] - 2026-09-29

### Fixed
- Local stdin now forwarded to remote by default (heredocs and pipes work)
- `cp` parser accepts alias-only form (`alias:/path`) without explicit fallback
- Added `--no-stdin` flag

## [0.1.0] - 2026-09-29

### Added
- Initial release
- Core commands: `init`, `list`, `show`, `add`, `remove`, `rename`, `connect`,
  `exec`, `cp`, `ping`, `doctor`, `config-path`
- TOML config with `[defaults]` and `[hosts.*]` sections
- Alias validation: `[A-Za-z0-9._-]{1,64}`, no leading `.` or `-`
- `--json` output for `list`, `show`, `doctor`
- Cross-platform config paths (Linux/macOS: `~/.config/ssh-cli/`,
  Windows: `%APPDATA%\ssh-cli\`)
- `SSH_CLI_CONFIG` env var for test-time overrides
- 12 unit tests