"""The allocation trace's report from its file (caveman-fx alloctrace.rs appends to
%LOCALAPPDATA%\\Caveman\\fx\\alloc-trace-<seconds>.jsonl every two seconds, so a game that dies mid-trace still
leaves the allocations before it). Usage: python -I alloctrace_report.py [FILE] [--sites N] [--after S] [--before S]
With no FILE, the newest. Lines: {"alloc": i, ...}, {"free": i, ...}, {"sample": ms, "video", "system"}."""
import collections
import glob
import json
import os
import sys

args = [a for a in sys.argv[1:] if not a.startswith("--")]
opts = {sys.argv[i]: sys.argv[i + 1] for i in range(1, len(sys.argv) - 1) if sys.argv[i].startswith("--")}
path = args[0] if args else max(glob.glob(os.path.join(os.environ["LOCALAPPDATA"], "Caveman", "fx", "alloc-trace-*.jsonl")), key=os.path.getmtime)
after = float(opts.get("--after", 0)) * 1000
before = float(opts.get("--before", 1e12)) * 1000
sites_n = int(opts.get("--sites", 16))

allocs = {}
samples = []
for line in open(path, encoding="utf-8"):
    try:
        d = json.loads(line)
    except json.JSONDecodeError:
        continue  # a line cut short by the crash
    if "alloc" in d:
        d["freed"] = -1
        d["touched"] = -1
        d["copied_to"] = 0
        d["copies"] = 0
        allocs[d["alloc"]] = d
    elif "free" in d and d["free"] in allocs:
        a = allocs[d["free"]]
        a["freed"], a["touched"], a["copied_to"], a["copies"] = d["t"], d["touched"], d["copied_to"], d["copies"]
    elif "copy" in d and d["copy"] in allocs:
        a = allocs[d["copy"]]
        a["dst"] = d
        a["copies"] = max(a["copies"], 1)
    elif "sample" in d:
        samples.append((d["sample"], d["video"], d["system"]))

HEAPS = {0: "?", 1: "default (video memory)", 2: "upload (system memory)", 3: "readback", 4: "custom", 9: "placed (a heap of the game's)"}
MB = 1048576.0
rows = [a for a in allocs.values() if after <= a["t"] <= before]
print(f"{path}: {len(allocs)} allocation(s) in the file, {len(rows)} in the window, {len(samples)} watch samples")
if not rows:
    sys.exit()
total = sum(a["info"]["bytes"] for a in rows)
by_heap = collections.Counter()
for a in rows:
    by_heap[HEAPS.get(a["info"]["heap"], "?")] += a["info"]["bytes"]
print(f"  total {total / MB:.0f} MB; by heap: " + "; ".join(f"{v / MB:.0f} MB {k}" for k, v in by_heap.most_common()))
if samples:
    sysmem = [s[2] for s in samples]
    print(f"  system memory for the GPU: {sysmem[0] >> 20} MB first, {max(sysmem) >> 20} MB peak, {sysmem[-1] >> 20} MB last; video {samples[0][1] >> 20} -> {max(s[1] for s in samples) >> 20} MB peak")


def site_of(stack):
    game = [s for s in stack if s.startswith("Skate.exe")][:2]
    if not game:
        game = [s for s in stack if not s.startswith(("caveman_fx", "ReSkate", "D3D12", "d3d12", "nvwgf2umx", "KERNEL", "ntdll"))][:2]
    return " < ".join(game) or "(no game frame)"


sites = collections.defaultdict(lambda: {"n": 0, "bytes": 0, "max": 0, "ms": 0.0, "freed": 0, "life": 0.0, "held": 0, "copied": [], "touched": [], "stack": "", "heaps": collections.Counter(), "sizes": []})
for a in rows:
    s = sites[site_of(a["stack"])]
    b = a["info"]["bytes"]
    s["n"] += 1
    s["bytes"] += b
    s["max"] = max(s["max"], b)
    s["ms"] += a["ms"]
    s["sizes"].append(b)
    s["heaps"][HEAPS.get(a["info"]["heap"], "?")] += b
    if a["freed"] >= 0:
        s["freed"] += 1
        s["life"] += a["freed"] - a["t"]
    elif a["raw"] != "0x0":
        s["held"] += 1
    if a["copies"] > 0 and b:
        s["copied"].append(min(a["copied_to"], b) / b)
    if a["touched"] >= 0 and b:
        s["touched"].append(min(a["touched"], b) / b)
    if not s["stack"]:
        s["stack"] = " < ".join(x for x in a["stack"] if not x.startswith("caveman_fx"))[:230]


