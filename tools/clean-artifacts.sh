#!/usr/bin/env bash
set -euo pipefail

root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)

case "${1:-}" in
  --help|-h)
    echo "Usage: $0 [--dry-run]"
    echo "Remove this checkout's target/ and dist/; authored files and asset caches are preserved."
    exit 0
    ;;
  --dry-run|"") ;;
  *) echo "Unknown option: $1" >&2; exit 2 ;;
esac
if (( $# > 1 )); then
  echo "Usage: $0 [--dry-run]" >&2
  exit 2
fi

cd -- "$root"
for output in target dist; do
  if [[ -L "$output" ]]; then
    echo "Refusing to clean symlink: $root/$output" >&2
    exit 1
  fi
  if [[ -e "$output" ]]; then
    du -sh -- "$output"
  fi
done
if [[ "${1:-}" == --dry-run ]]; then
  exit 0
fi

# Pin the directory so an external CARGO_TARGET_DIR is never cleaned accidentally.
cargo clean --target-dir "$root/target"
rm -rf -- "$root/dist"
