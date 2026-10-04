# The exact FFmpeg build Dusk links against. Single source of truth for
# scripts/setup-ffmpeg.ps1 and scripts/dev-env.ps1; docs/SETUP.md mirrors it.
# Changing any value here is a documented decision (see docs/ARCHITECTURE.md, "FFmpeg (Windows)").
@{
    # BtbN/FFmpeg-Builds release tag and asset. This is the first 8.1 build with lcms2
    # (enabled in BtbN commit 793eb5e7, 2026-10-02). BtbN keeps only its 14 newest builds,
    # plus the last build of each month for two years, so this one disappears in about two
    # weeks. Move to the 8.1 asset of the last October 2026 build (normally
    # autobuild-2026-10-31-*) once it is published.
    Tag      = 'autobuild-2026-10-03-18-14'
    FileName = 'ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-shared-8.1.zip'
    Url      = 'https://github.com/BtbN/FFmpeg-Builds/releases/download/autobuild-2026-10-03-18-14/ffmpeg-n8.1.3-14-g330caae0c1-win64-lgpl-shared-8.1.zip'
    Size     = 80991898
    Sha256   = '11a4b44bc69721274909619a779d82544c8b83a6557c5b1be92dce9a41a968be'

    # Configure flags the build must contain (checked in avutil's embedded configuration
    # string, without running any FFmpeg executable).
    RequiredConfig  = @('--enable-lcms2', '--enable-libopenh264', '--enable-libkvazaar', '--enable-libsvtav1', '--enable-libvpx', '--enable-version3')
    # Flags that must be absent: no GPL code in the default build.
    ForbiddenConfig = @('--enable-gpl', '--enable-libx264', '--enable-libx265')

    # The five DLLs Dusk ships (name prefixes; the suffix is the library major version).
    ShipDlls = @('avcodec-', 'avformat-', 'avutil-', 'swscale-', 'swresample-')
}
