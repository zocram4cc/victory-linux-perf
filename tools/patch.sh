#!/bin/bash
set -eu
source "$(dirname "$(readlink -f "$0")")/../env.sh"
name=${1:-}
dir=$root/deps/$name
patch=$root/patches/$name.patch
if [ -z "$name" ] || [ ! -f "$patch" ]; then echo "usage: patch.sh mesa|vkd3d|wine"; exit 1; fi
git -C "$dir" add -u
git -C "$dir" diff HEAD > "$patch"
cp "$patch" "$dir/.git/um-patch"
stamp=$(cat "$dir/.git/um-stamp")
echo "${stamp%% *} $(sha256sum < "$patch")" > "$dir/.git/um-stamp"
