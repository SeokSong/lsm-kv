# generates md/diagrams/threads-and-locks.drawio (2 pages: plain language, technical).
# usage: python3 scripts/gen_threads_diagram.py md/diagrams/threads-and-locks.drawio
# overwrites the file: edits made in the draw.io app are lost, make them here instead
import sys
from xml.sax.saxutils import quoteattr

class Page:
    def __init__(self, name):
        self.name = name; self.cells = []; self.n = 0
    def nid(self):
        self.n += 1; return f"{self.name[:3].lower()}{self.n}"
    def box(self, x, y, w, h, label, style):
        i = self.nid()
        self.cells.append(f'<mxCell id="{i}" value={quoteattr(label)} style={quoteattr(style)} vertex="1" parent="1"><mxGeometry x="{x}" y="{y}" width="{w}" height="{h}" as="geometry"/></mxCell>')
        return i
    def edge(self, s, t, label="", style=""):
        i = self.nid()
        base = "edgeStyle=orthogonalEdgeStyle;rounded=1;html=1;endArrow=block;endFill=1;strokeWidth=2;fontSize=12;labelBackgroundColor=#ffffff;"
        self.cells.append(f'<mxCell id="{i}" value={quoteattr(label)} style={quoteattr(base + style)} edge="1" parent="1" source="{s}" target="{t}"><mxGeometry relative="1" as="geometry"/></mxCell>')
        return i
    def xml(self):
        body = "".join(self.cells)
        return f'<diagram id="{self.name[:3].lower()}" name={quoteattr(self.name)}><mxGraphModel dx="1400" dy="900" grid="1" gridSize="10" guides="1" tooltips="1" connect="1" arrows="1" fold="1" page="1" pageScale="1" pageWidth="1400" pageHeight="1100" math="0" shadow="0"><root><mxCell id="0"/><mxCell id="1" parent="0"/>{body}</root></mxGraphModel></diagram>'

R = "rounded=1;whiteSpace=wrap;html=1;arcSize=8;fontSize=13;align=left;spacingLeft=10;spacingRight=10;verticalAlign=middle;"
TITLE = "text;html=1;fontSize=24;fontStyle=1;align=left;verticalAlign=top;"
SUB = "text;html=1;fontSize=14;align=left;verticalAlign=top;whiteSpace=wrap;fontColor=#555555;"
HEAD = "text;html=1;fontSize=16;fontStyle=1;align=left;verticalAlign=middle;"
BLUE = "fillColor=#dae8fc;strokeColor=#6c8ebf;"
GREEN = "fillColor=#d5e8d4;strokeColor=#82b366;"
ORANGE = "fillColor=#ffe6cc;strokeColor=#d79b00;dashed=1;"
YELLOW = "fillColor=#fff2cc;strokeColor=#d6b656;"
PURPLE = "fillColor=#e1d5e7;strokeColor=#9673a6;"
GREY = "fillColor=#f5f5f5;strokeColor=#666666;"
RED = "fillColor=#f8cecc;strokeColor=#b85450;"

# ---------------- page 1: plain language ----------------
p = Page("1. Overview (plain language)")
p.box(40, 20, 1000, 40, "How lsm-kv shares work between threads", TITLE)
p.box(40, 62, 1300, 40, "Several things happen at the same time inside the store. This page shows who does what, which shared things they touch, and the rules that stop them from getting stuck. The next page is the technical version.", SUB)

p.box(40, 120, 400, 30, "Who does the work", HEAD)
p.box(900, 120, 460, 30, "Shared things (each has one key, only one holder at a time)", HEAD)

app = p.box(40, 160, 400, 110, "<b>Your app</b> (any number at once)<br>Saves data (put, delete) and looks it up (get)", R + BLUE)
fl = p.box(40, 330, 400, 110, "<b>Flush worker</b> (one, always running)<br>When the in-memory table is full, writes it to disk as a sorted file", R + GREEN)
cp = p.box(40, 500, 400, 110, "<b>Compaction worker</b> (one, coming in week 4)<br>Tidies the disk files: merges small files into bigger sorted ones and throws away old versions", R + ORANGE)

j = p.box(900, 160, 460, 110, "<b>1. The journal</b> (write-ahead log)<br>Every save is written here first, so nothing is lost if the power goes out. One save at a time", R + YELLOW)
c = p.box(900, 330, 460, 110, "<b>2. The catalog</b> (manifest)<br>The official list of which files belong to the store. Updated when a file is added or removed", R + YELLOW)
s = p.box(900, 500, 460, 110, "<b>3. The shelf</b> (in-memory state)<br>The data still in memory, and the list of files to read. Only quick memory work while holding this key", R + YELLOW)

