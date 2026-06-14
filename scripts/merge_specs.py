#!/usr/bin/env python3
"""
Merge two instance-builder spec JSON files.

Usage:
  python3 scripts/merge_specs.py spec1.json spec2.json out.json

Rules:
  - spec2 has priority over spec1 for all fields.
  - For dictionaries, merge recursively (spec2 wins on conflicts).
  - For lists/other values (except "instances"), spec2 replaces spec1 when provided.
  - "instances" is merged specially:
      * Keep instances from spec1 whose name is NOT present in spec2.
      * Then append all instances from spec2.
    This ensures spec2's instance definitions override spec1 by name.
"""

from __future__ import annotations

import json
import sys
from pathlib import Path
from typing import Any, Dict, List, Tuple


def die(msg: str) -> None:
    print(f"error: {msg}", file=sys.stderr)
    raise SystemExit(2)


def read_json(path: Path) -> Any:
    try:
        raw = path.read_text(encoding="utf-8")
    except FileNotFoundError:
        die(f"file not found: {path}")
    except Exception as e:
        die(f"failed to read {path}: {e}")

    raw = raw.strip()
    if not raw:
        return {}
    try:
        return json.loads(raw)
    except Exception as e:
        die(f"failed to parse JSON {path}: {e}")


def instance_name(inst: Any) -> Tuple[bool, str]:
    if isinstance(inst, dict) and isinstance(inst.get("name"), str):
        return True, inst["name"]
    return False, ""


def merge_instances(spec1: Dict[str, Any], spec2: Dict[str, Any]) -> List[Any]:
    i1 = spec1.get("instances", [])
    i2 = spec2.get("instances", [])

    if not isinstance(i1, list):
        i1 = []
    if not isinstance(i2, list):
        i2 = []

    names2 = set()
    for inst in i2:
        ok, name = instance_name(inst)
        if ok:
            names2.add(name)

    out: List[Any] = []
    for inst in i1:
        ok, name = instance_name(inst)
        if ok and name in names2:
            continue
        out.append(inst)

    out.extend(i2)
    return out


def merge_value(v1: Any, v2: Any) -> Any:
    # spec2 wins if it provides a value; for dicts, merge recursively.
    if isinstance(v1, dict) and isinstance(v2, dict):
        return merge_dict(v1, v2)
    return v2


def merge_dict(d1: Dict[str, Any], d2: Dict[str, Any]) -> Dict[str, Any]:
    out: Dict[str, Any] = dict(d1)
    for k, v2 in d2.items():
        if k == "instances":
            # handled at top-level merge
            out[k] = v2
            continue
        if k in out:
            out[k] = merge_value(out[k], v2)
        else:
            out[k] = v2
    return out


def merge_specs(spec1: Any, spec2: Any) -> Dict[str, Any]:
    if not isinstance(spec1, dict):
        die("spec1 must be a JSON object")
    if not isinstance(spec2, dict):
        die("spec2 must be a JSON object")

    out = merge_dict(spec1, spec2)
    out["instances"] = merge_instances(spec1, spec2)
    return out


def main(argv: List[str]) -> int:
    if len(argv) != 4 or argv[1] in ("-h", "--help"):
        print(__doc__.strip())
        return 2

    spec1_path = Path(argv[1])
    spec2_path = Path(argv[2])
    out_path = Path(argv[3])

    spec1 = read_json(spec1_path)
    spec2 = read_json(spec2_path)
    merged = merge_specs(spec1, spec2)

    out_path.parent.mkdir(parents=True, exist_ok=True)
    out_path.write_text(
        json.dumps(merged, indent=2, ensure_ascii=False) + "\n", encoding="utf-8"
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv))
