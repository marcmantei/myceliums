#!/bin/bash
# The pilot of #183, exactly as pre-registered (README, "Pilot"): five sessions of arm `none`, its report, then one
# pipeline check each in C and D, one session at a time. Real sessions on the caller's normal login; resumable (the run
# labels). `--dry-run` prints every command and starts nothing.
set -euo pipefail

dry=no
case "${1-}" in
  "") ;;
  --dry-run) dry=yes ;;
  *) echo "usage: $0 [--dry-run]" >&2; exit 64 ;;
esac

EVAL=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
CACHE=${AEVAL_CACHE:-$HOME/.cache/myceliums-agent-eval}
MYC=$CACHE/bin/myc-787920c
MYC_SHA=bf84b0e854f68b7ae39401723e30e70c7ab714c86891b33bf066bf100dccf857
CLAUDE_VERSION=2.1.331
export AEVAL_WORK_ROOT=/private/tmp/aeval182   # the path of the derived indexes
export AEVAL_PROTECT=/private/tmp/aeval183
export JAVA_HOME=$HOME/.cache/myc-java/jdk-ipv4/Contents/Home
T=$EVAL/tasks/tasks-commit-replay.json
P=(--profile realistic --rep 1 --bash --timeout 2400 --concurrency 1 --myc "$MYC" --models-dir "$HOME/.cache/myceliums-eval-models")

show() { printf '+'; printf ' %q' "$@"; printf '\n'; }
ready() {  # before each run: the binary and the CLI the pre-registration names
  [ "$(shasum -a 256 "$MYC" | cut -d' ' -f1)" = "$MYC_SHA" ] || { echo "$MYC is not the pre-registered binary" >&2; exit 1; }
  case "$(claude --version)" in "$CLAUDE_VERSION "*) ;; *) echo "claude is not $CLAUDE_VERSION" >&2; exit 1 ;; esac
}
run() {
  show "$@"
  if [ "$dry" = no ]; then ready; "$@"; fi
}

echo "AEVAL_WORK_ROOT=$AEVAL_WORK_ROOT AEVAL_PROTECT=$AEVAL_PROTECT JAVA_HOME=$JAVA_HOME"
run python3 "$EVAL/agent_eval.py" --tasks "$T" run --label pilot-183 --conditions none \
  --task retry-max-attempts-to-max-retries --task api-version-deprecation-handler-handler-arg \
  --task beans-max-nested-path-depth --task mime-type-parameter-names-case-insensitive \
  --task type-mismatch-required-type-arg "${P[@]}"

# The report ends with 1 when it projects more than the limits: that stops the main run, not the two checks. Any other
# failure (it prints no report) stops here.
report=(python3 "$EVAL/agent_eval.py" --tasks "$T" pilot pilot-183)
show "${report[@]}"
if [ "$dry" = no ]; then
  out=$EVAL/runs/pilot-183/pilot.json
  rc=0
  "${report[@]}" > "$out" || rc=$?
  cat "$out"
  python3 -I -c 'import json,sys; d=json.load(open(sys.argv[1])); sys.exit(bool(d["stop"]) != (sys.argv[2] == "1") or sys.argv[2] not in "01")' "$out" "$rc" \
    || { echo "no pilot report (exit $rc)" >&2; exit 1; }
fi

run python3 "$EVAL/agent_eval.py" --tasks "$T" run --label pilot-183-check-C --conditions C \
  --task api-version-deprecation-handler-handler-arg "${P[@]}"
run python3 "$EVAL/agent_eval.py" --tasks "$T" run --label pilot-183-check-D --conditions D \
  --task retry-max-attempts-to-max-retries "${P[@]}"
if [ "$dry" = yes ]; then echo "dry run: nothing was started"; else echo "pilot ended"; fi
