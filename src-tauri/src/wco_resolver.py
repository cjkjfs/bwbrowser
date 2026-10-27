"""wco.tv / wcostream 解析脚本（curl_cffi 模拟 Chrome TLS 过 Cloudflare）。

用法: python - "<page_url>"   （stdin 传本脚本）
输出: 单行 JSON {"ok":true,"url":"...","title":"..."} 或 {"ok":false,"error":"..."}
依赖: pip install curl_cffi（托管 Python 环境已装）
"""
import sys
import json
import base64
import re

def out(obj):
    print(json.dumps(obj, ensure_ascii=False), flush=True)

def main():
    try:
        from curl_cffi import requests as cr
    except ImportError:
        out({"ok": False, "error": "Python 未安装 curl_cffi：pip install curl_cffi"})
        return

    page_url = sys.argv[1] if len(sys.argv) > 1 else ""
    if "wco" not in page_url:
        out({"ok": False, "error": "非 wco 链接"})
        return

    s = cr.Session(impersonate="chrome")
    try:
        # 1. 剧集页（Cloudflare 拦截呈间歇性：同 UA curl/脚本约 1/3 概率 5xx，必须重试）
        page = None
        last_err = ""
        for attempt in range(5):
            try:
                page = s.get(page_url, timeout=40)
                if page.status_code == 200:
                    break
                last_err = f"HTTP {page.status_code}"
            except Exception as e:
                last_err = str(e)[:120]
            import time
            time.sleep(4)
        if page is None or page.status_code != 200:
            out({"ok": False, "error": f"页面获取失败 {last_err}（Cloudflare 拦截，请重试或换线路）"})
            return

        # 2. 混淆数组逐字符解码 → embed iframe
        m = re.search(r'zYR = \[(.*?)\];', page.text, re.S)
        if not m:
            # 尝试其他常见变量（站点改版时数组名可能变化）：找任意 base64 数组 + ixQ 函数特征
            m = re.search(r'=\s*\[((?:"[A-Za-z0-9+/=]{8,60}"\s*,?\s*){5,})\]', page.text)
            if not m:
                out({"ok": False, "error": "页面结构变化：未找到视频数据（请反馈给开发者）"})
                return
        arr = re.findall(r'"([^"]+)"', m.group(1))
        kUn = ""
        for v in arr:
            try:
                d = base64.b64decode(v).decode("utf-8", errors="ignore")
                digits = re.sub(r"\D", "", d)
                if not digits:
                    continue
                # 标准算法：charcode = digits - 25605536；容错处理其他偏移
                kUn += chr(int(digits) - 25605536)
            except Exception:
                continue
        emu_m = re.search(r'src="(https://embed\.wcostream\.com[^"]+)"', kUn)
        if not emu_m:
            out({"ok": False, "error": "未解析出 embed 地址（页面结构可能变化）"})
            return
        emu = emu_m.group(1)

        # 标题
        title_m = re.search(r"<title>(.*?)</title>", page.text, re.S)
        title = title_m.group(1).strip() if title_m else "wco_video"

        # 3. 真播放器页
        vj = s.get(
            "https://embed.wcostream.com/inc/embed/video-js.php" + emu.split("index.php")[1],
            timeout=40,
            headers={"Referer": "https://www.wco.tv/"},
        )
        if vj.status_code != 200:
            out({"ok": False, "error": f"播放器页获取失败 HTTP {vj.status_code}"})
            return
        m2 = re.search(r"getvidlink\.php\?v=([^\"&]+)&embed=([^\"&]+)&fullhd=([^\"&]+)", vj.text)
        if not m2:
            out({"ok": False, "error": "未找到 getvidlink 接口参数"})
            return

        # 4. getvidlink → token
        api = f"https://embed.wcostream.com/inc/embed/getvidlink.php?v={m2.group(1)}&embed={m2.group(2)}&fullhd={m2.group(3)}"
        d = s.get(api, timeout=40, headers={
            "Referer": "https://embed.wcostream.com/",
            "X-Requested-With": "XMLHttpRequest",
        }).json()

        # 5. 关键一步：server/getvid?evid=enc&json → 真实 CDN 直链（u2x 服务器）
        token = d.get("enc") or d.get("hd") or d.get("fhd")
        server = d.get("server") or d.get("cdn")
        if not token or not server:
            out({"ok": False, "error": "getvidlink 响应缺少 token/server"})
            return
        probe = server + "/getvid?evid=" + token + "&json"
        r = s.get(probe, timeout=40, headers={"Referer": "https://embed.wcostream.com/"})
        if r.status_code != 200:
            out({"ok": False, "error": f"CDN 直链探测失败 HTTP {r.status_code}"})
            return
        try:
            final = json.loads(r.text)
        except Exception:
            out({"ok": False, "error": "CDN 直链响应异常（非 JSON）"})
            return
        if not isinstance(final, str) or not final.startswith("http"):
            out({"ok": False, "error": "CDN 返回异常内容"})
            return

        out({"ok": True, "url": final, "title": title})
    except Exception as e:
        out({"ok": False, "error": str(e)[:200]})

main()
