#!/usr/bin/env python3
"""Verify the temporary Alloy API patches against checksum-pinned registry sources."""
from __future__ import annotations

import hashlib
import io
import json
import os
from pathlib import Path
import subprocess
import tarfile
from urllib.request import urlopen

ROOT = Path(__file__).resolve().parent.parent
OMITTED = {"Cargo.lock", "Cargo.toml.orig", ".cargo_vcs_info.json"}


def sha256(data: bytes) -> str:
    return hashlib.sha256(data).hexdigest()


def check_sources(packages: list[dict]) -> None:
    cargo_home = Path(os.environ.get("CARGO_HOME", Path.home() / ".cargo"))
    for package in packages:
        name, version = package["name"], package["version"]
        archive_name = f"{name}-{version}.crate"
        cached = list((cargo_home / "registry/cache").glob(f"*/{archive_name}"))
        if cached:
            data = cached[0].read_bytes()
        else:
            with urlopen(f"https://static.crates.io/crates/{name}/{archive_name}", timeout=30) as response:
                data = response.read()
        if sha256(data) != package["sha256"]:
            raise ValueError(f"{archive_name}: published archive checksum mismatch")
        directory = ROOT / "vendor" / name
        expected = set()
        changed = set()
        with tarfile.open(fileobj=io.BytesIO(data), mode="r:gz") as archive:
            for member in archive.getmembers():
                if not member.isfile():
                    continue
                relative = Path(member.name).relative_to(f"{name}-{version}")
                if relative.is_absolute() or ".." in relative.parts:
                    raise ValueError(f"{archive_name}: unsafe source path")
                if str(relative) in OMITTED:
                    continue
                expected.add(str(relative))
                original = archive.extractfile(member).read()
                path = directory / relative
                if path.is_symlink():
                    raise ValueError(f"{path}: unexpected symlink")
                patched = path.read_bytes()
                if original != patched:
                    changed.add(str(relative))
                    recorded = package["changes"].get(str(relative))
                    if recorded != {"original_sha256": sha256(original), "patched_sha256": sha256(patched)}:
                        raise ValueError(f"{path}: unrecorded source change")
        actual = {str(path.relative_to(directory)) for path in directory.rglob("*") if path.is_file()}
        if actual != expected or changed != set(package["changes"]):
            raise ValueError(f"{name}: source file inventory differs from recorded patch")
        print(f"verified {name} {version}: {len(changed)} patched files")


def check_resolution(packages: list[dict], manifest: str) -> None:
    metadata = json.loads(subprocess.check_output(
        ["cargo", "metadata", "--manifest-path", manifest, "--locked", "--all-features", "--format-version", "1"],
        cwd=ROOT,
    ))
    resolved = {package["name"]: package for package in metadata["packages"]}
    for package in packages:
        if sum(selected["name"] == package["name"] for selected in metadata["packages"]) != 1:
            raise ValueError(f"{manifest}: duplicate or missing {package['name']}")
        selected = resolved[package["name"]]
        expected = ROOT / "vendor" / package["name"] / "Cargo.toml"
        if selected["version"] != package["version"] or Path(selected["manifest_path"]) != expected:
            raise ValueError(f"{manifest}: {package['name']} bypasses the reviewed vendor patch")
    if "crypto-bigint" not in resolved:
        raise ValueError(f"{manifest}: expected crypto-bigint dependency is missing")
    for selected in metadata["packages"]:
        if selected["name"] == "crypto-bigint":
            version = tuple(int(part) for part in selected["version"].split("."))
            if version < (0, 7, 6):
                raise ValueError(f"{manifest}: vulnerable crypto-bigint {selected['version']}")
    # Optional keystores, mnemonic and HSM backends have separate dependency
    # contracts. This patch supports only the deployed deterministic signer.
    signer_id = resolved["alloy-signer-local"]["id"]
    signer = next(node for node in metadata["resolve"]["nodes"] if node["id"] == signer_id)
    if set(signer["features"]) - {"eip712", "zeroize"}:
        raise ValueError(f"{manifest}: unreviewed optional local-signer backend")
    print(f"verified {manifest}: reviewed signer and patched crypto-bigint")


def main() -> None:
    packages = json.loads((ROOT / "vendor/alloy-k256-provenance.json").read_text())
    check_sources(packages)
    for manifest in ["Cargo.toml", "fuzz/Cargo.toml"]:
        check_resolution(packages, manifest)


if __name__ == "__main__":
    main()
