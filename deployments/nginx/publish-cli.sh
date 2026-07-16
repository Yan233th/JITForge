#!/bin/sh
set -eu

script_dir=$(CDPATH= cd -- "$(dirname -- "$0")" && pwd)
repo_dir=$(CDPATH= cd -- "$script_dir/../.." && pwd)
download_dir=${JITFORGE_DOWNLOAD_DIR:-"$repo_dir/.data/downloads"}
case "$download_dir" in
    /*) ;;
    *) download_dir="$repo_dir/$download_dir" ;;
esac
destination="$download_dir/jit-linux-x86_64"
temporary="$download_dir/.jit-linux-x86_64.$$"

cleanup() {
    rm -f "$temporary"
}
trap cleanup EXIT HUP INT TERM

cargo build --release -p jit-cli --manifest-path "$repo_dir/Cargo.toml"
install -d -m 0755 "$download_dir"
install -m 0555 "$repo_dir/target/release/jit" "$temporary"
mv -f "$temporary" "$destination"
trap - EXIT HUP INT TERM

printf 'Published %s\n' "$destination"
sha256sum "$destination"
