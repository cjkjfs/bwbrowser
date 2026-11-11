import urllib.request, json, websocket

tt = None
for t in json.load(urllib.request.urlopen('http://127.0.0.1:58028/json', timeout=2)):
    if t.get('url', '').startswith('https://www.tiktok.com'):
        tt = t
        break

ws = websocket.create_connection(tt['webSocketDebuggerUrl'], timeout=8, suppress_origin=True)
def ev(i, m, p=None):
    msg = {'id': i, 'method': m}
    if p is not None:
        msg['params'] = p
    ws.send(json.dumps(msg))
    while True:
        r = json.loads(ws.recv())
        if r.get('id') == i:
            return r

script = """
JSON.stringify({
  topMM: matchMedia("(prefers-color-scheme: dark)").matches,
  iframeMM: (() => { const f = document.createElement("iframe"); f.style.display="none";
    document.body.appendChild(f);
    const d = f.contentWindow.matchMedia("(prefers-color-scheme: dark)").matches;
    const s = f.contentWindow.matchMedia.toString().slice(0,50);
    f.remove(); return d + " | iframe.toString:" + s; })(),
  topMMtoString: matchMedia.toString().slice(0,50),
  mmIsOwn: Object.getOwnPropertyDescriptor(window, "matchMedia") !== undefined
})
"""
r = ev(2, 'Runtime.evaluate', {'expression': script, 'returnByValue': True})
print(r['result']['result']['value'])