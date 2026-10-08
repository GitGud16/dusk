"""Copies a JPEG, adding an EXIF segment that says how to turn it upright.

    python scripts/exif-orientation.py in.jpg out.jpg 6

The tag is the EXIF orientation, 1 to 8 (6: turn a quarter clockwise). Used to make the test
photos in testdata/ (see testdata/README.md); FFmpeg cannot write EXIF itself.
"""

import struct
import sys


def with_orientation(jpeg: bytes, tag: int) -> bytes:
    if jpeg[:2] != b"\xff\xd8":
        raise SystemExit("not a JPEG")
    # A little-endian TIFF header, then IFD0 with one entry: Orientation (0x0112), SHORT,
    # count 1, the value, and no next IFD.
    tiff = b"II*\x00" + struct.pack("<I", 8)
    tiff += struct.pack("<H", 1) + struct.pack("<HHIHH", 0x0112, 3, 1, tag, 0)
    tiff += struct.pack("<I", 0)
    payload = b"Exif\x00\x00" + tiff
    segment = b"\xff\xe1" + struct.pack(">H", len(payload) + 2) + payload
    return jpeg[:2] + segment + jpeg[2:]


if __name__ == "__main__":
    source, target, tag = sys.argv[1], sys.argv[2], int(sys.argv[3])
    with open(source, "rb") as file:
        jpeg = file.read()
    with open(target, "wb") as file:
        file.write(with_orientation(jpeg, tag))
