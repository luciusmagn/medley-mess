#!/usr/bin/env python3
"""Scan a live Maiko/Medley GC table from outside Lisp.

This is intentionally read-only.  It reads /proc/<pid>/mem, locates Maiko's
GC table pointers from the ldex symbol table, and summarizes the saturated
HTCOLL collision table without evaluating forms inside a GC-disabled image.
"""

import argparse
import collections
import os
import re
import struct
import subprocess
import sys

DEFAULT_LDEX = "/home/mag/src/maiko/linux.x86_64/ldex"

TYPE_NAMES = {
    0: "ARRAYBLOCK",
    1: "SMALLP",
    2: "FIXP",
    3: "FLOATP",
    4: "LITATOM",
    5: "LISTP",
    6: "ARRAYP",
    7: "STRINGP",
    8: "STACKP",
    9: "CHARACTERP",
    10: "VMEMPAGEP",
    11: "STREAM",
    12: "BITMAP",
    13: "COMPILED_CLOSURE",
    14: "ONED_ARRAY",
    15: "TWOD_ARRAY",
    16: "GENERAL_ARRAY",
    17: "BIGNUM",
    18: "RATIO",
    19: "COMPLEX",
    20: "PATHNAME",
    21: "NEWATOM",
}
for i in range(31, 44):
    TYPE_NAMES[i] = f"PTRHUNK{i - 30}"
for i in range(44, 64):
    TYPE_NAMES[i] = f"UNBOXEDHUNK{i - 43}"
for i in range(64, 78):
    TYPE_NAMES[i] = f"CODEHUNK{i - 63}"

SYMBOLS = {
    "HTmain",
    "HTbigcount",
    "HTcoll",
    "MDStypetbl",
    "GcDisabled_word",
    "Reclaim_cnt_word",
    "ReclaimMin_word",
    "DTDspace",
    "MaxTypeNumber_word",
    "MDS_free_page_word",
    "Next_MDSpage_word",
    "LASTVMEMFILEPAGE_word",
}

