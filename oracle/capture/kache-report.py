"""Classify the kache events of the last source-oracle build and fail when C/C++ compiles go uncached.

Runs in the builder image with the cache volume at /ccache and the work volume at /work. The build
records kache stats in /work/metadata/build-kache-before.json as it starts; events from then on
belong to that build. Prints one JSON object.
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
CACHED = {"miss", "dup", "local_hit"}


def events(cache: Path) -> Iterator[str]:
    archive = cache / "events.jsonl.archive"
    for path in sorted(archive.glob("*.jsonl.zst")) if archive.is_dir() else []:
        text = subprocess.run(["zstd", "-dc", str(path)], check=True, capture_output=True, text=True).stdout
        yield from text.splitlines()
    current = cache / "events.jsonl"
    if current.exists():
        yield from current.read_text(encoding="utf-8").splitlines()


def main() -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--cache", type=Path, default=Path("/ccache/kache"), help="kache cache directory")
    parser.add_argument("--since-file", type=Path, default=Path("/work/metadata/build-kache-before.json"),
                        help="report events newer than this file's modification time")
    parser.add_argument("--max-uncached", type=float, default=0.01, help="largest tolerated uncached C/C++ share")
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
        result = event.get("result", "unknown")
        results[language][result] += 1
        if result not in CACHED:
            reason = event.get("passthrough_reason") or event.get("skip_reason") or event.get("miss_reason") or ""
            reasons[f"{language}: {reason[:160]}"] += 1
    c_family = results["c-family"]
    total = sum(c_family.values())
    uncached = total - sum(c_family[result] for result in CACHED)
    report = {
        "since": since.isoformat(),
        "results": {language: dict(counter) for language, counter in results.items()},
        "c_family_uncached_share": round(uncached / total, 4) if total else None,
        "top_uncached_reasons": reasons.most_common(10),
    }
    json.dump(report, sys.stdout, indent=2)
    print()
    if not total:
        print("kache-report: no C/C++ events since the build started", file=sys.stderr)
        return 1
    if uncached / total > args.max_uncached:
        print(f"kache-report: {uncached} of {total} C/C++ compiles were not cached", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
