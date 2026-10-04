#!/usr/bin/env bash
# Build the release packages of the log analytics plugin.
#
#   ./build.sh              every platform whose Rust target is installed
#   ./build.sh --host-only  the platform of this machine only
#   ./build.sh --prebuilt DIR  package executables built elsewhere
#   ./build.sh --webapp ARCHIVE  package this webapp archive
#   ./build.sh --webapp-only  only take the webapp into webapp/dist
#
# Every platform gets its own package, dist/<id>-<version>-<os>-<arch>.tar.gz,
# with one executable and a plugin.json whose server.executables names only that
# platform, as a per-platform package must. A <archive>.sha256 file sits next
# to each archive for the catalog.
#
# The browser bundle comes from plugin-log-analytics-webapp, which builds it
# for both log analytics plugins. webapp.lock names the release version. The
# archive is, in this order, the one --webapp names, the one built in a sibling
# checkout (../plugin-log-analytics-webapp/release), or the release download,
# checked against the .sha256 file of the release. The build of this plugin is taken
# into webapp/dist: the packages carry it with the chunks and the map and
# country files the bundle loads on demand.
#
# Native targets build with cargo, the others with cargo zigbuild. A platform
# whose target is not installed is skipped with the command that would add it.
# Set CARGO_NET_OFFLINE=true to build from the local crate cache.
#
# With --prebuilt DIR nothing is compiled. DIR holds one executable per
# platform named as in the package, log-analytics-tantivy-<os>-<arch> with .exe on
# Windows, and every platform must have one (--host-only narrows that to the
# platform of this machine). The release workflow builds the executables on
# native runners and packages them this way.
#
# The packages are unsigned. The release workflow signs them with the official
# plugin key through nginxui/plugin-release, which also writes plugin.sums. A
# local build installs on a host in developer mode, or after
# "nginx-ui plugin sign <package> --key <key>".
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST="${ROOT}/dist"
STAGE="${DIST}/stage"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT}/target}"

PLUGIN_ID="com.nginxui.log-analytics-tantivy"
BIN="log-analytics-tantivy"

# The platforms Nginx UI is released for, but MIPS and ARMv5: their Rust
# targets have no 64-bit atomics. The host names a platform by GOOS and GOARCH
# only, so one linux-arm package serves ARMv6 and ARMv7, built soft float.
#
# platform key | rust target | build tool
PLATFORMS=(
  "linux-amd64|x86_64-unknown-linux-musl|zigbuild"
  "linux-arm64|aarch64-unknown-linux-musl|zigbuild"
  "linux-386|i686-unknown-linux-musl|zigbuild"
  "linux-arm|arm-unknown-linux-musleabi|zigbuild"
  "linux-riscv64|riscv64gc-unknown-linux-musl|zigbuild"
  "linux-loong64|loongarch64-unknown-linux-musl|zigbuild"
  "darwin-amd64|x86_64-apple-darwin|cargo"
  "darwin-arm64|aarch64-apple-darwin|cargo"
  "windows-amd64|x86_64-pc-windows-gnu|zigbuild"
  "windows-arm64|aarch64-pc-windows-gnullvm|zigbuild"
  "windows-386|i686-pc-windows-gnu|zigbuild"
)

