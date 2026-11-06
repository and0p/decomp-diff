# Generates this collection: three boot roots with very deep chains of short idle nodes.
# Usage: python3 generate.py <collection dir>   (reads ../thwaite/collection.toml next to it)
import os, random, sys, uuid, shutil
out = sys.argv[1]
rnd = random.Random(7)
os.makedirs(f"{out}/replays", exist_ok=True)
src = open(f"{out}/../thwaite/collection.toml").read()
src = src.replace('name = "Thwaite"', 'name = "Thwaite (deep tree)"').replace('file = "build/thwaite.dbg"', 'file = "../thwaite/build/thwaite.dbg"')
open(f"{out}/collection.toml", "w").write(src)
open(f"{out}/.gitignore", "w").write(".local/\n")
count = 0
def node(parent, title, frames, lines=()):
    global count
    count += 1
    i = str(uuid.UUID(int=rnd.getrandbits(128), version=4))
    body = "\n".join(lines)
    open(f"{out}/replays/{i}.replay", "w").write(
        f"dj-replay 1\nid: {i}\nparent: {parent or 'none'}\ntitle: {title}\nframes: {frames}\n---\n" + (body + "\n" if body else ""))
    return i
def tap(btn, at=2):
    return [f"{at} P1 JOYPAD {btn} 1", f"{at + 3} P1 JOYPAD {btn} 0"]
def idle_chain(parent, n, label, start=1, events_every=0):
    """n idle nodes in a row; returns the ids."""
    ids = []
    for k in range(start, start + n):
        f = rnd.choice([4, 6, 8, 10, 12, 16, 20])
        lines = [f"0 [EVENT] {label} {k}"] if events_every and k % events_every == 0 else []
        parent = node(parent, f"{label} {k}: wait {f}f", f, lines)
        ids.append(parent)
    return ids

# Boot: START at the title screen reaches the main menu.
boot = ["10 P1 JOYPAD START 1", "14 P1 JOYPAD START 0"]

# A: one very long straight chain, with two long side chains.
a = node(None, "A. Boot to the menu, cursor on 1 Player", 120, boot)
main = idle_chain(a, 150, "A idle", events_every=25)
for at, n in [(40, 30), (100, 20)]:
    s = node(main[at - 1], f"A side at {at}: tap DOWN", 10, tap("DOWN"))
    idle_chain(s, n, f"A{at} idle")

# B: a chain that forks every 20 levels into a few short chains, some moving the cursor.
b = node(None, "B. Boot to the menu, cursor on 2 Players", 130, boot + tap("DOWN", 100))
trunk = idle_chain(b, 80, "B idle")
for at in range(20, 81, 20):
    for j, (btn, n) in enumerate([("UP", 8), ("DOWN", 12), (None, 5)]):
        first = node(trunk[at - 1], f"B fork {at}.{j + 1}: " + (f"tap {btn}" if btn else "just wait"), 12, tap(btn) if btn else [f"0 [EVENT] fork {at}.{j + 1}"])
        idle_chain(first, n, f"B{at}.{j + 1} idle")

# C: a shorter stem that fans out wide, with forks nested inside forks.
c = node(None, "C. Boot to the menu, cursor on Practice", 140, boot + tap("DOWN", 100) + tap("DOWN", 112))
stem = idle_chain(c, 40, "C idle")
def fan(parent, depth, path):
    if depth == 0:
        return
    for j in range(1, 4 if depth > 1 else 3):
        leg = idle_chain(node(parent, f"C fork {path}{j}", 8, [f"0 [EVENT] fork {path}{j}"]), rnd.randint(6, 15), f"C{path}{j} idle")
        if j == 1:
            fan(leg[-1], depth - 1, f"{path}{j}.")
fan(stem[-1], 4, "")
print(count, "nodes")
