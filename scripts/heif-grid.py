"""Writes a HEIC photo stored as a grid of HEVC tiles, as iPhones store theirs.

FFmpeg reads such files but cannot write them, so the test data is assembled here from tiles
FFmpeg encodes: each tile an HEVC still in Annex B form (`-f hevc`), all the same size, given
in row-major order.

    python scripts/heif-grid.py OUT.heic COLUMNS ROWS TILE_WIDTH TILE_HEIGHT WIDTH HEIGHT TURNS ICC TILE.hevc...

WIDTH and HEIGHT are the picture's size, cropped from the tiles' top-left corner as iPhones
crop 4096x3072 of tiles to 4032x3024; TURNS is the `irot` property, quarter turns
anticlockwise (0 for none); ICC is a JPEG whose embedded color profile the grid gets in a
`colr` property, as iPhones give theirs Display P3, or `-` for none.
"""

import struct
import sys


def box(kind, payload):
    return struct.pack(">I4s", 8 + len(payload), kind) + payload


def full_box(kind, version, flags, payload):
    return box(kind, struct.pack(">I", (version << 24) | flags) + payload)


def nal_units(annex_b):
    """The NAL units of an Annex B stream, without their start codes."""
    units, start, i = [], None, 0
    while i + 3 <= len(annex_b):
        if annex_b[i:i + 3] == b"\x00\x00\x01":
            if start is not None:
                units.append(annex_b[start:i].rstrip(b"\x00"))
            start = i = i + 3
        else:
            i += 1
    if start is not None:
        units.append(annex_b[start:])
    return [unit for unit in units if unit]


def nal_type(unit):
    return (unit[0] >> 1) & 0x3F


def jpeg_icc_profile(jpeg):
    """The ICC profile a JPEG carries in its APP2 segments, put together."""
    chunks, i = {}, 2
    while i + 4 <= len(jpeg) and jpeg[i] == 0xFF:
        marker, length = jpeg[i + 1], struct.unpack(">H", jpeg[i + 2:i + 4])[0]
        segment = jpeg[i + 4:i + 2 + length]
        if marker == 0xE2 and segment.startswith(b"ICC_PROFILE\x00"):
            chunks[segment[12]] = segment[14:]
        if marker == 0xDA:
            break
        i += 2 + length
    assert chunks, "no ICC profile in the JPEG"
    return b"".join(chunks[n] for n in sorted(chunks))


def hvcc(parameter_sets):
    """An HEVCDecoderConfigurationRecord: Main profile, 4-byte NAL lengths, and the VPS, SPS
    and PPS. FFmpeg reads the lengths and the arrays; the rest is plausible filler."""
    record = bytes([1, 0x01]) + struct.pack(">I", 0x60000000) + bytes(6) + bytes([93])
    record += struct.pack(">H", 0xF000) + bytes([0xFC, 0xFD, 0xF8, 0xF8])
    record += struct.pack(">H", 0) + bytes([0x0F, len(parameter_sets)])
    for unit in parameter_sets:
        record += bytes([0x80 | nal_type(unit)]) + struct.pack(">HH", 1, len(unit)) + unit
    return box(b"hvcC", record)


def main():
    out, *numbers = sys.argv[1:9]
    icc_source = sys.argv[9]
    tiles = sys.argv[10:]
    columns, rows, tile_width, tile_height, width, height, turns = map(int, numbers)
    profile = jpeg_icc_profile(open(icc_source, "rb").read()) if icc_source != "-" else None
    assert len(tiles) == columns * rows, "one tile per grid cell"
    assert columns * tile_width >= width and rows * tile_height >= height, "tiles cover it"
    streams = [nal_units(open(path, "rb").read()) for path in tiles]
    # Parameter sets (VPS 32, SPS 33, PPS 34) go in the shared hvcC; the slices are the data.
    parameter_sets = [u for u in streams[0] if nal_type(u) in (32, 33, 34)]
    samples = [b"".join(struct.pack(">I", len(u)) + u for u in units if nal_type(u) < 32)
               for units in streams]

    grid_id = len(tiles) + 1
    grid_data = struct.pack(">BBBBHH", 0, 0, rows - 1, columns - 1, width, height)

    def meta(offsets):
        hdlr = full_box(b"hdlr", 0, 0, bytes(4) + b"pict" + bytes(12) + b"\x00")
        pitm = full_box(b"pitm", 0, 0, struct.pack(">H", grid_id))
        entries = [full_box(b"infe", 2, 0, struct.pack(">HH4s", item, 0, b"hvc1") + b"\x00")
                   for item in range(1, grid_id)]
        entries.append(full_box(b"infe", 2, 0, struct.pack(">HH4s", grid_id, 0, b"grid") + b"\x00"))
        iinf = full_box(b"iinf", 0, 0, struct.pack(">H", len(entries)) + b"".join(entries))
        locations = b""
        for item, (offset, length) in enumerate(offsets, start=1):
            locations += struct.pack(">HHHII", item, 0, 1, offset, length)
        iloc = full_box(b"iloc", 0, 0, bytes([0x44, 0x00]) + struct.pack(">H", len(offsets)) + locations)
        dimg = box(b"dimg", struct.pack(">HH", grid_id, len(tiles))
                   + b"".join(struct.pack(">H", item) for item in range(1, grid_id)))
        iref = full_box(b"iref", 0, 0, dimg)
        properties = [hvcc(parameter_sets),
                      full_box(b"ispe", 0, 0, struct.pack(">II", tile_width, tile_height)),
                      full_box(b"ispe", 0, 0, struct.pack(">II", width, height))]
        grid_properties = [3]
        if profile:
            properties.append(box(b"colr", b"prof" + profile))
            grid_properties.append(len(properties))
        if turns:
            properties.append(box(b"irot", bytes([turns & 3])))
            grid_properties.append(0x80 | len(properties))
        ipco = box(b"ipco", b"".join(properties))
        associations = b""
        for item in range(1, grid_id):
            associations += struct.pack(">HB", item, 2) + bytes([0x80 | 1, 2])
        associations += struct.pack(">HB", grid_id, len(grid_properties)) + bytes(grid_properties)
        ipma = full_box(b"ipma", 0, 0, struct.pack(">I", grid_id) + associations)
        iprp = box(b"iprp", ipco + ipma)
        return full_box(b"meta", 0, 0, hdlr + pitm + iinf + iloc + iref + iprp)

    ftyp = box(b"ftyp", b"heic" + struct.pack(">I", 0) + b"mif1heic")
    payloads = samples + [grid_data]
    lengths = [len(p) for p in payloads]
    placeholder = meta([(0, n) for n in lengths])
    start = len(ftyp) + len(placeholder) + 8
    offsets, position = [], start
    for n in lengths:
        offsets.append((position, n))
        position += n
    final_meta = meta(offsets)
    assert len(final_meta) == len(placeholder)
    with open(out, "wb") as f:
        f.write(ftyp + final_meta + box(b"mdat", b"".join(payloads)))


if __name__ == "__main__":
    main()
