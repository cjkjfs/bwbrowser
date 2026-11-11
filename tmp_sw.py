import urllib.request, json, websocket, time

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

ev(1, 'Network.enable')
# 注销 SW + 删主题 cookie
script_unreg = """
(async () => {
  const regs = await navigator.serviceWorker.getRegistrations();
  for (const r of regs) await r.unregister();
  return regs.length;
})()
"""
r = ev(2, 'Runtime.evaluate', {'expression': script_unreg, 'returnByValue': True, 'awaitPromise': True})
print('unregistered SWs:', r.get('result', {}).get('result', {}).get('value'))

for name, dom in [('tiktok_webapp_theme', '.www.tiktok.com'), ('tiktok_webapp_theme_source', '.www.tiktok.com'),
                  ('tiktok_webapp_theme', '.tiktok.com'), ('tiktok_webapp_theme_source', '.tiktok.com')]:
    ev(3, 'Network.deleteCookies', {'name': name, 'domain': dom, 'path': '/'})

ev(4, 'Page.enable')
ev(5, 'Page.reload', {'ignoreCache': True})
time.sleep(12)

script = """
JSON.stringify({
  tux: document.documentElement.getAttribute("data-tux-color-scheme"),
  bg: getComputedStyle(document.body).backgroundColor,
  cookie: document.cookie.match(/tiktok_webapp_theme[a-z_]*=[^;]*/g)
})
"""
r = ev(6, 'Runtime.evaluate', {'expression': script, 'returnByValue': True})
res = r.get('result', {}).get('result', {})
print('after reload:', res.get('value') or json.dumps(r)[:300])