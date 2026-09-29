#!/usr/bin/env bash
set -euo pipefail
cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.."

if ! command -v cargo >/dev/null && [[ -f "$HOME/.cargo/env" ]]; then
    source "$HOME/.cargo/env"
fi

command -v wasm-bindgen >/dev/null || {
    echo "Install wasm-bindgen-cli as described in docs/development.md." >&2
    exit 1
}

cargo build --locked -p struction-smoke-test --target wasm32-unknown-unknown
mkdir -p dist/smoke-test
wasm-bindgen --target web --out-name struction --out-dir dist/smoke-test \
    target/wasm32-unknown-unknown/debug/struction-smoke-test.wasm
cp tools/smoke-test/index.html dist/smoke-test/index.html
echo "Serve with: python3 -m http.server 8000 --bind 127.0.0.1 --directory dist/smoke-test"
