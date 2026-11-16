#!/bin/sh
# Copy the upstream A2UI files listed in vendor/a2ui/FILES from a checkout of
# https://github.com/google/A2UI, and record the checkout's commit in
# vendor/a2ui/UPSTREAM. Run it again after editing FILES.
#
# Usage: vendor-a2ui.sh <upstream-checkout>
set -eu

if [ "$#" -ne 1 ]; then
    echo "usage: $0 <upstream-checkout>" >&2
    exit 2
fi

src=$1
here=$(cd "$(dirname "$0")" && pwd)
dest=$here/../vendor/a2ui

commit=$(git -C "$src" rev-parse HEAD)

while IFS= read -r path || [ -n "$path" ]; do
    [ -n "$path" ] || continue
    if [ ! -f "$src/$path" ]; then
        echo "missing in upstream checkout: $path" >&2
        exit 1
    fi
    mkdir -p "$dest/$(dirname "$path")"
    cp "$src/$path" "$dest/$path"
done < "$dest/FILES"

printf '%s\n' "$commit" > "$dest/UPSTREAM"
echo "vendored $(grep -c . "$dest/FILES") files from $commit"
