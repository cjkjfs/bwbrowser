"""Probe the running account browser's doubao page DOM over CDP (DOM domain only)."""
import json
import sys
import urllib.request

import websocket

PORT = sys.argv[1] if len(sys.argv) > 1 else "61001"
KEYWORDS = ["视频生成", "生成视频", "更多", "图片生成", "数字人", "安全确认"]


def get_target():
    with urllib.request.urlopen("http://127.0.0.1:%s/json" % PORT, timeout=5) as r:
        data = json.load(r)
    for t in data:
        if t.get("type") == "page" and "doubao.com" in (t.get("url") or ""):
            return t
    raise SystemExit("no doubao page target: %s" % [t.get("url") for t in data])


target = get_target()
print("URL %s" % target["url"])
print("TITLE %s" % json.dumps(target.get("title"), ensure_ascii=True))

ws = websocket.create_connection(target["webSocketDebuggerUrl"], timeout=60, suppress_origin=True, max_size=None)
_id = [0]


def call(method, params=None):
    _id[0] += 1
    ws.send(json.dumps({"id": _id[0], "method": method, "params": params or {}}))
    while True:
        msg = json.loads(ws.recv())
        if msg.get("id") == _id[0]:
            if "error" in msg:
                print("ERR %s -> %s" % (method, json.dumps(msg["error"], ensure_ascii=True)))
                return None
            return msg["result"]


call("DOM.enable")
doc = call("DOM.getDocument", {"depth": -1, "pierce": False})

nodes = []


def walk(node, parent=None):
    node["_parent"] = parent
    nodes.append(node)
    for c in node.get("children", []) or []:
        walk(c, node)


walk(doc["root"])
print("NODES %d" % len(nodes))

# map nodeId -> attributes for quick lookup
by_id = {n["nodeId"]: n for n in nodes if "nodeId" in n}


def element_text(el):
    out = []
    for c in el.get("children", []) or []:
        if c.get("nodeType") == 3:
            out.append((c.get("nodeValue") or "").strip())
        for g in c.get("children", []) or []:
            if g.get("nodeType") == 3:
                out.append((g.get("nodeValue") or "").strip())
    return " ".join(x for x in out if x)


def describe(el):
    attrs = el.get("attributes") or []
    pairs = {}
    for i in range(0, len(attrs) - 1, 2):
        pairs[attrs[i]] = attrs[i + 1]
    return "%s#%s cls=%s id=%s role=%s text=%s" % (
        el.get("nodeName"),
        el.get("nodeId"),
        json.dumps(pairs.get("class", "")[:70], ensure_ascii=True),
        pairs.get("id", ""),
        pairs.get("role", ""),
        json.dumps(element_text(el)[:60], ensure_ascii=True),
    )


def box(node_id):
    r = call("DOM.getBoxModel", {"nodeId": node_id})
    if not r:
        return None
    q = r["model"]["content"]
    xs = q[0::2]
    ys = q[1::2]
    return (round(sum(xs) / 4, 1), round(sum(ys) / 4, 1), round(max(xs) - min(xs), 1), round(max(ys) - min(ys), 1))


print("\n=== leaf text matches (element with no element children, trimmed own text) ===")
for n in nodes:
    if n.get("nodeType") != 1:
        continue
    kids = n.get("children", []) or []
    if any(k.get("nodeType") == 1 for k in kids):
        continue
    own = " ".join((c.get("nodeValue") or "").strip() for c in kids if c.get("nodeType") == 3)
    if own and any(k in own for k in KEYWORDS):
        print("EXACT   %-40s | own=%s | box=%s" % (describe(n)[:40], json.dumps(own[:40], ensure_ascii=True), box(n["nodeId"])))

print("\n=== elements whose own direct text contains 视频 ===")
count = 0
for n in nodes:
    if n.get("nodeType") != 1:
        continue
    kids = n.get("children", []) or []
    own = " ".join((c.get("nodeValue") or "").strip() for c in kids if c.get("nodeType") == 3)
    if "视频" in own:
        count += 1
        if count <= 25:
            print("  depth=%-3s %-70s | box=%s" % (len(str(n.get("_parent"))), describe(n)[:70], box(n["nodeId"])))
print("TOTAL %d" % count)

print("\n=== file inputs ===")
for n in nodes:
    if n.get("nodeName") == "INPUT":
        attrs = n.get("attributes") or []
        pairs = dict(zip(attrs[0::2], attrs[1::2]))
        if pairs.get("type") == "file" or "accept" in pairs:
            print("  INPUT#%s type=%s accept=%s multiple=%s box=%s" % (
                n["nodeId"], pairs.get("type"), json.dumps(pairs.get("accept", ""), ensure_ascii=True),
                "multiple" in pairs, box(n["nodeId"])))

print("\n=== open dialogs ===")
r = call("DOM.querySelectorAll", {"nodeId": doc["root"]["nodeId"], "selector": "[role=dialog][data-state=open]"})
for nid in (r or {}).get("nodeIds", []):
    el = by_id.get(nid)
    if el:
        print("  DIALOG %s" % describe(el)[:120])
ws.close()
