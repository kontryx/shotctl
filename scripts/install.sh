#!/bin/sh
set -eu

if [ "${1:-}" = "--help" ] || [ "${1:-}" = "-h" ]; then
    printf '%s\n' 'Usage: sh scripts/install.sh [ABSOLUTE_BIN_DIRECTORY]' \
        '' 'Installs shotctl into ~/.local/bin by default.' \
        'Builds with Cargo when available, otherwise uses target/release/shotctl.' \
        'Prints shell setup instructions if the directory is missing from PATH.'
    exit 0
fi

if [ "$#" -gt 1 ]; then
    printf '%s\n' 'Expected at most one installation directory. Use --help for usage.' >&2
    exit 1
fi

project_dir=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
install_dir=${1:-"$HOME/.local/bin"}
case "$install_dir" in
    /*) ;;
    *) printf '%s\n' 'The installation directory must be an absolute path.' >&2; exit 1 ;;
esac
case "$install_dir" in
    *:*) printf '%s\n' 'The installation directory cannot contain a colon (PATH separator).' >&2; exit 1 ;;
esac

binary="$project_dir/target/release/shotctl"
if command -v cargo >/dev/null 2>&1; then
    cargo build --release --locked --manifest-path "$project_dir/Cargo.toml" --target-dir "$project_dir/target"
elif [ ! -x "$binary" ]; then
    printf '%s\n' 'Rust/Cargo is required to build shotctl. Install Rust from https://rustup.rs,' \
        'restart your terminal, then run this installer again.' >&2
    exit 1
else
    printf '%s\n' 'Using the existing release build.'
fi

mkdir -p "$install_dir"
install -m 755 "$binary" "$install_dir/shotctl"
printf '\nInstalled: %s/shotctl\n' "$install_dir"

case ":${PATH:-}:" in
    *":$install_dir:"*)
        printf '%s\n' 'This directory is already on PATH. Run shotctl from any directory.'
        ;;
    *)
        # Quote the path for instructions that can be pasted into a shell.
        escaped_dir=$(printf '%s' "$install_dir" | sed "s/'/'\\\\''/g")
        shell_path=${SHELL:-/bin/sh}
        shell_name=${shell_path##*/}
        if [ "$shell_name" = 'fish' ]; then
            printf '\nRun this once in fish to add shotctl to PATH permanently:\n\n'
            printf "  fish_add_path '%s'\n" "$escaped_dir"
        else
            case "$shell_name" in
                zsh) startup_file='~/.zshrc' ;;
                bash)
                    if [ "$(uname -s)" = 'Darwin' ]; then startup_file='~/.bash_profile'; else startup_file='~/.bashrc'; fi
                    ;;
                *) startup_file='~/.profile' ;;
            esac
            printf '\nAdd this line to %s so new terminals can find shotctl:\n\n' "$startup_file"
            printf "  export PATH='%s':\"\$PATH\"\n" "$escaped_dir"
            printf '\nRun the same export in this terminal to use it immediately.\n'
        fi
        printf '\nThen run shotctl from any directory.\n'
        ;;
esac
