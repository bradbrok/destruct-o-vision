"""Prints the licenses of every crate compiled into the app.

Binary distributions must carry these: MIT, Apache, BSD and the font
licenses all ask for their notices to travel with the program. Crates that
ship license files are quoted as-is. For the few that don't, this prints
the standard text of the license they are offered under, with their
authors as the copyright holders.

Usage: third_party_licenses.py [TARGET_TRIPLE ...]
(defaults to the host; pass both Mac triples for a universal build)
"""

import json
import pathlib
import re
import subprocess
import sys

ROOT = pathlib.Path(__file__).resolve().parent.parent
LICENSE_FILE = re.compile(r"^(licen[cs]e|copying|notice|unlicense)", re.I)
# Font licenses ship next to the fonts rather than at the crate root.
FONT_NOTICES = re.compile(r"\.txt$", re.I)

MIT = """MIT License

Copyright (c) {holders}

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the "Software"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED "AS IS", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
"""


def host_triple():
    out = subprocess.run(["rustc", "-vV"], capture_output=True, text=True, check=True).stdout
    return next(line.split(": ")[1] for line in out.splitlines() if line.startswith("host: "))


def shipped(target):
    """Packages linked into dov-app for `target`: normal dependencies only."""
    meta = json.loads(
        subprocess.run(
            ["cargo", "metadata", "--format-version", "1", "--locked", "--filter-platform", target],
            capture_output=True,
            text=True,
            check=True,
            cwd=ROOT,
        ).stdout
    )
    packages = {p["id"]: p for p in meta["packages"]}
    nodes = {n["id"]: n for n in meta["resolve"]["nodes"]}
    root = next(p["id"] for p in meta["packages"] if p["name"] == "dov-app")
    seen, stack = set(), [root]
    while stack:
        node = stack.pop()
        if node in seen:
            continue
        seen.add(node)
        for dep in nodes[node]["deps"]:
            if any(kind["kind"] is None for kind in dep["dep_kinds"]):
                stack.append(dep["pkg"])
    return [packages[i] for i in seen - set(meta["workspace_members"])]


def notices(package):
    """The package's own license texts, as (file name, text)."""
    folder = pathlib.Path(package["manifest_path"]).parent
    files = [f for f in folder.iterdir() if f.is_file() and LICENSE_FILE.match(f.name)]
    if package.get("license_file"):
        files.append(folder / package["license_file"])
    fonts = folder / "fonts"
    if fonts.is_dir():
        files += [f for f in fonts.iterdir() if FONT_NOTICES.search(f.name)]
    texts = []
    for f in sorted(set(files)):
        try:
            texts.append((f.relative_to(folder).as_posix(), f.read_text(errors="replace").strip()))
        except OSError:
            pass
    return texts


def standard_text(package):
    """For packages without license files: the license they are offered under."""
    expr = package.get("license") or ""
    holders = ", ".join(a.split(" <")[0] for a in package.get("authors", [])) or f"the {package['name']} authors"
    if "MIT" in expr:
        return "MIT (standard text)", MIT.format(holders=holders).strip()
    if "Apache-2.0" in expr:
        return "Apache-2.0 (standard text)", (ROOT / "packaging/licenses/Apache-2.0.txt").read_text().strip()
    return None


def main():
    targets = sys.argv[1:] or [host_triple()]
    packages = {}
    for target in targets:
        for p in shipped(target):
            packages[(p["name"], p["version"])] = p
    rule = "=" * 78
    print("Destruct-O-Vision includes the following third-party software.")
    print("Each entry lists the crate, its license, and the license text.\n")
    missing = []
    for key in sorted(packages):
        p = packages[key]
        texts = notices(p) or [t for t in [standard_text(p)] if t]
        if not texts:
            missing.append(f"{p['name']} {p['version']}")
        print(rule)
        print(f"{p['name']} {p['version']}: {p.get('license') or 'see below'}")
        if p.get("repository"):
            print(p["repository"])
        for name, text in texts:
            print(f"\n--- {name} ---\n\n{text}\n")
    if missing:
        sys.exit("no license text found for: " + ", ".join(missing))


if __name__ == "__main__":
    main()
