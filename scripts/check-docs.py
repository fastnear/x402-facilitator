#!/usr/bin/env python3
"""Check local Markdown links and obvious tracked-secret mistakes."""

from __future__ import annotations

import json
import os
import re
import subprocess
import sys
from pathlib import Path
from urllib.parse import unquote, urlparse

ROOT = Path(__file__).resolve().parent.parent
LINK = re.compile(r"(?<!!)\[[^\]]*\]\(([^)]+)\)")
PRIVATE_KEY = re.compile(r"-----BEGIN [A-Z0-9 ]*PRIVATE KEY-----")
DATABASE_PASSWORD = re.compile(r"postgres(?:ql)?://[^/\s:@]+:[^@\s]+@", re.IGNORECASE)
RAW_API_KEY = re.compile(r"x402_(?:live|test)_[A-Za-z0-9_-]{4,}\.[A-Za-z0-9_-]{20,}")


def tracked_files() -> list[Path]:
    output = subprocess.check_output(
        ["git", "ls-files", "-z", "--cached", "--others", "--exclude-standard"],
        cwd=ROOT,
    )
    return [ROOT / item.decode() for item in output.split(b"\0") if item]


def markdown_link_errors(paths: list[Path]) -> list[str]:
    errors: list[str] = []
    for path in paths:
        if path.suffix.lower() != ".md":
            continue
        text = path.read_text(encoding="utf-8")
        for match in LINK.finditer(text):
            destination = match.group(1).strip()
            if destination.startswith("<") and destination.endswith(">"):
                destination = destination[1:-1]
            destination = destination.split(maxsplit=1)[0]
            if (
                not destination
                or destination.startswith(("#", "http://", "https://", "mailto:"))
            ):
                continue
            destination = unquote(destination.split("#", 1)[0])
            target = (path.parent / destination).resolve()
            if not target.exists():
                line = text.count("\n", 0, match.start()) + 1
                errors.append(
                    f"{path.relative_to(ROOT)}:{line}: missing local link {destination}"
                )
    return errors


def secret_errors(paths: list[Path]) -> list[str]:
    errors: list[str] = []
    forbidden_names = {".env", ".env.local", ".env.production"}
    forbidden_suffixes = {".credential", ".key", ".pem", ".secret"}
    for path in paths:
        relative = path.relative_to(ROOT)
        if path.name in forbidden_names or path.suffix.lower() in forbidden_suffixes:
            errors.append(f"{relative}: secret-like filename is tracked")
            continue
        if not path.is_file() or path.stat().st_size > 2_000_000:
            continue
        try:
            text = path.read_text(encoding="utf-8")
        except UnicodeDecodeError:
            continue
        for name, pattern in [
            ("private-key block", PRIVATE_KEY),
            ("database URL with password", DATABASE_PASSWORD),
            ("raw x402 API key", RAW_API_KEY),
        ]:
            if pattern.search(text):
                errors.append(f"{relative}: possible {name}")
    return errors


