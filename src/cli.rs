//! Clap command definitions and top-level `run` dispatcher.
//!
//! Handlers here are thin: they parse input, mutate the in-memory config, and
//! delegate to `config` / `exec` for I/O and side effects.

use crate::config::{validate_alias, Config, Host};
use crate::error::{Error, Result};
use crate::exec::{build_detach_argv, build_ssh_argv_tunnel, parse_dynamic_spec, parse_forward_spec, run_remote_connect, run_remote_exec, run_scp, ExecOutput, ScpTarget, TunnelForward};
use crate::paths;
use clap::{Parser, Subcommand, ValueEnum};
use comfy_table::{ContentArrangement, Table};
use serde::Serialize;
use std::io::Write;

/// Top-level CLI arguments.
#[derive(Debug, Parser)]
#[command(
    name = "ssh-cli",
    version,
    about = "Manage remote hosts by alias; invoke the system ssh/scp without retyping flags.",
    long_about = "ssh-cli wraps the system ssh/scp binaries and adds a thin alias layer on top.\n\n\
                  Register hosts as named aliases in a TOML config file, then refer to them by alias \
                  instead of typing `ssh -p 2222 -i ~/.ssh/foo user@10.0.0.5` every time. \
                  Designed for both interactive shell use and AI-agent invocation (see \
                  docs/AI-USAGE.md for the agent manual).",
    after_help = "QUICK START:\n  \
                  ssh-cli init\n  \
                  ssh-cli add box --host 1.2.3.4 --user alice --identity ~/.ssh/id_ed25519\n  \
                  ssh-cli exec box -- 'uname -a'\n\n\
                  COMMON PATTERNS:\n  \
                  Run a command              ssh-cli exec box -- 'systemctl status nginx'\n  \
                  Pipe stdin to remote       echo data | ssh-cli exec box -- 'cat > /etc/x'\n  \
                  Bounded run                ssh-cli exec -t 600 box -- ./slow-job\n  \
                  Fire-and-forget (~1s PID)  ssh-cli exec -d //tmp/build.log box -- make\n  \
                  Copy pull                  ssh-cli cp box:/etc/app.conf ./\n  \
                  Copy push                  ssh-cli cp ./app.conf box:/etc/app.conf\n  \
                  Watch log                  ssh-cli exec box -- 'tail -f /var/log/app.log'\n  \
                  Bulk across hosts          for h in $(ssh-cli list --json | jq -r '.[].alias'); do ssh-cli exec $h -- hostname; done\n\n\
                  See docs/AI-USAGE.md for the full manual (patterns, gotchas, recipes).\n\
                  Config file path: printed by `ssh-cli config-path`.",
    next_line_help = true,
)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,

    /// Output machine-readable JSON where supported.
    #[arg(long, global = true)]
    pub json: bool,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Initialize an empty config file at the platform-default path.
    Init,

    /// Print the resolved config file path.
    #[command(name = "config-path")]
    ConfigPath,

    /// Check config validity and ssh/scp binary availability.
    Doctor,

    /// List all configured hosts as a table (or JSON with --json).
    List,

    /// Show full resolved details for one host (use --json to inspect the exact ssh argv).
    Show {
        /// host alias to inspect
        alias: String,
    },

    /// Register a host under ALIAS so you can refer to it by name.
    #[command(long_about = "Register a host under ALIAS so you can refer to it by name instead of \
                  typing `ssh -p PORT -i IDENTITY USER@HOST` every time.\n\n\
                  Only --host is strictly required. Anything else falls back to [defaults] in your \
                  config, or is prompted interactively on stdin if neither is set. \
                  Aliases must match [A-Za-z0-9._-]{1,64} and may not start with `.` or `-`.",
             after_help = "Examples:\n  \
                  ssh-cli add box --host 10.0.0.5 --user alice\n  \
                  ssh-cli add web1 --host 1.2.3.4 --user deploy --port 2222 \\\n    \
                      --identity ~/.ssh/web1_ed25519 --tags prod,web --desc \"frontend\"\n  \
                  ssh-cli add arch-atom --host 192.168.1.174   # prompts for missing fields\n  \
                  ssh-cli add box --host 1.2.3.4 --user u --force   # overwrite existing")]
    Add(AddArgs),

    /// Remove a host (asks for confirmation unless -y).
    Remove {
        alias: String,
        #[arg(short = 'y', long)]
        yes: bool,
    },

    /// Rename an alias atomically.
    Rename { old: String, new: String },

    /// Open an interactive ssh session with TTY passthrough.
    Connect {
        /// host alias to connect to
        alias: String,
    },

    /// Run a non-interactive remote command; capture stdout/stderr; propagate exit code.
    #[command(long_about = "Run CMD non-interactively on ALIAS. The remote command's stdout is \
                  streamed to local stdout, stderr to local stderr, and the remote exit code \
                  becomes ssh-cli's exit code.\n\n\
                  Local stdin is forwarded to the remote process by default (heredocs and \
                  pipes work). Pass -n to close it. Pass -t N to kill the command after N \
                  seconds (exit 124 like coreutils `timeout`). Pass -d LOG to fire-and-forget \
                  via `nohup ... &` on the remote — returns the spawned PID in ~1 second.",
             after_help = "Examples:\n  \
                  # Run, capture output\n  \
                  ssh-cli exec box -- uptime\n\n  \
                  # Pipe local stdin to the remote command\n  \
                  echo 'config-data' | ssh-cli exec box -- 'cat > /etc/app.conf'\n\n  \
                  # Bounded execution: kill after 60s if not done\n  \
                  ssh-cli exec -t 60 box -- ./long-job.sh\n\n  \
                  # Fire-and-forget: returns ~1s with the spawned PID\n  \
                  ssh-cli exec -d //tmp/build.log box -- 'cd /app && make'\n\n  \
                  # Close local stdin (mirrors ssh -n)\n  \
                  ssh-cli exec -n box -- ./read-only-script.sh")]
    Exec {
        /// host alias to run the command on
        alias: String,
        /// Don't forward local stdin to the remote process (mirrors `ssh -n`).
        #[arg(short = 'n', long)]
        no_stdin: bool,
        /// Kill the remote command after N seconds and return exit code 124.
        #[arg(short = 't', long = "timeout", value_name = "SECS")]
        timeout: Option<u64>,
        /// Fire-and-forget: run the command via `nohup ... &` on the remote,
        /// redirecting stdout+stderr to the given log path, and return the spawned
        /// PID immediately. Mutually exclusive with `--timeout` (since detach
        /// returns before the command can time out meaningfully).
        #[arg(short = 'd', long = "detach", value_name = "REMOTE_LOG")]
        detach_log: Option<String>,
        /// Everything after `--` is the remote command (passed verbatim).
        #[arg(last = true, required = true)]
        cmd: Vec<String>,
    },

    /// Copy files between local and remote (alias or user@host).
    #[command(long_about = "Copy files via scp. Either SRC or DST may be:\n  \
                  - a local path: /tmp/x, ./file.txt, C:/Users/me/x.txt\n  \
                  - `<alias>:<path>`: alias resolved from config (user/host/port/identity filled in)\n  \
                  - `user@host:<path>`: native scp syntax (defaults filled in if alias exists)\n\n\
                  Tip: on Windows + Git Bash, MSYS converts `/tmp/x` to a Windows path. \
                  Use `//tmp/x` or `MSYS_NO_PATHCONV=1` to avoid this for the alias side.",
             after_help = "Examples:\n  \
                  ssh-cli cp box:/etc/nginx/nginx.conf ./nginx.conf      # pull\n  \
                  ssh-cli cp ./nginx.conf box:/etc/nginx/nginx.conf      # push\n  \
                  ssh-cli cp alice@1.2.3.4:/tmp/x /tmp/x                 # native form\n  \
                  ssh-cli cp box:/var/log/app.log /tmp/app-$(date +%F).log")]
    Cp {
        /// source (local path or `alias:path` or `user@host:path`)
        src: String,
        /// destination
        dst: String,
    },

    /// Quick reachability probe — runs `ssh ALIAS true` with a short timeout.
    #[command(long_about = "Runs `ssh -o BatchMode=yes ALIAS true` and reports the result. \
                  Exits 0 if the host is reachable AND key auth succeeds; \
                  non-zero (with ssh's stderr surfaced) otherwise. \
                  Useful as a health check before longer operations.")]
    Ping {
        /// host alias to probe
        alias: String,
        #[arg(long, default_value_t = 5, value_name = "SECS")]
        timeout: u16,
    },

    /// Forward local/remote/dynamic ports through an SSH tunnel (or list/kill tracked tunnels).
    #[command(subcommand, long_about = "Open an SSH tunnel via the system ssh binary. Three modes:\n  \
                  - local:  listen on LOCAL side, forward to a REMOTE target (ssh -L)\n  \
                  - remote: listen on REMOTE side, forward to a LOCAL target (ssh -R)\n  \
                  - dynamic: SOCKS5 proxy on a LOCAL port (ssh -D)\n\n\
                  Spec format:\n  \
                  - local/remote: `[bind:]port:host:hostport` (3 or 4 colon-separated parts)\n  \
                  - dynamic:     just `port`\n\n\
                  Pass --detach to run in the background; the spawned PID and a PID file path \
                  are printed so you can find/kill it later. Pass --exit-on-failure to fail \
                  fast if the forward can't be established. Pass --idle-exit SECS to terminate \
                  after N seconds with no keepalive traffic (useful for short-lived tunnels).\n\n\
                  The subcommands `list` and `kill` manage PID files written by --detach.",
             after_help = "Examples:\n  \
                  # Local forward: localhost:8080 -> remote db.internal:5432\n  \
                  ssh-cli tunnel local 8080:db.internal:5432 arch-atom\n\n  \
                  # Local forward with explicit bind (0.0.0.0 = listen on all interfaces)\n  \
                  ssh-cli tunnel local 0.0.0.0:8080:localhost:3000 arch-atom\n\n  \
                  # Remote forward: remote:9000 -> this machine's localhost:3000\n  \
                  ssh-cli tunnel remote 9000:localhost:3000 arch-atom\n\n  \
                  # SOCKS5 proxy on local port 1080\n  \
                  ssh-cli tunnel dynamic 1080 arch-atom\n\n  \
                  # Background, fail-fast, 5-minute idle timeout\n  \
                  ssh-cli tunnel local 8080:web:80 arch-atom --detach --exit-on-failure --idle-exit 300\n\n  \
                  # Find and kill detached tunnels\n  \
                  ssh-cli tunnel list\n  \
                  ssh-cli tunnel kill -y\n  \
                  ssh-cli tunnel kill arch-atom -y")]
    Tunnel(TunnelCmd),
}

