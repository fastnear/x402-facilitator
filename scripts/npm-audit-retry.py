#!/usr/bin/env python3
"""Run npm audit with retries for registry transport failures.

Real advisory failures are returned immediately. Transient registry failures and
hung audit requests are retried so CI does not fail because a single npm audit
endpoint request returned 503 or stopped responding.
"""

from __future__ import annotations

import os
import subprocess
import sys
import time

RETRY_PATTERNS = (
    "audit endpoint returned an error",
    "Service Unavailable",
    "Gateway Timeout",
    "ECONNRESET",
    "ETIMEDOUT",
    "EAI_AGAIN",
)


def main() -> int:
    if len(sys.argv) not in (2, 3):
        print(f"usage: {sys.argv[0]} <package-prefix> [audit-level]", file=sys.stderr)
        return 2

    package_prefix = sys.argv[1]
    audit_level = sys.argv[2] if len(sys.argv) == 3 else "high"
    max_attempts = int(os.environ.get("NPM_AUDIT_RETRY_ATTEMPTS", "3"))
    timeout_seconds = int(os.environ.get("NPM_AUDIT_RETRY_TIMEOUT_SECONDS", "180"))

    command = [
        "npm",
        "--prefix",
        package_prefix,
        "audit",
        f"--audit-level={audit_level}",
    ]

    for attempt in range(1, max_attempts + 1):
        try:
            completed = subprocess.run(
                command,
                check=False,
                stdout=subprocess.PIPE,
                stderr=subprocess.STDOUT,
                text=True,
                timeout=timeout_seconds,
            )
        except subprocess.TimeoutExpired as error:
            if error.output:
                print(error.output, end="" if error.output.endswith("\n") else "\n")
            if attempt == max_attempts:
                print(
                    f"npm audit for {package_prefix} timed out after "
                    f"{timeout_seconds}s on attempt {attempt} of {max_attempts}",
                    file=sys.stderr,
                )
                return 1
            print(
                f"npm audit for {package_prefix} timed out after {timeout_seconds}s; "
                f"retrying attempt {attempt + 1} of {max_attempts}",
                file=sys.stderr,
            )
            time.sleep((attempt + 1) * 5)
            continue

        output = completed.stdout or ""
        print(output, end="" if output.endswith("\n") or not output else "\n")
        if completed.returncode == 0:
            return 0

        retryable = any(pattern.lower() in output.lower() for pattern in RETRY_PATTERNS)
        if retryable and attempt < max_attempts:
            print(
                f"npm audit for {package_prefix} failed due to registry transport error; "
                f"retrying attempt {attempt + 1} of {max_attempts}",
                file=sys.stderr,
            )
            time.sleep((attempt + 1) * 5)
            continue

        return completed.returncode

    return 1


if __name__ == "__main__":
    raise SystemExit(main())
