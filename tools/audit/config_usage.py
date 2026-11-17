#!/usr/bin/env python3
"""Settings audit: which settings in src/config.rs are used where.

For every `pub` field of every struct in src/config.rs, counts how often it
is mentioned in the other Rust modules and in the web page (static/app.js,
static/index.html). Prints:

  * fields nothing outside config.rs mentions (candidates for removal: check
    migrations and serde compatibility first — never drop a setting a saved
    settings file may still contain without a migration);
  * fields only the web page mentions (stored for the page: fine, but check
    the page really reads them, not just passes them back).

Run from the repository root:  python3 tools/audit/config_usage.py
"""
import os
import re

cfg = open("src/config.rs").read()
structs = {}
for m in re.finditer(r"pub struct (\w+)\s*\{(.*?)\n\}", cfg, re.S):
    structs[m.group(1)] = re.findall(r"^\s*pub (\w+):", m.group(2), re.M)

rust = ""
for root, _, files in os.walk("src"):
    for f in files:
        p = os.path.join(root, f)
        if f.endswith(".rs") and p != os.path.join("src", "config.rs"):
            rust += open(p).read() + "\n"
web = open("static/app.js").read() + open("static/index.html").read()

total = zero = 0
web_only = []
unused = []
for st, fields in structs.items():
    for f in fields:
        total += 1
        r = len(re.findall(r"[.\s{(]" + re.escape(f) + r"\b", rust))
        w = len(re.findall(r"\b" + re.escape(f) + r"\b", web))
        if r == 0 and w == 0:
            unused.append(f"{st}.{f}")
        elif r == 0:
            web_only.append(f"{st}.{f}")

print(f"{len(structs)} structs, {total} settings")
print(f"\nNot mentioned outside config.rs ({len(unused)}):")
for u in unused:
    print("  ", u)
print(f"\nMentioned only by the web page ({len(web_only)}):")
for u in web_only:
    print("  ", u)
