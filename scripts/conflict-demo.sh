#!/usr/bin/env bash
# Create a throwaway repository with a merge conflict in three files, for
# trying the conflict UI. Usage: scripts/conflict-demo.sh [dir]
set -euo pipefail
dir="${1:-scratch/conflict-demo}"
rm -rf "$dir"
mkdir -p "$dir"
cd "$dir"
git init -q -b main
git config user.name Demo
git config user.email demo@example.com
printf 'fn greet(name: &str) -> String {\n    format!("Hello, {name}")\n}\n\nfn main() {\n    println!("{}", greet("world"));\n}\n' > app.rs
printf '# Demo\n\nA small project.\n\n## Usage\n\nRun it.\n' > README.md
printf 'alpha\nbeta\ngamma\n' > list.txt
git add -A
git commit -qm "Base"
git checkout -qb feature
sed -i.bak 's/Hello, {name}/Hi there, {name}!/' app.rs && rm app.rs.bak
sed -i.bak 's/Run it./Run it with cargo run./' README.md && rm README.md.bak
printf 'alpha\nbeta\ngamma\ndelta\n' > list.txt
git commit -qam "Feature: friendlier greeting, usage, delta"
git checkout -q main
sed -i.bak 's/Hello, {name}/Good day, {name}/' app.rs && rm app.rs.bak
sed -i.bak 's/Run it./Run it with make run./' README.md && rm README.md.bak
printf 'alpha\nbeta\ngamma\nepsilon\n' > list.txt
git commit -qam "Main: formal greeting, make, epsilon"
git merge feature >/dev/null 2>&1 || true
git status --short
echo "conflict demo at $(pwd)"