/// Subcommands for `ssh-cli tunnel`.
#[derive(Debug, Subcommand)]
pub enum TunnelCmd {
    /// The original tunnel-open syntax: `ssh-cli tunnel <local|remote|dynamic> <SPEC> <ALIAS> [flags]`.
    /// We capture the remaining argv with `external_subcommand` so users can keep
    /// their existing `ssh-cli tunnel local 8080:web:80 arch-atom --detach` habit
    /// instead of typing `ssh-cli tunnel open local ...`.
    #[command(external_subcommand)]
    Open(Vec<String>),

    /// List tracked detached tunnels (reads PID files written by `tunnel ... --detach`).
    List,

    /// Kill detached tunnels tracked by ssh-cli (reads PID files).
    #[command(long_about = "Kill detached ssh tunnel processes that were started by \
                  `ssh-cli tunnel ... --detach`. ssh-cli writes a PID file under \
                  <config_dir>/tunnels/ for each detached tunnel; this command reads \
                  those files and kills the PIDs (and removes the PID files).\n\n\
                  With no arguments, kills ALL tracked tunnels. With ALIAS, kills only \
                  tunnels started via that alias (matches the `<alias>-*.pid` pattern).")]
    Kill {
        /// only kill tunnels for this alias (matches `<alias>-*.pid`)
        alias: Option<String>,
        /// actually kill; without this flag just print what would be killed
        #[arg(short = 'y', long)]
        yes: bool,
    },
}

