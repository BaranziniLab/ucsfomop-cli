#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"

# Prefer /usr/local (system-wide) if writable or sudo available,
# otherwise fall back to ~/.local (user-only, no sudo required).
if [ -w "/usr/local/bin" ] || sudo -n true 2>/dev/null; then
    INSTALL_BIN="/usr/local/bin"
    INSTALL_LIB="/usr/local/lib/ucsfomop"
    USE_SUDO="sudo"
else
    INSTALL_BIN="$HOME/.local/bin"
    INSTALL_LIB="$HOME/.local/lib/ucsfomop"
    USE_SUDO=""
    mkdir -p "$INSTALL_BIN" "$INSTALL_LIB"
fi

echo "Installing ucsfomop-cli to $INSTALL_BIN ..."

$USE_SUDO mkdir -p "$INSTALL_BIN" "$INSTALL_LIB"
$USE_SUDO cp "$SCRIPT_DIR/bin/ucsfomop"         "$INSTALL_BIN/ucsfomop"
$USE_SUDO chmod +x                              "$INSTALL_BIN/ucsfomop"
$USE_SUDO cp "$SCRIPT_DIR/lib/ucsfomop/"*       "$INSTALL_LIB/"

echo ""
echo "Installed:"
echo "  Binary : $INSTALL_BIN/ucsfomop"
echo "  Libs   : $INSTALL_LIB/"
echo ""

# Warn if the bin dir is not on PATH
if ! echo "$PATH" | tr ':' '\n' | grep -qx "$INSTALL_BIN"; then
    echo "  ⚠  $INSTALL_BIN is not on your PATH."
    echo "     Add this to your ~/.zshrc or ~/.bashrc:"
    echo "       export PATH=\"$INSTALL_BIN:\$PATH\""
    echo ""
fi

echo "Run 'ucsfomop --help' to get started."
echo ""
echo "Create a .env file in your working directory with:"
echo "  CLINICAL_RECORDS_SERVER=..."
echo "  CLINICAL_RECORDS_DATABASE=..."
echo "  CLINICAL_RECORDS_USERNAME=DOMAIN\\\\user"
echo "  CLINICAL_RECORDS_PASSWORD=..."
