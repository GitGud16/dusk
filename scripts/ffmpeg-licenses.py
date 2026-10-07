"""Writes installer/ffmpeg-libraries.txt for the installer (docs/ARCHITECTURE.md, "Release
(0.1)"): the license texts of the libraries built into the pinned FFmpeg's DLLs, which ship
beside FFmpeg's own LGPL. Run it again whenever scripts/ffmpeg-pin.psd1 moves:

    python scripts/ffmpeg-licenses.py

It downloads BtbN's FFmpeg-Builds scripts at the pinned release tag, asks them which libraries
the win64 LGPL shared build of the pinned FFmpeg version contains (scripts/btbn-components.sh,
which uses the scripts' own logic), and fetches each library's license files at the commit
BtbN built from that library's own repository. Libraries sharing one text are listed together
above it. Two of them, rav1e and librsvg, are written in Rust: for those it checks out their
Cargo files and Rust sources at that commit, lets cargo resolve their builds for Windows into a
CARGO_HOME of its own (fetching their crates, some tens of MB, removed afterwards), and lists
the crates' licenses as scripts/third-party-notices.py does Dusk's. Needs the network, Git's
bash, cargo and an authenticated `gh` (for GitHub's API); Python's standard library otherwise.
It fails, naming them, when it finds no license text for a library or a crate.
"""

import base64
import importlib.util
import io
import json
import os
import re
import shutil
import stat
import subprocess
import sys
import tarfile
import tempfile
import urllib.error
import urllib.parse
import urllib.request
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
OUT = ROOT / "installer" / "ffmpeg-libraries.txt"
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|unlicense|copyright|patents)", re.IGNORECASE)
# Files named like licenses that are not one (a header template, a checker's settings).
NOT_LICENSE = re.compile(r"(template|checker)", re.IGNORECASE)
# Repositories that are not libraries of their own in the DLLs.
NOT_LIBRARIES = {
    # Its modules are copied into libiconv's sources and come under libiconv's license.
    "https://git.savannah.gnu.org/git/gnulib.git",
}
# License files beyond the top folder's.
EXTRA_FILES = {
    # The threads library every DLL links.
    "https://git.code.sf.net/p/mingw-w64/mingw-w64.git": ["mingw-w64-libraries/winpthreads/COPYING"],
}
# Libraries whose license is the notice at the top of a source file, which is quoted alone.
NOTICE_IN = {
    "https://github.com/FFmpeg/nv-codec-headers.git": "include/ffnvcodec/nvEncodeAPI.h",
}
# Libraries written in Rust, whose crates are built into the DLLs with them: the packages
# BtbN's scripts build, with their features (cargo-c builds rav1e with `capi`, and librsvg's
# meson build its C library with `avif`), for the GNU target BtbN builds for.
RUST_LIBRARIES = {
    "https://github.com/xiph/rav1e.git": [("rav1e", ["capi"])],
    "https://github.com/GNOME/librsvg.git": [("librsvg-c", ["avif"])],
}
RUST_TARGET = "x86_64-pc-windows-gnu"


def get(url, tries=3):
    """The body at `url`, asked again when a slow server times out."""
    request = urllib.request.Request(url, headers={"User-Agent": "dusk-ffmpeg-licenses"})
    for attempt in range(tries):
        try:
            with urllib.request.urlopen(request, timeout=60) as response:
                return response.read()
        except (TimeoutError, urllib.error.URLError) as error:
            if attempt == tries - 1 or (isinstance(error, urllib.error.HTTPError) and error.code < 500):
                raise


def gh(path, raw=False):
    command = ["gh", "api", path]
    if raw:
        command[2:2] = ["-H", "Accept: application/vnd.github.raw"]
    return subprocess.run(command, check=True, capture_output=True).stdout


def text(data):
    return data.decode("utf-8", errors="replace")


def license_names(entries):
    """The license files among a folder's (name, is_folder) entries, and its license folders."""
    files = [
        name for name, folder in entries if not folder and LICENSE_FILE.match(name) and not NOT_LICENSE.search(name)
    ]
    folders = [name for name, folder in entries if folder and (LICENSE_FILE.match(name) or name == "LICENSES")]
    return files, folders


