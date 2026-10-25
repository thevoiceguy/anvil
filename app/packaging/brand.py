#!/usr/bin/env python3
"""A branded Windows setup (docs/MANAGED_INSTALLS.md §2.3): the stock setup
with a company's profile inside it, named after its brand. It installs the
profile as `/PROFILE=` would, and updates from the same feed as every other
copy; the stock setup the app updates with keeps the profile.

    app/packaging/brand.py <profile dir or .zip> --bundle <the app's files>
        --version 0.2.7 [--label v0.2.7] [--out dist] [--iscc ISCC.exe]

The app's files are a release's `Anvil-<label>-windows-x86_64-bundle.zip`,
unzipped, or `flutter build windows`'s Release folder. Inno Setup 6 builds
it (Windows, or wine).
"""

import argparse
import json
import os
import re
import subprocess
import sys
import tempfile
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))


def read_profile(path):
    """profile.json's contents, from a directory or a .zip of one."""
    if os.path.isdir(path):
        with open(os.path.join(path, "profile.json"), "rb") as f:
            data = f.read()
    else:
        with zipfile.ZipFile(path) as z:
            names = [n for n in z.namelist() if n.rsplit("/", 1)[-1] == "profile.json"]
            # At the top, or in the one folder the zip was made of.
            names.sort(key=lambda n: n.count("/"))
            if not names or names[0].count("/") > 1:
                sys.exit(f"no profile.json in {path}")
            data = z.read(names[0])
    profile = json.loads(data.decode("utf-8-sig"))
    if not isinstance(profile, dict):
        sys.exit("profile.json is not a JSON object")
    return profile


def zipped(path, work):
    """The profile as a .zip, as the setup carries it."""
    if not os.path.isdir(path):
        return os.path.abspath(path)
    out = os.path.join(work, "profile.zip")
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for root, _, files in os.walk(path):
            for name in files:
                full = os.path.join(root, name)
                z.write(full, os.path.relpath(full, path))
    return out


def file_name(brand, label):
    """`Acme-Voice-v0.2.7-windows-x86_64-setup`."""
    stem = re.sub(r"[^A-Za-z0-9]+", "-", brand).strip("-") or "Anvil"
    return f"{stem}-{label}-windows-x86_64-setup"


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("profile")
    p.add_argument("--bundle", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--label")
    p.add_argument("--out", default="dist")
    p.add_argument("--iscc", default="iscc")
    a = p.parse_args()
    label = a.label or f"v{a.version}"

    profile = read_profile(a.profile)
    brand = str((profile.get("brand") or {}).get("app_name") or "").strip()
    if not brand:
        sys.exit("the profile has no brand.app_name to name the setup after")
    if '"' in brand:
        sys.exit("the brand's name cannot hold a quotation mark")

    with tempfile.TemporaryDirectory() as work:
        name = file_name(brand, label)
        cmd = [
            a.iscc,
            "/Q",
            f"/DVersion={a.version}",
            f"/DLabel={label}",
            f"/DBundle={os.path.abspath(a.bundle)}",
            f"/DOut={os.path.abspath(a.out)}",
            f"/DProfile={zipped(a.profile, work)}",
            f"/DName={brand}",
            f"/DOutName={name}",
            os.path.join(HERE, "windows.iss"),
        ]
        subprocess.run(cmd, check=True)
    print(os.path.join(os.path.abspath(a.out), name + ".exe"))


if __name__ == "__main__":
    main()
