#!/bin/bash
# Publish or update the on-chain IDL
#
# Usage: ./scripts/publish-idl.sh [--rpc <url>] [--keypair <path>] [--idl <path>] [--yes]
#
# The program-metadata CLI is installed from scripts/package-lock.json
# (exact version + integrity hashes) with `npm ci --ignore-scripts`; it is
# never fetched unpinned. To upgrade it, bump the version in
# scripts/package.json, regenerate the lockfile with
# `npm install --package-lock-only --ignore-scripts`, and review the diff.

set -e

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
PROGRAM_ID="ChiefYGYadRjMCgMNqbbFV8GUfiP2TqfRWzcWNynEoPh"
IDL_FILE="$(cd "$SCRIPT_DIR/.." && pwd)/idl.json"
RPC_URL="https://api.mainnet-beta.solana.com"
KEYPAIR="$HOME/.config/solana/id.json"
ASSUME_YES=0
AGAVE_LOCAL="$HOME/.local/share/solana/install/active_release/bin"
if [ -x "/pkg/main/net-p2p.agave.core/bin/solana" ]; then
    SOLANA_CLI="/pkg/main/net-p2p.agave.core/bin/solana"
elif [ -x "$AGAVE_LOCAL/solana" ]; then
    SOLANA_CLI="$AGAVE_LOCAL/solana"
else
    SOLANA_CLI="solana"
fi

usage() {
    echo "Usage: $0 [--rpc <url>] [--keypair <path>] [--idl <path>] [--yes]"
    echo ""
    echo "  --rpc <url>       RPC endpoint (default: mainnet-beta)"
    echo "  --keypair <path>  Authority/payer keypair (default: ~/.config/solana/id.json)"
    echo "  --idl <path>      IDL file to publish (default: idl.json in the repo root)"
    echo "  --yes, -y         Skip the confirmation prompt shown for non-local/non-test clusters"
}

need_value() {
    if [ $# -lt 2 ]; then
        echo "Error: $1 requires a value"
        exit 1
    fi
}

while [[ $# -gt 0 ]]; do
    case $1 in
        --rpc)     need_value "$@"; RPC_URL="$2";  shift 2 ;;
        --keypair) need_value "$@"; KEYPAIR="$2";  shift 2 ;;
        --idl)     need_value "$@"; IDL_FILE="$2"; shift 2 ;;
        -y|--yes)  ASSUME_YES=1; shift ;;
        -h|--help) usage; exit 0 ;;
        *) echo "Unknown option: $1"; usage; exit 1 ;;
    esac
done

if [ ! -f "$IDL_FILE" ]; then
    echo "Error: IDL file not found: $IDL_FILE"
    exit 1
fi

# Minify IDL. The file keeps a .json extension inside a private temp dir
# (BSD mktemp only substitutes trailing X's, so the X's must come last).
WORKDIR="$(mktemp -d "${TMPDIR:-/tmp}/idl-min.XXXXXX")"
trap 'rm -rf "$WORKDIR"' EXIT
MINIFIED="$WORKDIR/idl.json"
node -e 'process.stdout.write(JSON.stringify(JSON.parse(require("fs").readFileSync(process.argv[1],"utf8"))))' "$IDL_FILE" > "$MINIFIED"

ORIG_SIZE=$(wc -c < "$IDL_FILE")
MIN_SIZE=$(wc -c < "$MINIFIED")

echo "=== Publish IDL ==="
echo "Program:  $PROGRAM_ID"
echo "IDL file: $IDL_FILE ($ORIG_SIZE bytes, minified to $MIN_SIZE bytes)"
echo "RPC:      $RPC_URL"
echo "Keypair:  $KEYPAIR"

if command -v "$SOLANA_CLI" >/dev/null 2>&1; then
    BALANCE=$("$SOLANA_CLI" balance --keypair "$KEYPAIR" --url "$RPC_URL" 2>/dev/null || echo "unknown")
    echo "Balance:  $BALANCE"
fi
echo ""

# Anything that is not clearly a local validator, devnet or testnet is treated
# as mainnet (custom mainnet RPC URLs often do not contain the word "mainnet").
case "$RPC_URL" in
    *localhost*|*127.0.0.1*|*devnet*|*testnet*) IS_MAINNET=0 ;;
    *) IS_MAINNET=1 ;;
esac

if [ "$IS_MAINNET" -eq 1 ] && [ "$ASSUME_YES" -ne 1 ]; then
    echo "This will publish the IDL for $PROGRAM_ID on what looks like MAINNET."
    printf 'Type "yes" to continue: '
    REPLY=""
    read -r REPLY || true
    if [ "$REPLY" != "yes" ]; then
        echo "Aborted."
        exit 1
    fi
    echo ""
fi

# Install the pinned CLI from the committed lockfile (no lifecycle scripts).
(cd "$SCRIPT_DIR" && npm ci --ignore-scripts)

"$SCRIPT_DIR/node_modules/.bin/program-metadata" write idl \
    "$PROGRAM_ID" \
    "$MINIFIED" \
    --format json \
    --keypair "$KEYPAIR" \
    --rpc "$RPC_URL"

echo ""
echo "IDL published successfully!"
echo "View on Solscan: https://solscan.io/account/$PROGRAM_ID"