class GitHub:
    def __init__(self, url, commit):
        owner, repo = re.match(r"https://github\.com/([^/]+)/([^/]+?)(?:\.git)?/?$", url).groups()
        self.base, self.commit = f"repos/{owner}/{repo}/contents", urllib.parse.quote(commit)

    def entries(self, folder=""):
        items = json.loads(gh(f"{self.base}/{folder}?ref={self.commit}"))
        return [(item["name"], item["type"] == "dir") for item in items]

    def read(self, path):
        return text(gh(f"{self.base}/{urllib.parse.quote(path)}?ref={self.commit}", raw=True))


class GitLab:
    def __init__(self, url, commit):
        host, path = re.match(r"https://([^/]+)/(.+?)(?:\.git)?/?$", url).groups()
        self.api = f"https://{host}/api/v4/projects/{urllib.parse.quote(path, safe='')}/repository"
        self.commit = urllib.parse.quote(commit)

    def entries(self, folder=""):
        query = f"ref={self.commit}&per_page=100" + (f"&path={urllib.parse.quote(folder)}" if folder else "")
        items = json.loads(get(f"{self.api}/tree?{query}"))
        return [(item["name"], item["type"] == "tree") for item in items]

    def read(self, path):
        return text(get(f"{self.api}/files/{urllib.parse.quote(path, safe='')}/raw?ref={self.commit}"))


class Gitiles:
    def __init__(self, url, commit):
        self.base, self.commit = url.rstrip("/"), commit

    def entries(self, folder=""):
        page = text(get(f"{self.base}/+/{self.commit}/{folder}?format=JSON"))
        listing = json.loads(page.split("\n", 1)[1])
        return [(item["name"], item["type"] == "tree") for item in listing["entries"]]

    def read(self, path):
        return text(base64.b64decode(get(f"{self.base}/+/{self.commit}/{path}?format=TEXT")))


class SourceForge:
    """A SourceForge repository, git or Subversion, through its web pages."""

    def __init__(self, url, commit, revision):
        git = re.match(r"https://git\.code\.sf\.net/p/([^/]+)/([^/]+?)(?:\.git)?/?$", url)
        if git:
            project, repo = git.groups()
            self.tree = f"https://sourceforge.net/p/{project}/{repo}/ci/{commit}/tree/"
        else:
            project, path = re.match(r"https://svn\.code\.sf\.net/p/([^/]+)/svn/(.*?)/?$", url).groups()
            self.tree = f"https://sourceforge.net/p/{project}/svn/{revision}/tree/{path}/"

    def entries(self, folder=""):
        # Each entry of the listing is a link relative to the folder, with an icon.
        page = text(get(self.tree + (folder + "/" if folder else "")))
        found = re.findall(r'<a class="icon" href="([^"]+)"[^>]*>\s*<i class="fa fa-([a-z-]+)"', page)
        return sorted(
            {(urllib.parse.unquote(href).rstrip("/"), href.endswith("/") or "folder" in icon) for href, icon in found}
        )

    def read(self, path):
        return text(get(self.tree + path + "?format=raw"))


class Cgit:
    def __init__(self, url, commit):
        self.base = re.sub(r"/git/", "/cgit/", url.rstrip("/"), count=1)
        self.commit = commit

    def entries(self, folder=""):
        page = text(get(f"{self.base}/tree/{folder}?id={self.commit}"))
        prefix = urllib.parse.urlparse(self.base).path + "/tree/" + (folder + "/" if folder else "")
        found = re.findall(r"href='" + re.escape(prefix) + r"([^'/?]+)\?id=", page)
        return [(name, False) for name in sorted(set(found))]

    def read(self, path):
        return text(get(f"{self.base}/plain/{path}?id={self.commit}"))


def repository(url, commit, revision):
    if url.startswith("https://github.com/"):
        return GitHub(url, commit)
    if "googlesource.com" in url:
        return Gitiles(url, commit)
    if ".code.sf.net/" in url:
        return SourceForge(url, commit, revision)
    if "git.savannah.gnu.org" in url:
        return Cgit(url, commit)
    return GitLab(url, commit)


