#!/usr/bin/env sh
set -eu

REPO="matthewyjiang/rho"
BIN_NAME="rho"
INSTALL_DIR="${RHO_INSTALL_DIR:-$HOME/.local/bin}"
VERSION="${RHO_VERSION:-latest}"

need_cmd() {
  if ! command -v "$1" >/dev/null 2>&1; then
    echo "error: required command not found: $1" >&2
    exit 1
  fi
}

platform() {
  os="$(uname -s)"
  arch="$(uname -m)"

  case "$os" in
    Linux) os="unknown-linux-gnu" ;;
    Darwin) os="apple-darwin" ;;
    *)
      echo "error: unsupported OS: $os" >&2
      exit 1
      ;;
  esac

  case "$arch" in
    x86_64|amd64) arch="x86_64" ;;
    arm64|aarch64) arch="aarch64" ;;
    *)
      echo "error: unsupported architecture: $arch" >&2
      exit 1
      ;;
  esac

  target="$arch-$os"

  case "$target" in
    x86_64-unknown-linux-gnu|x86_64-apple-darwin|aarch64-apple-darwin)
      printf '%s' "$target"
      ;;
    *)
      echo "error: no prebuilt binary is available for $target" >&2
      echo "       install with Cargo instead: cargo install rho-coding-agent" >&2
      exit 1
      ;;
  esac
}

release_tag() {
  case "$VERSION" in
    latest)
      api="https://api.github.com/repos/$REPO/releases/latest"
      if command -v curl >/dev/null 2>&1; then
        curl --fail --location --proto '=https' --tlsv1.2 --silent --show-error \
          -H 'User-Agent: rho-installer' "$api" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1
      else
        wget -q --header='User-Agent: rho-installer' "$api" -O - | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1
      fi
      ;;
    rho-coding-agent-*) printf '%s' "$VERSION" ;;
    [0-9]*.[0-9]*.[0-9]*) printf 'rho-coding-agent-v%s' "$VERSION" ;;
    *) printf 'rho-coding-agent-%s' "$VERSION" ;;
  esac
}

asset_url() {
  target="$1"
  asset="rho-$target.tar.gz"
  tag="$(release_tag)"
  if [ -z "$tag" ]; then
    echo "error: could not determine latest release tag from GitHub API" >&2
    exit 1
  fi
  printf 'https://github.com/%s/releases/download/%s/%s' "$REPO" "$tag" "$asset"
}

need_cmd uname
need_cmd mktemp
need_cmd tar
need_cmd mkdir
need_cmd chmod

if command -v curl >/dev/null 2>&1; then
  download() { curl --fail --location --proto '=https' --tlsv1.2 --silent --show-error "$1" --output "$2"; }
elif command -v wget >/dev/null 2>&1; then
  download() { wget -q "$1" -O "$2"; }
else
  echo "error: required command not found: curl or wget" >&2
  exit 1
fi

target="$(platform)"
url="$(asset_url "$target")"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT INT TERM

archive="$tmp/rho.tar.gz"
echo "downloading rho for $target..."
download "$url" "$archive"

checksum="$tmp/rho.tar.gz.sha256"
if download "$url.sha256" "$checksum"; then
  expected="$(awk '{print $1}' "$checksum")"
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$archive" | awk '{print $1}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
  else
    actual=""
    echo "warning: could not verify checksum because sha256sum or shasum is not installed" >&2
  fi
  if [ -n "$actual" ] && [ "$actual" != "$expected" ]; then
    echo "error: checksum verification failed" >&2
    exit 1
  fi
else
  echo "warning: checksum file is unavailable, skipping verification" >&2
fi

tar -xzf "$archive" -C "$tmp"
mkdir -p "$INSTALL_DIR"
install_path="$INSTALL_DIR/$BIN_NAME"

if command -v install >/dev/null 2>&1; then
  install -m 0755 "$tmp/$BIN_NAME" "$install_path"
else
  cp "$tmp/$BIN_NAME" "$install_path"
  chmod 0755 "$install_path"
fi

echo "rho installed to $install_path"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "note: add $INSTALL_DIR to your PATH to run rho from anywhere" ;;
esac
