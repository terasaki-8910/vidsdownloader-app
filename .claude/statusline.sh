#!/usr/bin/env sh
# Status line: current model + reasoning effort, when Claude Code exposes the latter.
# Input: Claude Code's session JSON on stdin (see CLAUDE.md > Model / effort indicator).
# Never guesses a value it can't read -- effort is simply omitted if the field is absent.
set -eu
input=$(cat)

if command -v jq >/dev/null 2>&1; then
  model=$(printf '%s' "$input" | jq -r '.model.display_name // .model.id // "unknown"')
  effort=$(printf '%s' "$input" | jq -r '.model.reasoning_effort // .model.effort // .reasoning_effort // .effort // empty')
else
  # No jq: best-effort plain-text extraction of the model name only. Don't attempt effort
  # without a real parser -- a wrong guess here is worse than not showing it.
  model=$(printf '%s' "$input" | grep -o '"display_name"[[:space:]]*:[[:space:]]*"[^"]*"' | head -1 | sed -E 's/.*"([^"]*)"$/\1/')
  [ -n "$model" ] || model="unknown"
  effort=""
fi

if [ -n "$effort" ]; then
  printf '%s (%s)\n' "$model" "$effort"
else
  printf '%s\n' "$model"
fi
