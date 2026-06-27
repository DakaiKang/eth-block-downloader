#!/usr/bin/env bash
#
# Download the 4 study block ranges (20000 blocks) with both downloader binaries.
#
# Each binary is run as a SINGLE process over all 4 ranges so it processes blocks
# serially — this stays within Alchemy's compute-units/second cap and avoids the
# 429 rate-limit / dropped-block problem that parallel-per-range runs hit.
# (See CLAUDE.md and the project memory note.)
#
# Usage:
#   ./download.sh            # build (incremental) then download all 20000 blocks
#   ETHEREUM_RPC_URL=... ./download.sh
#
# RPC URL resolution: $ETHEREUM_RPC_URL if set, otherwise the gitignored api.key file.

set -euo pipefail
cd "$(dirname "$0")"

# Make cargo available if installed via rustup but not yet on PATH.
[ -f "$HOME/.cargo/env" ] && . "$HOME/.cargo/env"

# Resolve the RPC URL.
if [ -z "${ETHEREUM_RPC_URL:-}" ]; then
  if [ -f api.key ]; then
    ETHEREUM_RPC_URL="$(cat api.key)"
    export ETHEREUM_RPC_URL
  else
    echo "ERROR: set ETHEREUM_RPC_URL or create an api.key file with the RPC URL" >&2
    exit 1
  fi
fi

# 4 segments × 5000 blocks, "start:end" with end exclusive:
#   A           19,557,289 – 19,562,288
#   B           22,606,458 – 22,611,457
#   March 2023  16,774,645 – 16,779,644
#   Nov  2023   18,581,726 – 18,586,725
RANGES=(19557289:19562289 22606458:22611458 16774645:16779645 18581726:18586726)

cargo build --release --bin eth-block-downloader --bin download_rw

echo "==> main: writing test_data/blocks/ ..."
./target/release/eth-block-downloader "${RANGES[@]}"

echo "==> download_rw: writing test_data/blocks_rw/ + test_data/rw_gas/ ..."
./target/release/download_rw "${RANGES[@]}"

echo "==> Done. 20000 blocks downloaded across blocks/, blocks_rw/, rw_gas/."