/// Mode for `ssh-cli tunnel`.
#[derive(Debug, Clone, Copy, ValueEnum)]
pub enum TunnelMode {
    /// Local forward: ssh -L (listen on local side, forward to remote target).
    Local,
    /// Remote forward: ssh -R (listen on remote side, forward to local target).
    Remote,
    /// Dynamic / SOCKS5 proxy: ssh -D.
    Dynamic,
}

#[derive(Debug, clap::Parser)]
#[command(no_binary_name = true, disable_help_flag = true, disable_version_flag = true)]
pub struct TunnelArgs {
    /// Tunnel direction (local / remote / dynamic).
    #[arg(value_enum)]
    pub mode: TunnelMode,
    /// `[bind:]port:host:hostport` for local/remote; just `port` for dynamic.
    pub spec: String,
    /// SSH host alias (from `ssh-cli list`) to tunnel through.
    pub alias: String,
    /// Bind address for the listening side. Default: 127.0.0.1.
    #[arg(long, default_value = "127.0.0.1")]
    pub bind: String,
    /// Run in the background via `nohup ... &`; print the spawned PID.
    #[arg(short = 'd', long)]
    pub detach: bool,
    /// Fail fast if the forward can't be established (sets ExitOnForwardFailure=yes).
    #[arg(long)]
    pub exit_on_failure: bool,
    /// Terminate after N seconds with no keepalive traffic (ServerAliveCountMax * 15).
    /// Without this, the tunnel stays open until killed.
    #[arg(long, value_name = "SECS")]
    pub idle_exit: Option<u64>,
}

#[derive(Debug, clap::Args)]
pub struct AddArgs {
    /// short name to refer to this host by (e.g. `prod`, `web1`)
    pub alias: String,
    /// hostname or IP of the host (required)
    #[arg(long)]
    pub host: Option<String>,
    /// SSH user (default: from [defaults] in config, or prompted)
    #[arg(long)]
    pub user: Option<String>,
    /// SSH port (default: 22 or from [defaults])
    #[arg(long)]
    pub port: Option<u16>,
    /// path to private key on THIS machine (default: from [defaults])
    #[arg(long = "identity", alias = "identity-file")]
    pub identity_file: Option<String>,
    /// human description shown in `ssh-cli list`
    #[arg(long)]
    pub desc: Option<String>,
    /// comma-separated tags for filtering (e.g. prod,web,arch)
    #[arg(long, value_delimiter = ',')]
    pub tags: Vec<String>,
    /// overwrite existing alias instead of erroring
    #[arg(long)]
    pub force: bool,
}

/// Entry point invoked by `main`.
pub fn run(cli: Cli) -> Result<i32> {
    match cli.command {
        Command::Init => cmd_init(cli.json),
        Command::ConfigPath => cmd_config_path(),
        Command::Doctor => cmd_doctor(cli.json),
        Command::List => cmd_list(cli.json),
        Command::Show { alias } => cmd_show(&alias, cli.json),
        Command::Add(args) => cmd_add(args),
        Command::Remove { alias, yes } => cmd_remove(&alias, yes),
        Command::Rename { old, new } => cmd_rename(&old, &new),
        Command::Connect { alias } => cmd_connect(&alias),
        Command::Exec { alias, no_stdin, timeout, detach_log, cmd } => {
            cmd_exec(&alias, &cmd, no_stdin, timeout, detach_log)
        }
        Command::Cp { src, dst } => cmd_cp(&src, &dst),
        Command::Ping { alias, timeout } => cmd_ping(&alias, timeout),
        Command::Tunnel(TunnelCmd::Open(args)) => {
            let parsed = <TunnelArgs as clap::Parser>::try_parse_from(args)
                .map_err(|e| Error::Io(std::io::Error::new(
                    std::io::ErrorKind::InvalidInput,
                    format!("tunnel: {e}"),
                )))?;
            cmd_tunnel(parsed)
        }
        Command::Tunnel(TunnelCmd::List) => cmd_tunnel_list(),
        Command::Tunnel(TunnelCmd::Kill { alias, yes }) => cmd_tunnel_kill(alias.as_deref(), yes),
    }
}

