#!/bin/sh
# Exercises scripts/install.sh end to end against a locally staged release,
# twice in a row. The second run is the one that matters: it installs over
# binaries the first run already placed, which is the path a real upgrade
# takes and the one that shipped broken (dune leaves its output read-only,
# tar preserves the mode, and cp cannot overwrite a read-only file).
#
# No network: a stub `curl` earlier on PATH serves the staged tarball and
# checksum file from disk, so the real script's own download, checksum,
# extract and install logic all run unchanged.
set -eu

root=$(cd -- "$(dirname -- "$0")/.." && pwd)
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

case "$(uname -s)" in
  Darwin) platform_os="apple-darwin" ;;
  Linux) platform_os="unknown-linux-gnu" ;;
  *) echo "skipping: unsupported OS $(uname -s)" >&2; exit 0 ;;
esac
case "$(uname -m)" in
  arm64 | aarch64) platform_arch="aarch64" ;;
  x86_64 | amd64) platform_arch="x86_64" ;;
  *) echo "skipping: unsupported arch $(uname -m)" >&2; exit 0 ;;
esac

version="0.0.0-test"
tag="v${version}"
target="${platform_arch}-${platform_os}"
stage="tono-${version}-${target}"
asset="${stage}.tar.gz"

# Staged read-only on purpose: this is exactly how dune leaves the OCaml
# binaries, and the mode tar carries to the user's machine.
mkdir -p "${work}/dist/${stage}"
for bin in tono tono-frontend tono-lsp; do
  printf '#!/bin/sh\necho %s\n' "$bin" > "${work}/dist/${stage}/${bin}"
  chmod 555 "${work}/dist/${stage}/${bin}"
done
: > "${work}/dist/${stage}/README.md"
: > "${work}/dist/${stage}/LICENSE"
(cd "${work}/dist" && tar czf "${asset}" "${stage}")

if command -v sha256sum >/dev/null 2>&1; then
  (cd "${work}/dist" && sha256sum "${asset}" > SHA256SUMS)
else
  (cd "${work}/dist" && shasum -a 256 "${asset}" > SHA256SUMS)
fi

# The stub answers the three requests install.sh makes: the "latest"
# redirect probe (-w '%{url_effective}'), the archive, and SHA256SUMS.
mkdir -p "${work}/bin"
cat > "${work}/bin/curl" <<STUB
#!/bin/sh
out=""
url=""
for a in "\$@"; do
  case "\$prev" in -o) out="\$a" ;; esac
  case "\$a" in https://*) url="\$a" ;; esac
  prev="\$a"
done
case " \$* " in
  *" %{url_effective} "*) printf 'https://github.com/tono-lang/tono/releases/tag/${tag}\n'; exit 0 ;;
esac
case "\$url" in
  *SHA256SUMS) cp "${work}/dist/SHA256SUMS" "\$out" ;;
  *.tar.gz) cp "${work}/dist/${asset}" "\$out" ;;
  *) echo "stub curl: unexpected url \$url" >&2; exit 1 ;;
esac
STUB
chmod 755 "${work}/bin/curl"

TONO_INSTALL_DIR="${work}/install"
export TONO_INSTALL_DIR
export PATH="${work}/bin:$PATH"

echo "run 1 (fresh install)"
sh "${root}/scripts/install.sh" >/dev/null

echo "run 2 (upgrade over the first install)"
sh "${root}/scripts/install.sh" >/dev/null

for bin in tono tono-frontend tono-lsp; do
  path="${TONO_INSTALL_DIR}/${bin}"
  [ -x "$path" ] || { echo "error: ${bin} is not executable after install" >&2; exit 1; }
  [ -w "$path" ] || { echo "error: ${bin} is read-only, so the next upgrade cannot replace it" >&2; exit 1; }
done

echo "ok: install.sh is idempotent and leaves replaceable binaries"
