#!/bin/sh
# ccnotify hook forwarder — installed to ~/.claude/hooks/ccnotify-forward.sh
#
# Reads the hook event JSON from stdin and POSTs it to the wrapper that
# launched this Claude Code session. If CCNOTIFY_PORT is unset, Claude was
# started without the wrapper: exit 0 and do nothing, so having this hook
# globally configured is always safe.

[ -n "$CCNOTIFY_PORT" ] || exit 0

payload=$(cat)

response=$(curl -sS -m 590 \
  -X POST \
  -H "Content-Type: application/json" \
  -H "X-CCNotify-Token: $CCNOTIFY_TOKEN" \
  --data-binary "$payload" \
  "http://127.0.0.1:$CCNOTIFY_PORT/event" 2>/dev/null) || exit 0

# For PreToolUse the wrapper answers with a permission decision that must
# reach Claude Code via stdout. Empty responses print nothing (no-op).
[ -n "$response" ] && printf '%s' "$response"
exit 0
