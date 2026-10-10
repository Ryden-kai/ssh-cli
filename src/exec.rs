//! Build ssh/scp argv and spawn the system binaries.
//!
//! We deliberately do NOT reimplement the SSH protocol — that would mean losing
//! ssh-agent, ProxyJump, ControlMaster, known_hosts trust, and so on.
//! Instead we manage host config here and delegate the actual transport to the
//! OpenSSH client that's already on the user's PATH.

use crate::config::{expand_home, EffectiveHost};
use crate::error::{Error, Result};
use std::io::{Read, Write};
use std::path::PathBuf;
use std::process::{Command, Stdio};

/// Resolve the on-disk path of an external binary (ssh, scp, ...).
pub fn find_binary(name: &str) -> Result<PathBuf> {
    if let Some(p) = which(name) {
        return Ok(p);
    }
    #[cfg(windows)]
    {
        if let Some(sys) = std::env::var_os("SystemRoot") {
            let candidate = PathBuf::from(sys)
                .join("System32")
                .join("OpenSSH")
                .join(format!("{name}.exe"));
            if candidate.is_file() {
                return Ok(candidate);
            }
        }
    }
    Err(if name == "scp" {
        Error::ScpMissing
    } else {
        Error::SshMissing
    })
}

/// Build the `ssh` argv for an interactive connect or a remote command.
pub fn build_ssh_argv(h: &EffectiveHost<'_>, remote_cmd: &[&str]) -> Vec<String> {
    let mut argv: Vec<String> = Vec::with_capacity(8 + remote_cmd.len());
    argv.push("-p".into());
    argv.push(h.port.to_string());
    argv.push("-o".into());
    argv.push("BatchMode=yes".into());
    argv.push("-o".into());
    argv.push(format!("ConnectTimeout={}", 15));

    if let Some(id) = h.identity_file.as_deref() {
        argv.push("-i".into());
        argv.push(expand_home(id).to_string_lossy().into_owned());
    }
    for opt in &h.extra_opts {
        argv.push(opt.clone());
    }
    argv.push(h.destination());
    argv.extend(remote_cmd.iter().map(|s| (*s).to_string()));
    argv
}

/// One side of a port forward — local listener or remote listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TunnelForward {
    /// `-L bind:port:host:hostport` — listen locally, forward to remote target.
    Local {
        bind: String,
        port: u16,
        host: String,
        host_port: u16,
    },
    /// `-R bind:port:host:hostport` — listen on remote, forward to local target.
    Remote {
        bind: String,
        port: u16,
        host: String,
        host_port: u16,
    },
    /// `-D bind:port` — SOCKS5 dynamic proxy on local side.
    Dynamic { bind: String, port: u16 },
}

/// Build ssh argv for a port-forwarding tunnel (no remote command).
///
/// Adds `-N` so ssh doesn't try to run a command on the remote end (it's a
/// pure forwarding session). The caller chooses whether to spawn this
/// foreground (inherits TTY for Ctrl+C / status messages) or wrap it in
/// `nohup ... &` for detach.
///
/// `auto_keepalive = true` adds `ServerAliveInterval=15` + `ServerAliveCountMax=3`
/// so transient network blips don't kill the tunnel silently. Used by the
/// detached path; the foreground path inherits the user's `extra_opts` instead.
pub fn build_ssh_argv_tunnel(
    h: &EffectiveHost<'_>,
    forward: &TunnelForward,
    exit_on_failure: bool,
    idle_exit_secs: Option<u64>,
    auto_keepalive: bool,
) -> Vec<String> {
    let mut argv: Vec<String> = Vec::with_capacity(16);
    argv.push("-p".into());
    argv.push(h.port.to_string());
    argv.push("-N".into());
    argv.push("-o".into());
    argv.push("BatchMode=yes".into());
    argv.push("-o".into());
    argv.push(format!("ConnectTimeout={}", 15));

    if let Some(id) = h.identity_file.as_deref() {
        argv.push("-i".into());
        argv.push(expand_home(id).to_string_lossy().into_owned());
    }
    for opt in &h.extra_opts {
        argv.push(opt.clone());
    }

    match forward {
        TunnelForward::Local { bind, port, host, host_port } => {
            argv.push("-L".into());
            argv.push(format!("{bind}:{port}:{host}:{host_port}"));
        }
        TunnelForward::Remote { bind, port, host, host_port } => {
            argv.push("-R".into());
            argv.push(format!("{bind}:{port}:{host}:{host_port}"));
        }
        TunnelForward::Dynamic { bind, port } => {
            argv.push("-D".into());
            argv.push(format!("{bind}:{port}"));
        }
    }

    if exit_on_failure {
        argv.push("-o".into());
        argv.push("ExitOnForwardFailure=yes".into());
    }

    if auto_keepalive {
        // Detect silent drops (sleeping host, brief network blips, etc.) and
        // exit non-zero instead of leaving a half-dead listener behind.
        argv.push("-o".into());
        argv.push("ServerAliveInterval=15".into());
        argv.push("-o".into());
        argv.push("ServerAliveCountMax=3".into());
    }

    if let Some(secs) = idle_exit_secs {
        // ServerAliveInterval=15 + ServerAliveCountMax=N gives ~N*15s idle timeout
        // before the connection drops (and the foreground ssh exits).
        let max_count = secs.div_ceil(15).max(1);
        argv.push("-o".into());
        argv.push("ServerAliveInterval=15".into());
        argv.push("-o".into());
        argv.push(format!("ServerAliveCountMax={max_count}"));
    }

    argv.push(h.destination());
    argv
}

