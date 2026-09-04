# Maintainer: Rumen Cholakov <https://github.com/rumen-cholakov>

pkgname=hyprwake
pkgver=0.1.0
pkgrel=1
pkgdesc="Save and restore Hyprland sessions, including terminal directories and TUI sessions"
arch=('x86_64' 'aarch64')
url="https://github.com/rumen-cholakov/hyprwake"
license=('MIT')
depends=('gcc-libs' 'glibc' 'hyprland')
makedepends=('cargo')
optdepends=(
  'sqlite: resume codex sessions'
  'uwsm: launch restored applications in their own systemd scopes'
)
source=("$pkgname-$pkgver.tar.gz::$url/archive/refs/tags/v$pkgver.tar.gz")
sha256sums=('SKIP')

prepare() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  cargo fetch --locked --target "$(rustc -vV | sed -n 's/host: //p')"
}

build() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  export CARGO_TARGET_DIR=target
  cargo build --frozen --release
}

check() {
  cd "$pkgname-$pkgver"
  export RUSTUP_TOOLCHAIN=stable
  # The suite needs no compositor: hyprctl and /proc both sit behind traits.
  cargo test --frozen
}

package() {
  cd "$pkgname-$pkgver"
  install -Dm0755 -t "$pkgdir/usr/bin/" "target/release/$pkgname"
  install -Dm0644 LICENSE "$pkgdir/usr/share/licenses/$pkgname/LICENSE"
  install -Dm0644 README.md "$pkgdir/usr/share/doc/$pkgname/README.md"
}
