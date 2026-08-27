#!/usr/bin/env python3
"""Dump SQLite pragma/schema/index inventory for FSZero perf db/ artifacts.

Writes under <out>/db/ (or <out>/<label>/ when dumping multiple DBs):

  pragma_dump.txt   -- page_count, page_size, freelist, journal_mode, ...
  schema.sql        -- sqlite_master CREATE statements
  index_list.txt    -- PRAGMA index_list / index_info per table

Covers durable fsqlite store.sqlite3 and the rusqlite AST sidecar
(store.sqlite3.ast) when present.

Usage:
  uv run python scripts/sqlite_db_profile_dump.py \\
    --db .zerostack/fszero/store.sqlite3 \\
    --out tests/artifacts/perf/$RUN_ID

  uv run python scripts/sqlite_db_profile_dump.py \\
    --store-root .zerostack \\
    --out tests/artifacts/perf/$RUN_ID

Requires sqlite3 CLI on PATH. Read-only (opens file as-is; does not migrate).
Bead: fszero-sqlite-pragma-schema-dump-mxrg
"""
from __future__ import annotations

import argparse
import json
import shutil
import subprocess
import sys
from pathlib import Path

PRAGMAS = (
    "page_count",
    "page_size",
    "freelist_count",
    "journal_mode",
    "synchronous",
    "cache_size",
    "wal_autocheckpoint",
    "mmap_size",
    "auto_vacuum",
    "encoding",
    "user_version",
    "application_id",
    "schema_version",
    "busy_timeout",
    "locking_mode",
    "temp_store",
    "compile_options",
)


def find_sqlite3() -> str:
    path = shutil.which("sqlite3")
    if not path:
        raise SystemExit("sqlite3 CLI not found on PATH")
    return path


def run_sql(sqlite3: str, db: Path, sql: str) -> tuple[int, str, str]:
    run = subprocess.run(
        [sqlite3, str(db), sql],
        capture_output=True,
        text=True,
        timeout=60,
        check=False,
    )
    return run.returncode, run.stdout, run.stderr


def dump_one(sqlite3: str, db: Path, out_dir: Path, label: str) -> dict[str, object]:
    out_dir.mkdir(parents=True, exist_ok=True)
    meta: dict[str, object] = {
        "label": label,
        "db_path": str(db),
        "exists": db.is_file(),
        "size_bytes": db.stat().st_size if db.is_file() else None,
    }
    if not db.is_file():
        meta["status"] = "missing"
        (out_dir / "MISSING.txt").write_text(
            f"database not found: {db}\n", encoding="utf-8"
        )
        return meta

    # pragma_dump.txt
    lines: list[str] = [
        f"# FSZero SQLite pragma dump",
        f"# label={label}",
        f"# db={db}",
        f"# size_bytes={db.stat().st_size}",
        "",
    ]
    for name in PRAGMAS:
        code, out, err = run_sql(sqlite3, db, f"PRAGMA {name};")
        if code != 0:
            lines.append(f"{name}: ERROR exit={code} {err.strip()}")
        else:
            value = out.strip().replace("\n", " | ")
            lines.append(f"{name}: {value}")

    # WAL/SHM sidecar sizes
    for suffix in ("-wal", "-shm", "-journal"):
        side = Path(str(db) + suffix)
        if side.is_file():
            lines.append(f"sidecar{suffix}_bytes: {side.stat().st_size}")
        else:
            lines.append(f"sidecar{suffix}_bytes: 0")

    (out_dir / "pragma_dump.txt").write_text("\n".join(lines) + "\n", encoding="utf-8")

    # schema.sql
    code, out, err = run_sql(
        sqlite3,
        db,
        "SELECT sql || ';' FROM sqlite_master WHERE sql IS NOT NULL "
        "ORDER BY type, name;",
    )
    schema = out if code == 0 else f"-- ERROR exit={code}: {err}"
    (out_dir / "schema.sql").write_text(schema, encoding="utf-8")

    # index_list.txt
    code, tables_out, err = run_sql(
        sqlite3,
        db,
        "SELECT name FROM sqlite_master WHERE type='table' "
        "AND name NOT LIKE 'sqlite_%' ORDER BY name;",
    )
    idx_lines: list[str] = [
        f"# index_list / index_info for {db}",
        "",
    ]
    if code != 0:
        idx_lines.append(f"ERROR listing tables: {err.strip()}")
    else:
        tables = [t for t in tables_out.splitlines() if t.strip()]
        for table in tables:
            idx_lines.append(f"## table {table}")
            c2, iout, ierr = run_sql(sqlite3, db, f"PRAGMA index_list('{table}');")
            if c2 != 0:
                idx_lines.append(f"index_list ERROR: {ierr.strip()}")
            else:
                idx_lines.append("index_list:")
                idx_lines.append(iout.rstrip() or "(none)")
                # index_info for each index name (col0 of index_list is seq, col1 name)
                for line in iout.splitlines():
                    parts = line.split("|")
                    if len(parts) >= 2:
                        iname = parts[1]
                        c3, iinfo, _ = run_sql(
                            sqlite3, db, f"PRAGMA index_info('{iname}');"
                        )
                        idx_lines.append(f"index_info {iname}:")
                        idx_lines.append(
                            iinfo.rstrip() if c3 == 0 else f"ERROR exit={c3}"
                        )
            idx_lines.append("")
    (out_dir / "index_list.txt").write_text(
        "\n".join(idx_lines) + "\n", encoding="utf-8"
    )

    meta["status"] = "ok"
    meta["artifacts"] = [
        "pragma_dump.txt",
        "schema.sql",
        "index_list.txt",
    ]
    return meta


