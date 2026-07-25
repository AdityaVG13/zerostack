#!/usr/bin/env python3
"""One-time bd(Dolt) -> br(SQLite) reconcile.

br/SQLite is authoritative for issues it already has; bd/Dolt contributes
issues br is missing. comments[].id is normalized to i64 because bd emits
strings (digits in engine repos, UUIDv7 in the hub) and br's deserializer
requires an integer.
"""
import json, sys, collections

def load(path):
    out = {}
    dupes = 0
    with open(path) as fh:
        for line in fh:
            line = line.strip()
            if not line:
                continue
            rec = json.loads(line)
            if rec.get("_type") not in (None, "issue"):
                continue
            rid = rec.get("id")
            if rid is None:
                continue
            if rid in out:
                dupes += 1
            out[rid] = rec
    return out, dupes

def norm_comments(rec, counter):
    cs = rec.get("comments")
    if not isinstance(cs, list):
        return
    for c in cs:
        if not isinstance(c, dict):
            continue
        cid = c.get("id")
        if isinstance(cid, int):
            continue
        if isinstance(cid, str) and cid.isdigit():
            c["id"] = int(cid)
            counter["digits"] += 1
        else:
            counter["synth"] += 1
            c["id"] = counter["next"]
            counter["next"] += 1

def ts(rec):
    return rec.get("updated_at") or rec.get("created_at") or ""

def main(br_path, bd_path, out_path):
    br, br_dupes = load(br_path)
    bd, bd_dupes = load(bd_path)
    counter = collections.Counter()
    counter["next"] = 1_000_000

    merged = {}
    stats = collections.Counter()
    for rid in set(br) | set(bd):
        a, b = br.get(rid), bd.get(rid)
        if a and not b:
            merged[rid] = a; stats["br_only"] += 1
        elif b and not a:
            merged[rid] = b; stats["bd_only_added"] += 1
        else:
            # br/SQLite is authoritative. bd bulk-stamped updated_at during its
            # own init, so a newer bd timestamp does not imply newer content.
            merged[rid] = a
            stats["br_kept"] += 1
            if ts(b) > ts(a):
                stats["br_kept_despite_newer_bd_ts"] += 1

    for rec in merged.values():
        norm_comments(rec, counter)

    with open(out_path, "w") as fh:
        for rid in sorted(merged):
            fh.write(json.dumps(merged[rid], sort_keys=True) + "\n")

    print(json.dumps({
        "br_in": len(br), "bd_in": len(bd), "merged_out": len(merged),
        "br_dupe_lines": br_dupes, "bd_dupe_lines": bd_dupes,
        "comment_ids_from_digits": counter["digits"],
        "comment_ids_synthesized": counter["synth"],
        **{k: v for k, v in stats.items()},
    }, indent=2))

if __name__ == "__main__":
    main(*sys.argv[1:4])
