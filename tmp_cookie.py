import urllib.request, json, websocket

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

# 读 cookie 值
res = ev(1, 'Runtime.evaluate', {'expression':
  "JSON.stringify({t:document.cookie.match(/tiktok_webapp_theme=[^;]*/)?.[0], s:document.cookie.match(/tiktok_webapp_theme_source=[^;]*/)?.[0]})",
  'returnByValue': True})
print('cookie values:', res['result']['result']['value'])

# 改成 light
ev(2, 'Runtime.evaluate', {'expression': "document.cookie='tiktok_webapp_theme=light;path=/;domain=.tiktok.com;max-age=31536000';document.cookie='tiktok_webapp_theme_source=user;path=/;domain=.tiktok.com;max-age=31536000'", 'returnByValue': True})
res = ev(3, 'Runtime.evaluate', {'expression':
  "JSON.stringify({t:document.cookie.match(/tiktok_webapp_theme=[^;]*/)?.[0], s:document.cookie.match(/tiktok_webapp_theme_source=[^;]*/)?.[0]})",
  'returnByValue': True})
print('after set:', res['result']['result']['value'])
ws.close()