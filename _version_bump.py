# -*- coding: utf-8 -*-
"""Increments / reads the version in src-tauri/tauri.conf.json.

Usage:
    python _version_bump.py            -> prints current version
    python _version_bump.py 2.2.0      -> sets version to 2.2.0 and prints it
"""
import io
import os
import re
import sys

BASE = os.path.dirname(os.path.abspath(__file__))
CONF = os.path.join(BASE, "src-tauri", "tauri.conf.json")

with io.open(CONF, "r", encoding="utf-8") as f:
    txt = f.read()

token_re = re.compile(r'"version"\s*:\s*"([^"]*)"')
m = token_re.search(txt)
if not m:
    print("ERROR: no version field found in %s" % CONF, file=sys.stderr)
    sys.exit(1)

cur = m.group(1)

if len(sys.argv) > 1:
    new = sys.argv[1].strip()
    if not re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", new):
        print("ERROR: invalid version %r (expected e.g. 2.2.0)" % new, file=sys.stderr)
        sys.exit(1)
    new_token = m.group(0).replace(cur, new, 1)
    txt = txt[: m.start()] + new_token + txt[m.end() :]
    with io.open(CONF, "w", encoding="utf-8", newline="") as f:
        f.write(txt)
    print(new)
else:
    print(cur)
