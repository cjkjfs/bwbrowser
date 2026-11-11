import urllib.request, json, websocket, time

METADATA = r"C:\Users\Administrator\AppData\Local\BwBrowser\profiles\6bccb2aa-e1c8-4e04-b529-651a77582075\metadata.json"
meta = json.load(open(METADATA, encoding='utf-8'))
fp = json.loads(meta['wayfern_config']['fingerprint'])

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

fp2 = dict(fp)
fp2['prefersDarkMode'] = False
r = ev(1, 'Wayfern.setFingerprint', fp2)
print('setFingerprint(dark:false) sent, resp ok')

ev(2, 'Network.enable')
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
print('after fp(dark:false)+reload:', res.get('value') or json.dumps(r)[:300])