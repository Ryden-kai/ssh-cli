#!/usr/bin/env bash
# Run the deepseek-harness build on arch-atom, properly detached via ssh-cli.
#
# Why this script exists:
#   The naive pattern
#       ssh user@host 'cd /app && long-cmd &' ; sleep ; pgrep ...
#   adds 5-10s of overhead before returning and is fragile under agent
#   timeouts. ssh-cli's `-d` flag does it right in ~1s.
#
# Usage (from your LOCAL box):
#   ./deepseek-harness-build.sh                       # start build, return immediately
#   ./deepseek-harness-build.sh --watch               # also tail -f the log
#   ./deepseek-harness-build.sh --log //tmp/x.log     # custom remote log path
#   ./deepseek-harness-build.sh --alias other-box    # different host
#
# After this script returns, the build keeps running on the remote. Use:
#   ssh-cli exec arch-atom -- 'pgrep -af "pnpm run build"'   # liveness check
#   ssh-cli exec arch-atom -- 'tail -f <log>'                # stream progress
#   ssh-cli exec arch-atom -- 'pkill -f "pnpm run build"'    # stop it
#
# On Windows + Git Bash, paths starting with `/` get MSYS-converted. The `//`
# prefix here escapes that conversion. On real Linux/macOS, `//tmp/x` normalizes
# to `/tmp/x` and works the same.
#
# NOTE: This file is a USAGE EXAMPLE — see docs/AI-USAGE.md Pattern E for the
# general "fire-and-forget" pattern this illustrates.

set -euo pipefail

ALIAS="${SSH_CLI_ALIAS:-arch-atom}"
LOG="//tmp/dsh_build_$(date +%Y%m%d_%H%M%S).log"
WATCH=0

while [[ $# -gt 0 ]]; do
    case "$1" in
        --alias)  ALIAS="$2"; shift 2 ;;
        --log)    LOG="$2";   shift 2 ;;
        --watch)  WATCH=1;    shift   ;;
        -h|--help)
            sed -n '2,26p' "$0"; exit 0 ;;
        *) echo "unknown arg: $1" >&2; exit 2 ;;
    esac
done

# arch-atom has ~3.7 GiB RAM + 9 GiB swap. pnpm + tsx + tsc -b can OOM without
# a heap cap; 2560 MiB leaves headroom for the kernel cache.
REMOTE_CMD='cd /opt/deepseek-harness && NODE_OPTIONS=--max-old-space-size=2560 exec pnpm run build'

echo "==> starting build on $ALIAS"
echo "    log: $LOG"
echo "    cmd: $REMOTE_CMD"
echo

# The `-d` flag wraps the command in `nohup bash -c '...' </dev/null >>LOG 2>&1 & disown`
# and returns the spawned PID. Total round-trip is ~1 second.
ssh-cli exec -d "$LOG" "$ALIAS" -- "$REMOTE_CMD"

cat <<EOF

build detached. Next steps:

  # Stream progress (Ctrl+C to stop watching; build keeps running)
  ssh-cli exec $ALIAS -- 'tail -f $LOG'

  # Liveness — should always show the pnpm PID while building
  ssh-cli exec $ALIAS -- 'pgrep -af "pnpm run build"'

  # Stop the build
  ssh-cli exec $ALIAS -- 'pkill -f "pnpm run build"'
EOF

if [[ "$WATCH" -eq 1 ]]; then
    echo
    echo "==> entering tail -f (Ctrl+C to detach; build keeps running)"
    exec ssh-cli exec "$ALIAS" -- "tail -f $LOG"
fi
