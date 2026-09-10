#!/usr/bin/env bash
# How many people use gitgui: release asset downloads, stars, forks, and the
# 14-day clone / view traffic GitHub keeps. Needs the gh CLI, logged in as a
# user with push access (traffic is owner-only; downloads and stars are public).
#
#   bash scripts/stats.sh            # summary
#   bash scripts/stats.sh --per-release
set -euo pipefail

repo="${GITGUI_REPO:-antonellof/gitgui}"
per_release=0
[ "${1:-}" = "--per-release" ] && per_release=1

command -v gh >/dev/null || { echo "stats.sh needs the gh CLI" >&2; exit 1; }

gh api "repos/$repo" --jq '"stars       \(.stargazers_count)
forks       \(.forks_count)
watchers    \(.subscribers_count)
open issues \(.open_issues_count)"'

# Release assets carry a download_count each; sum them over every release.
gh api --paginate "repos/$repo/releases" --jq '.[].assets[].download_count' |
  awk '{n+=$1} END {printf "downloads   %d (all releases, all platforms)\n", n}'

# Traffic is a rolling 14-day window, so this is a rate, not a total.
for kind in clones views; do
  gh api "repos/$repo/traffic/$kind" \
    --jq "\"$kind\" + (\" \" * (12 - (\"$kind\" | length))) + \"\(.count) total, \(.uniques) unique (last 14 days)\"" \
    2>/dev/null || echo "$kind       needs push access to the repository"
done

if [ "$per_release" = 1 ]; then
  echo
  echo "per release:"
  gh api --paginate "repos/$repo/releases" \
    --jq '.[] | "  \(.tag_name)  \([.assets[].download_count] | add // 0)  \(.published_at[0:10])"'
fi
