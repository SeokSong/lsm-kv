#!/usr/bin/env python3
# where a track's official F1 sector lines are, as a fraction of the lap (lap_dist_pct).
# OpenF1 gives each lap's sector times but not where the lines are, so: for every clean lap,
# find where the car was when sector 1 and 2 ended, as a fraction of that lap's driven
# distance (from the x / y positions). prints each lap and the median.
#
# usage: python3 -I scripts/openf1_sector_lines.py <session_key> [driver_number ...]
#        python3 -I scripts/openf1_sector_lines.py 9141 1 16 44
# downloads are cached in data/openf1/ (git ignored)
import bisect, json, math, os, statistics, sys, urllib.request
from datetime import datetime

CACHE = os.path.join(os.path.dirname(__file__), "..", "data", "openf1")

def get(endpoint, **params):
    query = "&".join(f"{k}={v}" for k, v in params.items())
    path = os.path.join(CACHE, f"{endpoint}_{query.replace('&', '_').replace('=', '-')}.json")
    if not os.path.exists(path):
        os.makedirs(CACHE, exist_ok=True)
        with urllib.request.urlopen(f"https://api.openf1.org/v1/{endpoint}?{query}", timeout=120) as r:
            data = r.read()
        with open(path, "wb") as f:
            f.write(data)
    with open(path) as f:
        return json.load(f)

def ts(text):
    return datetime.fromisoformat(text).timestamp()

def sector_lines(session, driver):
    laps = get("laps", session_key=session, driver_number=driver)
    loc = [p for p in get("location", session_key=session, driver_number=driver) if p["x"] or p["y"]]
    times = [ts(p["date"]) for p in loc]
    # cumulative driven distance along the x / y trace
    dist = [0.0]
    for a, b in zip(loc, loc[1:]):
        dist.append(dist[-1] + math.hypot(b["x"] - a["x"], b["y"] - a["y"]))

    def dist_at(t):
        i = bisect.bisect_left(times, t)
        if i <= 0 or i >= len(times):
            return None
        f = (t - times[i - 1]) / (times[i] - times[i - 1])
        return dist[i - 1] + f * (dist[i] - dist[i - 1])

    rows = []
    for lap in laps:
        d = [lap.get(f"duration_sector_{n}") for n in (1, 2, 3)]
        if not lap.get("date_start") or None in d or lap.get("is_pit_out_lap"):
            continue
        start = ts(lap["date_start"])
        points = [dist_at(start), dist_at(start + d[0]), dist_at(start + d[0] + d[1]),
                  dist_at(start + sum(d))]
        if None in points:
            continue
        length = points[3] - points[0]
        rows.append((lap["lap_number"], (points[1] - points[0]) / length,
                     (points[2] - points[0]) / length, length))
    return rows

def main():
    session = sys.argv[1]
    drivers = sys.argv[2:] or ["1"]
    s1, s2 = [], []
    for driver in drivers:
        rows = sector_lines(session, driver)
        # first and last laps are often odd (start, safety car): use the middle of the race
        for lap, a, b, length in rows:
            print(f"driver {driver:>2} lap {lap:>2}: s1 end {a:.4f}  s2 end {b:.4f}  lap length {length:.0f}")
            s1.append(a)
            s2.append(b)
    print(f"\nmedian over {len(s1)} laps: sector 2 starts at {statistics.median(s1):.4f}, "
          f"sector 3 starts at {statistics.median(s2):.4f}")
    print(f"spread (p10-p90): s1 {statistics.quantiles(s1, n=10)[0]:.4f}-{statistics.quantiles(s1, n=10)[-1]:.4f}, "
          f"s2 {statistics.quantiles(s2, n=10)[0]:.4f}-{statistics.quantiles(s2, n=10)[-1]:.4f}")

if __name__ == "__main__":
    main()
