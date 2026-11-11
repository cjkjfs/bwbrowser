import urllib.request, json, websocket

tt = None
for t in json.load(urllib.request.urlopen('http://127.0.0.1:58028/json', timeout=2)):
    if t.get('url', '').startswith('https://www.tiktok.com'):
        tt = t
        break

ws = websocket.create_connection(tt['webSocketDebuggerUrl'], timeout=20, suppress_origin=True)
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
  const dbs = await indexedDB.databases();
  const names = dbs.map(d => d.name);
  // 粗查每个库的 object store 名
  const detail = [];
  for (const d of dbs) {
    await new Promise(res => {
      const rq = indexedDB.open(d.name);
      rq.onsuccess = () => {
        const db = rq.result;
        detail.push(d.name + ': [' + Array.from(db.objectStoreNames).join(',') + ']');
        db.close();
        res();
      };
      rq.onerror = () => res();
    });
  }
  return JSON.stringify({dbs: detail});
})()
"""
r = ev(2, 'Runtime.evaluate', {'expression': script, 'returnByValue': True, 'awaitPromise': True})
res = r.get('result', {}).get('result', {})
print('IndexedDB:', res.get('value') or json.dumps(r)[:400])