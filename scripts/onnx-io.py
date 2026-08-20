#!/usr/bin/env python3
"""Dump an ONNX graph's inputs/outputs without the `onnx` package.

Only walks the protobuf wire format far enough to reach GraphProto's `input`
(field 11) and `output` (field 12); initializers are skipped by length, so a
150 MB model costs a scan, not a parse.
"""
import mmap, sys

ELEM = {0: "undef", 1: "f32", 2: "u8", 3: "i8", 4: "u16", 5: "i16", 6: "i32",
        7: "i64", 8: "str", 9: "bool", 10: "f16", 11: "f64", 12: "u32",
        13: "u64", 14: "c64", 15: "c128", 16: "bf16"}


def varint(buf, i):
    r = s = 0
    while True:
        b = buf[i]; i += 1
        r |= (b & 0x7F) << s
        if not (b & 0x80):
            return r, i
        s += 7


def fields(buf, start, end):
    """Yield (field_no, wire_type, payload_start, payload_end) in [start,end)."""
    i = start
    while i < end:
        key, i = varint(buf, i)
        fno, wt = key >> 3, key & 7
        if wt == 0:
            v, j = varint(buf, i); yield fno, wt, i, j; i = j
        elif wt == 1:
            yield fno, wt, i, i + 8; i += 8
        elif wt == 2:
            ln, j = varint(buf, i); yield fno, wt, j, j + ln; i = j + ln
        elif wt == 5:
            yield fno, wt, i, i + 4; i += 4
        else:
            raise ValueError(f"wire type {wt} at {i}")


def parse_shape(buf, s, e):
    dims = []
    for fno, _, ps, pe in fields(buf, s, e):
        if fno != 1:
            continue
        d = "?"
        for f2, _, s2, e2 in fields(buf, ps, pe):
            if f2 == 1:
                d = varint(buf, s2)[0]
            elif f2 == 2:
                d = buf[s2:e2].decode("utf8", "replace")
        dims.append(d)
    return dims


def parse_value_info(buf, s, e):
    name, elem, dims = "?", None, []
    for fno, _, ps, pe in fields(buf, s, e):
        if fno == 1:
            name = buf[ps:pe].decode("utf8", "replace")
        elif fno == 2:                                    # TypeProto
            for f2, _, s2, e2 in fields(buf, ps, pe):
                if f2 != 1:                               # tensor_type
                    continue
                for f3, _, s3, e3 in fields(buf, s2, e2):
                    if f3 == 1:
                        elem = varint(buf, s3)[0]
                    elif f3 == 2:
                        dims = parse_shape(buf, s3, e3)
    return name, ELEM.get(elem, str(elem)), dims


def dump(path):
    with open(path, "rb") as fh:
        buf = mmap.mmap(fh.fileno(), 0, access=mmap.ACCESS_READ)
    n = len(buf)
    graph = None
    producer = ""
    for fno, _, s, e in fields(buf, 0, n):
        if fno == 7:
            graph = (s, e)
        elif fno == 2:
            producer = buf[s:e].decode("utf8", "replace")
    if graph is None:
        print("  no GraphProto"); return
    ins, outs = [], []
    for fno, _, s, e in fields(buf, *graph):
        if fno == 11:
            ins.append(parse_value_info(buf, s, e))
        elif fno == 12:
            outs.append(parse_value_info(buf, s, e))
    if producer:
        print(f"  producer: {producer}")
    for nm, el, dm in ins:
        print(f"  IN   {nm:<24} {el:<5} {dm}")
    for nm, el, dm in outs:
        print(f"  OUT  {nm:<24} {el:<5} {dm}")


if __name__ == "__main__":
    for p in sys.argv[1:]:
        print(f"=== {p.rsplit('/', 1)[-1]} ===")
        try:
            dump(p)
        except Exception as exc:
            print(f"  parse failed: {exc}")