/// Parsed `[bind:]port:host:hostport` components. The caller turns this into
/// a `TunnelForward::Local` or `TunnelForward::Remote` based on CLI mode.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardParts {
    pub bind: String,
    pub port: u16,
    pub host: String,
    pub host_port: u16,
}

/// Parse `[bind:]port:host:hostport` for local/remote forwards.
///
/// Accepts either 3 parts (`port:host:hostport` — bind defaults) or 4 parts
/// (`bind:port:host:hostport`).
pub fn parse_forward_spec(spec: &str, default_bind: &str) -> Result<ForwardParts> {
    let parts: Vec<&str> = spec.split(':').collect();
    let parse_port = |s: &str| -> Result<u16> {
        s.parse::<u16>()
            .map_err(|_| Error::InvalidPort(format!("in spec `{spec}`: `{s}`")))
    };
    match parts.len() {
        3 => Ok(ForwardParts {
            bind: default_bind.to_string(),
            port: parse_port(parts[0])?,
            host: parts[1].to_string(),
            host_port: parse_port(parts[2])?,
        }),
        4 => Ok(ForwardParts {
            bind: parts[0].to_string(),
            port: parse_port(parts[1])?,
            host: parts[2].to_string(),
            host_port: parse_port(parts[3])?,
        }),
        _ => Err(Error::InvalidTunnelSpec(spec.to_string())),
    }
}

/// Parse `port` for dynamic (SOCKS5) forwards.
pub fn parse_dynamic_spec(spec: &str, default_bind: &str) -> Result<TunnelForward> {
    let port: u16 = spec
        .parse()
        .map_err(|_| Error::InvalidPort(format!("dynamic spec `{spec}`")))?;
    Ok(TunnelForward::Dynamic {
        bind: default_bind.to_string(),
        port,
    })
}

/// One side of an `scp` transfer.
#[derive(Debug, Clone)]
pub enum ScpTarget {
    Local(PathBuf),
    Remote {
        user: String,
        host: String,
        port: u16,
        path: String,
        identity_file: Option<String>,
    },
}

