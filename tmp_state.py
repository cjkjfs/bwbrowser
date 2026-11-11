import urllib.request, json, websocket

tt = None
for t in json.load(urllib.request.urlopen('http://127.0.0.1:58028/json', timeout=2)):
    if t.get('url', '').startswith('https://www.tiktok.com'):
        tt = t
        break

ws = websocket.create_connection(tt['webSocketDebuggerUrl'], timeout=15, suppress_origin=True)
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
(async () => {
  const html = await fetch(location.href, {credentials: 'include'}).then(r => r.text());
  // 找状态 blob 里的 theme 相关片段
  const out = [];
  const re = /[\\\"'](theme[a-zA-Z_]*|darkMode|colorScheme)[\\\"']\\s*:\\s*[\\\"'][a-z]+[\\\"']/gi;
  let m; let n = 0;
  while ((m = re.exec(html)) && n < 12) { out.push(m[0]); n++; }
  return JSON.stringify({found: out});
})()
"""
r = ev(4, 'Runtime.evaluate', {'expression': script, 'returnByValue': True, 'awaitPromise': True})
res = r.get('result', {}).get('result', {})
print('state blobs:', res.get('value') or json.dumps(r)[:300])