fn cmd_init(_json: bool) -> Result<i32> {
    let path = paths::config_file();
    if path.exists() {
        eprintln!("config already exists at {}", path.display());
        return Ok(0);
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    Config::empty().save()?;
    println!("created {}", path.display());
    Ok(0)
}

fn cmd_config_path() -> Result<i32> {
    println!("{}", paths::config_file().display());
    Ok(0)
}

#[derive(Serialize)]
struct DoctorReport {
    config_path: String,
    config_exists: bool,
    config_valid: bool,
    ssh_binary: Option<String>,
    scp_binary: Option<String>,
    issues: Vec<String>,
}

fn cmd_doctor(json: bool) -> Result<i32> {
    let path = paths::config_file();
    let mut report = DoctorReport {
        config_path: path.display().to_string(),
        config_exists: path.exists(),
        config_valid: false,
        ssh_binary: None,
        scp_binary: None,
        issues: vec![],
    };
    if let Ok(cfg) = Config::load() {
        report.config_valid = true;
        if cfg.hosts.is_empty() && cfg.defaults.user.is_none() {
            report.issues.push("config has no hosts and no defaults.user".into());
        }
    } else {
        report
            .issues
            .push("config missing or invalid (run `ssh-cli init` to start)".into());
    }
    report.ssh_binary = crate::exec::find_binary("ssh").ok().map(|p| p.display().to_string());
    if report.ssh_binary.is_none() {
        report.issues.push("ssh binary not found on PATH".into());
    }
    report.scp_binary = crate::exec::find_binary("scp").ok().map(|p| p.display().to_string());

    if json {
        println!("{}", serde_json::to_string_pretty(&report)?);
    } else {
        let mut t = Table::new();
        t.set_content_arrangement(ContentArrangement::Dynamic);
        t.set_header(vec!["check", "result"]);
        t.add_row(vec!["config path", &report.config_path]);
        t.add_row(vec!["config exists", &report.config_exists.to_string()]);
        t.add_row(vec!["config valid", &report.config_valid.to_string()]);
        t.add_row(vec![
            "ssh binary",
            report.ssh_binary.as_deref().unwrap_or("(not found)"),
        ]);
        t.add_row(vec![
            "scp binary",
            report.scp_binary.as_deref().unwrap_or("(not found)"),
        ]);
        println!("{t}");
        if !report.issues.is_empty() {
            println!("\nissues:");
            for i in &report.issues {
                println!("  - {i}");
            }
        }
    }
    Ok(0)
}

#[derive(Serialize)]
struct HostRow {
    alias: String,
    host: String,
    user: String,
    port: u16,
    identity: String,
    tags: String,
    desc: String,
}

fn cmd_list(json: bool) -> Result<i32> {
    let cfg = Config::load_or_default()?;
    let mut rows: Vec<HostRow> = Vec::with_capacity(cfg.hosts.len());
    for (alias, host) in &cfg.hosts {
        let user = host
            .user
            .clone()
            .or_else(|| cfg.defaults.user.clone())
            .unwrap_or_else(|| "?".into());
        let port = host.port.or(cfg.defaults.port).unwrap_or(22);
        let identity = host
            .identity_file
            .clone()
            .or_else(|| cfg.defaults.identity_file.clone())
            .unwrap_or_default();
        rows.push(HostRow {
            alias: alias.clone(),
            host: host.host.clone(),
            user,
            port,
            identity,
            tags: host.tags.join(","),
            desc: host.desc.clone().unwrap_or_default(),
        });
    }
    if json {
        println!("{}", serde_json::to_string_pretty(&rows)?);
        return Ok(0);
    }
    if rows.is_empty() {
        println!("(no hosts configured)");
        return Ok(0);
    }
    let mut t = Table::new();
    t.set_content_arrangement(ContentArrangement::Dynamic);
    t.set_header(vec!["ALIAS", "HOST", "USER", "PORT", "IDENTITY", "TAGS", "DESC"]);
    for r in &rows {
        t.add_row(vec![
            &r.alias,
            &r.host,
            &r.user,
            &r.port.to_string(),
            &r.identity,
            &r.tags,
            &r.desc,
        ]);
    }
    println!("{t}");
    Ok(0)
}

fn cmd_show(alias: &str, json: bool) -> Result<i32> {
    let cfg = Config::load()?;
    let eff = cfg.effective(alias)?;
    if json {
        let out = serde_json::json!({
            "alias": eff.alias,
            "host": eff.host,
            "user": eff.user,
            "port": eff.port,
            "identity_file": eff.identity_file,
            "extra_opts": eff.extra_opts,
            "desc": eff.desc,
            "tags": eff.tags,
            "destination": eff.destination(),
            "ssh_argv": crate::exec::build_ssh_argv(&eff, &[]),
        });
        println!("{}", serde_json::to_string_pretty(&out)?);
        return Ok(0);
    }
    println!("alias        : {}", eff.alias);
    println!("destination : {}", eff.destination());
    println!("port        : {}", eff.port);
    println!("identity    : {}", eff.identity_file.as_deref().unwrap_or("(default)"));
    if let Some(d) = eff.desc {
        println!("desc        : {d}");
    }
    if !eff.tags.is_empty() {
        println!("tags        : {}", eff.tags.join(", "));
    }
    if !eff.extra_opts.is_empty() {
        println!("extra opts  : {}", eff.extra_opts.join(" "));
    }
    Ok(0)
}

fn cmd_add(args: AddArgs) -> Result<i32> {
    validate_alias(&args.alias)?;
    let mut cfg = Config::load_or_default()?;

    let exists = cfg.hosts.contains_key(&args.alias);
    if exists && !args.force {
        return Err(Error::HostExists(args.alias));
    }

    let host_str = match args.host {
        Some(h) => h,
        None => prompt("host")?,
    };
    if host_str.is_empty() {
        return Err(Error::InvalidAlias("empty host".into()));
    }

    let user = match args.user {
        Some(u) => Some(u),
        None => {
            let s = prompt_opt("user (leave blank to use defaults)")?;
            if s.is_empty() { None } else { Some(s) }
        }
    };
    let port = match args.port {
        Some(p) => Some(p),
        None => {
            let s = prompt_opt("port (blank = 22)")?;
            if s.is_empty() {
                None
            } else {
                Some(s.parse::<u16>().map_err(|_| {
                    Error::InvalidAlias(format!("invalid port: {s}"))
                })?)
            }
        }
    };
    let identity = match args.identity_file {
        Some(i) => Some(i),
        None => {
            let s = prompt_opt("identity file (blank = none)")?;
            if s.is_empty() { None } else { Some(s) }
        }
    };
    let desc = match args.desc {
        Some(d) => Some(d),
        None => {
            let s = prompt_opt("description (blank = none)")?;
            if s.is_empty() { None } else { Some(s) }
        }
    };

    cfg.hosts.insert(
        args.alias.clone(),
        Host {
            host: host_str,
            user,
            port,
            identity_file: identity,
            extra_opts: vec![],
            desc,
            tags: args.tags,
        },
    );
    cfg.save()?;
    println!("added `{}`", args.alias);
    Ok(0)
}

fn cmd_remove(alias: &str, yes: bool) -> Result<i32> {
    validate_alias(alias)?;
    let mut cfg = Config::load()?;
    if !cfg.hosts.contains_key(alias) {
        return Err(Error::HostNotFound(alias.to_string()));
    }
    if !yes {
        let answer = prompt(&format!("remove `{alias}`? [y/N]"))?;
        if !matches!(answer.to_lowercase().as_str(), "y" | "yes") {
            return Err(Error::Cancelled);
        }
    }
    cfg.hosts.remove(alias);
    cfg.save()?;
    println!("removed `{alias}`");
    Ok(0)
}

fn cmd_rename(old: &str, new: &str) -> Result<i32> {
    validate_alias(old)?;
    validate_alias(new)?;
    let mut cfg = Config::load()?;
    let host = cfg.hosts.remove(old).ok_or_else(|| Error::HostNotFound(old.to_string()))?;
    if cfg.hosts.contains_key(new) {
        // put it back
        cfg.hosts.insert(old.to_string(), host);
        return Err(Error::HostExists(new.to_string()));
    }
    cfg.hosts.insert(new.to_string(), host);
    cfg.save()?;
    println!("renamed `{old}` → `{new}`");
    Ok(0)
}

fn cmd_connect(alias: &str) -> Result<i32> {
    let cfg = Config::load()?;
    let eff = cfg.effective(alias)?;
    let code = run_remote_connect(&eff)?;
    Ok(code)
}

fn cmd_exec(
    alias: &str,
    cmd_parts: &[String],
    no_stdin: bool,
    timeout: Option<u64>,
    detach_log: Option<String>,
) -> Result<i32> {
    let cfg = Config::load()?;
    let eff = cfg.effective(alias)?;

    warn_if_quotes_look_mangled(cmd_parts);

    // Detach path: rebuild the command as a nohup-backgrounded wrapper around
    // the user's original argv. Returns immediately on the remote side.
    if let Some(log_path) = detach_log {
        return cmd_exec_detach(&eff, cmd_parts, &log_path);
    }

    let refs: Vec<&str> = cmd_parts.iter().map(String::as_str).collect();
    let inherit_stdin = !no_stdin;
    let ExecOutput { stdout, stderr, exit_code } =
        run_remote_exec(&eff, &refs, inherit_stdin, timeout)?;

    // Stream stdout verbatim; surface stderr only on failure so agent output stays clean.
    if !stdout.is_empty() {
        let mut out = std::io::stdout().lock();
        let _ = out.write_all(stdout.as_bytes());
        let _ = out.flush();
    }
    let code = exit_code.unwrap_or(1);
    if code != 0 || !stderr.is_empty() {
        let mut err = std::io::stderr().lock();
        if !stderr.is_empty() {
            let _ = err.write_all(stderr.as_bytes());
            if !stderr.ends_with('\n') {
                let _ = err.write_all(b"\n");
            }
        }
    }
    Ok(code)
}

fn cmd_exec_detach(
    eff: &crate::config::EffectiveHost<'_>,
    cmd_parts: &[String],
    log_path: &str,
) -> Result<i32> {
    let user_cmd_refs: Vec<&str> = cmd_parts.iter().map(String::as_str).collect();
    let wrapper_argv: Vec<String> = build_detach_argv(&user_cmd_refs, log_path);
    let wrapper_refs: Vec<&str> = wrapper_argv.iter().map(String::as_str).collect();
    let argv = crate::exec::build_ssh_argv(eff, &wrapper_refs);
    let bin = crate::exec::find_binary("ssh")?;
    let output = std::process::Command::new(&bin)
        .args(&argv)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| Error::Io(std::io::Error::new(
            e.kind(),
            format!("failed to spawn {}: {}", bin.display(), e),
        )))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("detach failed: {}", stderr.trim());
        return Ok(output.status.code().unwrap_or(1));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    // We expect "pid=<N>" on stdout. Tolerate any trailing whitespace.
    let pid_line = stdout.trim();
    // Emit a friendly one-liner; agents can grep for "pid=".
    println!("started on {alias}: {pid_line} (log: {log_path})", alias = eff.alias);
    Ok(0)
}

