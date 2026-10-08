# The exact FFmpeg build Dusk links against. Single source of truth for
# scripts/setup-ffmpeg.ps1 and scripts/dev-env.ps1; docs/SETUP.md mirrors it.
# Changing any value here is a documented decision (see docs/ARCHITECTURE.md, "FFmpeg (Windows)").
@{
    # BtbN/FFmpeg-Builds release tag and asset. 8.1 builds have lcms2 since BtbN commit
    # 793eb5e7 (2026-10-02). BtbN keeps only its 14 newest builds, plus the last build of each
    # month for two years, so a daily build disappears about two weeks after it is published.
    # The pin moves along until it can rest on the last October 2026 build, on the dates in
    # docs/SETUP.md, "FFmpeg (pinned)": move 1 of 3, downloadable until about 2026-10-21.
    Tag      = 'autobuild-2026-10-07-13-07'
    FileName = 'ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-shared-8.1.zip'
    Url      = 'https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-07-13-07/ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-shared-8.1.zip'
    Size     = 80993041
    Sha256   = '30fdaaeb116730fdd6432c2663af0e9ecdf1d6cc387ac085cc1faf8494942510'

    # Configure flags the build must contain (checked in avutil's embedded configuration
    # string, without running any FFmpeg executable).
    RequiredConfig  = @('--enable-lcms2', '--enable-libopenh264', '--enable-libkvazaar', '--enable-libsvtav1', '--enable-libvpx', '--enable-version3')
    # Flags that must be absent: no GPL code in the default build.
    ForbiddenConfig = @('--enable-gpl', '--enable-libx264', '--enable-libx265')

    # The five DLLs Dusk ships (name prefixes; the suffix is the library major version).
    ShipDlls = @('avcodec-', 'avformat-', 'avutil-', 'swscale-', 'swresample-')
}
