#!/usr/bin/env bash
# docs/build.sh — copy .omne/cfg/*.md into docs/src/ then run `mdbook build`.
#
# Rationale: mdBook's `src` must live inside the book root, and symlinks are
# unreliable on Windows. We copy at build time so the SSOT stays at
# `.omne/cfg/*.md` and `docs/src/*.md` is a regenerable build artefact.
# `docs/src/SUMMARY.md` and `docs/src/introduction.md` are authored directly
# (not copies) — the copy step preserves them.
set -euo pipefail

HERE="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd -- "${HERE}/.." && pwd)"
CFG_DIR="${REPO_ROOT}/.omne/cfg"
SRC_DIR="${HERE}/src"

if [ ! -d "${CFG_DIR}" ]; then
    echo "error: ${CFG_DIR} not found" >&2
    exit 1
fi

mkdir -p "${SRC_DIR}"

# Copy every *.md from .omne/cfg/ EXCEPT README.md (the index is replaced by
# SUMMARY.md in the mdBook world) and pack-template.md (authored directly
# under docs/src/, not sourced from .omne/cfg/). Authored files under
# docs/src/ (SUMMARY.md, introduction.md, pack-template.md) are never
# overwritten because their names do not exist under .omne/cfg/.
for src in "${CFG_DIR}"/*.md; do
    base="$(basename "${src}")"
    if [ "${base}" = "README.md" ] || [ "${base}" = "pack-template.md" ]; then
        continue
    fi
    dest="${SRC_DIR}/${base}"
    cp -f "${src}" "${dest}"
    # Prepend AUTO-GENERATED banner at build time so the copy trail is obvious
    # when a reader lands on the generated file in docs/src/.
    banner="<!-- AUTO-GENERATED from .omne/cfg/${base}. DO NOT EDIT HERE. Edit the source and re-run build.sh / build.ps1. -->"
    tmp="${dest}.tmp"
    { printf '%s\n\n' "${banner}"; cat "${dest}"; } > "${tmp}"
    mv -f "${tmp}" "${dest}"
done

cd "${HERE}"

if ! command -v mdbook >/dev/null 2>&1; then
    echo "error: mdbook not on PATH. Install with: cargo install mdbook --locked" >&2
    exit 127
fi

exec mdbook build