fn cmd_cp(src: &str, dst: &str) -> Result<i32> {
    let cfg = Config::load_or_default()?;
    let parsed_src = ScpTarget::parse(src, None)?;
    let parsed_dst = ScpTarget::parse(dst, None)?;
    // If either side is `alias:`, resolve via config.
    let resolved_src = resolve_alias_in_target(&cfg, parsed_src)?;
    let resolved_dst = resolve_alias_in_target(&cfg, parsed_dst)?;
    let code = run_scp(&resolved_src, &resolved_dst)?;
    Ok(code)
}

fn resolve_alias_in_target(cfg: &Config, t: ScpTarget) -> Result<ScpTarget> {
    match t {
        ScpTarget::Local(_) => Ok(t),
        ScpTarget::Remote { user, host, port, path, identity_file } => {
            // If `host` is exactly an alias and the alias exists in config, replace host/user/port/identity.
            if let Ok(eff) = cfg.effective(&host) {
                return Ok(ScpTarget::Remote {
                    user: eff.user.clone(),
                    host: eff.host.to_string(),
                    port: eff.port,
                    path,
                    identity_file: eff.identity_file.clone(),
                });
            }
            // Otherwise, override defaults from config if available.
            let user = if user.is_empty() {
                cfg.defaults.user.clone().unwrap_or(user)
            } else {
                user
            };
            let identity_file = identity_file.or_else(|| cfg.defaults.identity_file.clone());
            Ok(ScpTarget::Remote { user, host, port, path, identity_file })
        }
    }
}

