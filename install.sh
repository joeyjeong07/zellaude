#!/usr/bin/env bash
# install.sh — Build and install zellaude (Zellij plugin + agent hooks)
#
# Usage:
#   ./install.sh                  # install everything
#   ./install.sh --no-permissions # install without pre-granting plugin permissions
#   ./install.sh --uninstall      # remove everything
set -euo pipefail

PLUGIN_DIR="$HOME/.config/zellij/plugins"
PLUGIN_PATH="$PLUGIN_DIR/zellaude.wasm"
PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
dim()   { printf '\033[2m%s\033[0m\n' "$*"; }

UNINSTALL=false
GRANT_PERMISSIONS=true

for arg in "$@"; do
    case "$arg" in
        --uninstall)      UNINSTALL=true ;;
        --no-permissions) GRANT_PERMISSIONS=false ;;
        *)
            red "Unknown option: $arg"
            echo "Usage: ./install.sh [--no-permissions] [--uninstall]"
            exit 1
            ;;
    esac
done

# ── Uninstall ──────────────────────────────────────────────

if [ "$UNINSTALL" = true ]; then
    echo "Uninstalling zellaude..."
    rm -f "$PLUGIN_PATH" && dim "  removed $PLUGIN_PATH"
    "$PROJECT_DIR/scripts/install-hooks.sh" --uninstall
    "$PROJECT_DIR/scripts/install-permissions.sh" --uninstall
    green "Done. Restart Zellij to take effect."
    exit 0
fi

# ── Prerequisites ──────────────────────────────────────────

missing=()
command -v jq    &>/dev/null || missing+=(jq)
command -v cargo &>/dev/null || {
    # Try common install location
    export PATH="$HOME/.cargo/bin:$PATH"
    command -v cargo &>/dev/null || missing+=(rust/cargo)
}

# Suggest a command the reader can actually run: brew is macOS-only, and this
# plugin is just as at home on Linux.
jq_install_hint() {
    if command -v brew    &>/dev/null; then echo "  brew install jq"
    elif command -v apt   &>/dev/null; then echo "  sudo apt install jq"
    elif command -v dnf   &>/dev/null; then echo "  sudo dnf install jq"
    elif command -v pacman &>/dev/null; then echo "  sudo pacman -S jq"
    elif [ "$(uname -s)" = "Darwin" ]; then echo "  brew install jq"
    else echo "  see https://jqlang.github.io/jq/download/"
    fi
}

if [ ${#missing[@]} -gt 0 ]; then
    red "Missing: ${missing[*]}"
    echo "Install with:"
    for dep in "${missing[@]}"; do
        case "$dep" in
            jq)          jq_install_hint ;;
            rust/cargo)  echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" ;;
        esac
    done
    exit 1
fi

# ── Ensure wasm target ─────────────────────────────────────

if ! rustup target list --installed 2>/dev/null | grep -q wasm32-wasip1; then
    echo "Adding wasm32-wasip1 target..."
    rustup target add wasm32-wasip1
fi

# ── Build ──────────────────────────────────────────────────

echo "Building zellaude..."
cargo build --release --manifest-path "$PROJECT_DIR/Cargo.toml" 2>&1 | tail -1

# ── Install plugin ─────────────────────────────────────────

mkdir -p "$PLUGIN_DIR"
cp "$PROJECT_DIR/target/wasm32-wasip1/release/zellaude.wasm" "$PLUGIN_PATH"
dim "  installed $PLUGIN_PATH"

# ── Pre-grant plugin permissions ───────────────────────────
#
# Zellij draws its permission prompt inside the plugin's pane. As a one-row
# borderless status bar there is nothing to draw into and nothing to focus, so
# without this the first run is an empty bar that cannot be approved.

if [ "$GRANT_PERMISSIONS" = true ]; then
    "$PROJECT_DIR/scripts/install-permissions.sh"
else
    dim "  skipped permission pre-grant (--no-permissions)"
fi

# ── Install hooks ──────────────────────────────────────────

"$PROJECT_DIR/scripts/install-hooks.sh"

# ── Done ───────────────────────────────────────────────────

green ""
green "Installed! To use, add this to your Zellij layout:"
echo ""
echo '  default_tab_template {'
echo '      pane size=1 borderless=true {'
echo '          plugin location="file:~/.config/zellij/plugins/zellaude.wasm"'
echo '      }'
echo '      children'
echo '  }'
echo ""
dim "Or start with the included layout: zellij --layout $PROJECT_DIR/layout.kdl"
