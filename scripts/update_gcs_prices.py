#!/usr/bin/env python3
"""Regenerate crates/providers/src/gcs/prices.rs from the public Cloud Storage pricing page.

Reads https://cloud.google.com/storage/pricing (no credentials needed), which embeds
the storage prices of every location as data for its location picker, and writes the
per-GiB-month list prices that CloudDirStat uses for its monthly cost estimates, plus
the price of Class A operations (which include listing objects) per storage class.

The page is not an API, so the parsing is checked against prices that rarely change:
if Google redesigns the page, this script stops with an error instead of writing
nonsense.

Usage: python scripts/update_gcs_prices.py
"""

import datetime
import html
import re
import urllib.request
from pathlib import Path

PAGE = "https://cloud.google.com/storage/pricing?hl=en"
OUTPUT = Path(__file__).resolve().parent.parent / "crates/providers/src/gcs/prices.rs"
CLASSES = ["standard", "nearline", "coldline", "archive"]

# One location's entry in the embedded data: its prices, then its name and code.
ENTRY = re.compile(r'"([^"\[\]]{2,80}? \(([a-z0-9-]+)\))",\[(\d+)\]\]')
# The list of location names that starts each block (regions, dual-regions, ...).
BLOCK_START = re.compile(r'(?:"[^"\[\]]{2,80}? \([a-z0-9-]+\)",){4,}"[^"\[\]]{2,80}? \([a-z0-9-]+\)"')
HEADING = re.compile(r"\\u003cp\\u003e([^\\]*?) storage\\u003c/p\\u003e")
# "$0.02 / 1 gibibyte month", sometimes followed by a free-tier note.
MONTHLY_PRICE = re.compile(r'"\$([0-9.]+) / 1 gibibyte month[^"]*"')


def download():
    request = urllib.request.Request(PAGE, headers={"User-Agent": "Mozilla/5.0"})
    with urllib.request.urlopen(request) as response:
        return response.read().decode("utf-8", errors="replace")


def storage_blocks(page):
    """Monthly prices per location code, for each block of the embedded data."""
    starts = [match.start() for match in BLOCK_START.finditer(page)]
    blocks = {}
    previous = 0
    for match in ENTRY.finditer(page):
        entry = page[previous : match.start()]
        previous = match.end()
        headings = [heading.lower() for heading in HEADING.findall(entry)]
        prices = [float(price) for price in MONTHLY_PRICE.findall(entry)]
        # Columns without a price (e.g. no Rapid Cache in a region) come last.
        if not prices or len(prices) > len(headings):
            continue
        block = sum(1 for start in starts if start < match.start())
        by_class = {c: p for c, p in zip(headings, prices) if c in CLASSES}
        blocks.setdefault(block, {})[match.group(2)] = by_class
    return blocks


def class_a_prices(page):
    """Class A price per 1,000 operations (flat namespace) for each storage class."""
    for table in re.findall(r"<table.*?</table>", page, re.S):
        if "Class A operations" not in table:
            continue
        prices = {}
        for row in re.findall(r"<tr.*?</tr>", table, re.S):
            cells = [
                html.unescape(re.sub(r"\s+", " ", re.sub(r"<[^>]+>", "", cell))).strip()
                for cell in re.findall(r"<t[dh].*?</t[dh]>", row, re.S)
            ]
            if len(cells) > 1 and cells[1].startswith("$"):
                name = cells[0].split()[0].lower()
                if name in CLASSES:
                    prices[name] = float(cells[1][1:])
        if len(prices) == len(CLASSES):
            return prices
    raise SystemExit("could not find the Class A operations table")


def pick_blocks(blocks):
    """The regional block and the dual- and multi-region block, found by what they hold."""
    regional = next(b for b in blocks.values() if "us-central1" in b and "us" not in b and len(b) > 30)
    multi = next(b for b in blocks.values() if {"us", "eu", "asia", "nam4"} <= b.keys())
    return regional, multi


def check(regional, multi, class_a):
    """Stops if the parsed prices do not look like Cloud Storage prices."""
    expected = {
        (id(regional), "us-central1", "standard"): 0.02,
        (id(multi), "us", "standard"): 0.026,
    }
    for (table, location, storage_class), price in expected.items():
        prices = regional if table == id(regional) else multi
        found = prices.get(location, {}).get(storage_class)
        if found is None or not 0.5 * price <= found <= 2 * price:
            raise SystemExit(f"{location} {storage_class}: parsed {found}, expected about {price}")
    if not 0.001 <= class_a["standard"] <= 0.05:
        raise SystemExit(f"Class A Standard: parsed {class_a['standard']}")


def rust_option(value):
    return "None" if value is None else f"Some({value!r})"


def rust_table(name, prices):
    lines = [f"pub const {name}: &[LocationPrices] = &["]
    for location in sorted(prices):
        lines.append("    LocationPrices {")
        lines.append(f'        location: "{location}",')
        for storage_class in CLASSES:
            lines.append(f"        {storage_class}: {rust_option(prices[location].get(storage_class))},")
        lines.append("    },")
    lines.append("];")
    return "\n".join(lines)


def main():
    page = download()
    regional, multi = pick_blocks(storage_blocks(page))
    class_a = class_a_prices(page)
    check(regional, multi, class_a)

    today = datetime.date.today().isoformat()
    class_a_fields = "\n".join(f"    {c}: {class_a[c]!r}," for c in CLASSES)
    source = f"""// Generated by scripts/update_gcs_prices.py from the Cloud Storage pricing page. Do not
// edit by hand. Storage prices are USD per GiB-month; Class A prices per 1,000 operations.

use super::pricing::{{ClassPrices, LocationPrices}};

pub const PUBLISHED: &str = "{today}";

/// Class A operations (which include listing objects), by the bucket's default class.
pub const CLASS_A_PER_1000: ClassPrices = ClassPrices {{
{class_a_fields}
}};

/// Regions, by region code.
{rust_table("REGIONS", regional)}

/// Multi-regions (us, eu, asia), predefined dual-regions (nam4, eur4, ...), and
/// configurable dual-regions by the region code of their first region.
{rust_table("MULTI_REGIONS", multi)}
"""
    OUTPUT.write_text(source, encoding="utf-8", newline="\n")
    print(f"wrote {OUTPUT} ({len(regional)} regions, {len(multi)} dual- and multi-regions)")


if __name__ == "__main__":
    main()
