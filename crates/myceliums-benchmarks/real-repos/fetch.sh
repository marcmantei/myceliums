#!/usr/bin/env bash
# Fetch the real-repository corpus of the held-out retrieval set (#95).
#
# Every repository in `queries.json` is checked out at the exact commit the
# labels were written against, into a cache directory outside this checkout.
# Nothing is vendored: the sources are fetched, never committed.
#
#   crates/myceliums-benchmarks/real-repos/fetch.sh            # default cache dir
#   MYC_EVAL_REPOS_DIR=/some/dir .../fetch.sh                 # explicit cache dir
#
# Default cache dir: ${XDG_CACHE_HOME:-$HOME/.cache}/myceliums-eval-repos.
# Re-running is cheap: a clean checkout already at its pinned commit is left
# alone. A checkout at any other commit, or with edited tracked files, is
# refused rather than silently reused, so a score can never be measured against
# code the labels were not written for.
#
# Needs git and jq.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
dataset="${here}/queries.json"
cache="${MYC_EVAL_REPOS_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/myceliums-eval-repos}"

for tool in git jq; do
  command -v "$tool" >/dev/null || { echo "fetch.sh: $tool is required" >&2; exit 1; }
done

mkdir -p "$cache"

jq -r '.repos[] | [.name, .url, .commit] | @tsv' "$dataset" |
while IFS=$'\t' read -r name url commit; do
  dir="${cache}/${name}"
  if [ -d "${dir}/.git" ]; then
    head="$(git -C "$dir" rev-parse HEAD 2>/dev/null || true)"
    if [ "$head" = "$commit" ]; then
      # The pinned commit with edited files is not the pinned code.
      if [ -n "$(git -C "$dir" status --porcelain --untracked-files=no)" ]; then
        echo "${name}: ${dir} is at ${commit} but has local modifications." >&2
        echo "Restore them (git -C ${dir} checkout -- .) and re-run." >&2
        exit 1
      fi
      echo "${name}: already at ${commit}"
      continue
    fi
    echo "${name}: ${dir} is at '${head}', not the pinned ${commit}." >&2
    echo "Move it aside (or point MYC_EVAL_REPOS_DIR elsewhere) and re-run." >&2
    exit 1
  fi
  echo "${name}: fetching ${url} @ ${commit}"
  git init --quiet "$dir"
  git -C "$dir" remote add origin "$url"
  # A single commit, no history: GitHub serves any reachable sha by id.
  git -C "$dir" fetch --quiet --depth 1 origin "$commit"
  git -C "$dir" -c advice.detachedHead=false checkout --quiet FETCH_HEAD
  test "$(git -C "$dir" rev-parse HEAD)" = "$commit"
done

echo "corpus ready in ${cache}"
