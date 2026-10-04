# Test data

Synthetic files generated with the pinned FFmpeg build (`scripts/ffmpeg-pin.psd1`); no
third-party content, so they fall under the repository's MIT license.

## `sample-h264-aac.mp4`

One second of FFmpeg's `testsrc2` pattern (320x240, 30 fps, H.264 via `libopenh264`) and a
440 Hz tone (48 kHz mono, AAC). Regenerate from a shell that ran `scripts\dev-env.ps1`:

```powershell
ffmpeg -hide_banner -y -f lavfi -i "testsrc2=size=320x240:rate=30:duration=1" -f lavfi -i "sine=frequency=440:sample_rate=48000:duration=1" -c:v libopenh264 -b:v 300k -g 30 -pix_fmt yuv420p -c:a aac -b:a 64k -ac 1 -shortest -movflags +faststart -map_metadata -1 -fflags +bitexact -flags:v +bitexact -flags:a +bitexact testdata/sample-h264-aac.mp4
```

## `sample-vp9-10bit.webm`

Half a second of `testsrc2` (320x240, 30 fps) as 10-bit VP9 (profile 2, `yuv420p10le`, via `libvpx-vp9`), for the P010 decoding path:

```powershell
ffmpeg -hide_banner -y -f lavfi -i "testsrc2=size=320x240:rate=30:duration=0.5" -c:v libvpx-vp9 -pix_fmt yuv420p10le -profile:v 2 -b:v 200k -g 15 -row-mt 0 -threads 1 -map_metadata -1 -fflags +bitexact -flags:v +bitexact testdata/sample-vp9-10bit.webm
```
