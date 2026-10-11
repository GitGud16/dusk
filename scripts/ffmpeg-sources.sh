#!/bin/bash
# Gathers the source of every library built into Dusk's FFmpeg DLLs, which the LGPL asks to be
# offered beside them (docs/SETUP.md, "Changing the FFmpeg pin"): each library as BtbN's
# scripts download it for the build (the script's own ffbuild_dockerdl, run as BtbN's
# download.sh runs it, in the image the build used), and BtbN's scripts themselves, which say
# how each was configured and built. Which libraries are built in is
# scripts/ffmpeg-licenses.py's answer, the one the license file is written from.
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

scripts=$(python3 "$here/ffmpeg-licenses.py" --scripts "$btbn" 8.1)
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
mkdir -p "/sources/libraries/$dir"
cd "/sources/libraries/$dir"
eval "set -e; \$step"
EOF
done

docker pull "$image"
docker run --rm -i -u "$(id -u):$(id -g)" -v "$work/steps":/steps:ro -v "$work/sources":/sources \
    -v "$btbn/scripts.d":/scripts.d:ro -v "$btbn/util/dl_functions.sh":/dl_functions.sh:ro \
    "$image" bash -c 'set -e; for step in /steps/*.sh; do bash "$step"; done'

tar --exclude-vcs -C "$btbn" -cf - . | tar -xf - -C "$work/sources/FFmpeg-Builds"
cat >"$work/sources/README.txt" <<EOF
The source of every library built into Dusk's FFmpeg DLLs, as BtbN's FFmpeg-Builds scripts
download it for the build. libraries/<name> holds what FFmpeg-Builds/scripts.d/<name>.sh
downloads, and that script says how the library was configured and built, in the image
$image.
FFmpeg's own source is the ffmpeg-source-<commit>.tar.gz published beside this file.
EOF
tar --exclude-vcs -C "$work/sources" -cf - . | xz -T0 >"$out/ffmpeg-libraries-source.tar.xz"
echo "wrote $out/ffmpeg-libraries-source.tar.xz ($(stat -c %s "$out/ffmpeg-libraries-source.tar.xz") bytes)"
