import urllib.request, json, websocket, time

tt = None
for t in json.load(urllib.request.urlopen('http://127.0.0.1:58028/json', timeout=2)):
    if t.get('url', '').startswith('https://www.tiktok.com'):
        tt = t
        break

ws = websocket.create_connection(tt['webSocketDebuggerUrl'], timeout=6, suppress_origin=True)
def ev(i, m, p=None):
    msg = {'id': i, 'method': m}
    if p is not None: msg['params'] = p
    ws.send(json.dumps(msg))
    while True:
        r = json.loads(ws.recv())
        if r.get('id') == i: return r

ev(1, 'Network.enable')
# 覆写两个主题 cookie
ev(2, 'Network.setCookie', {'name': 'tiktok_webapp_theme', 'value': 'light',
    'domain': '.tiktok.com', 'path': '/', 'secure': True, 'httpOnly': False, 'url': 'https://www.tiktok.com/'})
ev(3, 'Network.setCookie', {'name': 'tiktok_webapp_theme_source', 'value': 'user',
    'domain': '.tiktok.com', 'path': '/', 'secure': True, 'httpOnly': False, 'url': 'https://www.tiktok.com/'})
res = ev(4, 'Runtime.evaluate', {'expression':
  "JSON.stringify({t:document.cookie.match(/tiktok_webapp_theme=[^;]*/)?.[0], s:document.cookie.match(/tiktok_webapp_theme_source=[^;]*/)?.[0]})",
  'returnByValue': True})
print('after CDP set:', res['result']['result']['value'])

# 刷新页面
ev(5, 'Page.enable')
ev(6, 'Page.reload', {'ignoreCache': False})
time.sleep(9)
script = """
JSON.stringify({
  realDark: (() => { try { const f = document.createElement('iframe'); f.style.display='none';
    document.body.appendChild(f); const d = f.contentWindow.matchMedia('(prefers-color-scheme: dark)').matches;
    f.remove(); return d; } catch(e) { return 'err'; } })(),
  ttDataTheme: document.documentElement.getAttribute('data-theme'),
  bodyBg: getComputedStyle(document.body).backgroundColor
})
"""
res = ev(7, 'Runtime.evaluate', {'expression': script, 'returnByValue': True})
print('after reload:', res['result']['result']['value'])
ws.close()