p.edge(app, j, "saves go here first", "exitX=1;exitY=0.3;entryX=0;entryY=0.3;")
p.edge(app, s, "looks up data, adds saved data", "exitX=1;exitY=0.85;entryX=0;entryY=0.15;")
p.edge(app, c, "only when starting a new journal page", "exitX=1;exitY=0.6;entryX=0;entryY=0.2;dashed=1;")
p.edge(fl, c, "adds the new file", "exitX=1;exitY=0.5;entryX=0;entryY=0.5;")
p.edge(fl, s, "makes the new file readable", "exitX=1;exitY=0.8;entryX=0;entryY=0.45;")
p.edge(cp, c, "swaps old files for merged ones", "exitX=1;exitY=0.2;entryX=0;entryY=0.8;dashed=1;")
p.edge(cp, s, "swaps the file list", "exitX=1;exitY=0.6;entryX=0;entryY=0.75;dashed=1;")
p.box(470, 300, 400, 30, "The workers never touch the journal: only your app does", "text;html=1;fontSize=12;fontStyle=2;align=center;fontColor=#b85450;")

p.box(40, 650, 1300, 30, "Why rules are needed: a freeze (deadlock)", HEAD)
p.box(40, 690, 1320, 70, "A freeze happens when two workers each hold a key the other one needs, and both wait for the other to let go. Neither ever does, so the store stops with no error and no crash. The rules below make that impossible.", R + RED)

p.box(40, 790, 1300, 30, "The four rules that keep it from freezing", HEAD)
p.box(40, 830, 310, 120, "<b>Same order, always</b><br>Keys are always picked up in the same order: journal, then catalog, then shelf. Never backwards", R + PURPLE)
p.box(370, 830, 310, 120, "<b>Don't wait while holding</b><br>Nobody waits for a worker while holding the catalog or the shelf key", R + PURPLE)
p.box(700, 830, 310, 120, "<b>Workers stay out of the journal</b><br>So a save waiting on a worker can never block that worker", R + PURPLE)
p.box(1030, 830, 330, 120, "<b>Reading never waits on disk</b><br>Looking data up only takes the shelf key, and only for quick memory work, never a disk write", R + PURPLE)

p.box(40, 970, 1320, 50, "Legend: solid line = happens today. Dashed line = only sometimes, or planned (compaction, week 4). Dashed box = not built yet.", SUB)

# ---------------- page 2: technical ----------------
t = Page("2. Threads and locks (technical)")
t.box(40, 20, 1200, 40, "Threads, locks, and waits in Store", TITLE)
t.box(40, 62, 1300, 40, "Lock order is WAL -> Manifest -> State, top to bottom on the right. A thread may skip a lock, never take one above a lock it holds. Source: src/store.rs, rules in md/CONCURRENCY.md.", SUB)

t.box(40, 115, 430, 30, "Threads", HEAD)
t.box(900, 115, 460, 30, "Locks (in lock order)", HEAD)

put = t.box(40, 155, 430, 120, "<b>Caller threads: put / delete</b> (N)<br>1. lock WAL (held for the whole put)<br>2. if the memtable is full: State wait for a frozen slot, then Manifest save, then State swap<br>3. append + fsync, then State insert", R + BLUE)
get = t.box(40, 300, 430, 85, "<b>Caller threads: get</b> (N)<br>State: look up active memtable, clone frozen + SSTable Arcs, unlock<br>then search frozen memtables + SSTables with no lock", R + BLUE)
fth = t.box(40, 410, 430, 120, "<b>Flush thread</b> (1)<br>1. State: take oldest frozen memtable not waiting on a retry<br>2. write + fsync + open SSTable, no lock<br>3. Manifest: add SSTable, drop segment, save<br>4. delete segment, no lock<br>5. State: install reader, notify_all", R + GREEN)
cth = t.box(40, 555, 430, 135, "<b>Compaction thread</b> (1, planned PR 4d)<br>1. State: pick inputs (L0 >= 4 files or a level over target),<br>   never L0 files newer than the oldest waiting memtable<br>2. merge + write outputs, no lock<br>3. Manifest: add outputs, remove inputs, save<br>4. State: swap SSTable lists, notify_all<br>5. delete inputs, no lock (open Arcs keep them readable)", R + ORANGE)

