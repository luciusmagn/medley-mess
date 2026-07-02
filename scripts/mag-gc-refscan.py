#!/usr/bin/env python3
"""Scan a live Maiko/Medley image for references to Lisp pointers.

This is a read-only companion to mag-gc-scan-live.py.  It is intentionally
simple: given one or more Lisp pointer values from the GC table report, scan the
mapped Lisp address space for 32-bit words equal to those pointers and summarize
the types of the containing pages.
"""

import argparse
import collections
import importlib.util
import os
import struct


HERE = os.path.dirname(os.path.abspath(__file__))
GC_SCAN = os.path.join(HERE, "mag-gc-scan-live.py")


def load_gc_scan():
    spec = importlib.util.spec_from_file_location("mag_gc_scan_live", GC_SCAN)
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    return module


def parse_args():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("pid", type=int, help="ldex PID")
    parser.add_argument("pointers", nargs="+", help="Lisp pointer values, e.g. 0x7504a4")
    parser.add_argument("--ldex", default=None, help="path to ldex binary")
    parser.add_argument("--limit", type=int, default=80, help="reference rows to print per pointer")
    parser.add_argument(
        "--scan-laddr-end",
        type=lambda value: int(value, 0),
        default=0x900000,
        help="exclusive Lisp laddr scan end; default covers loaded low MDS/array pages",
    )
    return parser.parse_args()


def mapped_ranges(pid, start, end):
    ranges = []
    with open(f"/proc/{pid}/maps", "r", encoding="utf-8", errors="replace") as maps:
        for line in maps:
            first = line.split(None, 1)[0]
            lo_text, hi_text = first.split("-")
            lo = int(lo_text, 16)
            hi = int(hi_text, 16)
            if hi <= start or lo >= end:
                continue
            ranges.append((max(lo, start), min(hi, end)))
    return ranges


def main():
    args = parse_args()
    scan = load_gc_scan()
    ldex = args.ldex or scan.DEFAULT_LDEX
    symbols = scan.load_symbols(ldex)
    base = scan.find_exe_base(args.pid, ldex)
    mem = scan.Mem(args.pid)
    ptrs = {name: mem.u64(base + offset) for name, offset in symbols.items()}
    world = scan.lisp_world_from_ptrs(ptrs)
    mdst = ptrs["MDStypetbl"]
    end_addr = world + args.scan_laddr_end * 2
    ranges = mapped_ranges(args.pid, world, end_addr)

    def laddr_from_native(addr):
        return (addr - world) // 2

    def type_number(laddr):
        try:
            return mem.lisp_u16(mdst + ((laddr >> 9) * 2)) & 0x7FF
        except OSError:
            return None

    print(
        f"pid={args.pid} world=0x{world:x} "
        f"scan-laddr-end=0x{args.scan_laddr_end:x} ranges={len(ranges)}"
    )
    for pointer_text in args.pointers:
        target = int(pointer_text, 0)
        pattern = struct.pack("<I", target)
        found = []
        for lo, hi in ranges:
            try:
                data = mem.read(lo, hi - lo)
            except OSError:
                continue
            start = 0
            while True:
                idx = data.find(pattern, start)
                if idx < 0:
                    break
                addr = lo + idx
                if (addr - world) % 2 == 0:
                    found.append(laddr_from_native(addr))
                start = idx + 1

        buckets = collections.Counter(type_number(ref) for ref in found)
        bucket_text = " ".join(
            f"{scan.TYPE_NAMES.get(typ, typ)}:{count}" for typ, count in buckets.most_common(12)
        )
        print(f"target=0x{target:x} refs={len(found)} type-buckets={bucket_text}")
        for ref in found[: args.limit]:
            typ = type_number(ref)
            print(
                f"  ref-laddr=0x{ref:x} "
                f"ref-type={scan.TYPE_NAMES.get(typ, typ)} "
                f"native=0x{world + ref * 2:x}"
            )


if __name__ == "__main__":
    main()
