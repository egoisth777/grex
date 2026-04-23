#!/usr/bin/env bash
# docs/build.sh — assemble docs/src/ then run `mdbook build`.
#
# Chapter sources (priority order):
#   1. ${REPO_ROOT}/.omne/cfg/*.md   — private design-doc SSOT (local only).
#   2. ${REPO_ROOT}/docs/src-authored/*.md — vendored fallback (git-tracked).
#
# On a fresh CI checkout `.omne/cfg/` is absent (gitignored at the grex repo
# level), so the vendored copies under `docs/src-authored/` are used instead.
# Locally, `.omne/cfg/` wins and also refreshes the vendored copies so the
# tree stays in step with the SSOT.
#
# Authored chapters (`SUMMARY.md`, `introduction.md`, `pack-template.md`) live
# directly under `docs/src/` and are never overwritten because those filenames
# do not appear in either source dir.
set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${HERE}/.." && pwd)"
CFG_DIR="${REPO_ROOT}/.omne/cfg"
VENDOR_DIR="${HERE}/src-authored"
SRC_DIR="${HERE}/src"

mkdir -p "${SRC_DIR}"

# Resolve which chapter source wins for this build.
if [ -d "${CFG_DIR}" ]; then
    SOURCE_DIR="${CFG_DIR}"
    SOURCE_LABEL=".omne/cfg"
    echo "info: chapter source = ${CFG_DIR}" >&2
elif [ -d "${VENDOR_DIR}" ]; then
    SOURCE_DIR="${VENDOR_DIR}"
    SOURCE_LABEL="docs/src-authored"
    echo "info: chapter source = ${VENDOR_DIR} (vendored fallback — .omne/cfg/ not present)" >&2
else
    echo "error: neither ${CFG_DIR} nor ${VENDOR_DIR} exists — cannot assemble docs/src/" >&2
    exit 1
fi

# Copy every *.md EXCEPT README.md (the index is replaced by SUMMARY.md in
# the mdBook world) and pack-template.md (authored directly under docs/src/).
for src in "${SOURCE_DIR}"/*.md; do
    base="$(basename "${src}")"
    if [ "${base}" = "README.md" ] || [ "${base}" = "pack-template.md" ]; then
        continue
    fi
    dest="${SRC_DIR}/${base}"
    cp -f "${src}" "${dest}"
    # Prepend AUTO-GENERATED banner at build time so the copy trail is obvious
    # when a reader lands on the generated file in docs/src/.
    banner="<!-- AUTO-GENERATED from ${SOURCE_LABEL}/${base}. DO NOT EDIT HERE. Edit the source and re-run build.sh / build.ps1. -->"
    tmp="${dest}.tmp"
    { printf '%s\n\n' "${banner}"; cat "${dest}"; } > "${tmp}"
    mv -f "${tmp}" "${dest}"
done

# When .omne/cfg/ is the winning source, keep the vendored fallback in step
# with the SSOT so CI checkouts (where .omne/cfg/ is absent) see the same
# content on the next build.
if [ "${SOURCE_DIR}" = "${CFG_DIR}" ] && [ -d "${VENDOR_DIR}" ]; then
    for src in "${CFG_DIR}"/*.md; do
        base="$(basename "${src}")"
        if [ "${base}" = "README.md" ] || [ "${base}" = "pack-template.md" ]; then
            continue
        fi
        cp -f "${src}" "${VENDOR_DIR}/${base}"
    done
fi

cd "${HERE}"

if ! command -v mdbook >/dev/null 2>&1; then
    echo "error: mdbook not on PATH. Install with: cargo install mdbook --locked" >&2
    exit 127
fi

exec mdbook build
