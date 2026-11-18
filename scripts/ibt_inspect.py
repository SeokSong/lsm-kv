#!/usr/bin/env python3
# looks inside an iRacing .ibt telemetry file: header, channels, session info, a few samples.
# usage: python3 -I scripts/ibt_inspect.py <file.ibt> [--channels] [--yaml] [--samples N] [--laps]
# layout (irsdk): 112-byte header, 32-byte disk sub header, 144-byte var headers,
# session info yaml, then fixed-size sample rows
import struct, sys

TYPES = {0: ("char", 1, "c"), 1: ("bool", 1, "?"), 2: ("int", 4, "i"),
         3: ("bitfield", 4, "I"), 4: ("float", 4, "f"), 5: ("double", 8, "d")}

def read(path):
    data = open(path, "rb").read()
    h = struct.unpack_from("<10i", data, 0)
    header = dict(zip(["ver", "status", "tick_rate", "session_info_update", "session_info_len",
                       "session_info_offset", "num_vars", "var_header_offset", "num_buf", "buf_len"], h))
    # first var buffer: tick count, offset of the first sample row
    header["buf_offset"] = struct.unpack_from("<i", data, 48 + 4)[0]
    start_date, start_time, end_time, lap_count, record_count = struct.unpack_from("<qddii", data, 112)
    header.update(start_date=start_date, start_time=start_time, end_time=end_time,
                  lap_count=lap_count, record_count=record_count, file_size=len(data))
    variables = []
    for i in range(header["num_vars"]):
        base = header["var_header_offset"] + i * 144
        vtype, offset, count = struct.unpack_from("<3i", data, base)
        name, desc, unit = (data[base + a:base + b].split(b"\0")[0].decode("latin-1")
                            for a, b in ((16, 48), (48, 112), (112, 144)))
        variables.append(dict(name=name, type=vtype, offset=offset, count=count, desc=desc, unit=unit))
    yaml = data[header["session_info_offset"]:header["session_info_offset"] + header["session_info_len"]]
    return data, header, variables, yaml.split(b"\0")[0].decode("latin-1")

def sample(data, header, variables, row, names):
    base = header["buf_offset"] + row * header["buf_len"]
    out = {}
    for v in variables:
        if v["name"] in names and v["count"] == 1:
            out[v["name"]] = struct.unpack_from("<" + TYPES[v["type"]][2], data, base + v["offset"])[0]
    return out

def main():
    path = sys.argv[1]
    data, header, variables, yaml = read(path)
    rows_by_size = (header["file_size"] - header["buf_offset"]) // header["buf_len"]
    print(f"file: {path}")
    for k, v in header.items():
        print(f"  {k}: {v}")
    print(f"  rows (from file size): {rows_by_size}, seconds at tick rate: {rows_by_size / header['tick_rate']:.0f}")
    print(f"  channels: {len(variables)}, bytes per row: {header['buf_len']}, session info: {len(yaml)} chars")
    if "--channels" in sys.argv:
        for v in variables:
            print(f"    {v['name']:32s} {TYPES[v['type']][0]:8s} x{v['count']:<3d} {v['unit']:12s} {v['desc']}")
    if "--yaml" in sys.argv:
        print(yaml)
    if "--laps" in sys.argv:
        laps(data, header, variables, rows_by_size)
    if "--samples" in sys.argv:
        n = int(sys.argv[sys.argv.index("--samples") + 1])
        names = {"SessionTime", "Lap", "LapDist", "LapDistPct", "Speed", "Throttle", "Brake",
                 "SteeringWheelAngle", "Gear", "RPM", "Lat", "Lon", "LapCurrentLapTime"}
        step = max(1, rows_by_size // n)
        for row in range(0, rows_by_size, step)[:n]:
            print("   ", {k: round(v, 3) if isinstance(v, float) else v
                          for k, v in sample(data, header, variables, row, names).items()})

def laps(data, header, variables, rows):
    # per lap: rows, first / last LapDistPct, on track, pit road, and the time iRacing reports
    names = {"SessionTime", "Lap", "LapDistPct", "IsOnTrack", "OnPitRoad", "LapLastLapTime"}
    current, first, prev = None, None, None
    stats = {}
    boundary = []
    for row in range(rows):
        s = sample(data, header, variables, row, names)
        lap = s["Lap"]
        st = stats.setdefault(lap, {"rows": 0, "first": s, "last": s, "off": 0, "pit": 0})
        st["rows"] += 1
        st["last"] = s
        st["off"] += not s["IsOnTrack"]
        st["pit"] += s["OnPitRoad"]
        if prev is not None and lap != prev["Lap"]:
            boundary.append((row, prev, s))
        prev = s
    for lap, st in sorted(stats.items()):
        print(f"  lap {lap:>3}: rows {st['rows']:>6}  pct {st['first']['LapDistPct']:.4f} -> {st['last']['LapDistPct']:.4f}"
              f"  time {st['first']['SessionTime']:8.2f} -> {st['last']['SessionTime']:8.2f}"
              f"  off track rows {st['off']:>5}  pit road rows {st['pit']:>5}  last lap time after: {st['last']['LapLastLapTime']:.3f}")
    print("  lap changes (pct before -> after):")
    for row, before, after in boundary:
        print(f"    row {row}: lap {before['Lap']} -> {after['Lap']}  pct {before['LapDistPct']:.4f} -> {after['LapDistPct']:.4f}"
              f"  t {after['SessionTime']:.3f}")

if __name__ == "__main__":
    main()
