#!/bin/bash
# Gathers the source of every library built into Dusk's FFmpeg DLLs, which the LGPL asks to be
# offered beside them (docs/SETUP.md, "Changing the FFmpeg pin"): each library at the commit
# BtbN's scripts download, by the script's own ffbuild_dockerdl, run as BtbN's download.sh runs
# it but in the image the build used, without version control metadata; and BtbN's scripts
# themselves, which say how each was configured and built. Which libraries are built in is
# scripts/ffmpeg-built-in.py's answer, the one the license file is written from.
#
#   scripts/ffmpeg-sources.sh <image digest> <BtbN scripts checkout> <output dir>
#
# Runs on Linux with Docker and Python (CI's ffmpeg workflow). Writes
# ffmpeg-libraries-source.tar.xz.
set -euo pipefail
shopt -s inherit_errexit

digest="$1"
btbn="$(cd "$2" && pwd)"
out="$(mkdir -p "$3" && cd "$3" && pwd)"
here="$(cd "$(dirname "$0")" && pwd)"
image="ghcr.io/btbn/ffmpeg-builds/win64-lgpl-shared-8.1@${digest}"

scripts=$(python3 "$here/ffmpeg-built-in.py" "$btbn" 8.1)
work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
mkdir -p "$work/steps" "$work/sources/libraries" "$work/sources/FFmpeg-Builds"
for script in $scripts; do
    # libraries/45-libvorbis, libraries/50-librsvg/10-glib: the script's own path.
    dir="${script#scripts.d/}"
    dir="${dir%.sh}"
    cat >"$work/steps/${dir//\//_}.sh" <<EOF
set -euo pipefail
shopt -s dotglob
source /dl_functions.sh
source "/$script"
step="\$(ffbuild_dockerdl)"
# A script that downloads nothing (15-base) gets no folder, as in BtbN's download.sh.
[[ -n "\$step" ]] || exit 0
echo "::group::$dir"
mkdir -p "/sources/libraries/$dir"
cd "/sources/libraries/$dir"
eval "set -e; \$step"
echo "::endgroup::"
EOF
done

docker pull "$image"
docker run --rm -i -u "$(id -u):$(id -g)" -v "$work/steps":/steps:ro -v "$work/sources":/sources \
    -v "$btbn/scripts.d":/scripts.d:ro -v "$btbn/util/dl_functions.sh":/dl_functions.sh:ro \
    "$image" bash -c 'set -e; for step in /steps/*.sh; do bash "$step" || { echo "Downloading ${step##*/} failed" >&2; exit 1; }; done'

tar --exclude-vcs -C "$btbn" -cf - . | tar -xf - -C "$work/sources/FFmpeg-Builds"
cat >"$work/sources/README.txt" <<EOF
The source of every library built into Dusk's FFmpeg DLLs, at the commits BtbN's
FFmpeg-Builds scripts download. libraries/<name> holds what FFmpeg-Builds/scripts.d/<name>.sh
downloads, without version control metadata, and that script says how the library was
configured and built, in the image
$image.
A script that applies patches with git am (aribb24's, from FFmpeg-Builds/patches) needs the
folder made a repository first (git init && git add -A && git commit -m source), or the
patches applied with git apply.
FFmpeg's own source is the ffmpeg-source-<commit>.tar.gz published beside this file.
EOF
tar --exclude-vcs -C "$work/sources" -cf - . | xz -T0 >"$out/ffmpeg-libraries-source.tar.xz"
echo "wrote $out/ffmpeg-libraries-source.tar.xz ($(stat -c %s "$out/ffmpeg-libraries-source.tar.xz") bytes)"
