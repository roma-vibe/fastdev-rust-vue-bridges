#!/bin/sh
# Runs the app in the container as BRIDGES and SEPARATE_SERVICES say (docs/bridges.md); the rule
# itself lives in the Rust config: `app-all --plan` prints the binaries to run.
#
# - One binary (the default, app-all with every service): exec, so the app is PID 1, gets SIGTERM
#   directly and nothing else stays in memory.
# - Several (BRIDGES=off, SEPARATE_SERVICES): runs them all; when one exits the container stops
#   (Docker restarts it per the Compose restart policy); SIGTERM stops them all.
set -eu

plan=$(/app/bin/app-all --plan)
if [ "$(printf '%s\n' "$plan" | wc -l)" -eq 1 ]; then
  exec "/app/bin/$plan"
fi

pids=""
for binary in $plan; do
  "/app/bin/$binary" &
  pids="$pids $!"
done

stop() {
  # shellcheck disable=SC2086
  kill -TERM $pids 2>/dev/null || true
  wait
  exit 0
}
trap stop TERM INT

# Poll the children: stop the whole container as soon as one of them is gone.
while :; do
  for pid in $pids; do
    if ! kill -0 "$pid" 2>/dev/null; then
      echo "a service exited (pid $pid); stopping the container" >&2
      stop
    fi
  done
  sleep 2
done
