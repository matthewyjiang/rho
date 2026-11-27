# Fedora package published to mjiang-extras by
# .github/workflows/publish-fedora-package.yml, which rewrites Version from the
# release tag and builds with `rpmbuild --nocheck`. Mirrors the PKGBUILD.

# The release profile strips symbols, so the -debuginfo package would be empty.
%global debug_package %{nil}
# Fedora's automatic build flags export RUSTFLAGS from %%build_rustflags over
# the release profile, including -Cdebuginfo=2, which fat LTO carries through
# the single-unit link: that OOM-killed the Arch package build on a 16 GB
# runner (see publish-arch-package.yml). They also add GCC LTO to CFLAGS for
# the C dependencies, which the PKGBUILD turns off with !lto. Build with the
# release profile alone, as the release binaries do.
%undefine _auto_set_build_flags

Name:           rho-coding-agent
# x-release-please-start-version
Version:        3.6.0
# x-release-please-end
Release:        1%{?dist}
Summary:        A fast Rust agent harness with a small footprint and opinionated defaults
# Cargo.toml: MIT AND Apache-2.0 (bundled starlark and others, see THIRD-PARTY-NOTICES).
License:        MIT AND Apache-2.0
URL:            https://github.com/matthewyjiang/rho
%global tag rho-coding-agent-v%{version}
Source0:        %{url}/archive/refs/tags/%{tag}.tar.gz#/%{name}-%{version}.tar.gz

BuildRequires:  cargo
BuildRequires:  rust
BuildRequires:  gcc
BuildRequires:  sqlite-devel
# Runtime libraries (glibc, libgcc, sqlite-libs) come from rpm's automatic
# ELF dependencies; the PKGBUILD lists them only because pacman has none.

%global cargo_home %{_builddir}/%{name}-cargo

%description
Rho is a coding-agent harness for the terminal: an interactive TUI, one-off
prompts, and a per-user daemon that keeps sessions running in the background.

%prep
%autosetup -n rho-%{tag}
export CARGO_HOME=%{cargo_home}
cargo fetch --locked --target %{_arch}-unknown-linux-gnu

%build
export CARGO_HOME=%{cargo_home}
# No bundled-sqlite feature: link the system libsqlite3.
cargo build --release --locked --bin rho --no-default-features

%install
install -Dpm0755 target/release/rho %{buildroot}%{_bindir}/rho

%check
export CARGO_HOME=%{cargo_home}
cargo test --locked --no-default-features

%files
%license LICENSE crates/rho/THIRD-PARTY-NOTICES
%{_bindir}/rho
