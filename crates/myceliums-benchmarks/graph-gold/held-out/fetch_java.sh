#!/usr/bin/env bash
# Fetch the checkout of the held-out Java CALLS set (#177): spring-framework
# at the commit `java-spring-heldout.json` pins (the v7.1.0 release), one commit, no
# history, into the same cache directory as `real-repos/fetch.sh`.
#
#   crates/myceliums-benchmarks/graph-gold/held-out/fetch_java.sh
#   MYC_EVAL_REPOS_DIR=/some/dir .../fetch_java.sh
#
# Kept apart from `real-repos/fetch.sh` on purpose: CI fetches that corpus on
# every pull request, and analysing spring-framework (~9,700 files) there
# would add about a minute and 2.7 GB of memory to every run for a report
# that is never gated. Needs git and jq.

set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cache="${MYC_EVAL_REPOS_DIR:-${XDG_CACHE_HOME:-$HOME/.cache}/myceliums-eval-repos}"
name="spring-framework"
url="https://github.com/spring-projects/spring-framework.git"
commit="$(jq -r --arg name "$name" '.repos[$name]' "${here}/java-spring-heldout.json")"
dir="${cache}/${name}"

for tool in git jq; do
  command -v "$tool" >/dev/null || { echo "fetch_java.sh: $tool is required" >&2; exit 1; }
done

if [ -d "${dir}/.git" ] || [ -L "$dir" ]; then
  head="$(git -C "$dir" rev-parse HEAD 2>/dev/null || true)"
  if [ "$head" != "$commit" ]; then
    echo "${name}: ${dir} is at '${head}', not the pinned ${commit}." >&2
    echo "Move it aside (or point MYC_EVAL_REPOS_DIR elsewhere) and re-run." >&2
    exit 1
  fi
  if [ -n "$(git -C "$dir" status --porcelain --untracked-files=no)" ]; then
    echo "${name}: ${dir} is at ${commit} but has local modifications." >&2
    exit 1
  fi
  echo "${name}: already at ${commit}"
  exit 0
fi

mkdir -p "$cache"
echo "${name}: fetching ${url} @ ${commit}"
git init --quiet "$dir"
git -C "$dir" remote add origin "$url"
git -C "$dir" fetch --quiet --depth 1 origin "$commit"
git -C "$dir" -c advice.detachedHead=false checkout --quiet FETCH_HEAD
test "$(git -C "$dir" rev-parse HEAD)" = "$commit"
echo "${name} ready in ${dir}"