wal = t.box(900, 155, 460, 110, "<b>1. Mutex&lt;WalState&gt;</b><br>WAL writer (active segment), segment number, next seq<br><i>held: a whole put, including the fsync</i><br><i>taken by: put / delete only</i>", R + YELLOW)
man = t.box(900, 300, 460, 120, "<b>2. Mutex&lt;Manifest&gt;</b><br>live WAL segments, SSTables (+ level, planned), next file number, last seq<br><i>held: change a copy + save (tmp, fsync, rename, fsync dir)</i><br><i>taken by: put at a swap, flush, compaction</i>", R + YELLOW)
st = t.box(900, 455, 460, 150, "<b>3. Mutex&lt;State&gt; + Condvar state_changed</b><br>active memtable, frozen queue (VecDeque, oldest first), SSTables (newest max_seq first, by level planned), failed, closing, flush_error<br><i>held: memory-only steps, never file I/O</i><br><i>taken by: every thread</i>", R + YELLOW)

t.edge(put, wal, "1. whole put", "exitX=1;exitY=0.15;entryX=0;entryY=0.3;")
t.edge(put, man, "2. only at a swap", "exitX=1;exitY=0.5;entryX=0;entryY=0.2;")
t.edge(put, st, "wait / swap / insert", "exitX=1;exitY=0.85;entryX=0;entryY=0.1;")
t.edge(get, st, "lookup + clone Arcs", "exitX=1;exitY=0.5;entryX=0;entryY=0.3;")
t.edge(fth, man, "step 3", "exitX=1;exitY=0.3;entryX=0;entryY=0.6;")
t.edge(fth, st, "steps 1, 5", "exitX=1;exitY=0.7;entryX=0;entryY=0.55;")
t.edge(cth, man, "step 3", "exitX=1;exitY=0.2;entryX=0;entryY=0.9;dashed=1;")
t.edge(cth, st, "steps 1, 4", "exitX=1;exitY=0.6;entryX=0;entryY=0.85;dashed=1;")
t.box(490, 420, 390, 40, "flush + compaction threads never take the WAL lock (rule 2)", "text;html=1;fontSize=12;fontStyle=3;align=center;fontColor=#b85450;whiteSpace=wrap;")

t.box(40, 715, 640, 30, "Waits (all on state_changed, which releases State while asleep)", HEAD)
t.box(40, 750, 640, 135, "<b>put</b> waits for a free frozen slot (queue full) ... woken by the flush thread<br><b>flush thread</b> waits for a frozen memtable, or a retry time ... woken by a swapping put<br><b>compaction thread</b> waits for L0 &gt;= 4 or a level over target ... woken by flush, and by itself<br><b>everyone</b> is woken by failed / closing (rule 7)<br>Always in a loop: while !ready &amp;&amp; !failed &amp;&amp; !closing { wait } (rule 6)", R + GREY)

t.box(700, 715, 660, 30, "Rules (md/CONCURRENCY.md)", HEAD)
t.box(700, 750, 660, 300, "1. Lock order WAL -&gt; Manifest -&gt; State, skipping is fine, never backwards<br>2. Flush thread never takes WAL<br>3. Never wait for the flush thread holding Manifest or State<br>4. No file I/O under State<br>5. Readers never take WAL or Manifest<br>6. Wait in a loop (spurious wakeups, another writer took the slot)<br>7. Every waiter has a way out: failed / closing -&gt; notify_all<br>8. Hold WAL across append + memtable insert (same order in WAL and memtable)<br>9. Join threads with no lock held<br>10. Poisoned lock = store failed<br><b>11. (planned) Compaction thread never takes WAL</b><br><b>12. (planned) Nobody waits for the compaction thread</b><br><b>13. (planned) L0 -&gt; L1 never takes an L0 file newer than the oldest waiting memtable, and always takes every older overlapping L0 file</b>", R + PURPLE)

t.box(40, 905, 640, 30, "No lock held", HEAD)
t.box(40, 940, 640, 110, "Writing an SSTable (tmp, fsync, rename, fsync dir)<br>Reading SSTables and frozen memtables (Arc keeps them alive)<br>Deleting a WAL segment or an old SSTable (an open Arc keeps an unlinked file readable)<br>Joining the flush / compaction thread in close", R + GREY)

xml = '<mxfile host="app.diagrams.net" version="24.0.0">' + p.xml() + t.xml() + '</mxfile>'
open(sys.argv[1], "w").write(xml)
