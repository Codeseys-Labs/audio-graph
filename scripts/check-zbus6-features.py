#!/usr/bin/env python3
"""Check the resolved Linux features of each zbus 6 package."""

import json
import subprocess
from pathlib import Path


def check(metadata, profile):
    packages = {
        package["id"]: package["version"]
        for package in metadata["packages"]
        if package["name"] == "zbus" and package["version"].split(".")[0] == "6"
    }
    nodes = {
        node["id"]: set(node["features"])
        for node in metadata["resolve"]["nodes"]
        if node["id"] in packages
    }
    if not nodes:
        raise SystemExit(f"{profile}: zbus 6 is missing from the Linux resolution")
    for package, features in nodes.items():
        forbidden = sorted(features & {"tokio", "tracing"})
        if forbidden:
            raise SystemExit(
                f"{profile}: zbus {packages[package]} enables {', '.join(forbidden)}"
            )
    print(f"{profile}: resolved zbus 6 excludes tokio and tracing")


def main():
    manifest = Path(__file__).resolve().parents[1] / "src-tauri" / "Cargo.toml"
    for profile, features in (
        ("default", []),
        ("cloud", ["--no-default-features", "--features", "cloud"]),
    ):
        raw = subprocess.check_output(
            [
                "cargo", "metadata", "--locked", "--format-version", "1",
                "--filter-platform", "x86_64-unknown-linux-gnu",
                "--manifest-path", str(manifest), *features,
            ]
        )
        check(json.loads(raw), profile)


if __name__ == "__main__":
    main()
