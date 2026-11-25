"""Classify the kache events of the last source-oracle build and fail when its compiles go uncached.

Runs in the builder image with the cache volume at /ccache and the work volume at /work. source-oracle
touches /work/metadata/kache-build-start before each build-target phase; events from then on belong
to that build. Two figures are gated separately:

- coverage: the share of C/C++ compiles kache could cache (stored, already stored, or hit), which
  measures supported invocations whatever the cache held;
- hits: with --min-hit-share, the share of cacheable C/C++ compiles answered from the cache, which a
  rebuild of unchanged inputs must reach.

A compile whose entry failed to store counts as uncached: the next build recompiles it. Prints one
JSON object.
"""

from __future__ import annotations

import argparse
import collections
import datetime
import json
import os
import re
import subprocess
import sys
from collections.abc import Iterator
from pathlib import Path

C_FAMILY = re.compile(r"\.(c|cc|cpp|cxx|C|m|mm|s|S)$")
STORED = {"miss", "dup"}
HIT = "local_hit"


def events(cache: Path) -> Iterator[str]:
    archive = cache / "events.jsonl.archive"
    for path in sorted(archive.glob("*.jsonl.zst")) if archive.is_dir() else []:
        text = subprocess.run(["zstd", "-dc", str(path)], check=True, capture_output=True, text=True).stdout
        yield from text.splitlines()
    current = cache / "events.jsonl"
    if current.exists():
        yield from current.read_text(encoding="utf-8").splitlines()


def outcome(event: dict) -> str:
    result = event.get("result", "unknown")
    if result in STORED and event.get("store_error"):
        return "store_failed"
    return result


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--cache", type=Path, default=Path("/ccache/kache"), help="kache cache directory")
    parser.add_argument("--since-file", type=Path, default=Path("/work/metadata/kache-build-start"),
                        help="report events newer than this file's modification time")
    parser.add_argument("--max-uncached", type=float, default=0.01, help="largest tolerated uncached C/C++ share")
    parser.add_argument("--min-hit-share", type=float, help="smallest tolerated hit share of cacheable C/C++ compiles")
    args = parser.parse_args()
    since = datetime.datetime.fromtimestamp(os.stat(args.since_file).st_mtime, datetime.UTC)
    results: dict[str, collections.Counter[str]] = {"c-family": collections.Counter(), "rust": collections.Counter()}
    reasons: collections.Counter[str] = collections.Counter()
    for line in events(args.cache):
        event = json.loads(line)
        stamp = datetime.datetime.fromisoformat(event["ts"].replace("Z", "+00:00"))
        if stamp < since:
            continue
        language = "c-family" if C_FAMILY.search(event.get("crate_name", "")) else "rust"
        result = outcome(event)
        results[language][result] += 1
        if result not in STORED | {HIT}:
            reason = (event.get("store_error") or event.get("passthrough_reason") or event.get("skip_reason")
                      or event.get("miss_reason") or "")
            reasons[f"{language} {result}: {reason[:160]}"] += 1
    c_family = results["c-family"]
    total = sum(c_family.values())
    cacheable = sum(c_family[result] for result in STORED) + c_family[HIT]
    report = {
        "since": since.isoformat(),
        "results": {language: dict(counter) for language, counter in results.items()},
        "c_family_uncached_share": round((total - cacheable) / total, 4) if total else None,
        "c_family_hit_share": round(c_family[HIT] / cacheable, 4) if cacheable else None,
        "top_uncached_reasons": reasons.most_common(10),
    }
    json.dump(report, sys.stdout, indent=2)
    print()
    if not total:
        print("kache-report: no C/C++ events since the build started", file=sys.stderr)
        return 1
    failed = False
    if (total - cacheable) / total > args.max_uncached:
        print(f"kache-report: {total - cacheable} of {total} C/C++ compiles were not cached", file=sys.stderr)
        failed = True
    if args.min_hit_share is not None and (not cacheable or c_family[HIT] / cacheable < args.min_hit_share):
        print(f"kache-report: {c_family[HIT]} of {cacheable} cacheable C/C++ compiles hit, "
              f"below {args.min_hit_share}", file=sys.stderr)
        failed = True
    return 1 if failed else 0


if __name__ == "__main__":
    raise SystemExit(main())
