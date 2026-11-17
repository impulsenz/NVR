#!/bin/bash
# (Re)start GAZE for the benchmark: the given program, or this checkout's build.
: "${BENCH:?set BENCH to a scratch folder}"
BIN=${1:-$(dirname "$0")/../../target/release/nvr}
pkill -f "/release/nvr $BENCH/run/nvr.toml"; sleep 1
cd "$BENCH/run" && (nohup "$BIN" "$BENCH/run/nvr.toml" > server.log 2>&1 &)
sleep 3
# The first time: the admin account.
curl -s -H Content-Type:application/json -H Origin:http://localhost:18080 \
  -X POST localhost:18080/api/auth/setup -d '{"username":"admin","password":"testpass123"}' >/dev/null || true
