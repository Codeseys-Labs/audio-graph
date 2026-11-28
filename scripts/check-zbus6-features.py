#!/usr/bin/env python3
"""Check the resolved Linux runtime and logging features."""

import json
import subprocess
from pathlib import Path


def check(metadata, profile):
    packages = {
        package["id"]: (package["name"], package["version"])
        for package in metadata["packages"]
        if package["name"] == "zruntime"
        or (package["name"] == "zbus" and package["version"].split(".")[0] == "6")
    }
    nodes = {
        node["id"]: set(node["features"])
        for node in metadata["resolve"]["nodes"]
        if node["id"] in packages
    }
    missing = {"zbus", "zruntime"} - {packages[package][0] for package in nodes}
    if missing:
        raise SystemExit(f"{profile}: Linux resolution is missing {', '.join(sorted(missing))}")
    for package, features in nodes.items():
        name, version = packages[package]
        blocked = {"tokio", "tracing"} if name == "zbus" else {"tracing"}
        forbidden = sorted(features & blocked)
        if forbidden:
            raise SystemExit(
                f"{profile}: {name} {version} enables {', '.join(forbidden)}"
            )
    print(f"{profile}: resolved zbus 6 and zruntime exclude the forbidden features")


def main():
    manifest = Path(__file__).resolve().parents[1] / "src-tauri" / "Cargo.toml"
    optional = "diarization,sherpa-streaming,surrealdb-embedded"
    for profile, features in (
        ("default", []),
        ("cloud", ["--no-default-features", "--features", "cloud"]),
        ("default + optional", ["--features", optional]),
        ("cloud + optional", ["--no-default-features", "--features", f"cloud,{optional}"]),
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
