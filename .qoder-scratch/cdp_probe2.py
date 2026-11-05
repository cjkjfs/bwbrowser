"""Probe #2: pierced DOM snapshot (shadow roots + same-origin iframes)."""
import json
import sys
import urllib.request

import websocket

PORT = sys.argv[1] if len(sys.argv) > 1 else "61001"
OUT = sys.argv[2] if len(sys.argv) > 2 else "probe2_out.txt"

lines = []


def log(s):
    lines.append(s)


def get_target():
    with urllib.request.urlopen("http://127.0.0.1:%s/json" % PORT, timeout=5) as r:
        data = json.load(r)
    pages = [t for t in data if t.get("type") == "page"]
    for t in pages:
        if "doubao.com" in (t.get("url") or ""):
            return t
    raise SystemExit("no doubao page: %s" % [t.get("url") for t in pages])


target = get_target()
log("URL %s" % target["url"])
ws = websocket.create_connection(target["webSocketDebuggerUrl"], timeout=120, suppress_origin=True, max_size=None)
_id = [0]


def call(method, params=None):
    _id[0] += 1
    ws.send(json.dumps({"id": _id[0], "method": method, "params": params or {}}))
    while True:
        msg = json.loads(ws.recv())
        if msg.get("id") == _id[0]:
            if "error" in msg:
                log("ERR %s -> %s" % (method, json.dumps(msg["error"], ensure_ascii=True)))
                return None
            return msg["result"]


call("DOM.enable")
doc = call("DOM.getDocument", {"depth": -1, "pierce": True})
nodes = []


def walk(node, depth):
    node["_depth"] = depth
    nodes.append(node)
    for c in node.get("children", []) or []:
        walk(c, depth + 1)
    for sr in node.get("shadowRoots", []) or []:
        walk(sr, depth + 1)
    for iframe in node.get("contentDocument", {}) and [node.get("contentDocument")] or []:
        walk(iframe, depth + 1)


walk(doc["root"], 0)
log("NODES %d" % len(nodes))


def attrs(el):
    a = el.get("attributes") or []
    return dict(zip(a[0::2], a[1::2]))


def own_text(el):
    kids = el.get("children", []) or []
    return " ".join((c.get("nodeValue") or "").strip() for c in kids if c.get("nodeType") == 3).strip()


def has_element_child(el):
    return any(k.get("nodeType") == 1 for k in (el.get("children") or []))


def desc(el):
    p = attrs(el)
    return "%s#%s id=%s cls=%s role=%s tagtext=%s" % (
        el.get("nodeName"), el.get("nodeId"), p.get("id", ""),
        json.dumps((p.get("class") or "")[:60], ensure_ascii=True),
        p.get("role", ""), json.dumps(own_text(el)[:40], ensure_ascii=True))


log("\n=== LEAF text containing 视频/更多/图片 ===")
for n in nodes:
    if n.get("nodeType") != 1 or has_element_child(n):
        continue
    t = own_text(n)
    if t and any(k in t for k in ("视频", "更多", "图片生成", "安全确认", "数字人")):
        log("  d=%-3s %s" % (n["_depth"], desc(n)))

log("\n=== INPUT file ===")
for n in nodes:
    if n.get("nodeName") == "INPUT":
        p = attrs(n)
        if p.get("type") == "file" or "accept" in p:
            log("  %s type=%s accept=%s multiple=%s" % (desc(n), p.get("type"),
                json.dumps(p.get("accept", "")[:120], ensure_ascii=True), "multiple" in p))

log("\n=== TEXTAREA / contenteditable ===")
for n in nodes:
    if n.get("nodeType") != 1:
        continue
    p = attrs(n)
    if n.get("nodeName") == "TEXTAREA" or p.get("contenteditable") == "true" or "editor" in (p.get("class") or ""):
        log("  %s ce=%s ph=%s" % (desc(n)[:110], p.get("contenteditable"), json.dumps((p.get("placeholder") or "")[:40], ensure_ascii=True)))

log("\n=== BODY visible text head ===")
texts = [(n["_depth"], (n.get("nodeValue") or "").strip()) for n in nodes if n.get("nodeType") == 3]
joined = " | ".join(t for _, t in texts if t)
log(json.dumps(joined[:1500], ensure_ascii=True))

log("\n=== SCRIPT route data (first 600) ===")
for n in nodes:
    if n.get("nodeName") == "SCRIPT":
        t = own_text(n) or " ".join((c.get("nodeValue") or "") for c in (n.get("children") or []) if c.get("nodeType") == 3)
        if "_ROUTER_DATA" in t:
            log(json.dumps(t[:600], ensure_ascii=True))
            break
ws.close()
with open(OUT, "w", encoding="utf-8") as f:
    f.write("\n".join(lines))
print("WROTE %s lines=%d" % (OUT, len(lines)))
