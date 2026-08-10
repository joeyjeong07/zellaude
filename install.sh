#!/usr/bin/env bash
# install.sh — Build and install zellaude (Zellij plugin + agent hooks)
#
# Usage:
#   ./install.sh                  # install everything
#   ./install.sh --no-permissions # install without pre-granting plugin permissions
#   ./install.sh --uninstall      # remove everything
#   ./install.sh --help           # show all options and environment overrides
set -euo pipefail

PROJECT_DIR="$(cd "$(dirname "$0")" && pwd)"
INSTALL_HOME="${ZELLAUDE_INSTALL_HOME:-$HOME}"
BUILD_DIR="${ZELLAUDE_BUILD_DIR:-$PROJECT_DIR/target}"
INSTALL_HOME_OVERRIDDEN=false
if [ -n "${ZELLAUDE_INSTALL_HOME:-}" ]; then
    INSTALL_HOME_OVERRIDDEN=true
fi

# Resolve a relative target directory against the project, not the caller's
# current directory. This keeps invocation from another directory predictable.
case "$BUILD_DIR" in
    /*) ;;
    *) BUILD_DIR="$PROJECT_DIR/$BUILD_DIR" ;;
esac

PLUGIN_DIR="$INSTALL_HOME/.config/zellij/plugins"
PLUGIN_PATH="$PLUGIN_DIR/zellaude.wasm"

red()   { printf '\033[31m%s\033[0m\n' "$*"; }
green() { printf '\033[32m%s\033[0m\n' "$*"; }
dim()   { printf '\033[2m%s\033[0m\n' "$*"; }

usage() {
    cat <<EOF
Usage: ${0##*/} [--no-permissions] [--uninstall]

Build and install Zellaude, or remove an existing installation.

Options:
  --no-permissions  Do not pre-grant Zellij plugin permissions
  --uninstall       Remove the plugin, hooks, and permission grant
  -h, --help        Show this help

Environment:
  ZELLAUDE_INSTALL_HOME  Installation home (default: \$HOME)
  ZELLAUDE_BUILD_DIR     Cargo target directory (default: project/target)
EOF
}

# The helper scripts use HOME to choose their destinations. Export the selected
# installation home only to those children so Cargo and rustup keep using the
# invoking user's normal toolchain configuration.
run_helper() (
    export HOME="$INSTALL_HOME"
    export ZELLAUDE_INSTALL_HOME="$INSTALL_HOME"
    if [ "$INSTALL_HOME_OVERRIDDEN" = true ]; then
        # A staging home must not inherit destinations from the invoking
        # account. Explicit Zellaude overrides still take precedence.
        export ZELLAUDE_CODEX_HOME="${ZELLAUDE_CODEX_HOME:-$INSTALL_HOME/.codex}"
        export ZELLAUDE_CACHE_DIR="${ZELLAUDE_CACHE_DIR:-$INSTALL_HOME/.cache/zellij}"
    fi
    "$@"
)

UNINSTALL=false
GRANT_PERMISSIONS=true

for arg in "$@"; do
    case "$arg" in
        --uninstall)      UNINSTALL=true ;;
        --no-permissions) GRANT_PERMISSIONS=false ;;
        -h|--help)
            usage
            exit 0
            ;;
        *)
            red "Unknown option: $arg"
            usage
            exit 1
            ;;
    esac
done

# ── Uninstall ──────────────────────────────────────────────

if [ "$UNINSTALL" = true ]; then
    if ! command -v jq &>/dev/null; then
        red "Missing: jq"
        echo "jq is required to remove Zellaude's hook registrations safely."
        exit 1
    fi
    run_helper "$PROJECT_DIR/scripts/install-hooks.sh" --check >/dev/null
    run_helper "$PROJECT_DIR/scripts/install-permissions.sh" --check >/dev/null
    echo "Uninstalling zellaude..."
    rm -f "$PLUGIN_PATH"
    dim "  removed $PLUGIN_PATH"
    run_helper "$PROJECT_DIR/scripts/install-hooks.sh" --uninstall
    run_helper "$PROJECT_DIR/scripts/install-permissions.sh" --uninstall
    green "Done. Restart Zellij to take effect."
    exit 0
fi

# ── Prerequisites ──────────────────────────────────────────

# Try rustup's standard bin directory before reporting Rust tools as missing.
if ! command -v cargo &>/dev/null || ! command -v rustup &>/dev/null; then
    export PATH="$HOME/.cargo/bin:$PATH"
fi

missing=()
command -v jq     &>/dev/null || missing+=(jq)
command -v cargo  &>/dev/null || missing+=(cargo)
command -v rustup &>/dev/null || missing+=(rustup)

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