impl ScpTarget {
    pub fn parse(spec: &str, fallback: Option<&EffectiveHost<'_>>) -> Result<Self> {
        // Three accepted shapes:
        //   - local path (no `:` before any path separator, or Windows drive letter)
        //   - `user@host:path`   (SSH-native syntax)
        //   - `alias:path`       (looks up the alias in config)
        if spec.contains(':') {
            // If the colon is at position 1 and the first char is a letter and the second is `:`
            // it's almost certainly a Windows drive letter — treat as local.
            let bytes = spec.as_bytes();
            if bytes.len() >= 2 && bytes[1] == b':' && bytes[0].is_ascii_alphabetic() {
                return Ok(ScpTarget::Local(PathBuf::from(spec)));
            }
            if let Some((left, right)) = spec.split_once(':') {
                let left = left.trim();
                let right = right.to_string();
                // alias-only form (no '@'): if we have a fallback effective host, fill in
                // immediately; otherwise return a Remote whose `host` field still holds the
                // alias string so a later `resolve_alias_in_target` pass can look it up.
                if !left.contains('@') {
                    if let Some(eff) = fallback {
                        return Ok(ScpTarget::Remote {
                            user: eff.user.clone(),
                            host: eff.host.to_string(),
                            port: eff.port,
                            path: right,
                            identity_file: eff.identity_file.clone(),
                        });
                    }
                    return Ok(ScpTarget::Remote {
                        user: String::new(),
                        host: left.to_string(),
                        port: 22,
                        path: right,
                        identity_file: None,
                    });
                }
                // user@host form
                let (user, host) = left.split_once('@').ok_or_else(|| Error::InvalidScpTarget(spec.to_string()))?;
                let port = if let Some(eff) = fallback {
                    eff.port
                } else {
                    22
                };
                let identity_file = fallback.and_then(|e| e.identity_file.clone());
                return Ok(ScpTarget::Remote {
                    user: user.to_string(),
                    host: host.to_string(),
                    port,
                    path: right,
                    identity_file,
                });
            }
        }
        Ok(ScpTarget::Local(PathBuf::from(spec)))
    }

    pub fn port(&self) -> u16 {
        match self {
            ScpTarget::Remote { port, .. } => *port,
            _ => 22,
        }
    }

    pub fn identity_file(&self) -> Option<String> {
        match self {
            ScpTarget::Remote { identity_file, .. } => identity_file.clone(),
            _ => None,
        }
    }

    pub fn to_arg(&self) -> String {
        match self {
            ScpTarget::Local(p) => p.to_string_lossy().into_owned(),
            ScpTarget::Remote { user, host, path, .. } => {
                if host.contains(':') {
                    format!("{user}@[{host}]:{path}")
                } else {
                    format!("{user}@{host}:{path}")
                }
            }
        }
    }
}

/// Build the `scp` argv for one source → one destination.
pub fn build_scp_argv(src: &ScpTarget, dst: &ScpTarget, extra: &[&str]) -> Vec<String> {
    let mut argv: Vec<String> = Vec::with_capacity(10);
    let port = if src.port() != 22 { src.port() } else { dst.port() };
    argv.push("-P".into());
    argv.push(port.to_string());
    argv.push("-o".into());
    argv.push("BatchMode=yes".into());
    if let Some(id) = src.identity_file().clone().or_else(|| dst.identity_file()) {
        argv.push("-i".into());
        argv.push(expand_home(&id).to_string_lossy().into_owned());
    }
    for opt in extra {
        argv.push((*opt).to_string());
    }
    argv.push(src.to_arg());
    argv.push(dst.to_arg());
    argv
}

/// Output captured from a non-interactive remote exec.
#[derive(Debug, Default, Clone)]
pub struct ExecOutput {
    pub stdout: String,
    pub stderr: String,
    /// Exit code of the remote command. `None` means ssh itself failed to launch.
    pub exit_code: Option<i32>,
}