USAGE="usage: $0 [--host-only] [--prebuilt DIR] [--webapp ARCHIVE] [--webapp-only]"
HOST_ONLY=0
WEBAPP_ONLY=0
PREBUILT=""
WEBAPP_ARCHIVE="${WEBAPP_ARCHIVE:-}"
while [[ $# -gt 0 ]]; do
  case "$1" in
    --host-only) HOST_ONLY=1 ;;
    --webapp-only) WEBAPP_ONLY=1 ;;
    --prebuilt)
      if [[ $# -lt 2 ]]; then
        echo "${USAGE}" >&2
        exit 2
      fi
      PREBUILT="$2"
      shift
      ;;
    --webapp)
      if [[ $# -lt 2 ]]; then
        echo "${USAGE}" >&2
        exit 2
      fi
      WEBAPP_ARCHIVE="$2"
      shift
      ;;
    -h | --help)
      echo "${USAGE}"
      exit 0
      ;;
    *)
      echo "${USAGE}" >&2
      exit 2
      ;;
  esac
  shift
done
if [[ -n "${PREBUILT}" ]]; then
  if [[ ! -d "${PREBUILT}" ]]; then
    echo "--prebuilt does not name a directory: ${PREBUILT}" >&2
    exit 1
  fi
  PREBUILT="$(cd "${PREBUILT}" && pwd)"
fi

cd "${ROOT}"

# The webapp of this plugin, taken from the release archive into webapp/dist.
WEBAPP_VERSION="$(sed -n 's/^version=//p' webapp.lock)"
WEBAPP_NAME="plugin-log-analytics-webapp-${WEBAPP_VERSION}.tar.gz"
if [[ -z "${WEBAPP_ARCHIVE}" && -f "../plugin-log-analytics-webapp/release/${WEBAPP_NAME}" ]]; then
  WEBAPP_ARCHIVE="../plugin-log-analytics-webapp/release/${WEBAPP_NAME}"
fi
if [[ -z "${WEBAPP_ARCHIVE}" ]]; then
  # Kept apart from the packages, dist/*.tar.gz is what a release publishes.
  # A later run reuses the download while it matches the checksum kept with it.
  WEBAPP_ARCHIVE="${DIST}/cache/${WEBAPP_NAME}"
  if [[ ! -f "${WEBAPP_ARCHIVE}.sha256" || ! -f "${WEBAPP_ARCHIVE}" ]] ||
    [[ "$(shasum -a 256 "${WEBAPP_ARCHIVE}" | cut -d' ' -f1)" != "$(cat "${WEBAPP_ARCHIVE}.sha256")" ]]; then
    url="https://github.com/nginxui/plugin-log-analytics-webapp/releases/download/v${WEBAPP_VERSION}/${WEBAPP_NAME}"
    mkdir -p "${DIST}/cache"
    rm -f "${WEBAPP_ARCHIVE}.sha256"
    # Retries ride out a passing server error of the download.
    curl -fsSL --retry 5 --retry-delay 3 -o "${WEBAPP_ARCHIVE}" "${url}"
    # The release publishes the checksum next to the archive
    expected="$(curl -fsSL --retry 5 --retry-delay 3 "${url}.sha256" | cut -d' ' -f1)"
    actual="$(shasum -a 256 "${WEBAPP_ARCHIVE}" | cut -d' ' -f1)"
    if [[ -z "${expected}" || "${actual}" != "${expected}" ]]; then
      echo "the webapp archive does not match the checksum of its release: ${actual}" >&2
      exit 1
    fi
    printf '%s' "${expected}" >"${WEBAPP_ARCHIVE}.sha256"
  fi
fi
if [[ ! -f "${WEBAPP_ARCHIVE}" ]]; then
  echo "no webapp archive at ${WEBAPP_ARCHIVE}" >&2
  exit 1
fi
echo "webapp: ${WEBAPP_ARCHIVE}"
rm -rf webapp/dist "${DIST}/webapp"
mkdir -p webapp "${DIST}/webapp"
tar -xzf "${WEBAPP_ARCHIVE}" -C "${DIST}/webapp" "${PLUGIN_ID}"
mv "${DIST}/webapp/${PLUGIN_ID}" webapp/dist
rm -rf "${DIST}/webapp"

for file in main.js style.css icon.svg chunks/search.js chunks/dashboard.js manifest.webapp.json; do
  if [[ ! -f "webapp/dist/${file}" ]]; then
    echo "webapp/dist/${file} is missing from the webapp archive" >&2
    exit 1
  fi
done
if [[ "${WEBAPP_ONLY}" == 1 ]]; then
  exit 0
fi

# plugin.json is written by hand, the manifest tool narrows it to one
# platform per package, see src/manifest.rs.
cargo build --quiet --release --bin manifest
MANIFEST_TOOL="${TARGET_DIR}/release/manifest"

VERSION="$(sed -n 's/^  "version": "\(.*\)",$/\1/p' plugin.json | head -n 1)"
if [[ -z "${VERSION}" ]]; then
  echo "could not read the version from plugin.json" >&2
  exit 1
fi

HOST_KEY=""
case "$(uname -s)-$(uname -m)" in
  Darwin-arm64) HOST_KEY="darwin-arm64" ;;
  Darwin-x86_64) HOST_KEY="darwin-amd64" ;;
  Linux-x86_64) HOST_KEY="linux-amd64" ;;
  Linux-aarch64) HOST_KEY="linux-arm64" ;;
