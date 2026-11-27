#!/usr/bin/env bash
# A checkpoint to play and compare later: a git tag and a ready-to-run copy
# of the build (release victory.exe plus this tree's linux/ launcher and
# huge-page helper), so that any checkpoint starts without rebuilding:
#
#   K_REPO=~/Games/victory-data/checkpoints/<name> ~/Games/victory-data/run-kroeeee.sh
#
#   scripts/checkpoint.sh NAME "what this state is" [COMMIT]
#
# COMMIT (default HEAD) is built in a temporary worktree when it is not
# HEAD. The list is kept in docs/royale/CHECKPOINTS.md (appended here).
set -euo pipefail
ROOT=$(cd "$(dirname "$0")/.." && pwd)
name=$1 what=$2 commit=${3:-HEAD}
dest=$HOME/Games/victory-data/checkpoints/$name
hash=$(git -C "$ROOT" rev-parse --short "$commit")
[[ -e $dest ]] && { echo "checkpoint $name exists" >&2; exit 1; }
src=$ROOT
if [[ $(git -C "$ROOT" rev-parse "$commit") != $(git -C "$ROOT" rev-parse HEAD) || -n $(git -C "$ROOT" status --porcelain -- crates apps) ]]; then
    src=$(mktemp -d "${TMPDIR:-/tmp}/checkpoint-XXXX")
    git -C "$ROOT" worktree add -q --detach "$src" "$commit"
    trap 'git -C "$ROOT" worktree remove --force "$src"' EXIT
fi
# Another commit builds in a target directory of its own (the current build stays).
target=$ROOT/target
[[ $src != "$ROOT" ]] && target=$ROOT/target-cp
(cd "$src" && PATH=$HOME/.cargo/bin:$PATH CARGO_TARGET_DIR=$target cargo xwin build --release -p victory --target x86_64-pc-windows-msvc 2>&1 | tail -1)
mkdir -p "$dest/target/x86_64-pc-windows-msvc/release"
cp "$target/x86_64-pc-windows-msvc/release/victory.exe" "$dest/target/x86_64-pc-windows-msvc/release/"
cp -a "$src/linux" "$dest/"
# The huge-page helper: a link to the one that holds the capability
# (`setcap cap_sys_nice+ep`, given once), which a copy would lose. The
# checkpoint's launcher keeps its own way of reading the log.
mkdir -p "$dest/linux/thp"
ln -sf "$ROOT/linux/thp/victory-thp-collapse" "$dest/linux/thp/victory-thp-collapse"
git -C "$ROOT" tag -f "cp/$name" "$commit" >/dev/null
git -C "$ROOT" push -q -f origin "cp/$name" || true
printf '| `%s` | `%s` | %s | %s |\n' "$name" "$hash" "$(date +%Y-%m-%d\ %H:%M)" "$what" >>"$ROOT/docs/royale/CHECKPOINTS.md"
echo "checkpoint $name: $hash -> $dest"
