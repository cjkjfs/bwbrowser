import sqlite3

conn = sqlite3.connect(r"D:\bian\simprint-main\simprint.db")
cur = conn.cursor()
cur.execute("SELECT name FROM sqlite_master WHERE type='table'")
tables = [r[0] for r in cur.fetchall()]
print("TABLES:", tables)

for t in tables:
    if "proxy" in t.lower() or "account" in t.lower():
        cur.execute(f"PRAGMA table_info({t})")
        cols = [(r[1], r[2]) for r in cur.fetchall()]
        print(f"\n-- {t} --")
        for c in cols:
            print("  ", c)
conn.close()
