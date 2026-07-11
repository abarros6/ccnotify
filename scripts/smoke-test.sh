#!/bin/bash
# End-to-end smoke test of the wrapper without the real Claude Code:
# a fake `claude` on PATH plays both Claude and the hook script — it
# POSTs a blocking PreToolUse event, prints the decision it gets back,
# then waits on stdin for the overlay-reply text the wrapper types in.
set -eu

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="$root/target/debug/ccnotify"
[ -x "$bin" ] || { echo "build first: cargo build"; exit 1; }

work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

cat > "$work/claude" <<'EOF'
#!/bin/bash
# fake claude: leak port+token to the test driver, then act like a hook
echo "port=$CCNOTIFY_PORT token=$CCNOTIFY_TOKEN alias=$CCNOTIFY_ALIAS color=$CCNOTIFY_COLOR" > "$CC_TEST_ENVFILE"
payload='{"hook_event_name":"PreToolUse","tool_name":"Bash","tool_input":{"command":"rm -rf build/","description":"clean build dir"}}'
response=$(curl -s -m 30 -X POST -H "X-CCNotify-Token: $CCNOTIFY_TOKEN" \
  --data-binary "$payload" "http://127.0.0.1:$CCNOTIFY_PORT/event")
echo "DECISION:$response"
read -r line
echo "REPLY_RECEIVED:$line"
EOF
chmod +x "$work/claude"

export CC_TEST_ENVFILE="$work/env"
out="$work/out.log"
PATH="$work:$PATH" "$bin" claude > "$out" 2>&1 < /dev/null &
wrapper_pid=$!

# Wait for the fake claude to report its inherited env.
for _ in $(seq 1 50); do [ -s "$CC_TEST_ENVFILE" ] && break; sleep 0.1; done
[ -s "$CC_TEST_ENVFILE" ] || { echo "FAIL: env vars never reached the child"; cat "$out"; exit 1; }
read -r envline < "$CC_TEST_ENVFILE"
echo "child env ok: $envline"
port=$(sed 's/.*port=\([0-9]*\).*/\1/' <<<"$envline")
token=$(sed 's/.*token=\([a-f0-9]*\).*/\1/' <<<"$envline")

sleep 0.5
state=$(curl -s "http://127.0.0.1:$port/overlay/state?token=$token")
echo "state while blocked: $state"
grep -q '"needs_input"' <<<"$state" || { echo "FAIL: expected needs_input state"; exit 1; }
grep -q '"rm -rf build/"' <<<"$state" || { echo "FAIL: pending tool_input missing"; exit 1; }
id=$(sed 's/.*"id":\([0-9]*\).*/\1/' <<<"$state")

# Bad token must be rejected (checked while the server is still up —
# the reply below ends the session).
code=$(curl -s -o /dev/null -w '%{http_code}' "http://127.0.0.1:$port/overlay/state?token=wrong")
[ "$code" = "403" ] || { echo "FAIL: bad token got $code, wanted 403"; exit 1; }
echo "auth check ok (403 for bad token)"

# Overlay says allow -> the blocked hook call should return the decision.
curl -s -X POST -H "X-CCNotify-Token: $token" \
  --data "{\"id\":$id,\"decision\":\"allow\"}" "http://127.0.0.1:$port/overlay/decision" > /dev/null

# Overlay sends a text reply -> should land on the fake claude's stdin.
sleep 0.5
curl -s -X POST -H "X-CCNotify-Token: $token" \
  --data '{"text":"hello from the overlay"}' "http://127.0.0.1:$port/overlay/reply" > /dev/null

wait "$wrapper_pid" || true

echo "--- wrapper output ---"
cat "$out"
grep -q '"permissionDecision":"allow"' "$out" || { echo "FAIL: allow decision never reached the hook"; exit 1; }
grep -q 'REPLY_RECEIVED:hello from the overlay' "$out" || { echo "FAIL: overlay reply never reached stdin"; exit 1; }
echo "--- SMOKE TEST PASSED ---"
