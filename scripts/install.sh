#!/bin/sh
# Install the latest tono release for the current OS/arch. Resolves "latest"
# at run time via the GitHub Releases redirect (avoids the stricter,
# unauthenticated api.github.com rate limit and a jq dependency), so this
# script itself never changes across releases.
set -eu

REPO="tono-lang/tono"
INSTALL_DIR="${TONO_INSTALL_DIR:-$HOME/.tono/bin}"

os="$(uname -s)"
arch="$(uname -m)"

case "$os" in
  Darwin) platform_os="apple-darwin" ;;
  Linux) platform_os="unknown-linux-gnu" ;;
  *)
    echo "error: unsupported OS: $os" >&2
    exit 1
    ;;
esac

case "$arch" in
  arm64 | aarch64) platform_arch="aarch64" ;;
  x86_64 | amd64) platform_arch="x86_64" ;;
  *)
    echo "error: unsupported architecture: $arch" >&2
    exit 1
    ;;
esac

target="${platform_arch}-${platform_os}"

latest_url=$(curl -fsSL -o /dev/null -w '%{url_effective}' "https://github.com/${REPO}/releases/latest")
tag="${latest_url##*/}"
version="${tag#v}"
asset="tono-${version}-${target}.tar.gz"
base_url="https://github.com/${REPO}/releases/download/${tag}"

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT

echo "Downloading tono ${tag} for ${target}..."
curl -fsSL -o "${workdir}/${asset}" "${base_url}/${asset}"
curl -fsSL -o "${workdir}/SHA256SUMS" "${base_url}/SHA256SUMS"

cd "$workdir"
expected=$(grep " ${asset}\$" SHA256SUMS | cut -d' ' -f1)
if [ -z "$expected" ]; then
  echo "error: no checksum found for ${asset} in SHA256SUMS" >&2
  exit 1
fi

if command -v sha256sum >/dev/null 2>&1; then
  actual=$(sha256sum "$asset" | cut -d' ' -f1)
else
  actual=$(shasum -a 256 "$asset" | cut -d' ' -f1)
fi

if [ "$expected" != "$actual" ]; then
  echo "error: checksum mismatch for ${asset}" >&2
  echo "  expected: $expected" >&2
  echo "  actual:   $actual" >&2
  exit 1
fi

tar xzf "$asset"
extracted="tono-${version}-${target}"

mkdir -p "$INSTALL_DIR"
cp "${extracted}/tono" "${extracted}/tono-frontend" "$INSTALL_DIR/"
chmod +x "${INSTALL_DIR}/tono" "${INSTALL_DIR}/tono-frontend"

echo "Installed tono ${tag} to ${INSTALL_DIR}"

case ":$PATH:" in
  *":${INSTALL_DIR}:"*) ;;
  *)
    echo ""
    echo "${INSTALL_DIR} is not on your PATH. Add it, e.g.:"
    echo "  export PATH=\"${INSTALL_DIR}:\$PATH\""
    ;;
esac
