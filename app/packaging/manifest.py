#!/usr/bin/env python3
"""A release's latest.json (crates/anvil-update): its version, its notes'
page, and each installer by target with its URL, size and SHA-256.

    app/packaging/manifest.py <version> <tag> <owner/repo> <dist dir>
"""

import hashlib
import json
import os
import re
import sys

TARGETS = [
    (r"^Anvil-.+-windows-x86_64-setup\.exe$", "windows-x86_64-setup"),
    (r"^Anvil-.+-macos\.dmg$", "macos-dmg"),
    (r"^Anvil-.+-x86_64\.AppImage$", "linux-x86_64-appimage"),
    (r"^anvil_.+_amd64\.deb$", "linux-amd64-deb"),
]


def main():
    version, tag, repo, dist = sys.argv[1:5]
    files = {}
    for name in sorted(os.listdir(dist)):
        for pattern, target in TARGETS:
            if re.match(pattern, name):
                with open(os.path.join(dist, name), "rb") as f:
                    data = f.read()
                files[target] = {
                    "name": name,
                    "url": f"https://github.com/{repo}/releases/download/{tag}/{name}",
                    "sha256": hashlib.sha256(data).hexdigest(),
                    "size": len(data),
                }
    missing = {t for _, t in TARGETS} - files.keys()
    if missing:
        sys.exit(f"no file for {', '.join(sorted(missing))}")
    manifest = {
        "version": version,
        "notes": f"https://github.com/{repo}/releases/tag/{tag}",
        "files": files,
    }
    with open(os.path.join(dist, "latest.json"), "w") as f:
        json.dump(manifest, f, indent=2, sort_keys=True)
        f.write("\n")
    print(json.dumps(manifest, indent=2, sort_keys=True))


if __name__ == "__main__":
    main()
