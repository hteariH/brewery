#!/usr/bin/env python3
"""Writes the Info.plist of Brewery's macOS .app (script/brewery/bundle-mac).

Brewery's replacement for cargo-bundle's output: the same keys Zed's bundle has (document
types, usage descriptions from crates/zed/resources/info/*.plist), under Brewery's names,
without the zed:// URL scheme. CFBundleIdentifier equals ReleaseChannel::app_id()
(crates/release_channel), as release_channel documents.

    mac_info_plist.py --channel <dev|nightly|preview|stable> --version <ver> --out <Info.plist>
        [--info-dir crates/zed/resources/info]
"""

import argparse
import plistlib
import re
import sys
from pathlib import Path

# channel -> (display name, bundle identifier)
CHANNELS = {
    "dev": ("Brewery Dev", "io.github.htearih.Brewery-Dev"),
    "nightly": ("Brewery Nightly", "io.github.htearih.Brewery-Nightly"),
    "preview": ("Brewery Preview", "io.github.htearih.Brewery-Preview"),
    "stable": ("Brewery", "io.github.htearih.Brewery"),
}
MIN_MACOS = "11.0"  # the app is arm64-only; Apple Silicon starts at macOS 11


def build_info_plist(channel, version, info_dir):
    if channel not in CHANNELS:
        sys.exit(f"mac_info_plist: unknown channel {channel!r}")
    if not re.fullmatch(r"\d+\.\d+\.\d+", version):
        sys.exit(f"mac_info_plist: bad version {version!r} (need x.y.z for CFBundleVersion)")
    name, identifier = CHANNELS[channel]
    info = {
        "CFBundleDevelopmentRegion": "en",
        "CFBundleExecutable": "brewery",
        "CFBundleIconFile": "Brewery",
        "CFBundleIdentifier": identifier,
        "CFBundleInfoDictionaryVersion": "6.0",
        "CFBundleName": name,
        "CFBundleDisplayName": name,
        "CFBundlePackageType": "APPL",
        "CFBundleShortVersionString": version,
        "CFBundleVersion": version,
        "LSApplicationCategoryType": "public.app-category.developer-tools",
        "LSMinimumSystemVersion": MIN_MACOS,
        "NSHighResolutionCapable": True,
    }
    # The fragments are bare key/value pairs (cargo-bundle's osx_info_plist_exts).
    for fragment in sorted(Path(info_dir).glob("*.plist")):
        text = fragment.read_text(encoding="utf-8").replace("Zed", name)
        wrapped = (
            '<?xml version="1.0" encoding="UTF-8"?><plist version="1.0"><dict>'
            + text
            + "</dict></plist>"
        )
        info.update(plistlib.loads(wrapped.encode("utf-8")))
    return plistlib.dumps(info)


def main():
    parser = argparse.ArgumentParser(description=__doc__.split("\n\n")[0])
    parser.add_argument("--channel", required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--out", required=True)
    parser.add_argument("--info-dir", default="crates/zed/resources/info")
    args = parser.parse_args()
    Path(args.out).write_bytes(build_info_plist(args.channel, args.version, args.info_dir))


if __name__ == "__main__":
    main()
