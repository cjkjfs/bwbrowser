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

# 先确保 cookie 是 light+user
ev(1, 'Network.enable')
ev(2, 'Network.setCookie', {'name': 'tiktok_webapp_theme', 'value': 'light',
    'domain': '.www.tiktok.com', 'path': '/', 'secure': True, 'url': 'https://www.tiktok.com/'})
ev(3, 'Network.setCookie', {'name': 'tiktok_webapp_theme_source', 'value': 'user',
    'domain': '.www.tiktok.com', 'path': '/', 'secure': True, 'url': 'https://www.tiktok.com/'})

# fetch 同一 URL 看 SSR HTML 的主题属性
script = """
(async () => {
  const html = await fetch(location.href, {credentials: 'include'}).then(r => r.text());
  const m1 = html.match(/data-tux-color-scheme=\\\"?[a-z]+\\\"?/);
  const m2 = html.match(/data-theme=\\\"?[a-z]+\\\"?/);
  return JSON.stringify({ssrTux: m1 && m1[0], ssrTheme: m2 && m2[0]});
})()
"""
r = ev(4, 'Runtime.evaluate', {'expression': script, 'returnByValue': True, 'awaitPromise': True})
res = r.get('result', {}).get('result', {})
print('SSR html:', res.get('value') or json.dumps(r)[:300])