/// Run a remote command non-interactively; capture stdout/stderr, propagate exit code.
///
/// `inherit_stdin = false` (default) closes local stdin so the remote process sees EOF
/// immediately — matches `BatchMode=yes` semantics and avoids accidental hangs if the
/// remote command tries to read input. Set `true` when the caller wants to forward
/// local stdin (heredoc / pipe) to the remote command.
///
/// If `timeout_secs` is `Some(n)`, the child is killed after N seconds and `exit_code`
/// is reported as 124 (the convention coreutils `timeout` uses).
pub fn run_remote_exec(
    h: &EffectiveHost<'_>,
    remote_cmd: &[&str],
    inherit_stdin: bool,
    timeout_secs: Option<u64>,
) -> Result<ExecOutput> {
    let bin = find_binary("ssh")?;
    let argv = build_ssh_argv(h, remote_cmd);
    let mut cmd = Command::new(&bin);
    cmd.args(&argv);
    cmd.stdin(if inherit_stdin { Stdio::inherit() } else { Stdio::null() });
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());
    // Inherit env but scrub SSH_AUTH_SOCK only if user opted in (not for now).
    let mut child = cmd.spawn().map_err(|e| {
        Error::Io(std::io::Error::new(
            e.kind(),
            format!("failed to spawn {}: {}", bin.display(), e),
        ))
    })?;

    // Optional watchdog: poll try_wait() every 100ms; if elapsed > timeout, kill the child.
    if let Some(secs) = timeout_secs {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(secs);
        loop {
            match child.try_wait() {
                Ok(Some(_)) => break, // child exited on its own
                Ok(None) => {
                    if std::time::Instant::now() >= deadline {
                        let _ = child.kill();
                        let _ = child.wait();
                        // Drain pipes so they don't block on a dead process, then report 124.
                        let mut stdout = child.stdout.take().expect("piped");
                        let mut stderr = child.stderr.take().expect("piped");
                        let t_out = std::thread::spawn(move || {
                            let mut s = String::new();
                            let _ = stdout.read_to_string(&mut s);
                            s
                        });
                        let t_err = std::thread::spawn(move || {
                            let mut s = String::new();
                            let _ = stderr.read_to_string(&mut s);
                            s
                        });
                        return Ok(ExecOutput {
                            stdout: t_out.join().unwrap_or_default(),
                            stderr: t_err.join().unwrap_or_default(),
                            exit_code: Some(124),
                        });
                    }
                    std::thread::sleep(std::time::Duration::from_millis(100));
                }
                Err(e) => return Err(Error::Io(e)),
            }
        }
    }

    let mut stdout = child.stdout.take().expect("piped");
    let mut stderr = child.stderr.take().expect("piped");

    // Concurrently drain both pipes so a chatty writer can't deadlock on a full pipe buffer.
    let t_out = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stdout.read_to_string(&mut s);
        s
    });
    let t_err = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let status = child.wait()?;
    let out_buf = t_out.join().unwrap_or_default();
    let err_buf = t_err.join().unwrap_or_default();
    Ok(ExecOutput {
        stdout: out_buf,
        stderr: err_buf,
        exit_code: status.code(),
    })
}

/// Run ssh interactively — pass TTY through, propagate exit code to caller.
pub fn run_remote_connect(h: &EffectiveHost<'_>) -> Result<i32> {
    let bin = find_binary("ssh")?;
    let argv = build_ssh_argv(h, &[]);
    let status = Command::new(&bin)
        .args(&argv)
        .stdin(Stdio::inherit())
        .stdout(Stdio::inherit())
        .stderr(Stdio::inherit())
        .status()?;
    Ok(status.code().unwrap_or(1))
}

/// Build the nohup-backgrounded wrapper that ssh-cli uses for `exec --detach`.
///
/// Returns a single shell-safe string the remote shell will execute. The wrapper:
///   - puts the user's command in a `bash -c '...'` (handles quoting),
///   - redirects stdin/stdout/stderr to log_path,
///   - runs with `nohup` and `&` so it survives the SSH session closing,
///   - `disown`s so SIGHUP doesn't reach it,
///   - echoes the spawned PID to stdout so ssh-cli can report it.
pub fn build_detach_argv(user_cmd: &[&str], log_path: &str) -> Vec<String> {
    let inner = user_cmd.join(" ");
    let inner_quoted = format!("'{}'", inner.replace('\'', "'\\''"));
    let log_quoted = format!("'{}'", log_path.replace('\'', "'\\''"));
    let wrapper = format!(
        "nohup bash -c {inner_quoted} </dev/null >>{log_quoted} 2>&1 & disown; echo \"pid=$!\"",
    );
    vec![wrapper]
}

/// Run scp; propagate exit code.
pub fn run_scp(src: &ScpTarget, dst: &ScpTarget) -> Result<i32> {
    let bin = find_binary("scp")?;
    let argv = build_scp_argv(src, dst, &[]);
    let mut cmd = Command::new(&bin);
    cmd.args(&argv);
    cmd.stdin(Stdio::inherit());
    cmd.stdout(Stdio::inherit());
    // Capture stderr so we can prefix it on failure.
    cmd.stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let mut err_pipe = child.stderr.take().expect("piped");
    let mut err_buf = String::new();
    let t = std::thread::spawn(move || {
        let _ = err_pipe.read_to_string(&mut err_buf);
        err_buf
    });
    let status = child.wait()?;
    let err_final = t.join().unwrap_or_default();
    if !status.success() {
        // Surface scp's own error text to the caller via stderr.
        let mut sys = std::io::stderr().lock();
        let _ = writeln!(sys, "{err_final}");
    }
    Ok(status.code().unwrap_or(1))
}