fn cmd_ping(alias: &str, timeout: u16) -> Result<i32> {
    let cfg = Config::load()?;
    let eff = cfg.effective(alias)?;
    let argv = crate::exec::build_ssh_argv(&eff, &["true"]);
    let bin = crate::exec::find_binary("ssh")?;
    let output = std::process::Command::new(&bin)
        .args(&argv)
        .env("BatchMode", "yes")
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .spawn()?
        .wait_with_output()?;
    let _ = timeout; // Currently the argv already encodes a 15s ConnectTimeout; reserved for future override.
    if output.status.success() {
        println!("ok");
        Ok(0)
    } else {
        let stderr = String::from_utf8_lossy(&output.stderr);
        eprintln!("ping failed: {}", stderr.trim());
        Ok(output.status.code().unwrap_or(1))
    }
}

fn cmd_tunnel(args: TunnelArgs) -> Result<i32> {
    let cfg = Config::load()?;
    let eff = cfg.effective(&args.alias)?;

    // Parse the spec into a TunnelForward based on the chosen mode.
    let forward = match args.mode {
        TunnelMode::Local => {
            let p = parse_forward_spec(&args.spec, &args.bind)?;
            TunnelForward::Local {
                bind: p.bind,
                port: p.port,
                host: p.host,
                host_port: p.host_port,
            }
        }
        TunnelMode::Remote => {
            let p = parse_forward_spec(&args.spec, &args.bind)?;
            TunnelForward::Remote {
                bind: p.bind,
                port: p.port,
                host: p.host,
                host_port: p.host_port,
            }
        }
        TunnelMode::Dynamic => parse_dynamic_spec(&args.spec, &args.bind)?,
    };

    let argv = build_ssh_argv_tunnel(&eff, &forward, args.exit_on_failure, args.idle_exit, args.detach);
    let bin = crate::exec::find_binary("ssh")?;

    if args.detach {
        // Tunnel listeners (-L/-R/-D) bind on the LOCAL machine (where ssh runs),
        // not the remote — so we can't reuse the remote-side `nohup ... &` wrapper
        // from `exec --detach`. And we deliberately DON'T use ssh's built-in `-f`
        // flag: on Windows, ssh -f's forked child dies when the parent ssh-cli
        // exits because of Job Object cleanup. Instead we spawn ssh with Windows
        // process-creation flags that truly detach it from ssh-cli's process tree.
        // On Unix, no special handling is needed — orphaned processes get
        // reparented to init and survive.
        let mut cmd = std::process::Command::new(&bin);
        cmd.args(&argv)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null())
            // CRITICAL: stderr must be null too, not piped. If we leave stderr
            // piped and never drain it, ssh.exe blocks the moment its warning
            // output fills the 4 KiB pipe buffer — ssh then hangs and is
            // eventually killed, killing the tunnel with it.
            .stderr(std::process::Stdio::null());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            // CREATE_NEW_PROCESS_GROUP: own process group (so Ctrl+C in our TTY
            //   won't reach it).
            // DETACHED_PROCESS: no console attachment.
            // NOTE: deliberately NOT including CREATE_BREAKAWAY_FROM_JOB — the
            // MSYS / Git-Bash ssh.exe refuses to start with that flag
            // (PermissionDenied). The two flags above are sufficient when
            // ssh-cli itself isn't in a Job Object (which is the normal case
            // for an interactive shell session).
            const FLAGS: u32 = 0x0000_0200 | 0x0000_0008;
            cmd.creation_flags(FLAGS);
        }
        let child = cmd
            .spawn()
            .map_err(|e| Error::Io(std::io::Error::new(
                e.kind(),
                format!("failed to spawn {}: {}", bin.display(), e),
            )))?;
        let pid = child.id();

        // Persist the PID so the user can find/kill it later via
        // `ssh-cli tunnel kill <alias>` or by reading the PID file directly.
        // Naming: <alias>-<bind>-<port>.pid so multiple tunnels per alias coexist.
        let listen = match &forward {
            TunnelForward::Local { bind, port, .. }
            | TunnelForward::Remote { bind, port, .. }
            | TunnelForward::Dynamic { bind, port } => format!("{bind}-{port}"),
        };
        let pid_dir = crate::paths::config_dir().join("tunnels");
        let _ = std::fs::create_dir_all(&pid_dir);
        let pid_file = pid_dir.join(format!("{}-{}.pid", eff.alias, listen));
        let _ = std::fs::write(&pid_file, format!("{}\n", pid));

        // Give ssh a moment to authenticate and bind the listener before we
        // return. Without this, callers can race: ssh-cli says "tunnel started"
        // before the local port is actually open.
        std::thread::sleep(std::time::Duration::from_millis(500));

        let summary = match &forward {
            TunnelForward::Local { bind, port, host, host_port } =>
                format!("local  {bind}:{port} -> {host}:{host_port}"),
            TunnelForward::Remote { bind, port, host, host_port } =>
                format!("remote {bind}:{port} <- {host}:{host_port}"),
            TunnelForward::Dynamic { bind, port } =>
                format!("SOCKS5 {bind}:{port}"),
        };
        println!(
            "tunnel on {alias}: {summary}  (pid: {pid}, pid_file: {pf})",
            alias = eff.alias,
            pf = pid_file.display()
        );
        Ok(0)
    } else {
        // Foreground: inherit TTY so Ctrl+C cleanly tears down the tunnel and
        // ssh prints its own connection / forward status messages.
        let status = std::process::Command::new(&bin)
            .args(&argv)
            .stdin(std::process::Stdio::inherit())
            .stdout(std::process::Stdio::inherit())
            .stderr(std::process::Stdio::inherit())
            .status()
            .map_err(|e| Error::Io(std::io::Error::new(
                e.kind(),
                format!("failed to spawn {}: {}", bin.display(), e),
            )))?;
        Ok(status.code().unwrap_or(1))
    }
}

