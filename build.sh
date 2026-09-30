#!/usr/bin/env bash
# Build the release packages of the log analytics plugin.
#
#   ./build.sh              every platform whose Rust target is installed
#   ./build.sh --host-only  the platform of this machine only
#   ./build.sh --prebuilt DIR  package executables built elsewhere
#
# Every platform gets its own package, dist/<id>-<version>-<os>-<arch>.tar.gz,
# with one executable and a plugin.json whose server.executables names only that
# platform (plugin spec PKG-12). A <archive>.sha256 file sits next to each
# archive for the catalog.
#
# The browser bundle has to be built first (cd webapp && bun install && bun run
# build): the packages carry webapp/dist, with the chunks and the map and
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
# Every package also carries plugin.sums at its root: the sha256 of each file of
# the package in sha256sum format, sorted by path. When MINISIGN_KEY names a
# minisign secret key file, plugin.sums is signed into plugin.sums.minisig and
# nginx-ui derives the trust level from the signing key. Without it the packages
# are unsigned, and a host installs them only in developer mode.
#
#   MINISIGN_KEY=/path/to/plugin.key ./build.sh
#
# MINISIGN_PASSWORD answers the password prompt without a terminal.
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST="${ROOT}/dist"
STAGE="${DIST}/stage"
TARGET_DIR="${CARGO_TARGET_DIR:-${ROOT}/target}"

PLUGIN_ID="com.nginxui.log-analytics-tantivy"
BIN="log-analytics-tantivy"

# platform key | rust target | build tool
PLATFORMS=(
  "linux-amd64|x86_64-unknown-linux-musl|zigbuild"
  "linux-arm64|aarch64-unknown-linux-musl|zigbuild"
  "darwin-amd64|x86_64-apple-darwin|cargo"
  "darwin-arm64|aarch64-apple-darwin|cargo"
  "windows-amd64|x86_64-pc-windows-gnu|zigbuild"
  "windows-arm64|aarch64-pc-windows-gnullvm|zigbuild"
)

USAGE="usage: $0 [--host-only] [--prebuilt DIR]"
HOST_ONLY=0
PREBUILT=""
while [[ $# -gt 0 ]]; do
  case "$1" in
    --host-only) HOST_ONLY=1 ;;
    --prebuilt)
      if [[ $# -lt 2 ]]; then
        echo "${USAGE}" >&2
        exit 2
      fi
      PREBUILT="$2"
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

MINISIGN_KEY="${MINISIGN_KEY:-}"
MINISIGN_PASSWORD="${MINISIGN_PASSWORD:-}"
if [[ -n "${MINISIGN_KEY}" ]]; then
  if [[ ! -f "${MINISIGN_KEY}" ]]; then
    echo "MINISIGN_KEY does not name a file: ${MINISIGN_KEY}" >&2
    exit 1
  fi
  if ! command -v minisign >/dev/null 2>&1; then
    echo "MINISIGN_KEY is set but minisign is not installed" >&2
    exit 1
  fi
  MINISIGN_KEY="$(cd "$(dirname "${MINISIGN_KEY}")" && pwd)/$(basename "${MINISIGN_KEY}")"
fi

cd "${ROOT}"

for file in main.js style.css icon.svg chunks/search.js chunks/dashboard.js manifest.webapp.json; do
  if [[ ! -f "webapp/dist/${file}" ]]; then
    echo "webapp/dist/${file} is missing, build the webapp first: cd webapp && bun install && bun run build" >&2
    exit 1
  fi
done

# The manifest is generated from the code and the bundle, see src/manifest.rs.
cargo run --quiet --release --bin manifest
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
  for doc in README.md LICENSE CHANGELOG.md; do
    if [[ -f "${ROOT}/${doc}" ]]; then
      cp "${ROOT}/${doc}" "${dir}/${doc}"
    fi
  done
}

write_sums() {
  local dir="$1" file
  rm -f "${dir}/plugin.sums" "${dir}/plugin.sums.minisig"
  (
    cd "${dir}"
    find . -type f | sed 's|^\./||' | LC_ALL=C sort | while IFS= read -r file; do
      printf '%s  %s\n' "$(sha256_hex "${file}")" "${file}"
    done
  ) >"${dir}.sums"
  mv "${dir}.sums" "${dir}/plugin.sums"
}

sign_sums() {
  local dir="$1"
  if [[ -z "${MINISIGN_KEY}" ]]; then
    return 0
  fi
  (
    cd "${dir}"
    if [[ -n "${MINISIGN_PASSWORD}" ]]; then
      printf '%s\n' "${MINISIGN_PASSWORD}" \
        | minisign -S -m plugin.sums -x plugin.sums.minisig -s "${MINISIGN_KEY}" -t "${PLUGIN_ID} ${VERSION}"
    else
      minisign -S -m plugin.sums -x plugin.sums.minisig -s "${MINISIGN_KEY}" -t "${PLUGIN_ID} ${VERSION}"
    fi
  )
}

package_dir() {
  local dir="$1" archive="$2"
  local entries=(plugin.json plugin.sums)
  for entry in plugin.sums.minisig README.md LICENSE CHANGELOG.md server webapp; do
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
if [[ -z "${MINISIGN_KEY}" ]]; then
  echo "MINISIGN_KEY is not set, the packages are unsigned"
fi

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

  write_sums "${dir}"
  sign_sums "${dir}"
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