zellij_version=""
zellij_version_supported() {
    local output="$1"
    local major minor

    if [[ ! "$output" =~ ([0-9]+)\.([0-9]+)(\.[0-9]+)? ]]; then
        return 1
    fi

    zellij_version="${BASH_REMATCH[0]}"
    major="${BASH_REMATCH[1]}"
    minor="${BASH_REMATCH[2]}"
    (( 10#$major > 0 || (10#$major == 0 && 10#$minor >= 44) ))
}

if command -v zellij &>/dev/null; then
    zellij_version_output="$(zellij --version 2>/dev/null || true)"
    if ! zellij_version_supported "$zellij_version_output"; then
        missing+=(zellij-0.44+)
    fi
else
    missing+=(zellij-0.44+)
fi

if [ ${#missing[@]} -gt 0 ]; then
    red "Missing: ${missing[*]}"
    echo "Install or update with:"
    rust_install_hint_shown=false
    for dep in "${missing[@]}"; do
        case "$dep" in
            jq)      jq_install_hint ;;
            cargo|rustup)
                if [ "$rust_install_hint_shown" = false ]; then
                    echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh"
                    rust_install_hint_shown=true
                fi
                ;;
            zellij-0.44+)
                if [ -n "$zellij_version" ]; then
                    echo "  Zellij 0.44 or newer is required (found $zellij_version)."
                else
                    echo "  Zellij 0.44 or newer is required."
                fi
                echo "  See https://zellij.dev/documentation/installation"
                ;;
        esac
    done
    exit 1
fi

# Refuse malformed user hook configuration before rustup, Cargo, or any install
# destination is changed. The mutating helper validates again under its lock.
run_helper "$PROJECT_DIR/scripts/install-hooks.sh" --check >/dev/null
if [ "$GRANT_PERMISSIONS" = true ]; then
    run_helper "$PROJECT_DIR/scripts/install-permissions.sh" --check >/dev/null
fi
if [ -e "$PLUGIN_PATH" ] && [ ! -f "$PLUGIN_PATH" ]; then
    red "Cannot install over non-file destination: $PLUGIN_PATH"
    exit 1
fi

# ── Ensure wasm target ─────────────────────────────────────

if ! rustup target list --installed 2>/dev/null | grep -qx 'wasm32-wasip1'; then
    echo "Adding wasm32-wasip1 target..."
    rustup target add wasm32-wasip1
fi

# ── Build ──────────────────────────────────────────────────

echo "Building zellaude..."
(
    cd "$PROJECT_DIR"
    cargo build --locked --release --target wasm32-wasip1 --target-dir "$BUILD_DIR" --manifest-path "$PROJECT_DIR/Cargo.toml"
)

ARTIFACT="$BUILD_DIR/wasm32-wasip1/release/zellaude.wasm"
if [ ! -f "$ARTIFACT" ] || [ ! -s "$ARTIFACT" ]; then
    red "Build succeeded but no nonempty plugin artifact was produced at:"
    echo "  $ARTIFACT"
    exit 1
fi

# ── Install plugin ─────────────────────────────────────────

if [ -e "$PLUGIN_PATH" ] && [ ! -f "$PLUGIN_PATH" ]; then
    red "Cannot install over non-file destination: $PLUGIN_PATH"
    exit 1
fi

mkdir -p "$PLUGIN_DIR"
PLUGIN_TMP=""
cleanup_plugin_tmp() {
    if [ -n "${PLUGIN_TMP:-}" ]; then
        rm -f "$PLUGIN_TMP"
    fi
}
trap cleanup_plugin_tmp EXIT
trap 'exit 1' HUP INT TERM

PLUGIN_TMP="$(mktemp "$PLUGIN_DIR/.zellaude.wasm.XXXXXX")"
cp "$ARTIFACT" "$PLUGIN_TMP"
if [ ! -s "$PLUGIN_TMP" ]; then
    red "Refusing to install an empty plugin artifact."
    exit 1
fi
chmod 0644 "$PLUGIN_TMP"
mv -f "$PLUGIN_TMP" "$PLUGIN_PATH"
PLUGIN_TMP=""
trap - EXIT HUP INT TERM
dim "  installed $PLUGIN_PATH"

# ── Pre-grant plugin permissions ───────────────────────────
#
# Zellij draws its permission prompt inside the plugin's pane. As a one-row
# borderless status bar there is nothing to draw into and nothing to focus, so
# without this the first run is an empty bar that cannot be approved.

if [ "$GRANT_PERMISSIONS" = true ]; then
    run_helper "$PROJECT_DIR/scripts/install-permissions.sh"
else
    dim "  skipped permission pre-grant (--no-permissions)"
fi

# ── Install hooks ──────────────────────────────────────────

run_helper "$PROJECT_DIR/scripts/install-hooks.sh"

# ── Done ───────────────────────────────────────────────────

green ""
green "Installed! To use, add this to your Zellij layout:"
echo ""
echo '  default_tab_template {'
echo '      pane size=1 borderless=true {'
if [ "$INSTALL_HOME" = "$HOME" ]; then
    echo '          plugin location="file:~/.config/zellij/plugins/zellaude.wasm"'
else
    echo "          plugin location=\"file:$PLUGIN_PATH\""
fi
echo '      }'
echo '      children'
echo '  }'
echo ""
dim "Or start with the included layout: zellij --layout $PROJECT_DIR/layout.kdl"
