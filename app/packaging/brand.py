#!/usr/bin/env python3
"""A branded build of a release (docs/MANAGED_INSTALLS.md §2.3, §2.5): the
stock app with a company's profile inside it, named after its brand.

    app/packaging/brand.py <profile dir or .zip> --bundle <the release's>
        --version 0.2.9 [--label v0.2.9] [--out dist] [--target …]

- Windows (`--bundle` a release's `Anvil-<label>-windows-x86_64-bundle.zip`,
  unzipped, or `flutter build windows`'s Release folder): a setup that
  installs the profile as `/PROFILE=` would and updates from the same feed
  as every other copy. Inno Setup 6 builds it (`--iscc`; Windows, or wine).
- macOS (`--bundle` the release's `.dmg` or an `Anvil.app`): a `.pkg` that
  installs the app renamed (its name and icon) in /Applications and the
  profile in /Library/Application Support/Anvil/profile. Needs macOS (sips,
  iconutil, codesign, pkgbuild).
- Linux (`--bundle` the release's `.deb`): a `.deb` with the profile in
  /etc/anvil/profile, kept when the stock package replaces it, and its menu
  entry named from the brand. Needs dpkg-deb.

The target is taken from the bundle (`.dmg`/`.app`, `.deb`, else Windows)
unless `--target` says.
"""

import argparse
import json
import os
import plistlib
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile

HERE = os.path.dirname(os.path.abspath(__file__))
# The app's identity on macOS and Linux (app/macos, app/packaging/linux.sh).
APP_ID = "com.thevoiceguy.anvil"
MAC_PROFILE = "Library/Application Support/Anvil/profile"
LINUX_PROFILE = "etc/anvil/profile"


def profile_root(path, work):
    """The directory holding profile.json: `path`, or the .zip unpacked (at
    its top, or in the one folder it was made of)."""
    if os.path.isdir(path):
        root = path
    else:
        root = os.path.join(work, "profile")
        with zipfile.ZipFile(path) as z:
            z.extractall(root)
        if not os.path.exists(os.path.join(root, "profile.json")):
            inner = [d for d in os.listdir(root) if os.path.isfile(os.path.join(root, d, "profile.json"))]
            if len(inner) == 1:
                root = os.path.join(root, inner[0])
    if not os.path.isfile(os.path.join(root, "profile.json")):
        sys.exit(f"no profile.json in {path}")
    return root


def read_profile(root):
    with open(os.path.join(root, "profile.json"), "rb") as f:
        profile = json.loads(f.read().decode("utf-8-sig"))
    if not isinstance(profile, dict):
        sys.exit("profile.json is not a JSON object")
    return profile


def brand_icon(root, profile):
    """The brand's icon file beside profile.json, if it names one there."""
    name = str((profile.get("brand") or {}).get("icon") or "")
    if not name:
        return None
    if os.path.basename(name) != name or not os.path.isfile(os.path.join(root, name)):
        sys.exit(f"the brand's icon {name!r} is not a file beside profile.json")
    return os.path.join(root, name)


def zipped(root, work):
    """The profile as a .zip, as the Windows setup carries it."""
    out = os.path.join(work, "profile.zip")
    with zipfile.ZipFile(out, "w", zipfile.ZIP_DEFLATED) as z:
        for base, _, files in os.walk(root):
            for name in files:
                full = os.path.join(base, name)
                z.write(full, os.path.relpath(full, root))
    return out


def slug(brand):
    """`Acme-Voice`, for a file's name."""
    return re.sub(r"[^A-Za-z0-9]+", "-", brand).strip("-") or "Anvil"


def file_name(brand):
    """A name macOS takes for the app's folder."""
    kept = "".join(c for c in brand if c not in '/:' and c.isprintable()).strip().rstrip(".")
    return kept or "Anvil"


def run(*cmd):
    subprocess.run(cmd, check=True)


def copy_profile(root, dest):
    """The profile's files at `dest`, readable by everyone."""
    shutil.copytree(root, dest)
    for base, dirs, files in os.walk(dest):
        for d in dirs:
            os.chmod(os.path.join(base, d), 0o755)
        for f in files:
            os.chmod(os.path.join(base, f), 0o644)
    os.chmod(dest, 0o755)


def windows(a, root, brand, label, work):
    if '"' in brand:
        sys.exit("the brand's name cannot hold a quotation mark")
    name = f"{slug(brand)}-{label}-windows-x86_64-setup"
    run(
        a.iscc,
        "/Q",
        f"/DVersion={a.version}",
        f"/DLabel={label}",
        f"/DBundle={os.path.abspath(a.bundle)}",
        f"/DOut={os.path.abspath(a.out)}",
        f"/DProfile={zipped(root, work)}",
        f"/DName={brand}",
        f"/DOutName={name}",
        os.path.join(HERE, "windows.iss"),
    )
    return os.path.join(os.path.abspath(a.out), name + ".exe")


def icns(png, out, work):
    """The brand's image as an .icns, at every size macOS asks for."""
    iconset = os.path.join(work, "Brand.iconset")
    os.makedirs(iconset)
    for size in (16, 32, 128, 256, 512):
        for scale in (1, 2):
            px = size * scale
            suffix = "" if scale == 1 else "@2x"
            run("sips", "-s", "format", "png", "-z", str(px), str(px), png,
                "--out", os.path.join(iconset, f"icon_{size}x{size}{suffix}.png"))
    run("iconutil", "-c", "icns", iconset, "-o", out)


