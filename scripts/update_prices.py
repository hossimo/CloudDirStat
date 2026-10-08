#!/usr/bin/env python3
"""Regenerate the S3, Google Cloud Storage, and Azure price tables in one go.

Runs update_s3_prices.py, update_gcs_prices.py, and update_azure_prices.py, keeps going
when one fails, and ends with a summary: which failed, and which tables have new prices
(rather than only a new date). Exits with status 1 if any failed.

Usage: python scripts/update_prices.py
"""

import subprocess
import sys
from pathlib import Path

SCRIPTS = Path(__file__).resolve().parent
CRATE = SCRIPTS.parent / "crates/providers/src"
PROVIDERS = [
    ("S3", "update_s3_prices.py", CRATE / "s3/prices.rs"),
    ("Google Cloud Storage", "update_gcs_prices.py", CRATE / "gcs/prices.rs"),
    ("Azure", "update_azure_prices.py", CRATE / "azure/prices.rs"),
]


def read(path):
    return path.read_text(encoding="utf-8") if path.exists() else None


def without_date(source):
    """The table without its PUBLISHED date, so a rerun with the same prices compares equal."""
    if source is None:
        return None
    return [line for line in source.splitlines() if "PUBLISHED" not in line]


def run(name, script, output):
    print(f"\n=== {name} ===", flush=True)
    before = read(output)
    result = subprocess.run([sys.executable, str(SCRIPTS / script)])
    if result.returncode != 0:
        return "FAILED (see the output above; the old table is unchanged)"
    after = read(output)
    if after == before:
        return "ok, no change"
    if without_date(after) == without_date(before):
        return "ok, same prices (only the date changed)"
    return "ok, prices CHANGED"


def main():
    results = [(name, run(name, script, output)) for name, script, output in PROVIDERS]

    print("\n=== Summary ===")
    width = max(len(name) for name, _ in results)
    for name, outcome in results:
        print(f"  {name:<{width}}  {outcome}")

    failed = [name for name, outcome in results if outcome.startswith("FAILED")]
    if failed:
        print(f"\n{len(failed)} of {len(results)} failed: {', '.join(failed)}")
        sys.exit(1)
    print("\nAll price tables updated. Review with: git diff crates/providers/src")


if __name__ == "__main__":
    main()