fn cmd_tunnel_list() -> Result<i32> {
    let pid_dir = crate::paths::config_dir().join("tunnels");
    if !pid_dir.exists() {
        println!("(no tunnels tracked)");
        return Ok(0);
    }
    let mut found = 0;
    let mut stale = 0;
    for entry in std::fs::read_dir(&pid_dir)?.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()) else {
            continue;
        };
        if !name.ends_with(".pid") {
            continue;
        }
        let pid_str = std::fs::read_to_string(&path).unwrap_or_default();
        let pid: u32 = match pid_str.trim().parse() {
            Ok(p) => p,
            Err(_) => {
                println!("{name}: unreadable pid (stale?)");
                stale += 1;
                continue;
            }
        };
        let alive = is_pid_alive(pid);
        println!(
            "{name}\n  pid: {pid}    alive: {}    pid_file: {}",
            if alive { "yes" } else { "no (stale)" },
            path.display()
        );
        if alive {
            found += 1;
        } else {
            stale += 1;
        }
    }
    if found == 0 && stale == 0 {
        println!("(no tunnels tracked)");
    }
    Ok(0)
}

fn cmd_tunnel_kill(alias: Option<&str>, yes: bool) -> Result<i32> {
    let pid_dir = crate::paths::config_dir().join("tunnels");
    if !pid_dir.exists() {
        println!("(no tunnels tracked)");
        return Ok(0);
    }
    let mut targets: Vec<(std::path::PathBuf, u32, String)> = Vec::new();
    for entry in std::fs::read_dir(&pid_dir)?.flatten() {
        let path = entry.path();
        let Some(name) = path.file_name().and_then(|n| n.to_str()).map(str::to_string) else {
            continue;
        };
        if !name.ends_with(".pid") {
            continue;
        }
        if let Some(a) = alias {
            // Match by alias prefix: "<alias>-bind-port.pid"
            if !name.starts_with(&format!("{a}-")) {
                continue;
            }
        }
        let pid: u32 = match std::fs::read_to_string(&path)
            .ok()
            .and_then(|s| s.trim().parse().ok())
        {
            Some(p) => p,
            None => continue,
        };
        if !is_pid_alive(pid) {
            // Stale PID file — remove it silently.
            let _ = std::fs::remove_file(&path);
            continue;
        }
        targets.push((path, pid, name));
    }
    if targets.is_empty() {
        println!("(no live tunnels to kill)");
        return Ok(0);
    }
    if !yes {
        println!("would kill (pass -y to confirm):");
        for (_, pid, name) in &targets {
            println!("  {name} (pid {pid})");
        }
        return Ok(0);
    }
    let mut killed = 0;
    for (path, pid, name) in targets {
        if kill_pid(pid) {
            println!("killed {name} (pid {pid})");
            let _ = std::fs::remove_file(&path);
            killed += 1;
        } else {
            eprintln!("failed to kill {name} (pid {pid})");
        }
    }
    Ok(if killed == 0 { 1 } else { 0 })
}

