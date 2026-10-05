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

## `sample-rotated.mp4`

`sample-h264-aac.mp4` copied with a display matrix that turns it a quarter clockwise, as a
portrait phone video is stored (ffprobe reports `rotation=-90`):

```powershell
ffmpeg -hide_banner -y -display_rotation:v:0 -90 -i testdata/sample-h264-aac.mp4 -c copy -map_metadata -1 -fflags +bitexact testdata/sample-rotated.mp4
```

## `photo.png` and `photo-turned.jpg`

One frame of `testsrc2` (320x240) as an RGB PNG, and as a full-range JPEG carrying EXIF
orientation 6 (turn a quarter clockwise), for the still image path. FFmpeg cannot write EXIF,
so `scripts/exif-orientation.py` adds it:

```powershell
ffmpeg -hide_banner -y -f lavfi -i "testsrc2=size=320x240:rate=1" -frames:v 1 -pix_fmt rgb24 -map_metadata -1 -fflags +bitexact -flags:v +bitexact testdata/photo.png
ffmpeg -hide_banner -y -f lavfi -i "testsrc2=size=320x240:rate=1" -frames:v 1 -c:v mjpeg -q:v 3 -pix_fmt yuvj420p -map_metadata -1 -fflags +bitexact -flags:v +bitexact $env:TEMP\dusk-photo.jpg
python scripts/exif-orientation.py $env:TEMP\dusk-photo.jpg testdata/photo-turned.jpg 6
```

## `photo-p3.jpg`

The same frame as `photo.png` as a JPEG with an embedded Display P3 ICC profile (FFmpeg's
`iccgen` filter), for reading primaries through FFmpeg's ICC support:

```powershell
ffmpeg -hide_banner -y -f lavfi -i "testsrc2=size=320x240:rate=1" -frames:v 1 -vf "format=yuvj420p,iccgen=color_primaries=smpte432:color_trc=iec61966-2-1:force=1" -c:v mjpeg -q:v 3 -map_metadata -1 -fflags +bitexact -flags:v +bitexact testdata/photo-p3.jpg
```

## `chirp.wav`

One second of a sine sweeping up from 200 Hz (48 kHz mono, 16-bit PCM). PCM decodes and seeks
to the exact sample, so mixing it backwards can be checked against mixing it forwards, and
WAV leaves the channel order unspecified, which decoding has to cope with:

```powershell
ffmpeg -hide_banner -y -f lavfi -i "aevalsrc=0.5*sin(2*PI*t*(200+400*t)):s=48000:d=1" -c:a pcm_s16le -ac 1 -map_metadata -1 -fflags +bitexact -flags:a +bitexact testdata/chirp.wav
```
