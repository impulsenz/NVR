#!/usr/bin/env python3
"""CSS drift check: selectors defined more than once in static/app.css.

Layered rules (an old rule, then a newer one overriding it further down)
are how a style sheet accumulates dead declarations. Prints each selector
that appears at several places, with the line numbers.

Run from the repository root:  python3 tools/audit/css_duplicates.py
"""
import re
from collections import defaultdict

css = open("static/app.css").read()
# strip comments, remember line numbers
css_nc = re.sub(r"/\*.*?\*/", lambda m: "\n" * m.group(0).count("\n"), css, flags=re.S)
seen = defaultdict(list)
for m in re.finditer(r"(^|\})\s*([^{}@][^{}]*)\{", css_nc):
    sel = " ".join(m.group(2).split())
    if sel and not sel.startswith(("from", "to", "0%", "100%")):
        line = css_nc.count("\n", 0, m.start(2)) + 1
        seen[sel].append(line)
dups = {s: l for s, l in seen.items() if len(l) > 1}
print(f"{len(seen)} distinct selectors, {len(dups)} defined more than once")
for s, l in sorted(dups.items(), key=lambda x: -len(x[1]))[:40]:
    print(f"  x{len(l)}  {s[:70]}  lines {l[:6]}")
