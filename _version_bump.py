import re, sys

p = r'src-tauri\tauri.conf.json'
s = open(p, encoding='utf-8').read()

if len(sys.argv) > 1:
    new_ver = sys.argv[1]
    s = re.sub(r'("version"\s*:\s*")[^"]+(")', r'\g<1>' + new_ver + r'\g<2>', s, count=1)
    open(p, 'w', encoding='utf-8', newline='').write(s)
    print('version updated to', new_ver)
else:
    m = re.search(r'"version"\s*:\s*"([^"]+)"', s)
    print(m.group(1) if m else 'unknown')