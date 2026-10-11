#!/bin/bash
# Builds Dusk's own FFmpeg (docs/ROADMAP.md, M7; docs/DECISIONS.md, "An FFmpeg of Dusk's
# own"): FFmpeg at one commit, compiled in BtbN's win64 LGPL shared 8.1 image at one digest,
# which holds every library already built, with BtbN's configure options as
# scripts/ffmpeg-options.txt changes them. Packaged as BtbN packages its builds, so
# scripts/setup-ffmpeg.ps1 takes it as it takes theirs.
#
#   scripts/ffmpeg-build.sh <ffmpeg commit> <image digest> <BtbN scripts checkout> <output dir>
#
# Runs on Linux with Docker (CI's ffmpeg workflow). Writes the zip and its SHA-256, FFmpeg's
# source at that commit, the configure line, and the lists of what it enabled.
set -euo pipefail
shopt -s inherit_errexit

commit="$1"
digest="$2"
btbn="$(cd "$3" && pwd)"
out="$(mkdir -p "$4" && cd "$4" && pwd)"
here="$(cd "$(dirname "$0")" && pwd)"
image="ghcr.io/btbn/ffmpeg-builds/win64-lgpl-shared-8.1@${digest}"

drops=$(grep '^drop ' "$here/ffmpeg-options.txt" | cut -d' ' -f2 | tr '\n' ' ')
adds=$(grep '^add ' "$here/ffmpeg-options.txt" | cut -d' ' -f2- | tr '\n' ' ')
# Every component the options ask for by name, as configure's lists name them.
wanted=""
for option in $adds; do
    case "$option" in
    --enable-encoder=* | --enable-decoder=* | --enable-muxer=* | --enable-demuxer=* | \
        --enable-protocol=* | --enable-parser=* | --enable-bsf=*)
        kind="${option%%=*}"
        kind="${kind#--enable-}"
        ;;
    --enable-*=*) echo "The build cannot check $option; add its kind to scripts/ffmpeg-build.sh" >&2; exit 1 ;;
    *) continue ;;
    esac
    list="${option#*=}"
    for name in ${list//,/ }; do
        wanted="$wanted ff_${name}_${kind}"
    done
done

work="$(mktemp -d)"
trap 'rm -rf -- "$work"' EXIT
git clone --quiet --filter=blob:none --no-checkout https://github.com/FFmpeg/FFmpeg.git "$work/ffmpeg"
git -C "$work/ffmpeg" -c advice.detachedHead=false checkout --quiet "$commit"
git -C "$work/ffmpeg" archive --format=tar.gz --prefix="ffmpeg-$commit/" \
    -o "$out/ffmpeg-source-$commit.tar.gz" "$commit"

docker pull "$image"
cat >"$work/build.sh" <<EOF
set -euo pipefail
cd /ffbuild/ffmpeg
kept=""
for option in \$FF_CONFIGURE; do
    case " $drops " in
    *" \$option "*) ;;
    *) kept="\$kept \$option" ;;
    esac
done
# A drop that names nothing BtbN enables means this list has gone stale.
for option in $drops; do
    case " \$FF_CONFIGURE " in
    *" \$option "*) ;;
    *) echo "BtbN's build does not have \$option; update scripts/ffmpeg-options.txt" >&2; exit 1 ;;
    esac
done
echo "\$kept $adds" >/ffbuild/configure-options.txt
./configure --prefix=/ffbuild/prefix --pkg-config-flags="--static" \$FFBUILD_TARGET_FLAGS \$kept $adds \\
    --extra-cflags="\$FF_CFLAGS" --extra-cxxflags="\$FF_CXXFLAGS" --extra-libs="\$FF_LIBS" \\
    --extra-ldflags="\$FF_LDFLAGS" --extra-ldexeflags="\$FF_LDEXEFLAGS" \\
    --cc="\$CC" --cxx="\$CXX" --ar="\$AR" --ranlib="\$RANLIB" --nm="\$NM" \\
    --extra-version=dusk || { cat ffbuild/config.log; exit 1; }
# configure passes over a name it does not know without a word (mov_text, where its list
# says movtext), and leaves out a component whose library is missing, so each one asked for
# must be in its lists.
for symbol in $wanted; do
    grep -qw "\$symbol" libavcodec/codec_list.c libavcodec/parser_list.c libavcodec/bsf_list.c \
        libavformat/demuxer_list.c libavformat/muxer_list.c libavformat/protocol_list.c ||
        { echo "configure did not enable \$symbol; check its name in scripts/ffmpeg-options.txt" >&2; exit 1; }
done
make -j\$(nproc)
make install install-doc
cp libavcodec/codec_list.c libavformat/demuxer_list.c libavformat/muxer_list.c \\
    libavformat/protocol_list.c /ffbuild/
EOF
docker run --rm -i -u "$(id -u):$(id -g)" -v "$work":/ffbuild "$image" bash /ffbuild/build.sh

# Packaged as BtbN's own win64 shared builds are.
# shellcheck source=/dev/null
source "$btbn/variants/windows-install-shared.sh"
version=$("$work/ffmpeg/ffbuild/version.sh" "$work/ffmpeg")
name="ffmpeg-${version}-win64-lgpl-shared-8.1-dusk"
package_variant "$work/prefix" "$work/pkg/$name"
cp "$work/ffmpeg/COPYING.LGPLv3" "$work/pkg/$name/LICENSE.txt"
(cd "$work/pkg" && zip -9 -qr "$out/$name.zip" "$name")
(cd "$out" && sha256sum "$name.zip" >"$name.zip.sha256")
mkdir -p "$out/lists"
cp "$work"/{codec_list.c,demuxer_list.c,muxer_list.c,protocol_list.c,configure-options.txt} "$out/lists/"
echo "$image" >"$out/image.txt"
echo "built $out/$name.zip"
