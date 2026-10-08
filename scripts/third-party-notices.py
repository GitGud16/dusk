"""Writes THIRD-PARTY-NOTICES.txt for the installer (docs/ARCHITECTURE.md, "Release (0.1)"):
the license of every Rust crate built into dusk.exe and dusq.exe for Windows, with the texts
each crate ships. Crates sharing one text are listed together above it.

    python scripts/third-party-notices.py <file to write>

Reads cargo's own view of the build (`cargo tree`, `cargo metadata`) without the network, and
the license files in each crate's source in cargo's registry. A crate that ships no license
file gets its license's standard text: MIT with its authors, and the others (Apache-2.0,
BSL-1.0, WTFPL) as another crate of the build ships them. It fails, naming them, when a
crate's license has no text it can give; Python's standard library only.
"""

import json
import re
import subprocess
import sys
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
TARGET = "x86_64-pc-windows-msvc"
# The packages of the two binaries: dusk.exe and dusq.exe.
BINARIES = ["dusk-app", "dusk-cli"]
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|unlicense|copyright)", re.IGNORECASE)

# How to know a license's standard text among the files other crates ship.
STANDARD = {
    "Apache-2.0": ("Apache License", "Version 2.0"),
    "BSL-1.0": ("Boost Software License - Version 1.0",),
    "MPL-2.0": ("Mozilla Public License Version 2.0",),
    "WTFPL": ("DO WHAT THE FUCK YOU WANT TO PUBLIC LICENSE",),
}

MIT = """Permission is hereby granted, free of charge, to any person obtaining a copy of this
software and associated documentation files (the "Software"), to deal in the Software
without restriction, including without limitation the rights to use, copy, modify, merge,
publish, distribute, sublicense, and/or sell copies of the Software, and to permit persons
to whom the Software is furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all copies or
substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR IMPLIED,
INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY, FITNESS FOR A PARTICULAR
PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE AUTHORS OR COPYRIGHT HOLDERS BE LIABLE
FOR ANY CLAIM, DAMAGES OR OTHER LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR
OTHERWISE, ARISING FROM, OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER
DEALINGS IN THE SOFTWARE.
"""


def cargo(*args, cwd=ROOT, env=None):
    return subprocess.run(
        ["cargo", *args], cwd=cwd, env=env, check=True, capture_output=True, text=True,
        encoding="utf-8",
    ).stdout


def linked_crates(roots, target, cwd, env, offline):
    """(name, version) of every crate the packages `roots`, (package, features) pairs, link
    for `target`: normal dependencies only, as each root is built on its own."""
    crates = set()
    for package, features in roots:
        args = ["tree", "-p", package, "-e", "normal", "--target", target, "--prefix", "none", "-f", "{p}"]
        if features:
            args += ["--features", ",".join(features)]
        if offline:
            args.append("--offline")
        for line in cargo(*args, cwd=cwd, env=env).splitlines():
            parts = line.split()
            if len(parts) >= 2 and parts[1].startswith("v"):
                crates.add((parts[0], parts[1][1:]))
    return crates


def normalized(text):
    """A license text with its line ends and trailing spaces made even, to tell copies apart
    from texts that differ."""
    lines = [line.rstrip() for line in text.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    return "\n".join(lines).strip() + "\n"


def license_files(package):
    folder = Path(package["manifest_path"]).parent
    files = [path for path in folder.iterdir() if path.is_file() and LICENSE_FILE.match(path.name)]
    licenses = folder / "LICENSES"
    if licenses.is_dir():
        files += [path for path in licenses.iterdir() if path.is_file()]
    return sorted(files, key=lambda path: path.name.lower())


def crate_texts(roots, target, cwd=ROOT, env=None, offline=True):
    """The crates the packages `roots` ((package, features) pairs) of the Cargo project at `cwd`
    link for `target`, and their license texts, each with the crates that ship it:
    (crates, {text: [listing, ...]}). The project's own crates are left out. Offline, cargo
    reads only what a build fetched; otherwise it fetches what it lacks into its CARGO_HOME
    (`env`). Exits, naming them, when a crate's license has no text."""
    # Windows' packages only, with the roots' features: a build fetches no other crates.
    args = ["metadata", "--format-version", "1", "--filter-platform", target]
    selected = [f"{package}/{feature}" for package, features in roots for feature in features]
    if selected:
        args += ["--features", ",".join(selected)]
    if offline:
        args.append("--offline")
    packages = {
        (package["name"], package["version"]): package
        for package in json.loads(cargo(*args, cwd=cwd, env=env))["packages"]
    }
    crates = sorted(linked_crates(roots, target, cwd, env, offline))
    # The standard texts, as crates of the build ship them.
    standard = {}
    for key in crates:
        for path in license_files(packages[key]):
            text = normalized(path.read_text(encoding="utf-8", errors="replace"))
            for license, markers in STANDARD.items():
                if license not in standard and all(marker in text for marker in markers):
                    standard[license] = text

    groups = {}
    missing = []
    for key in crates:
        package = packages[key]
        # The project's own crates are under its own license.
        if package["source"] is None:
            continue
        expression = package.get("license") or "(no license field)"
        texts = [
            normalized(path.read_text(encoding="utf-8", errors="replace"))
            for path in license_files(package)
        ]
        if not texts:
            for license in re.split(r"\s+OR\s+|\s+AND\s+|/", expression):
                license = license.strip("() ")
                if license == "MIT":
                    authors = ", ".join(
                        re.sub(r"\s*<[^>]*>", "", author) for author in package.get("authors", [])
                    ) or f"the {package['name']} authors"
                    texts.append(f"MIT License\n\nCopyright (c) {authors}\n\n{MIT}")
                elif license in standard:
                    texts.append(standard[license])
        if not texts:
            missing.append(f"{package['name']} {package['version']} ({expression})")
            continue
        listing = f"{package['name']} {package['version']} ({expression})"
        if package.get("repository"):
            listing += f", {package['repository']}"
        for text in texts:
            groups.setdefault(text, []).append(listing)

    if missing:
        sys.exit("No license text for: " + "; ".join(missing))
    return crates, groups


def grouped(groups):
    """The lines that list `groups`, {text: [listing, ...]}: each text below the crates that
    ship it."""
    lines = []
    for text, listings in sorted(groups.items(), key=lambda item: sorted(item[1])[0].lower()):
        lines.append("=" * 78)
        lines += [f"  {listing}" for listing in sorted(set(listings), key=str.lower)]
        lines.append("-" * 78)
        lines.append(text)
    return lines


def main():
    if len(sys.argv) != 2:
        sys.exit(__doc__)
    out = Path(sys.argv[1])
    # dusq is built without the gpu feature, on its own.
    crates, groups = crate_texts([(package, []) for package in BINARIES], TARGET)
    lines = [
        "Third-party notices for Dusk",
        "",
        "dusk.exe and dusq.exe are built with the Rust crates listed below, each used under the",
        "license named beside it; where a crate offers a choice, its every text is given. Crates",
        "that ship the same text are listed together above it. Dusk's own code is under the MIT",
        "License (Dusk.txt), FFmpeg's libraries under the LGPL (FFmpeg.txt), and the bundled",
        "fonts under the SIL Open Font License (Inter.txt, JetBrains-Mono.txt).",
        "",
    ]
    lines += grouped(groups)
    out.parent.mkdir(parents=True, exist_ok=True)
    out.write_text("\n".join(lines), encoding="utf-8", newline="\r\n")
    print(f"{len(crates)} crates, {len(groups)} license texts: {out} ({out.stat().st_size} bytes)")


if __name__ == "__main__":
    main()
