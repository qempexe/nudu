#!/usr/bin/env bash
# Builds nudu and installs it system-wide, so both `nudu` and `sudo nudu` work.
#
#   ./install.sh               build and install to /usr/local/bin
#   ./install.sh --uninstall   remove it again
#   PREFIX=/opt ./install.sh   install somewhere else (/opt/bin/nudu)

set -euo pipefail

PREFIX="${PREFIX:-/usr/local}"
TARGET="$PREFIX/bin/nudu"

cd "$(dirname "$0")"

if [[ "${1:-}" == "--uninstall" ]]; then
    sudo rm -f "$TARGET"
    echo "removed $TARGET"
    exit 0
fi

cargo build --release
sudo install -Dm755 target/release/nudu "$TARGET"
echo "installed $TARGET"

# An older copy from `cargo install` would be found first on most setups.
if [[ -x "$HOME/.cargo/bin/nudu" ]]; then
    echo
    echo "note: an older nudu is also in ~/.cargo/bin and may run instead."
    echo "      remove it with: cargo uninstall nudu"
fi
