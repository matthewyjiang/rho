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

compact_json() {
  tr -d '[:space:]'
}

release_has_assets() {
  archive="$1"
  checksum="$2"
  json="$(compact_json)"
  printf '%s' "$json" | grep -Fq "\"name\":\"$archive\"" &&
    printf '%s' "$json" | grep -Fq "\"name\":\"$checksum\""
}

release_tag() {
  asset="$1"
  checksum="$asset.sha256"
  case "$VERSION" in
    latest)
      latest_api="https://api.github.com/repos/$REPO/releases/latest"
      latest_json="$(github_api "$latest_api")"
      tag="$(printf '%s' "$latest_json" | compact_json | sed -n 's/.*"tag_name":"\([^"]*\)".*/\1/p')"
      if printf '%s\n' "$latest_json" | release_has_assets "$asset" "$checksum"; then
        printf '%s' "$tag"
        return
      fi

      releases_api="https://api.github.com/repos/$REPO/releases?per_page=100"
      fallback="$(
        github_api "$releases_api" |
          compact_json |
          sed 's/"tag_name":/\
"tag_name":/g' |
          while IFS= read -r release; do
            if printf '%s' "$release" | release_has_assets "$asset" "$checksum"; then
              printf '%s' "$release" |
                sed -n 's/^"tag_name":"\([^"]*\)".*/\1/p'
              break
            fi
          done
      )"
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

configure_credential_store() {
  rho_bin="$1"

  if [ -n "${RHO_CREDENTIAL_STORE:-}" ]; then
    if [ "$RHO_CREDENTIAL_STORE" = "file" ] &&
      ! "$rho_bin" credential-store probe file; then
      echo "error: local file credential storage is unavailable" >&2
      return 1
    fi
    if ! "$rho_bin" credential-store set "$RHO_CREDENTIAL_STORE"; then
      echo "error: failed to set credential store to $RHO_CREDENTIAL_STORE" >&2
      return 1
    fi
    return
  fi

  status="$("$rho_bin" credential-store status 2>/dev/null || true)"
  if [ "$status" = "os" ] || [ "$status" = "file" ]; then
    echo "credential-store choice already configured ($status); keeping it" >&2
    return
  fi

  # CI and explicit noninteractive installs must not block on /dev/tty prompts.
  # curl|sh still prompts when a controlling terminal exists unless CI is set.
  if [ -n "${CI:-}" ] || [ -n "${RHO_INSTALL_NONINTERACTIVE:-}" ] ||
    ! (: </dev/tty) 2>/dev/null; then
    echo "note: credential store left unset (OS default)." >&2
    echo "      run '$rho_bin credential-store probe os' to check OS credential storage" >&2
    echo "      use '$rho_bin credential-store set file' for owner-only local file storage" >&2
    return
  fi

  set_file_backend() {
    echo "File storage uses owner-only permissions but is not encrypted at rest." >/dev/tty
    if ! "$rho_bin" credential-store probe file; then
      echo "warning: local file credential storage is unavailable; leaving the OS default" >&2
      return 1
    fi
    if ! "$rho_bin" credential-store set file; then
      echo "warning: failed to set credential store to file; leaving the OS default" >&2
      return 1
    fi
  }

  set_os_backend() {
    if ! "$rho_bin" credential-store set os; then
      echo "warning: failed to set credential store to os; leaving the OS default" >&2
      return 1
    fi
  }

  read_yes_no() {
    prompt="$1"
    printf '%s' "$prompt" >/dev/tty
    if ! IFS= read -r answer </dev/tty; then
      echo "warning: could not read credential-store choice; leaving the OS default" >&2
      return 2
    fi
    case "$answer" in
      ""|y|Y|yes|YES) return 0 ;;
      n|N|no|NO) return 1 ;;
      *)
        echo "warning: unrecognized choice; leaving the OS default" >&2
        return 2
        ;;
    esac
  }

  if "$rho_bin" credential-store probe os >/dev/null 2>&1; then
    if read_yes_no "OS credential store is available and recommended. Use it? [Y/n] "; then
      set_os_backend || true
    elif [ $? -eq 1 ]; then
      set_file_backend || true
    fi
    return
  fi

  echo "No usable OS credential store was found in this session." >/dev/tty
  if read_yes_no "Use local file credential storage? [Y/n] "; then
    set_file_backend || true
  elif [ $? -eq 1 ]; then
    echo "warning: leaving the OS default; configure the OS credential store before login" >&2
  fi
}

# Installer entry point. Function-only tests source the lines above this marker.
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
read -r expected _ < "$checksum"
if command -v sha256sum >/dev/null 2>&1; then
  actual_line="$(sha256sum "$archive")"
elif command -v shasum >/dev/null 2>&1; then
  actual_line="$(shasum -a 256 "$archive")"
else
  echo "error: sha256sum or shasum is required to verify the downloaded archive" >&2
  exit 1
fi
actual="${actual_line%% *}"
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
configure_credential_store "$install_path"
case ":$PATH:" in
  *":$INSTALL_DIR:"*) ;;
  *) echo "note: add $INSTALL_DIR to your PATH to run rho from anywhere" ;;
esac
