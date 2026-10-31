#!/bin/sh
# Installs studio, studiod and studio-web from a GitHub release.
#
#   STUDIO_VERSION    release to install, like 0.1.0 or v0.1.0 (default: latest)
#   STUDIO_INSTALL_DIR  where to put the binaries (default: $HOME/.local/bin)
#
# This script never uses sudo.
set -eu

REPO="marmolabs/studio"

say() {
    printf '%s\n' "$*"
}

fail() {
    printf 'install: %s\n' "$*" >&2
    exit 1
}

need() {
    command -v "$1" >/dev/null 2>&1 || fail "$1 is required but not installed"
}

need curl
need tar
need uname

os=$(uname -s)
arch=$(uname -m)

case "$os" in
    Darwin) os_part="apple-darwin" ;;
    Linux) os_part="unknown-linux-gnu" ;;
    *) fail "unsupported OS: $os (studio runs on macOS and Linux)" ;;
esac

case "$arch" in
    arm64 | aarch64) arch_part="aarch64" ;;
    x86_64 | amd64) arch_part="x86_64" ;;
    *) fail "unsupported CPU: $arch" ;;
esac

target="$arch_part-$os_part"
case "$target" in
    aarch64-apple-darwin | x86_64-apple-darwin | x86_64-unknown-linux-gnu | aarch64-unknown-linux-gnu) ;;
    *) fail "no release is built for $target" ;;
esac

if command -v sha256sum >/dev/null 2>&1; then
    sha256() { sha256sum "$1" | cut -d ' ' -f 1; }
elif command -v shasum >/dev/null 2>&1; then
    sha256() { shasum -a 256 "$1" | cut -d ' ' -f 1; }
else
    fail "sha256sum or shasum is required to verify the download"
fi

if [ -n "${STUDIO_VERSION:-}" ]; then
    tag="v${STUDIO_VERSION#v}"
    base="https://github.com/$REPO/releases/download/$tag"
else
    base="https://github.com/$REPO/releases/latest/download"
fi

install_dir="${STUDIO_INSTALL_DIR:-$HOME/.local/bin}"

tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT INT TERM

say "Downloading SHA256SUMS"
curl -fsSL "$base/SHA256SUMS" -o "$tmp/SHA256SUMS" ||
    fail "could not download $base/SHA256SUMS (is there a published release?)"

# Find this platform's tarball in the checksum list.
line=$(grep -E "  studio-.*-$target\.tar\.gz\$" "$tmp/SHA256SUMS" | head -n 1 || true)
[ -n "$line" ] || fail "the release has no build for $target"
expected=${line%% *}
file=${line##* }

say "Downloading $file"
curl -fsSL "$base/$file" -o "$tmp/$file" || fail "could not download $base/$file"

actual=$(sha256 "$tmp/$file")
if [ "$actual" != "$expected" ]; then
    fail "checksum mismatch for $file (expected $expected, got $actual)"
fi
say "Checksum ok"

tar -xzf "$tmp/$file" -C "$tmp"
dir="$tmp/${file%.tar.gz}"

mkdir -p "$install_dir" || fail "cannot create $install_dir; set STUDIO_INSTALL_DIR to a folder you own"
for bin in studio studiod studio-web; do
    [ -f "$dir/$bin" ] || fail "$bin is missing from $file"
    cp "$dir/$bin" "$install_dir/$bin.new" || fail "cannot write to $install_dir"
    chmod 755 "$install_dir/$bin.new"
    mv -f "$install_dir/$bin.new" "$install_dir/$bin"
done

say "Installed studio, studiod and studio-web to $install_dir"

case ":$PATH:" in
    *":$install_dir:"*) ;;
    *) say "Note: $install_dir is not on your PATH. Add it to your shell profile." ;;
esac

say ""
say "Next steps:"
say "  studio init                        write a starter studio.toml and hires"
say "  studio project add <path-to-repo>  register a git repository"
say "  studiod                            start the daemon and leave it running"
say "  studio-web                         in another terminal; open the link it prints"
say ""
say "You also need the claude CLI installed and logged in."
