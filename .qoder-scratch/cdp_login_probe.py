"""Check every running account browser's doubao page login state from the SSR ROUTER_DATA."""
import json
import re
import urllib.request

import websocket

PORTS = ["60548", "60916", "60919", "60994", "61001", "62910"]
out = []

for port in PORTS:
    try:
        with urllib.request.urlopen("http://127.0.0.1:%s/json" % port, timeout=3) as r:
            targets = json.load(r)
    except Exception as e:  # noqa: BLE001
        out.append("PORT %s -> not CDP (%s)" % (port, e.__class__.__name__))
        continue
    pages = [t for t in targets if t.get("type") == "page"]
    for t in pages:
        url = t.get("url") or ""
        if "doubao.com" not in url:
            out.append("PORT %s page %s" % (port, json.dumps(url[:60], ensure_ascii=True)))
            continue
        try:
            ws = websocket.create_connection(t["webSocketDebuggerUrl"], timeout=30,
                                             suppress_origin=True, max_size=None)
            _id = [0]

            def call(method, params=None):
                _id[0] += 1
                ws.send(json.dumps({"id": _id[0], "method": method, "params": params or {}}))
                while True:
                    msg = json.loads(ws.recv())
                    if msg.get("id") == _id[0]:
                        return msg.get("result")

            call("DOM.enable")
            doc = call("DOM.getDocument", {"depth": -1, "pierce": False})
            texts = []

            def walk(n):
                if n.get("nodeType") == 3:
                    texts.append(n.get("nodeValue") or "")
                for c in n.get("children") or []:
                    walk(c)

            walk(doc["root"])
            blob = "\n".join(texts)
            m = re.search(r"accountInfo.{0,400}", blob)
            acc = m.group(0)[:300] if m else "(no accountInfo in DOM)"
            has_editor = "ProseMirror" in blob
            has_video = "视频生成" in blob
            n_nodes = blob.count("\n")
            out.append("PORT %s url=%s nodes=%d editor=%s videoEntry=%s\n   accountInfo=%s" % (
                port, json.dumps(url[:40], ensure_ascii=True), n_nodes, has_editor, has_video,
                json.dumps(acc, ensure_ascii=True)))
            ws.close()
        except Exception as e:  # noqa: BLE001
            out.append("PORT %s doubao page probe failed: %s" % (port, e))

with open("login_probe.txt", "w", encoding="utf-8") as f:
    f.write("\n\n".join(out))
print("wrote %d entries" % len(out))