def license_files(url, commit, revision):
    """The license files of the repository at `url`, at `commit` (or Subversion `revision`):
    (path, text) pairs."""
    repo = repository(url, commit, revision)
    files, folders = license_names(repo.entries())
    paths = list(files)
    for folder in folders:
        inner, _ = license_names(repo.entries(folder))
        if folder == "LICENSES":
            inner = [name for name, is_folder in repo.entries(folder) if not is_folder]
        paths += [f"{folder}/{name}" for name in inner]
    paths += EXTRA_FILES.get(url, [])
    found = [(path, repo.read(path)) for path in paths]
    if url in NOTICE_IN:
        path = NOTICE_IN[url]
        notice = re.match(r"\s*/\*(.*?)\*/", repo.read(path), re.DOTALL).group(1)
        found.append((f"{path} (its opening notice)", re.sub(r"(?m)^[ \t]*\* ?", "", notice)))
    return found


def checkout(url, commit, folder):
    """The Cargo manifests, lock file and Rust sources of the repository at `url`, at `commit`,
    in `folder`: a sparse, partial clone, so its test files and pictures stay on the server."""

    def git(*args):
        subprocess.run(["git", *args], cwd=folder, check=True, capture_output=True)

    folder.mkdir(parents=True)
    git("init", "-q")
    git("remote", "add", "origin", url)
    git("sparse-checkout", "set", "--no-cone", "Cargo.toml", "Cargo.lock", "*.rs")
    git("fetch", "-q", "--depth", "1", "--filter=blob:none", "origin", commit)
    git("checkout", "-q", "FETCH_HEAD")


def remove(folder):
    """Removes `folder`, with git's read-only files."""

    def writable(function, path, _):
        os.chmod(path, stat.S_IWRITE)
        function(path)

    if sys.version_info >= (3, 12):
        shutil.rmtree(folder, onexc=writable)
    else:
        shutil.rmtree(folder, onerror=writable)


def rust_crates(libraries, temp):
    """The license texts of the Rust crates built into the DLLs with the Rust libraries among
    `libraries`, each with the crates that ship it, named with the library they are in:
    {text: [listing, ...]}. Cargo fetches the crates into a CARGO_HOME of its own in `temp`."""
    spec = importlib.util.spec_from_file_location("notices", ROOT / "scripts" / "third-party-notices.py")
    notices = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(notices)
    env = dict(os.environ, CARGO_HOME=str(Path(temp) / "cargo"))
    groups = {}
    for name, url, commit, _ in libraries:
        if url not in RUST_LIBRARIES:
            continue
        folder = Path(temp) / name
        checkout(url, commit, folder)
        crates, texts = notices.crate_texts(RUST_LIBRARIES[url], RUST_TARGET, cwd=folder, env=env, offline=False)
        for text, listings in texts.items():
            groups.setdefault(text, []).extend(f"{listing}, in {name}" for listing in listings)
        print(f"{name}: {len(crates)} crates")
    return groups, notices.grouped


def normalized(license):
    lines = [line.rstrip() for line in license.replace("\r\n", "\n").replace("\r", "\n").split("\n")]
    return "\n".join(lines).strip() + "\n"


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


def components(builds, version):
    """(name, url, commit, revision) of every library the build contains, from BtbN's scripts."""
    lister = ROOT / "scripts" / "btbn-components.sh"
    run = subprocess.run(
        [bash(), lister.as_posix(), builds.as_posix(), "win64", "lgpl-shared", version],
        capture_output=True, text=True, encoding="utf-8",
    )
    if run.returncode != 0:
        sys.exit(f"Listing BtbN's components failed: {run.stderr.strip()}")
    listing = run.stdout
    rows = [line.split("\t") for line in listing.splitlines() if line.strip()]
    enabled = {row[0] for row in rows}
    found, seen = [], set()
    for script, skip, *sources in rows:
        # A folder of scripts builds one library and what only it uses; when its last script is
        # off (libcurl in FFmpeg 8.1), nothing of it reaches FFmpeg.
        folder = builds / Path(script).parent
        if folder.name != "scripts.d" and sorted(folder.glob("??-*.sh"))[-1].relative_to(builds).as_posix() not in enabled:
            continue
        if skip:
            continue
        name = Path(script).stem.split("-", 1)[1]
        url, commit, revision = sources[0], sources[1], sources[2]
        pairs = [(url, commit, revision), (sources[3], sources[4], ""), (sources[5], sources[6], "")]
        for url, commit, revision in pairs:
            key = (url.removesuffix(".git"), commit or revision)
            if url and url not in NOT_LIBRARIES and key not in seen:
                seen.add(key)
                found.append((name, url, commit, revision))
    return found


