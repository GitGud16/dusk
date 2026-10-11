# The exact FFmpeg build Dusk links against. Single source of truth for
# scripts/setup-ffmpeg.ps1 and scripts/dev-env.ps1; docs/SETUP.md mirrors it.
# Changing any value here is a documented decision (see docs/ARCHITECTURE.md, "FFmpeg (Windows)").
@{
    # Dusk's own FFmpeg build, a release asset of Dusk's repository that stays as long as the
    # repository does: built by .github/workflows/ffmpeg.yml (scripts/ffmpeg-build.sh) from
    # what scripts/ffmpeg-source.txt names, FFmpeg n8.1.3-14-g330caae0c1 in BtbN's 8.1 image.
    Tag      = 'ffmpeg-8.1-dusk-1'
    FileName = 'ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-shared-8.1-dusk.zip'
    Url      = 'https://github.com/GitGud16/dusk/releases/download/ffmpeg-8.1-dusk-1/ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-shared-8.1-dusk.zip'
    Size     = 40504004
    Sha256   = '249d1f8c63cd2c4d8320080cde5ab7b6581df186ca7d4dfed0ad9b2a0b81fd5a'

    # Configure flags the build must contain (checked in avutil's embedded configuration
    # string, without running any FFmpeg executable). --disable-network is Dusk's build's.
    RequiredConfig  = @('--enable-lcms2', '--enable-libopenh264', '--enable-libkvazaar', '--enable-libsvtav1', '--enable-libvpx', '--enable-version3', '--disable-network')
    # Flags that must be absent: no GPL code in the default build.
    ForbiddenConfig = @('--enable-gpl', '--enable-libx264', '--enable-libx265')

    # The five DLLs Dusk ships (name prefixes; the suffix is the library major version).
    ShipDlls = @('avcodec-', 'avformat-', 'avutil-', 'swscale-', 'swresample-')
}