esac

# Keep macOS tar from adding AppleDouble "._*" entries.
export COPYFILE_DISABLE=1

sha256_hex() {
  if command -v sha256sum >/dev/null 2>&1; then
    sha256sum <"$1" | cut -d ' ' -f 1
  else
    shasum -a 256 <"$1" | cut -d ' ' -f 1
  fi
}

sha256_line() {
  printf '%s  %s\n' "$(sha256_hex "$1")" "$(basename "$1")"
}

installed_targets() {
  rustup target list --installed 2>/dev/null || true
}

stage_common() {
  local dir="$1"
  mkdir -p "${dir}/webapp/dist"
  cp -R "${ROOT}/webapp/dist/." "${dir}/webapp/dist/"
  rm -f "${dir}/webapp/dist/manifest.webapp.json"
  for doc in README.md LICENSE; do
    if [[ -f "${ROOT}/${doc}" ]]; then
      cp "${ROOT}/${doc}" "${dir}/${doc}"
    fi
  done
}

package_dir() {
  local dir="$1" archive="$2"
  local entries=(plugin.json)
  for entry in README.md LICENSE server webapp; do
    if [[ -e "${dir}/${entry}" ]]; then
      entries+=("${entry}")
    fi
  done
  rm -f "${archive}" "${archive}.sha256"
  tar -czf "${archive}" -C "${dir}" "${entries[@]}"
  sha256_line "${archive}" >"${archive}.sha256"
  OUTPUTS+=("${archive}" "${archive}.sha256")
}

rm -rf "${STAGE}"
rm -f "${DIST}/${PLUGIN_ID}-${VERSION}"*.tar.gz "${DIST}/${PLUGIN_ID}-${VERSION}"*.tar.gz.sha256
mkdir -p "${STAGE}"

echo "building ${PLUGIN_ID} ${VERSION}"

INSTALLED="$(installed_targets)"
OUTPUTS=()
SKIPPED=()
for entry in "${PLATFORMS[@]}"; do
  IFS='|' read -r key target tool <<<"${entry}"
  os="${key%%-*}"

  if [[ "${HOST_ONLY}" -eq 1 && "${key}" != "${HOST_KEY}" ]]; then
    continue
  fi

  name="${BIN}-${key}"
  file="${BIN}"
  if [[ "${os}" == "windows" ]]; then
    name="${name}.exe"
    file="${file}.exe"
  fi

  if [[ -n "${PREBUILT}" ]]; then
    source_file="${PREBUILT}/${name}"
    if [[ ! -f "${source_file}" ]]; then
      echo "${source_file} is missing" >&2
      exit 1
    fi
    echo "  ${key} (prebuilt)"
  else
    if ! grep -qx "${target}" <<<"${INSTALLED}"; then
      SKIPPED+=("${key}: rustup target add ${target}")
      continue
    fi
    echo "  ${key} (${target}, ${tool})"
    if [[ "${tool}" == "zigbuild" ]]; then
      cargo zigbuild --release --target "${target}" --bin "${BIN}"
    else
      cargo build --release --target "${target}" --bin "${BIN}"
    fi
    source_file="${TARGET_DIR}/${target}/release/${file}"
  fi

  dir="${STAGE}/${key}"
  mkdir -p "${dir}/server/dist"
  cp "${source_file}" "${dir}/server/dist/${name}"
  chmod 755 "${dir}/server/dist/${name}"
  echo "    ${name} ($(du -h "${dir}/server/dist/${name}" | cut -f1 | tr -d '[:space:]'))"
  stage_common "${dir}"
  "${MANIFEST_TOOL}" -platform "${key}" -out "${dir}/plugin.json" >/dev/null

  package_dir "${dir}" "${DIST}/${PLUGIN_ID}-${VERSION}-${key}.tar.gz"
done

echo "packages:"
for file in "${OUTPUTS[@]}"; do
  printf "  %-6s %s\n" "$(du -h "${file}" | cut -f1 | tr -d '[:space:]')" "${file#"${ROOT}/"}"
done
if [[ "${#SKIPPED[@]}" -gt 0 ]]; then
  echo "skipped, the Rust target is not installed:"
  for line in "${SKIPPED[@]}"; do
    echo "  ${line}"
  done
fi
