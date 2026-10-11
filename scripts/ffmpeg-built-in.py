"""Which libraries Dusk's FFmpeg build contains, by BtbN's own scripts: the libraries whose
configure options scripts/ffmpeg-options.txt keeps, and every library those depend on, dropped
or not, since a library a kept one links is in the DLLs too. scripts/ffmpeg-licenses.py lists
their licenses from this answer, and scripts/ffmpeg-sources.sh gathers their sources from it:

    python scripts/ffmpeg-built-in.py <BtbN FFmpeg-Builds checkout> <FFmpeg version>

prints the scripts whose libraries the build contains, one a line. Needs bash (Git's on
Windows); Python's standard library otherwise.
"""

import re
import shutil
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent


def bash():
    """Git's bash on Windows, where the bash.exe found first is WSL's; bash elsewhere."""
    git = shutil.which("git")
    if sys.platform == "win32" and git:
        # git.exe sits in Git's cmd, bin or mingw64\bin folder.
        for folder in list(Path(git).parents)[:3]:
            for candidate in ("bin/bash.exe", "usr/bin/bash.exe"):
                if (folder / candidate).exists():
                    return str(folder / candidate)
    return "bash"


def stage(script):
    """The stage a script of BtbN's belongs to, by the name other scripts depend on it by: a
    folder of scripts is one stage."""
    path = Path(script)
    unit = path.parent if path.parent.name != "scripts.d" else path.with_suffix("")
    return unit.name.split("-", 1)[1]


def dropped_options():
    """The options of BtbN's that Dusk's build drops (scripts/ffmpeg-options.txt)."""
    options = (ROOT / "scripts" / "ffmpeg-options.txt").read_text(encoding="utf-8")
    return set(re.findall(r"^drop (\S+)$", options, re.MULTILINE))


def built_in(builds, version, dropped):
    """The rows of scripts/btbn-components.sh for every script whose library the build contains:
    the stages that give FFmpeg an option `dropped` does not hold, and every stage those depend
    on (librsvg brings fontconfig, HarfBuzz and FriBidi; libopenmpt brings libvorbis;
    libaribcaption brings OpenSSL's libcrypto)."""
    lister = ROOT / "scripts" / "btbn-components.sh"
    run = subprocess.run(
        [bash(), lister.as_posix(), Path(builds).as_posix(), "win64", "lgpl-shared", version],
        capture_output=True, text=True, encoding="utf-8",
    )
    if run.returncode != 0:
        sys.exit(f"Listing BtbN's components failed: {run.stderr.strip()}")
    rows = [line.split("\t") for line in run.stdout.splitlines() if line.strip()]
    enabled = {row[0] for row in rows}
    stages = {}
    for row in rows:
        # A folder of scripts builds one library and what only it uses; when its last script is
        # off (libcurl in FFmpeg 8.1), nothing of it reaches FFmpeg.
        folder = Path(builds) / Path(row[0]).parent
        if folder.name != "scripts.d" and sorted(folder.glob("??-*.sh"))[-1].relative_to(builds).as_posix() not in enabled:
            continue
        stages.setdefault(stage(row[0]), []).append(row)
    wanted = [
        name for name, scripts in stages.items()
        if any(option not in dropped for row in scripts for option in row[9].split())
    ]
    kept = set()
    while wanted:
        name = wanted.pop()
        # A stage BtbN does not build for this target (xorg-macros) is in no row.
        if name in kept or name not in stages:
            continue
        kept.add(name)
        wanted.extend(dependency for row in stages[name] for dependency in row[10].split())
    # SCRIPT_SKIP only keeps BtbN's updater off a script; its library is built all the same.
    return [row for row in rows if stage(row[0]) in kept]


if __name__ == "__main__":
    if len(sys.argv) != 3:
        sys.exit("Usage: python scripts/ffmpeg-built-in.py <BtbN FFmpeg-Builds checkout> <FFmpeg version>")
    for row in built_in(Path(sys.argv[1]), sys.argv[2], dropped_options()):
        print(row[0])
