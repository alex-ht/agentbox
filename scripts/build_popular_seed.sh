#!/bin/sh
# Rebuild the bundled CrUX seeds. Needs network. Not used by tests.
# Latest month in zakird/crux-top-lists as of 2026-11-11 is 202610.
# Country directories have no current.csv.gz; global does, and it matched 202610.
set -eu
month=202610
root=$(CDPATH= cd -- "$(dirname "$0")/.." && pwd)
mkdir -p "$root/target/crux"
tw="$root/target/crux/tw-$month.csv.gz"
gl="$root/target/crux/global-$month.csv.gz"
curl -fsSL -o "$tw" "https://raw.githubusercontent.com/zakird/crux-top-lists/main/data/country/tw/$month.csv.gz"
curl -fsSL -o "$gl" "https://raw.githubusercontent.com/zakird/crux-top-lists/main/data/global/$month.csv.gz"
home=$(mktemp -d)
export AGENTBOX_HOME="$home"
cargo run --quiet --release -- kb import --crux "$tw" --scope tw --max-rank 5000 --verify --enrich --write-seed "$root/target/crux/popular_tw.json"
cargo run --quiet --release -- kb import --crux "$gl" --scope global --max-rank 1000 --verify --enrich --write-seed "$root/target/crux/popular_global.json"
python3 - << PY
import gzip, pathlib
root = pathlib.Path("$root")
for name in ("popular_tw", "popular_global"):
    raw = (root / "target/crux" / f"{name}.json").read_bytes()
    out = root / "src/kb" / f"{name}.json.gz"
    out.write_bytes(gzip.compress(raw, mtime=0))
    print(f"{out.name} raw {len(raw)} gz {out.stat().st_size}")
PY
