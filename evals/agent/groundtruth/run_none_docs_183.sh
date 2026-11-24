#!/bin/bash
# The control run of #183 exactly as pre-registered (README, "Control arm"): 20 sessions of arm `none-docs` and 9 concurrent
# ones of arm `none`, in the order of control_plan_183.json, with the command of the 20 recorded sessions of arm `none` but
# for the arms, the label, the plan and --hook-cd (the hook of that run let `cd` through). Real sessions on the caller's normal login; resumable. `--dry-run` prints the command and starts nothing.
# Run it detached: caffeinate -ims bash run_none_docs_183.sh. Clean stop: touch runs/<label>/STOP.
set -euo pipefail

dry=no
case "${1-}" in
  "") ;;
  --dry-run) dry=yes ;;
  *) echo "usage: $0 [--dry-run]" >&2; exit 64 ;;
esac

EVAL=$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)
LABEL=2026-11-24-control-none-docs
MYC=${AEVAL_CACHE:-$HOME/.cache/myceliums-agent-eval}/bin/myc-787920c
MYC_SHA=bf84b0e854f68b7ae39401723e30e70c7ab714c86891b33bf066bf100dccf857
CLAUDE_VERSION=2.1.331
TASKS_SHA=a8d649a1ef5129b59d0e5ed8c966114cab0350207f447f1b61d281578316d3f5   # decision_183.json
FROZEN=cc17a8254506                                                           # listing of the frozen dependencies
export AEVAL_WORK_ROOT=/private/tmp/aeval182 AEVAL_PROTECT=/private/tmp/aeval183
export JAVA_HOME=$HOME/.cache/myc-java/jdk-ipv4/Contents/Home
T=$EVAL/tasks/tasks-commit-replay.json
run=(python3 "$EVAL/agent_eval.py" --tasks "$T" run --label "$LABEL" --conditions none,none-docs --plan "$EVAL/groundtruth/control_plan_183.json" --profile realistic --bash --timeout 2400
  --concurrency 1 --max-sessions 35 --hook-cd --myc "$MYC" --models-dir "$HOME/.cache/myceliums-eval-models")

sha() { shasum -a 256 "$1" | cut -d' ' -f1; }
ready() {
  [ "$(sha "$MYC")" = "$MYC_SHA" ] || { echo "$MYC is not the registered binary" >&2; exit 1; }
  [ "$(sha "$T")" = "$TASKS_SHA" ] || { echo "the task file is not the one of the recorded decision" >&2; exit 1; }
  case "$(claude --version)" in "$CLAUDE_VERSION "*) ;; *) echo "claude is not $CLAUDE_VERSION" >&2; exit 1 ;; esac
  [ "$(python3 "$EVAL/agent_eval.py" gradle-cache --listing)" = "$FROZEN" ] || { echo "not the frozen dependencies of the 20 sessions of arm none" >&2; exit 1; }
  [ "$(df -g "$HOME" | tail -1 | awk '{print $4}')" -ge 31 ] || { echo "less than 31 GB free" >&2; exit 1; }
}

printf '+'; printf ' %q' "${run[@]}"; printf '\n'
if [ "$dry" = yes ]; then echo "dry run: nothing was started"; exit 0; fi
ready
mkdir -p "$EVAL/runs/$LABEL"
python3 -I "$EVAL/groundtruth/watch_run.py" $$ "$EVAL/runs/$LABEL/load-sockets.txt" "$EVAL/runs/$LABEL" &
"${run[@]}"
echo "run ended"
