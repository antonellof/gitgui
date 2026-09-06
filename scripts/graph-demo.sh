#!/usr/bin/env bash
# A throwaway repository with several branches, merges, tags and a remote,
# for screenshots of the commit graph. Usage: scripts/graph-demo.sh [dir]
set -euo pipefail
dir="${1:-scratch/graph-demo}"
rm -rf "$dir" "$dir.remote"
mkdir -p "$dir"
cd "$dir"
git init -q -b main
git config user.name "Mara Keller"
git config user.email mara@example.com
export GIT_AUTHOR_DATE="2026-08-01T09:00:00" GIT_COMMITTER_DATE="2026-08-01T09:00:00"
tick() { local d; d=$(date -j -v+"$1"H -f "%Y-%m-%dT%H:%M:%S" "2026-08-01T09:00:00" "+%Y-%m-%dT%H:%M:%S" 2>/dev/null || date -d "2026-08-01T09:00:00 + $1 hours" "+%Y-%m-%dT%H:%M:%S"); export GIT_AUTHOR_DATE="$d" GIT_COMMITTER_DATE="$d"; }
slug() { echo "$*" | tr 'A-Z' 'a-z' | sed 's/[^a-z0-9]\{1,\}/_/g; s/^_//; s/_$//' | cut -c1-20; }
c() { tick "$1"; shift; printf '%s\n' "$*" >> "src_$(slug "$*").txt"; git add -A; git -c user.name="$AUTHOR" -c user.email="$EMAIL" commit -qm "$*"; }
AUTHOR="Mara Keller"; EMAIL=mara@example.com
c 0 "Initial project layout"
c 2 "Add configuration loader"
c 4 "CLI argument parsing"
git tag -a v1.0.0 -m "1.0.0"
git checkout -qb feature/auth
AUTHOR="Tomás Ruiz"; EMAIL=tomas@example.com
c 6 "Auth: session tokens"
c 9 "Auth: refresh flow"
git checkout -q main
AUTHOR="Mara Keller"; EMAIL=mara@example.com
c 8 "Logging with levels"
git checkout -qb feature/search
AUTHOR="Ines Bauer"; EMAIL=ines@example.com
c 11 "Search: index builder"
c 14 "Search: query parser"
git checkout -q main
c 12 "Docs: getting started"
tick 16; git merge -q --no-ff feature/auth -m "Merge feature/auth"
git tag -a v1.1.0 -m "1.1.0"
git checkout -qb hotfix/token-expiry
AUTHOR="Tomás Ruiz"; EMAIL=tomas@example.com
c 18 "Fix token expiry off by one"
git checkout -q main
tick 19; git merge -q --no-ff hotfix/token-expiry -m "Merge hotfix/token-expiry"
git tag -a v1.1.1 -m "1.1.1"
git checkout -q feature/search
c 20 "Search: ranking"
c 23 "Search: highlight matches"
git checkout -q main
AUTHOR="Mara Keller"; EMAIL=mara@example.com
c 22 "Refactor config into a module"
tick 25; git merge -q --no-ff feature/search -m "Merge feature/search"
git tag -a v1.2.0 -m "1.2.0"
git checkout -qb release/1.2
c 27 "Release notes 1.2"
git checkout -q main
git checkout -qb feature/export
AUTHOR="Ines Bauer"; EMAIL=ines@example.com
c 28 "Export: CSV writer"
c 31 "Export: JSON writer"
git checkout -q main
AUTHOR="Mara Keller"; EMAIL=mara@example.com
c 30 "CI: run tests on pull requests"
c 33 "Bump dependencies"
git checkout -q feature/export
c 35 "Export: streaming output"
git checkout -q main
tick 36; git merge -q --no-ff feature/export -m "Merge feature/export"
git tag -a v1.3.0 -m "1.3.0"
c 38 "Docs: export formats"
git checkout -qb feature/themes
AUTHOR="Tomás Ruiz"; EMAIL=tomas@example.com
c 40 "Themes: dark and light palettes"
c 43 "Themes: follow the terminal background"
git checkout -q main
AUTHOR="Mara Keller"; EMAIL=mara@example.com
c 42 "Fix flaky integration test"
# A remote that mirrors main and one branch, so origin/* shows up.
git init -q --bare "../$(basename "$dir").remote"
git remote add origin "../$(basename "$dir").remote"
git push -q origin main feature/themes release/1.2 --tags
git checkout -q main
printf 'accent = blue\nbackground = dark\n' > theme.toml
git add theme.toml
c 45 "Prepare 1.4 changelog"
git checkout -q feature/themes
printf 'accent = orange\nbackground = dark\ncontrast = high\n' > theme.toml
git add theme.toml
c 47 "Themes: high contrast"
git checkout -q main
printf 'accent = teal\nbackground = dark\n' > theme.toml
git add theme.toml
c 48 "Tweak the default accent"
# A stash, then a merge that stops on a conflict, then some loose changes.
tick 49
printf 'timeout = 30\n' >> src_add_configuration_lo.txt
git stash push -q -m "WIP: config timeout"
git merge feature/themes >/dev/null 2>&1 || true
printf 'retries = 3\n' >> src_cli_argument_parsing.txt
printf '# Roadmap\n\n- 1.4: themes, export streaming\n' > ROADMAP.md
git add ROADMAP.md
printf 'level = debug\n' >> src_logging_with_levels.txt
printf 'notes\n' > scratch.txt
git status --short
echo "graph demo at $(pwd)"