def resolve_dbs(args: argparse.Namespace) -> list[tuple[str, Path]]:
    if args.db:
        db = Path(args.db).resolve()
        pairs = [("store", db)]
        ast = Path(str(db) + ".ast")
        if ast.is_file() or args.include_missing_ast:
            pairs.append(("ast_sidecar", ast))
        return pairs

    store_root = Path(args.store_root).resolve()
    candidates: list[tuple[str, Path]] = []
    # Common layouts
    for rel, label in (
        ("fszero/store.sqlite3", "store"),
        ("projects", None),  # expand below
    ):
        if label:
            p = store_root / rel
            candidates.append((label, p))
    # Per-project DBs
    projects = store_root / "projects"
    if projects.is_dir():
        for proj in sorted(projects.iterdir()):
            db = proj / "fszero" / "store.sqlite3"
            if db.is_file():
                candidates.append((f"project_{proj.name}", db))

    pairs: list[tuple[str, Path]] = []
    seen: set[Path] = set()
    for label, db in candidates:
        if db in seen:
            continue
        seen.add(db)
        if db.is_file() or label == "store":
            pairs.append((label, db))
            ast = Path(str(db) + ".ast")
            if ast.is_file() or args.include_missing_ast:
                pairs.append((f"{label}_ast", ast))
    return pairs


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    src = parser.add_mutually_exclusive_group(required=True)
    src.add_argument("--db", type=Path, help="path to store.sqlite3 (also probes .ast)")
    src.add_argument(
        "--store-root",
        type=Path,
        help="store root (e.g. .zerostack) — finds fszero/store.sqlite3 + projects/*",
    )
    parser.add_argument(
        "--out",
        type=Path,
        required=True,
        help="run artifact dir; writes db/ or db/<label>/",
    )
    parser.add_argument(
        "--include-missing-ast",
        action="store_true",
        help="also write MISSING.txt for absent AST sidecars",
    )
    args = parser.parse_args()
    sqlite3 = find_sqlite3()
    pairs = resolve_dbs(args)
    if not pairs:
        raise SystemExit("no databases resolved")

    out_root = Path(args.out).resolve() / "db"
    results = []
    multi = len(pairs) > 1
    for label, db in pairs:
        dest = out_root / label if multi else out_root
        results.append(dump_one(sqlite3, db, dest, label))

    summary = {
        "schema_version": "fszero.sqlite-db-profile-dump.v1",
        "results": results,
    }
    out_root.mkdir(parents=True, exist_ok=True)
    (out_root / "summary.json").write_text(
        json.dumps(summary, indent=2, sort_keys=True) + "\n", encoding="utf-8"
    )
    print(json.dumps(summary, indent=2, sort_keys=True))
    if all(r.get("status") == "missing" for r in results):
        sys.exit(2)


if __name__ == "__main__":
    main()