def main():
    pin = (ROOT / "scripts" / "ffmpeg-pin.psd1").read_text(encoding="utf-8")
    tag = re.search(r"^\s*Tag\s*=\s*'([^']+)'", pin, re.MULTILINE).group(1)
    file_name = re.search(r"^\s*FileName\s*=\s*'([^']+)'", pin, re.MULTILINE).group(1)
    version = re.search(r"-shared-([0-9.]+)\.zip$", file_name).group(1)
    temp = tempfile.mkdtemp(prefix="dusk-ffmpeg-licenses-")
    try:
        archive = get(f"https://codeload.github.com/BtbN/FFmpeg-Builds/tar.gz/refs/tags/{tag}")
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(temp, filter="data")
        builds = next(Path(temp).iterdir())
        libraries = components(builds, version)
        crates, grouped = rust_crates(libraries, temp)
    finally:
        remove(temp)

    groups, missing = {}, []
    for name, url, commit, revision in libraries:
        try:
            files = license_files(url, commit, revision)
        except (urllib.error.URLError, subprocess.CalledProcessError, AttributeError, KeyError, ValueError) as error:
            sys.exit(f"Could not read the license files of {name} ({url}): {error}")
        if not files:
            missing.append(f"{name} ({url})")
        at = f"r{revision}" if revision else commit[:12] if re.fullmatch(r"[0-9a-f]{40}", commit) else commit
        for path, license in files:
            groups.setdefault(normalized(license), []).append(f"{name} ({url.removesuffix('.git')}, {at}): {path}")
        print(f"{name}: {', '.join(path for path, _ in files) or 'none'}")
    if missing:
        sys.exit("No license file found for: " + "; ".join(missing))

    out = [
        "Libraries in FFmpeg",
        "",
        "dusk.exe and dusq.exe use five libraries of FFmpeg (FFmpeg.txt). The build Dusk ships,",
        f"{file_name} from release {tag} of BtbN's FFmpeg-Builds,",
        "has the libraries below built into those DLLs, each under its own license. Each is named",
        "with the repository and the commit it was built from, as BtbN's build scripts for that",
        "release give them. Every library those scripts build into this FFmpeg is listed, some used",
        "only by parts of FFmpeg that Dusk does not ship, rather than risk leaving one out.",
        "Libraries that ship the same text are listed together above it. The Rust crates inside",
        "two of them, rav1e and librsvg, follow the libraries.",
        "",
    ]
    for license, sources in sorted(groups.items(), key=lambda group: group[1][0].lower()):
        out.append("=" * 78)
        out += [f"  {source}" for source in sources]
        out.append("-" * 78)
        out.append(license)
    out += [
        "",
        "Rust crates in rav1e and librsvg",
        "",
        "rav1e and librsvg are written in Rust, and the Rust crates below are built into the DLLs",
        "with them: every crate their builds for Windows depend on, as cargo resolves those builds",
        "at the commits above, each under the license named beside it; where a crate offers a",
        "choice, its every text is given. Crates that ship the same text are listed together above",
        "it.",
        "",
    ]
    out += grouped(crates)
    OUT.write_text("\n".join(out), encoding="utf-8", newline="\n")
    print(
        f"{len(libraries)} libraries, {len(groups)} license texts and {len(crates)} for their Rust "
        f"crates: {OUT} ({OUT.stat().st_size} bytes)"
    )


if __name__ == "__main__":
    main()