def registry_submission_errors() -> list[str]:
    relative = Path("docs/registry/x402-list-submission.json")
    path = ROOT / relative
    if not path.exists():
        return [f"{relative}: missing registry submission body"]
    try:
        body = json.loads(path.read_text(encoding="utf-8"))
    except (json.JSONDecodeError, OSError) as error:
        return [f"{relative}: invalid JSON: {error}"]
    if not isinstance(body, dict):
        return [f"{relative}: submission body must be a JSON object"]

    errors: list[str] = []
    required = {
        "type",
        "email",
        "facilitator_name",
        "website_url",
        "settler_addresses",
        "networks",
    }
    allowed = required | {
        "description",
        "facilitator_id_slug",
        "token_claims",
        "claimed_volume_usd",
        "notes",
    }
    missing = sorted(required - body.keys())
    unknown = sorted(body.keys() - allowed)
    if missing:
        errors.append(f"{relative}: missing required fields: {', '.join(missing)}")
    if unknown:
        errors.append(f"{relative}: unknown fields: {', '.join(unknown)}")
    if body.get("type") != "facilitator":
        errors.append(f'{relative}: type must be "facilitator"')

    email = body.get("email")
    if not isinstance(email, str) or re.fullmatch(r"[^@\s]+@[^@\s]+", email) is None:
        errors.append(f"{relative}: email must be a complete submission contact")

    website = body.get("website_url")
    parsed_website = urlparse(website) if isinstance(website, str) else None
    if (
        parsed_website is None
        or parsed_website.scheme != "https"
        or parsed_website.hostname != "x402.mikedotexe.com"
    ):
        errors.append(f"{relative}: website_url must use the owned HTTPS facilitator domain")

    addresses = body.get("settler_addresses")
    if not isinstance(addresses, list) or not 1 <= len(addresses) <= 25:
        errors.append(f"{relative}: settler_addresses must contain 1 to 25 entries")
        addresses = []
    evm_address = re.compile(r"0x[0-9a-f]{40}")
    if any(not isinstance(value, str) or evm_address.fullmatch(value) is None for value in addresses):
        errors.append(f"{relative}: EVM settler addresses must be lowercase 20-byte hex")

    networks = body.get("networks")
    if (
        not isinstance(networks, list)
        or not 1 <= len(networks) <= 25
        or not {"base", "near"}.issubset(networks)
    ):
        errors.append(f'{relative}: networks must declare both "base" and "near"')

    slug = body.get("facilitator_id_slug")
    if not isinstance(slug, str) or re.fullmatch(
        r"[a-z0-9]+(?:-[a-z0-9]+)*", slug
    ) is None:
        errors.append(f"{relative}: facilitator_id_slug has an invalid format")

    evidence_relative = Path("docs/evidence/2026-07-26-v041-base-mainnet-canary.md")
    try:
        evidence = (ROOT / evidence_relative).read_text(encoding="utf-8").lower()
    except OSError as error:
        errors.append(f"{evidence_relative}: cannot read Base settlement evidence: {error}")
    else:
        if any(address not in evidence for address in addresses):
            errors.append(f"{relative}: Base settler is absent from paid-flow evidence")

    notes = body.get("notes")
    if not isinstance(notes, str) or "x402-relayer2.mike.near" not in notes:
        errors.append(f"{relative}: notes must identify the NEAR named settlement account")
    if not isinstance(notes, str) or "2026-07-26-v041-base-mainnet-canary.md" not in notes:
        errors.append(f"{relative}: notes must link the sanitized Base paid-flow evidence")
    if not isinstance(notes, str) or "2026-07-26-v051-reference-deployment.md" not in notes:
        errors.append(f"{relative}: notes must link the current deployment evidence")
    return errors


def admin_command_doc_errors() -> list[str]:
    """Keep the demo's client-create flag aligned with the Clap parser."""
    documentation_relative = Path("deploy/demo/README.md")
    admin_relative = Path("crates/x402-near-facilitator/src/bin/admin.rs")
    try:
        documentation = (ROOT / documentation_relative).read_text(encoding="utf-8")
        admin_source = (ROOT / admin_relative).read_text(encoding="utf-8")
    except OSError as error:
        return [f"admin command documentation: cannot read input: {error}"]

    errors: list[str] = []
    legacy_flag = "--daily-budget-yocto-near"
    canonical_flag = "--daily-yocto-near"
    if legacy_flag in documentation:
        errors.append(f"{documentation_relative}: obsolete {legacy_flag} flag")
    if canonical_flag not in documentation:
        errors.append(f"{documentation_relative}: missing {canonical_flag} flag")
    if re.search(
        r"#\[arg\(long\)\]\s+daily_yocto_near: Option<String>", admin_source
    ) is None:
        errors.append(
            f"{admin_relative}: client-create parser no longer exposes {canonical_flag}"
        )
    return errors


def main() -> int:
    os.chdir(ROOT)
    paths = tracked_files()
    errors = (
        markdown_link_errors(paths)
        + secret_errors(paths)
        + registry_submission_errors()
        + admin_command_doc_errors()
    )
    if errors:
        print("\n".join(errors), file=sys.stderr)
        return 1
    print("documentation links, registry submission, admin command, and secret-file guard passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