def pct(xs):
    return f"{100 * sum(xs) / len(xs):.0f}% of {len(xs)}" if xs else "?"


print("  by call site (the game's nearest two frames), the most bytes first:")
for key, s in sorted(sites.items(), key=lambda kv: -kv[1]["bytes"])[:sites_n]:
    life = f"{s['life'] / s['freed']:.0f} ms" if s["freed"] else "?"
    heaps = ", ".join(f"{v / MB:.0f} MB {k.split(' (')[0]}" for k, v in s["heaps"].most_common(2))
    print(f"    {s['n']:>5} x {s['bytes'] / MB:>8.1f} MB (largest {s['max'] / MB:>6.1f}; {s['ms'] / s['n']:>5.1f} ms each; freed {s['freed']} held {s['held']}, life {life}; copied {pct(s['copied'])}, touched {pct(s['touched'])}; {heaps})  {key}")
    print(f"          {s['stack']}")

# where the staging went: the first copy's destination per traced buffer, per site
KINDS = {0: "buffer", 1: "tex1D", 2: "tex2D", 3: "tex3D"}
dsts = collections.defaultdict(collections.Counter)
dst_res = collections.defaultdict(set)
for a in rows:
    d = a.get("dst")
    if not d:
        continue
    key = site_of(a["stack"])
    dsts[key][f"{KINDS.get(d['dst_kind'], '?')} {d['dst_w']}x{d['dst_h']}x{d['dst_d']} fmt {d['dst_format']} {d['dst_mips']} mip(s), sub {d['sub']}"] += 1
    dst_res[key].add(d["dst"])
if dsts:
    print("  where each site's buffers were copied to (first copy; distinct destination resources; the most common shapes):")
    for key, c in sorted(dsts.items(), key=lambda kv: -sum(kv[1].values()))[:8]:
        print(f"    {key}: {sum(c.values())} copies into {len(dst_res[key])} resource(s)")
        for shape, n in c.most_common(5):
            print(f"        {n:>5} x {shape}")

# the committed upload buffers: the staging burst's sizes in order, per thread
ups = sorted((a for a in rows if a["info"]["heap"] == 2 and a["info"]["kind"] == 0 and a["what"].startswith("CreateCommittedResource")), key=lambda a: a["t"])
if ups:
    print(f"  committed upload buffers: {len(ups)}, {sum(a['info']['bytes'] for a in ups) / MB:.0f} MB; by thread:")
    for tid, group in collections.OrderedDict((t, [u for u in ups if u["thread"] == t]) for t in dict.fromkeys(u["thread"] for u in ups)).items():
        sizes = [f"{u['info']['bytes'] / MB:.1f}" for u in group]
        lives = [u["freed"] - u["t"] for u in group if u["freed"] >= 0]
        print(f"    thread {tid}: {len(group)} buffer(s) from t={group[0]['t'] / 1000:.2f} to {group[-1]['t'] / 1000:.2f} s, {sum(u['ms'] for u in group):.0f} ms in the calls; lifetimes {min(lives) if lives else '?'}..{max(lives) if lives else '?'} ms ({len(lives)} freed of {len(group)})")
        print("      MB: " + " ".join(sizes[:40]) + (" ..." if len(sizes) > 40 else ""))

# the timeline per 250 ms where something happened
buckets = collections.defaultdict(lambda: [0, 0, 0])
for a in rows:
    b = int(a["t"] // 250)
    buckets[b][0] += 1
    buckets[b][1] += a["info"]["bytes"]
    if a["freed"] >= 0:
        buckets[int(a["freed"] // 250)][2] += a["info"]["bytes"]
watch = {}
for ms, v, s in samples:
    watch[int(ms // 250)] = (v, s)
print("  timeline (s: allocations, MB made, MB freed | video MB, system MB), busy quarter seconds only:")
for b in sorted(buckets):
    n, made, freed = buckets[b]
    if n == 0 and freed == 0:
        continue
    v, s = watch.get(b, (0, 0))
    print(f"    {b / 4:>7.2f}: {n:>4} {made / MB:>8.1f} {freed / MB:>8.1f} | {v >> 20:>6} {s >> 20:>6}")