HTMAIN_ENTRIES = 0x8000
HTCOLL_MAX = (0x100000 // 2) - 16


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", nargs="?", type=int, help="ldex PID; auto-detected if omitted")
    parser.add_argument("--ldex", default=DEFAULT_LDEX, help="path to the ldex binary")
    parser.add_argument("--top", type=int, default=40, help="number of type rows to print")
    return parser.parse_args()


def find_pid(ldex_path):
    candidates = []
    for entry in os.listdir("/proc"):
        if not entry.isdigit():
            continue
        pid = int(entry)
        try:
            cmdline = open(f"/proc/{pid}/cmdline", "rb").read().replace(b"\0", b" ").decode("utf-8", "replace")
        except OSError:
            continue
        if ldex_path in cmdline and "apps.sysout" in cmdline:
            stat = open(f"/proc/{pid}/stat", "r", encoding="utf-8", errors="replace").read().split()
            start_ticks = int(stat[21]) if len(stat) > 21 else 0
            candidates.append((start_ticks, pid))
    if not candidates:
        raise SystemExit("no running ldex Medley process found; pass PID explicitly")
    candidates.sort(reverse=True)
    return candidates[0][1]


def load_symbols(ldex_path):
    try:
        out = subprocess.check_output(["nm", "-an", ldex_path], text=True)
    except (OSError, subprocess.CalledProcessError) as exc:
        raise SystemExit(f"failed to run nm on {ldex_path}: {exc}")
    found = {}
    pattern = re.compile(r"^([0-9a-fA-F]+)\s+\S\s+(\S+)$")
    for line in out.splitlines():
        match = pattern.match(line)
        if not match:
            continue
        addr = int(match.group(1), 16)
        name = match.group(2)
        if name in SYMBOLS:
            found[name] = addr
    missing = sorted(SYMBOLS - set(found))
    if missing:
        raise SystemExit(f"missing symbols in {ldex_path}: {', '.join(missing)}")
    return found


def find_exe_base(pid, ldex_path):
    with open(f"/proc/{pid}/maps", "r", encoding="utf-8", errors="replace") as maps:
        for line in maps:
            parts = line.split()
            if len(parts) >= 6 and parts[5] == ldex_path and parts[2] == "00000000":
                return int(parts[0].split("-")[0], 16)
    raise SystemExit(f"could not find executable mapping for {ldex_path} in pid {pid}")


class Mem:
    def __init__(self, pid):
        self.file = open(f"/proc/{pid}/mem", "rb", buffering=0)

    def read(self, addr, size):
        self.file.seek(addr)
        return self.file.read(size)

    def u16(self, addr):
        return struct.unpack("<H", self.read(addr, 2))[0]

    def u32(self, addr):
        return struct.unpack("<I", self.read(addr, 4))[0]

    def u64(self, addr):
        return struct.unpack("<Q", self.read(addr, 8))[0]

    def i32(self, addr):
        return struct.unpack("<i", self.read(addr, 4))[0]


def small_value(value):
    tag = value & 0x0FFF0000
    if tag == 0x000E0000:
        return value & 0xFFFF
    if tag == 0x000F0000:
        return value | ~0xFFFF
    return None


def count_of(content):
    return (content >> 17) & 0x7FFF


def seg_of(content):
    return (content & 0xFFFE) >> 1


def ptr_of(content, index):
    return ((seg_of(content) & 0xFFFF) << 16) | ((index << 1) & 0xFFFF)


def bucket_count(count):
    if count == 0:
        return "0"
    if count == 1:
        return "1"
    if count == 2:
        return "2"
    if count <= 4:
        return "3-4"
    if count <= 8:
        return "5-8"
    if count <= 16:
        return "9-16"
    if count <= 64:
        return "17-64"
    if count <= 1024:
        return "65-1024"
    return "1025+"


def scan_gc(mem, ptrs):
    htmain = ptrs["HTmain"]
    htcoll = ptrs["HTcoll"]
    mdst = ptrs["MDStypetbl"]

    def coll_at(offset):
        return mem.u32(htcoll + offset * 4)

    def main_at(index):
        return mem.u32(htmain + index * 4)

    def type_number(ptr):
        if ptr == 0 or ptr > 0x0FFFFFFF:
            return None
        try:
            return mem.u16(mdst + ((ptr >> 9) * 2)) & 0x7FF
        except OSError:
            return None

    free = set()
    free_bad = False
    offset = coll_at(0)
    guard = 0
    while offset:
        if offset & 1 or offset + 1 >= HTCOLL_MAX or guard > HTCOLL_MAX:
            free_bad = True
            break
        free.add(offset)
        offset = coll_at(offset + 1)
        guard += 1

    type_counts = collections.Counter()
    type_coll_counts = collections.Counter()
    type_zero_counts = collections.Counter()
    count_buckets = collections.Counter()
    samples = collections.defaultdict(list)
    chain_lengths = []
    visited_coll = set()
    bad_chains = 0
    main_nonzero = 0
    collision_heads = 0
    coll_slots = 0
    coll_nonzero = 0

    def add_entry(content, index, coll_offset):
        nonlocal coll_nonzero, main_nonzero
        count = count_of(content)
        ptr = ptr_of(content, index)
        typ = type_number(ptr)
        count_buckets[bucket_count(count)] += 1
        if count:
            type_counts[typ] += 1
            if coll_offset is None:
                main_nonzero += 1
            else:
                coll_nonzero += 1
                type_coll_counts[typ] += 1
        else:
            type_zero_counts[typ] += 1
        if len(samples[typ]) < 5:
            samples[typ].append((ptr, count, coll_offset, index))

    for index in range(HTMAIN_ENTRIES):
        content = main_at(index)
        if content == 0:
            continue
        if content & 1:
            collision_heads += 1
            offset = content & 0x0FFFFFFE
            chain_len = 0
            while offset:
                if offset in visited_coll or offset in free or offset & 1 or offset + 1 >= HTCOLL_MAX:
                    bad_chains += 1
                    break
                visited_coll.add(offset)
                chain_len += 1
                coll_slots += 1
                add_entry(coll_at(offset), index, offset)
                offset = coll_at(offset + 1)
            if chain_len:
                chain_lengths.append(chain_len)
        else:
            add_entry(content, index, None)

    highwater_links = (coll_at(1) - 2) // 2 if coll_at(1) > 2 else 0
    return {
        "htcoll_head": coll_at(0),
        "htcoll_next": coll_at(1),
        "htcoll_max": HTCOLL_MAX,
        "htcoll_highwater_links": highwater_links,
        "htcoll_free_links": len(free),
        "htcoll_live_links": highwater_links - len(free),
        "free_bad": free_bad,
        "main_nonzero": main_nonzero,
        "collision_heads": collision_heads,
        "coll_slots": coll_slots,
        "coll_nonzero": coll_nonzero,
        "visited_coll": len(visited_coll),
        "bad_chains": bad_chains,
        "chain_lengths": chain_lengths,
        "type_counts": type_counts,
        "type_coll_counts": type_coll_counts,
        "type_zero_counts": type_zero_counts,
        "count_buckets": count_buckets,
        "samples": samples,
    }


def read_runtime_words(mem, ptrs):
    names = ["GcDisabled_word", "Reclaim_cnt_word", "ReclaimMin_word", "MaxTypeNumber_word",
             "MDS_free_page_word", "Next_MDSpage_word", "LASTVMEMFILEPAGE_word"]
    values = {}
    for name in names:
        ptr = ptrs.get(name)
        if ptr:
            values[name] = mem.u32(ptr)
    return values


def print_report(pid, base, ptrs, words, scan, top):
    print(f"pid={pid} base=0x{base:x}")
    print("pointers")
    for name in sorted(ptrs):
        print(f"  {name}=0x{ptrs[name]:x}")
    print("runtime")
    for name, value in words.items():
        small = small_value(value)
        suffix = f" small={small}" if small is not None else ""
        print(f"  {name}=0x{value:08x}{suffix}")
    print("htcoll")
    print(
        "  head={htcoll_head} next={htcoll_next} max={htcoll_max} "
        "hi-links={htcoll_highwater_links} free-links={htcoll_free_links} "
        "live-links={htcoll_live_links} free-bad={free_bad}".format(**scan)
    )
    print("walk")
    print(
        "  main-nonzero={main_nonzero} collision-heads={collision_heads} "
        "coll-slots={coll_slots} coll-nonzero={coll_nonzero} "
        "visited-coll={visited_coll} bad-chains={bad_chains}".format(**scan)
    )
    chains = sorted(scan["chain_lengths"])
    if chains:
        p50 = chains[len(chains) // 2]
        p95 = chains[int(len(chains) * 0.95)]
        p99 = chains[int(len(chains) * 0.99)]
        avg = sum(chains) / len(chains)
        print(f"chains count={len(chains)} max={max(chains)} avg={avg:.2f} p50={p50} p95={p95} p99={p99}")
    print("count-buckets")
    for label in ["0", "1", "2", "3-4", "5-8", "9-16", "17-64", "65-1024", "1025+"]:
        print(f"  {label}={scan['count_buckets'][label]}")
    print("top-types")
    for typ, count in scan["type_counts"].most_common(top):
        name = TYPE_NAMES.get(typ, f"TYPE{typ}") if typ is not None else "UNKNOWN"
        coll = scan["type_coll_counts"][typ]
        zero = scan["type_zero_counts"][typ]
        sample_text = " ".join(f"0x{ptr:x}:{cnt}" for ptr, cnt, _, _ in scan["samples"][typ][:3])
        print(f"  {typ!s:>4} {name:<18} total={count:7d} coll={coll:7d} zero-seen={zero:7d} sample={sample_text}")


def main():
    args = parse_args()
    pid = args.pid or find_pid(args.ldex)
    symbols = load_symbols(args.ldex)
    base = find_exe_base(pid, args.ldex)
    mem = Mem(pid)
    ptrs = {name: mem.u64(base + offset) for name, offset in symbols.items()}
    words = read_runtime_words(mem, ptrs)
    scan = scan_gc(mem, ptrs)
    print_report(pid, base, ptrs, words, scan, args.top)


if __name__ == "__main__":
    main()
