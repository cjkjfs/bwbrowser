import urllib.request, json, websocket

r = urllib.request.urlopen('http://127.0.0.1:58028/json', timeout=2)
for t in json.load(r):
    if t.get('type') == 'page':
        print('PAGE:', t.get('url','')[:70])

tt = None
for t in json.load(r2 := urllib.request.urlopen('http://127.0.0.1:58028/json', timeout=2)):
    if t.get('url', '').startswith('https://www.tiktok.com'):
        tt = t
        break
if not tt:
    print('NO TIKTOK TAB'); raise SystemExit

ws = websocket.create_connection(tt['webSocketDebuggerUrl'], timeout=6, suppress_origin=True)
def ev(i, m, p=None):
    msg = {'id': i, 'method': m}
    if p is not None: msg['params'] = p
    ws.send(json.dumps(msg))
    while True:
        r = json.loads(ws.recv())
        if r.get('id') == i: return r
script = """
JSON.stringify({
  realDark: (() => { try { const f = document.createElement('iframe'); f.style.display='none';
    document.body.appendChild(f); const d = f.contentWindow.matchMedia('(prefers-color-scheme: dark)').matches;
    f.remove(); return d; } catch(e) { return 'err:'+e.message; } })(),
  directDark: matchMedia('(prefers-color-scheme: dark)').matches,
  ttDataTheme: document.documentElement.getAttribute('data-theme'),
  bodyBg: getComputedStyle(document.body).backgroundColor,
  cookies: document.cookie.split(';').map(c=>c.trim().split('=')[0]).filter(k=>/theme|dark|mode|night/i.test(k)).join(',') || '(no theme cookie)',
  lsKeys: Object.keys(localStorage).filter(k=>/theme|dark|mode|night|color/i.test(k)).map(k=>k+'='+localStorage.getItem(k)).join(', ') || '(none)'
})
"""
res = ev(2, 'Runtime.evaluate', {'expression': script, 'returnByValue': True})
print('TIKTOK:', res['result']['result']['value'])
ws.close()