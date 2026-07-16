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

github_api() {
  api="$1"
  if command -v curl >/dev/null 2>&1; then
    curl --fail --location --proto '=https' --tlsv1.2 --silent --show-error \
      -H 'User-Agent: rho-installer' "$api"
  else
    wget -q --header='User-Agent: rho-installer' "$api" -O -
  fi
}

release_has_assets() {
  archive="$1"
  checksum="$2"
  awk -v archive="$archive" -v checksum="$checksum" '
    $0 ~ /"name"[[:space:]]*:/ {
      line=$0
      sub(/^.*"name"[[:space:]]*:[[:space:]]*"/, "", line)
      sub(/".*$/, "", line)
      if (line == archive) have_archive=1
      if (line == checksum) have_checksum=1
    }
    END { exit(have_archive && have_checksum ? 0 : 1) }
  '
}

release_tag() {
  asset="$1"
  checksum="$asset.sha256"
  case "$VERSION" in
    latest)
      latest_api="https://api.github.com/repos/$REPO/releases/latest"
      latest_json="$(github_api "$latest_api")"
      tag="$(printf '%s\n' "$latest_json" | sed -n 's/.*"tag_name": *"\([^"]*\)".*/\1/p' | head -n 1)"
      if printf '%s\n' "$latest_json" | release_has_assets "$asset" "$checksum"; then
        printf '%s' "$tag"
        return
      fi

      releases_api="https://api.github.com/repos/$REPO/releases?per_page=100"
      fallback="$(github_api "$releases_api" | awk -v archive="$asset" -v checksum="$checksum" '
        function select_release() {
          if (tag != "" && have_archive && have_checksum) {
            print tag
            found=1
            exit
          }
        }
        /"tag_name"[[:space:]]*:/ {
          select_release()
          tag=$0
          sub(/^.*"tag_name"[[:space:]]*:[[:space:]]*"/, "", tag)
          sub(/".*$/, "", tag)
          have_archive=0
          have_checksum=0
        }
        /"name"[[:space:]]*:/ {
          name=$0
          sub(/^.*"name"[[:space:]]*:[[:space:]]*"/, "", name)
          sub(/".*$/, "", name)
          if (name == archive) have_archive=1
          if (name == checksum) have_checksum=1
        }
        END { if (!found) select_release() }
      ')"
      if [ -z "$fallback" ]; then
        echo "error: $tag is tagged but required assets $asset and $checksum are not both published yet, and no earlier compatible release was found" >&2
        echo "       install from source instead: cargo install rho-coding-agent" >&2
        exit 1
      fi
      echo "warning: $tag is tagged but required assets are not both published yet; installing $fallback instead" >&2
      printf '%s' "$fallback"
      return
      ;;
    rho-coding-agent-*) tag="$VERSION" ;;
    [0-9]*.[0-9]*.[0-9]*) tag="rho-coding-agent-v$VERSION" ;;
    *) tag="rho-coding-agent-$VERSION" ;;
  esac

  release_api="https://api.github.com/repos/$REPO/releases/tags/$tag"
  if ! github_api "$release_api" | release_has_assets "$asset" "$checksum"; then
    echo "error: release $tag does not include both $asset and $checksum" >&2
    echo "       install from source instead: cargo install rho-coding-agent" >&2
    exit 1
  fi
  printf '%s' "$tag"
}

asset_url() {
  target="$1"
  asset="rho-$target.tar.gz"
  tag="$(release_tag "$asset")"
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
download "$url.sha256" "$checksum" || {
  echo "error: failed to download required checksum: $url.sha256" >&2
  exit 1
}
expected="$(awk '{print $1}' "$checksum")"
if command -v sha256sum >/dev/null 2>&1; then
  actual="$(sha256sum "$archive" | awk '{print $1}')"
elif command -v shasum >/dev/null 2>&1; then
  actual="$(shasum -a 256 "$archive" | awk '{print $1}')"
else
  echo "error: sha256sum or shasum is required to verify the downloaded archive" >&2
  exit 1
fi
if [ -z "$expected" ] || [ "$actual" != "$expected" ]; then
  echo "error: checksum verification failed for $url" >&2
  exit 1
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