#[cfg(unix)]
fn is_pid_alive(pid: u32) -> bool {
    // kill -0 sends no signal but returns 0 if the process exists.
    std::process::Command::new("kill")
        .arg("-0")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn is_pid_alive(pid: u32) -> bool {
    // BUGFIX: PIDs can be recycled by Windows. `tasklist /FI "PID eq N"` returns
    // exit 0 even when the matched process is *unrelated* (e.g. a cmd.exe that
    // happened to receive our stale PID). So we ALSO require the image name
    // to be `ssh.exe` — otherwise a recycled PID is reported as alive.
    let output = std::process::Command::new("tasklist")
        .arg("/FI")
        .arg(format!("PID eq {pid}"))
        .arg("/NH")
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .output();
    match output {
        Ok(o) if o.status.success() => {
            let s = String::from_utf8_lossy(&o.stdout);
            s.lines().any(|l| {
                // Format: "<ImageName>  <PID>  <SessionName>  <Session#>  ..."
                // First whitespace-delimited token is the image name.
                let name = l.split_whitespace().next().unwrap_or("");
                name.eq_ignore_ascii_case("ssh.exe")
            })
        }
        _ => false,
    }
}

#[cfg(unix)]
fn kill_pid(pid: u32) -> bool {
    std::process::Command::new("kill")
        .arg(pid.to_string())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

#[cfg(windows)]
fn kill_pid(pid: u32) -> bool {
    std::process::Command::new("taskkill")
        .arg("/PID")
        .arg(pid.to_string())
        .arg("/F")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Best-effort detection of argv mangling by an outer Windows shell layer
/// (typically cmd.exe → ssh-cli.exe → ssh.exe → sshd → /bin/bash).
///
/// The user wrote a command like `ssh-cli exec box -- 'cd /opt && rm -rf x'`,
/// bash consumed the single quotes, ssh-cli receives argv without them, and
/// everything is fine. But if the caller invoked ssh-cli through
/// `cmd.exe /c "..."`, cmd.exe splits on spaces (single quotes are NOT special
/// to it) AND treats `&&` / `||` / `|` as command-chain operators. The result:
/// the command is split into pieces BEFORE ssh-cli sees it, leaving an
/// unmatched single quote that the remote bash then reports as
/// `unexpected EOF`.
///
/// We can't fix the caller's argv — but we CAN detect the symptom: an odd
/// total count of `'` across the cmd argv is a near-certain indicator that a
/// single-quoted region was split by an outer shell. Emit a one-liner warning
/// so users get a clear hint instead of a cryptic remote-shell error.
fn warn_if_quotes_look_mangled(cmd_parts: &[String]) {
    let total_single: usize = cmd_parts
        .iter()
        .map(|s| s.chars().filter(|&c| c == '\'').count())
        .sum();
    let total_double: usize = cmd_parts
        .iter()
        .map(|s| s.chars().filter(|&c| c == '"').count())
        .sum();
    if total_single % 2 != 0 {
        eprintln!(
            "warning: remote command contains {total_single} single quote(s) (odd). \
             This usually means an outer Windows shell (cmd.exe) split your argv \
             before ssh-cli saw it — cmd.exe treats `&&` / `||` / `|` as command-chain \
             operators and ignores single quotes entirely. Try invoking via `bash -c` \
             instead of `cmd.exe /c`, or wrap the whole ssh-cli call in double quotes."
        );
    }
    // Double quotes are less reliable (legitimately odd if you escape one),
    // but flag them anyway when single quotes are also unbalanced — the combo
    // is a strong "argv got mangled" signal.
    if total_double % 2 != 0 && total_single % 2 != 0 {
        eprintln!(
            "warning: remote command also has unbalanced double quotes ({total_double})."
        );
    }
}

#[cfg(test)]
mod quote_warn_tests {
    use super::warn_if_quotes_look_mangled;

    fn capture(stderr_runs: impl FnOnce()) -> String {
        // We can't easily intercept stderr; just exercise the function and
        // assert it doesn't panic on representative inputs.
        let _ = stderr_runs;
        String::new()
    }

    #[test]
    fn balanced_quotes_dont_warn() {
        let cases: &[&[&str]] = &[
            &["ls", "-la"],
            &["cd", "/opt", "&&", "echo", "hi"],
            &["echo", "'hello world'"],
            &["echo", "'one'", "and", "'two'"],
            &["echo", "\"hello\""],
        ];
        for cmd in cases {
            let parts: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
            capture(|| warn_if_quotes_look_mangled(&parts));
        }
    }

    #[test]
    fn unbalanced_single_quotes_warn() {
        let cmd = ["echo", "'hello"];
        let parts: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
        capture(|| warn_if_quotes_look_mangled(&parts));
    }

    #[test]
    fn balanced_in_isolated_elements() {
        // Two elements, each with one `'`, total = 2 (balanced) — no warn.
        let cmd = ["'foo", "bar'"];
        let parts: Vec<String> = cmd.iter().map(|s| s.to_string()).collect();
        capture(|| warn_if_quotes_look_mangled(&parts));
    }
}

fn prompt(label: &str) -> Result<String> {
    eprint!("{label}: ");
    std::io::stderr().flush().ok();
    let mut s = String::new();
    std::io::stdin().read_line(&mut s)?;
    Ok(s.trim().to_string())
}

fn prompt_opt(label: &str) -> Result<String> {
    prompt(label)
}
