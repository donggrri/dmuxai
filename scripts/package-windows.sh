#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

cd "${REPO_ROOT}"

# Check for MinGW cross compiler (gcc-mingw-w64-x86-64)
if ! command -v x86_64-w64-mingw32-gcc >/dev/null 2>&1; then
    echo "Error: MinGW cross-compiler (gcc-mingw-w64-x86-64) is required for x86_64-pc-windows-gnu target." >&2
    echo "Please install it with: sudo apt-get install -y gcc-mingw-w64-x86-64" >&2
    exit 1
fi

# Check for zip utility
if ! command -v zip >/dev/null 2>&1; then
    echo "Error: 'zip' utility is required to package release archive." >&2
    echo "Please install it with: sudo apt-get install -y zip" >&2
    exit 1
fi

echo "Building Windows binaries with cargo for target x86_64-pc-windows-gnu..."
cargo build --release --target x86_64-pc-windows-gnu --bin dmuxai --bin dmuxai-launcher

STAGE_DIR="${REPO_ROOT}/target/windows-staging"
DIST_DIR="${REPO_ROOT}/dist"
ARCHIVE_NAME="dmuxai-windows-x86_64.zip"

rm -rf "${STAGE_DIR}"
mkdir -p "${STAGE_DIR}" "${DIST_DIR}"

echo "Staging release artifacts..."
cp "${REPO_ROOT}/target/x86_64-pc-windows-gnu/release/dmuxai.exe" "${STAGE_DIR}/"
cp "${REPO_ROOT}/target/x86_64-pc-windows-gnu/release/dmuxai-launcher.exe" "${STAGE_DIR}/"
cp "${REPO_ROOT}/packaging/windows/install.ps1" "${STAGE_DIR}/"
cp "${REPO_ROOT}/packaging/windows/uninstall.ps1" "${STAGE_DIR}/"
cp "${REPO_ROOT}/packaging/windows/README-windows.txt" "${STAGE_DIR}/"

echo "Creating ${DIST_DIR}/${ARCHIVE_NAME}..."
rm -f "${DIST_DIR}/${ARCHIVE_NAME}" "${DIST_DIR}/${ARCHIVE_NAME}.sha256"

(
    cd "${STAGE_DIR}"
    zip -q -r "${DIST_DIR}/${ARCHIVE_NAME}" \
        dmuxai.exe \
        dmuxai-launcher.exe \
        install.ps1 \
        uninstall.ps1 \
        README-windows.txt
)

echo "Generating SHA256 checksum..."
(
    cd "${DIST_DIR}"
    if command -v sha256sum >/dev/null 2>&1; then
        sha256sum "${ARCHIVE_NAME}" > "${ARCHIVE_NAME}.sha256"
    elif command -v shasum >/dev/null 2>&1; then
        shasum -a 256 "${ARCHIVE_NAME}" > "${ARCHIVE_NAME}.sha256"
    else
        echo "Warning: neither sha256sum nor shasum found; checksum file not created." >&2
    fi
)

echo "Packaged: ${DIST_DIR}/${ARCHIVE_NAME}"
if [ -f "${DIST_DIR}/${ARCHIVE_NAME}.sha256" ]; then
    echo "Checksum: $(cat "${DIST_DIR}/${ARCHIVE_NAME}.sha256")"
fi