def macos(a, root, brand, label, work):
    icon = brand_icon(root, read_profile(root))
    source = a.bundle
    mounted = None
    if source.endswith(".dmg"):
        mounted = os.path.join(work, "dmg")
        os.makedirs(mounted)
        run("hdiutil", "attach", "-nobrowse", "-readonly", "-mountpoint", mounted, source)
        source = os.path.join(mounted, "Anvil.app")
    stage = os.path.join(work, "root")
    bundle = f"Applications/{file_name(brand)}.app"
    app = os.path.join(stage, bundle)
    try:
        shutil.copytree(source, app, symlinks=True)
    finally:
        if mounted:
            run("hdiutil", "detach", mounted)

    # The name in Finder, the Dock and the menu bar; the bundle's identity
    # (and with it the app's files and preferences) stays Anvil's.
    info = os.path.join(app, "Contents", "Info.plist")
    with open(info, "rb") as f:
        plist = plistlib.load(f)
    plist["CFBundleName"] = brand
    plist["CFBundleDisplayName"] = brand
    if icon:
        icns(icon, os.path.join(app, "Contents", "Resources", "Brand.icns"), work)
        plist["CFBundleIconFile"] = "Brand"
        # The asset catalog's icon would win over the file.
        plist.pop("CFBundleIconName", None)
    with open(info, "wb") as f:
        plistlib.dump(plist, f)
    # Signed again as the stock app is (ad hoc until code signing, U3c).
    run("codesign", "--force", "--deep", "--sign", "-", app)

    copy_profile(root, os.path.join(stage, MAC_PROFILE))
    # Installed where it says, never moved onto a stock Anvil.app found
    # elsewhere with the same identity.
    component = os.path.join(work, "component.plist")
    with open(component, "wb") as f:
        plistlib.dump([{
            "RootRelativeBundlePath": bundle,
            "BundleIsRelocatable": False,
            "BundleIsVersionChecked": False,
            "BundleHasStrictIdentifier": False,
            "BundleOverwriteAction": "upgrade",
        }], f)
    os.makedirs(a.out, exist_ok=True)
    out = os.path.join(os.path.abspath(a.out), f"{slug(brand)}-{label}-macos.pkg")
    run("pkgbuild", "--root", stage, "--component-plist", component,
        "--identifier", f"{APP_ID}.brand.{slug(brand).lower()}",
        "--version", a.version, "--install-location", "/", out)
    return out


def desktop_entry(text, brand, icon):
    """The packaged menu entry with the brand's name (every language's) and
    icon."""
    out, main = [], False
    for line in text.splitlines():
        t = line.strip()
        if t.startswith("["):
            main = t == "[Desktop Entry]"
        elif main:
            key = t.split("=", 1)[0].strip()
            if key.split("[", 1)[0] == "Name":
                if key == "Name":
                    out.append(f"Name={brand}")
                continue
            if key == "Icon" and icon:
                out.append(f"Icon={icon}")
                continue
        out.append(line)
    return "\n".join(out) + "\n"


def linux(a, root, brand, label, work):
    icon = brand_icon(root, read_profile(root))
    stage = os.path.join(work, "deb")
    run("dpkg-deb", "-R", a.bundle, stage)
    target = os.path.join(stage, LINUX_PROFILE)
    copy_profile(root, target)
    # A conffile is kept when a package without it (the stock one, an
    # update) replaces this one, and goes only when the package is purged.
    conffiles = os.path.join(stage, "DEBIAN", "conffiles")
    with open(conffiles, "a") as f:
        for base, _, files in os.walk(target):
            for name in sorted(files):
                f.write("/" + os.path.relpath(os.path.join(base, name), stage) + "\n")
    entry = os.path.join(stage, "usr", "share", "applications", f"{APP_ID}.desktop")
    with open(entry) as f:
        text = f.read()
    shown = "/" + os.path.relpath(os.path.join(target, os.path.basename(icon)), stage) if icon else None
    with open(entry, "w") as f:
        f.write(desktop_entry(text, brand, shown))
    os.makedirs(a.out, exist_ok=True)
    out = os.path.join(os.path.abspath(a.out), f"{slug(brand).lower()}_{label}_amd64.deb")
    run("dpkg-deb", "--root-owner-group", "--build", stage, out)
    return out


def target_of(bundle):
    b = bundle.rstrip("/")
    if b.endswith(".dmg") or b.endswith(".app"):
        return "macos"
    if b.endswith(".deb"):
        return "linux"
    return "windows"


def main():
    p = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    p.add_argument("profile")
    p.add_argument("--bundle", required=True)
    p.add_argument("--version", required=True)
    p.add_argument("--label")
    p.add_argument("--out", default="dist")
    p.add_argument("--iscc", default="iscc")
    p.add_argument("--target", choices=["windows", "macos", "linux"])
    a = p.parse_args()
    # What the packages install is readable by everyone, as the stock ones.
    os.umask(0o022)
    label = a.label or f"v{a.version}"
    target = a.target or target_of(a.bundle)

    with tempfile.TemporaryDirectory() as work:
        root = profile_root(a.profile, work)
        brand = str((read_profile(root).get("brand") or {}).get("app_name") or "").strip()
        if not brand:
            sys.exit("the profile has no brand.app_name to name the build after")
        build = {"windows": windows, "macos": macos, "linux": linux}[target]
        print(build(a, root, brand, label, work))


if __name__ == "__main__":
    main()
