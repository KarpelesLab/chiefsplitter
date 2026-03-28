#!/bin/bash
# Publish or update the on-chain IDL
#
# Usage: ./scripts/publish-idl.sh [--rpc <url>] [--keypair <path>]

set -e

PROGRAM_ID="ChiefYGYadRjMCgMNqbbFV8GUfiP2TqfRWzcWNynEoPh"
IDL_FILE="$(cd "$(dirname "$0")/.." && pwd)/idl.json"
RPC_URL="https://api.mainnet-beta.solana.com"
KEYPAIR="$HOME/.config/solana/id.json"
AGAVE_LOCAL="$HOME/.local/share/solana/install/active_release/bin"
if [ -x "/pkg/main/net-p2p.agave.core/bin/solana" ]; then
    SOLANA_CLI="/pkg/main/net-p2p.agave.core/bin/solana"
elif [ -x "$AGAVE_LOCAL/solana" ]; then
    SOLANA_CLI="$AGAVE_LOCAL/solana"
else
    SOLANA_CLI="solana"
fi

while [[ $# -gt 0 ]]; do
    case $1 in
        --rpc)    RPC_URL="$2";  shift 2 ;;
        --keypair) KEYPAIR="$2"; shift 2 ;;
        --idl)    IDL_FILE="$2"; shift 2 ;;
        -h|--help)
            echo "Usage: $0 [--rpc <url>] [--keypair <path>] [--idl <path>]"
            exit 0 ;;
        *) echo "Unknown option: $1"; exit 1 ;;
    esac
done

if [ ! -f "$IDL_FILE" ]; then
    echo "Error: IDL file not found: $IDL_FILE"
    exit 1
fi

# Minify IDL
MINIFIED=$(mktemp /tmp/idl-min-XXXXXX.json)
trap "rm -f $MINIFIED" EXIT
node -e "process.stdout.write(JSON.stringify(JSON.parse(require('fs').readFileSync('$IDL_FILE','utf8'))))" > "$MINIFIED"

ORIG_SIZE=$(wc -c < "$IDL_FILE")
MIN_SIZE=$(wc -c < "$MINIFIED")

echo "=== Publish IDL ==="
echo "Program:  $PROGRAM_ID"
echo "IDL file: $IDL_FILE ($ORIG_SIZE bytes, minified to $MIN_SIZE bytes)"
echo "RPC:      $RPC_URL"
echo "Keypair:  $KEYPAIR"

if [ -x "$SOLANA_CLI" ]; then
    BALANCE=$($SOLANA_CLI balance --keypair "$KEYPAIR" --url "$RPC_URL" 2>/dev/null || echo "unknown")
    echo "Balance:  $BALANCE"
fi
echo ""

npx @solana-program/program-metadata@latest write idl \
    "$PROGRAM_ID" \
    "$MINIFIED" \
    --keypair "$KEYPAIR" \
    --rpc "$RPC_URL"

echo ""
echo "IDL published successfully!"
echo "View on Solscan: https://solscan.io/account/$PROGRAM_ID"