fn which(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    let exts: Vec<String> = if cfg!(windows) {
        std::env::var("PATHEXT")
            .unwrap_or_else(|_| ".EXE;.CMD;.BAT;.COM".into())
            .split(';')
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .collect()
    } else {
        vec![String::new()]
    };
    for dir in std::env::split_paths(&path) {
        for ext in &exts {
            let candidate = dir.join(format!("{name}{ext}"));
            if candidate.is_file() {
                return Some(candidate);
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ssh_argv_includes_user_host_and_identity() {
        let h = EffectiveHost {
            alias: "box",
            host: "10.0.0.5",
            user: "rayden".into(),
            port: 2222,
            identity_file: Some("~/.ssh/test_ed25519".into()),
            extra_opts: vec![],
            desc: None,
            tags: &[],
        };
        let argv = build_ssh_argv(&h, &["uptime"]);
        assert_eq!(argv[0], "-p");
        assert_eq!(argv[1], "2222");
        assert!(argv.contains(&"rayden@10.0.0.5".to_string()));
        assert!(argv.contains(&"uptime".to_string()));
        assert!(argv.iter().any(|s| s == "-i"));
    }

    #[test]
    fn scp_target_parses_alias() {
        let h = EffectiveHost {
            alias: "box",
            host: "10.0.0.5",
            user: "rayden".into(),
            port: 22,
            identity_file: None,
            extra_opts: vec![],
            desc: None,
            tags: &[],
        };
        let t = ScpTarget::parse("box:/tmp/x", Some(&h)).unwrap();
        assert_eq!(t.to_arg(), "rayden@10.0.0.5:/tmp/x");
    }

    #[test]
    fn scp_target_local_path() {
        let t = ScpTarget::parse("/tmp/foo", None).unwrap();
        match t {
            ScpTarget::Local(p) => assert_eq!(p.to_str().unwrap(), "/tmp/foo"),
            _ => panic!("expected local"),
        }
    }

    #[test]
    fn scp_target_windows_drive_letter_stays_local() {
        let t = ScpTarget::parse("C:/Users/me/file.txt", None).unwrap();
        assert!(matches!(t, ScpTarget::Local(_)));
    }

    #[test]
    fn scp_target_alias_only_without_fallback_returns_remote_with_alias_host() {
        // Without a fallback, alias-only form should NOT error — it should return a Remote
        // whose host field is the alias string, so resolve_alias_in_target can look it up.
        let t = ScpTarget::parse("box:/tmp/x", None).unwrap();
        match t {
            ScpTarget::Remote { host, user, path, .. } => {
                assert_eq!(host, "box");
                assert_eq!(path, "/tmp/x");
                assert!(user.is_empty(), "user must be empty when no fallback");
            }
            _ => panic!("expected Remote"),
        }
    }

    #[test]
    fn detach_wrapper_quotes_user_command_safely() {
        let argv = build_detach_argv(&["./long.sh", "--flag", "value with spaces"], "/tmp/log");
        assert_eq!(argv.len(), 1);
        let wrapper = &argv[0];
        assert!(wrapper.contains("nohup bash -c"));
        assert!(wrapper.contains("'./long.sh --flag value with spaces'"));
        assert!(wrapper.contains(">>'/tmp/log'"));
        assert!(wrapper.contains("echo \"pid=$!\""));
    }

    #[test]
    fn detach_wrapper_escapes_single_quotes_in_user_command() {
        let argv = build_detach_argv(&["echo", "it's fine"], "/var/log/x");
        let wrapper = &argv[0];
        // 'it's fine' becomes 'it'\''s fine' after escape
        assert!(wrapper.contains("'echo it'\\''s fine'"));
    }

    #[test]
    fn tunnel_argv_local_3_parts() {
        let h = make_test_host();
        let fwd = TunnelForward::Local {
            bind: "127.0.0.1".into(),
            port: 8080,
            host: "db.internal".into(),
            host_port: 5432,
        };
        let argv = build_ssh_argv_tunnel(&h, &fwd, false, None, false);
        assert!(argv.contains(&"-N".to_string()));
        assert!(argv.contains(&"-L".to_string()));
        assert!(argv.contains(&"127.0.0.1:8080:db.internal:5432".to_string()));
        assert!(!argv.contains(&"-R".to_string()));
        assert!(!argv.contains(&"-D".to_string()));
        // Last element must be the destination
        assert_eq!(argv.last().unwrap(), "alice@1.2.3.4");
    }

    #[test]
    fn tunnel_argv_remote_4_parts() {
        let h = make_test_host();
        let fwd = TunnelForward::Remote {
            bind: "0.0.0.0".into(),
            port: 9000,
            host: "localhost".into(),
            host_port: 3000,
        };
        let argv = build_ssh_argv_tunnel(&h, &fwd, true, Some(120), false);
        assert!(argv.contains(&"-R".to_string()));
        assert!(argv.contains(&"0.0.0.0:9000:localhost:3000".to_string()));
        assert!(argv.contains(&"ExitOnForwardFailure=yes".to_string()));
        // 120s / 15s per keepalive = 8 keepalives
        assert!(argv.contains(&"ServerAliveCountMax=8".to_string()));
    }

    #[test]
    fn tunnel_argv_dynamic() {
        let h = make_test_host();
        let fwd = TunnelForward::Dynamic {
            bind: "127.0.0.1".into(),
            port: 1080,
        };
        let argv = build_ssh_argv_tunnel(&h, &fwd, false, None, false);
        assert!(argv.contains(&"-D".to_string()));
        assert!(argv.contains(&"127.0.0.1:1080".to_string()));
    }

    #[test]
    fn tunnel_argv_auto_keepalive_adds_alive_opts() {
        let h = make_test_host();
        let fwd = TunnelForward::Local {
            bind: "127.0.0.1".into(),
            port: 8080,
            host: "x".into(),
            host_port: 80,
        };
        let argv = build_ssh_argv_tunnel(&h, &fwd, false, None, true);
        assert!(argv.contains(&"ServerAliveInterval=15".to_string()));
        assert!(argv.contains(&"ServerAliveCountMax=3".to_string()));
    }

    #[test]
    fn parse_forward_spec_3_parts_uses_default_bind() {
        let p = parse_forward_spec("8080:db:5432", "127.0.0.1").unwrap();
        assert_eq!(p.bind, "127.0.0.1");
        assert_eq!(p.port, 8080);
        assert_eq!(p.host, "db");
        assert_eq!(p.host_port, 5432);
    }

    #[test]
    fn parse_forward_spec_4_parts_uses_explicit_bind() {
        let p = parse_forward_spec("0.0.0.0:8080:db:5432", "127.0.0.1").unwrap();
        assert_eq!(p.bind, "0.0.0.0");
        assert_eq!(p.port, 8080);
        assert_eq!(p.host, "db");
        assert_eq!(p.host_port, 5432);
    }

    #[test]
    fn parse_forward_spec_rejects_bad_shape() {
        assert!(parse_forward_spec("8080", "127.0.0.1").is_err());
        assert!(parse_forward_spec("8080:db:5432:extra:nope", "127.0.0.1").is_err());
    }

    #[test]
    fn parse_forward_spec_rejects_non_numeric_port() {
        assert!(parse_forward_spec("abc:db:5432", "127.0.0.1").is_err());
        assert!(parse_forward_spec("99999:db:5432", "127.0.0.1").is_err());
    }

    #[test]
    fn dynamic_spec_parses_port() {
        let f = parse_dynamic_spec("1080", "127.0.0.1").unwrap();
        assert_eq!(f, TunnelForward::Dynamic { bind: "127.0.0.1".into(), port: 1080 });
        assert!(parse_dynamic_spec("not-a-port", "127.0.0.1").is_err());
    }

    fn make_test_host() -> EffectiveHost<'static> {
        // Avoid using a temporary by leaking a Host (it's just a test).
        let host: &'static str = "1.2.3.4";
        EffectiveHost {
            alias: "test",
            host,
            user: "alice".into(),
            port: 22,
            identity_file: None,
            extra_opts: vec![],
            desc: None,
            tags: &[],
        }
    }
}
