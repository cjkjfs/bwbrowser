#!/usr/bin/env python3
"""抖音视频下载桥接脚本

yt-dlp CLI 模式下抖音提取器无法自动获取访客 cookie（403 Forbidden），
Python API 模式的 YoutubeDL 对象在整个生命周期内保持 cookie jar，
可以自动接收和复用 session cookie。

用法: python douyin_download.py <url> <output_dir> <yt_dlp_path> [cookie_file] [max_height]
"""
import sys
import os
import json
import random
import re

def is_douyin_url(url):
    return bool(re.search(r"https?://(?:[^/]+\.)?(douyin|iesdouyin)\.com/", url or "", re.I))

def normalize_video_url(url):
    if not url:
        return url
    # 已经是标准格式
    match = re.search(r"/video/(\d{8,30})", url)
    if match:
        return f"https://www.douyin.com/video/{match.group(1)}"
    match = re.search(r"/note/(\d{8,30})", url)
    if match:
        return f"https://www.douyin.com/note/{match.group(1)}"
    match = re.search(r"(?:douyin|iesdouyin)\.com/(?:share/)?(?:video|note)/(\d{8,30})", url, re.I)
    if match:
        return f"https://www.douyin.com/video/{match.group(1)}"
    # 短链 v.douyin.com/xxx：跟随重定向拿到视频 ID
    if "v.douyin.com" in url or "iesdouyin.com" in url:
        try:
            import urllib.request
            req = urllib.request.Request(url, headers={
                "User-Agent": "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/131.0.0.0 Safari/537.36"
            })
            with urllib.request.urlopen(req, timeout=10) as resp:
                final_url = resp.geturl()
                match = re.search(r"/video/(\d{8,30})", final_url)
                if match:
                    return f"https://www.douyin.com/video/{match.group(1)}"
                match = re.search(r"/share/video/(\d{8,30})", final_url)
                if match:
                    return f"https://www.douyin.com/video/{match.group(1)}"
        except Exception:
            pass
    return url

def progress_hook(d):
    if d["status"] == "downloading":
        total = d.get("total_bytes") or d.get("total_bytes_estimate") or 0
        downloaded = d.get("downloaded_bytes") or 0
        if total > 0:
            percent = downloaded * 100 / total
        else:
            percent = 0
        speed = d.get("speed") or 0
        eta = d.get("eta") or 0
        print(json.dumps({
            "type": "progress",
            "percent": round(percent, 1),
            "speed": speed,
            "eta": eta,
            "downloaded": downloaded,
            "total": total,
            "filename": d.get("filename") or ""
        }), flush=True)
    elif d["status"] == "finished":
        print(json.dumps({"type": "finished", "filename": d.get("filename") or ""}), flush=True)

def main():
    if len(sys.argv) < 4:
        print(json.dumps({"type": "error", "message": "用法: douyin_download.py <url> <output_dir> <yt_dlp_path> [cookie_file] [max_height] [browser_profile]"}), flush=True)
        sys.exit(1)

    url = sys.argv[1]
    output_dir = sys.argv[2]
    yt_dlp_path = sys.argv[3]
    cookie_file = sys.argv[4] if len(sys.argv) > 4 and sys.argv[4] else ""
    max_height = int(sys.argv[5]) if len(sys.argv) > 5 and sys.argv[5] else 0
    browser_profile = sys.argv[6] if len(sys.argv) > 6 and sys.argv[6] else ""

    url = normalize_video_url(url)

    yt_dlp_dir = os.path.dirname(yt_dlp_path)
    if yt_dlp_dir and yt_dlp_dir not in sys.path:
        sys.path.insert(0, yt_dlp_dir)

    try:
        import yt_dlp
    except ImportError:
        try:
            os.environ["PYTHONPATH"] = yt_dlp_dir + os.pathsep + os.environ.get("PYTHONPATH", "")
            import yt_dlp
        except ImportError:
            print(json.dumps({"type": "error", "message": f"无法导入 yt_dlp 模块 (搜索路径: {yt_dlp_dir})"}), flush=True)
            sys.exit(1)

    format_str = "best/bv*+ba/bestvideo+bestaudio/best"
    if max_height > 0:
        format_str = f"bv*[height<={max_height}]+ba/bestvideo[height<={max_height}]+bestaudio/best[height<={max_height}]/best"

    base_options = {
        "outtmpl": os.path.join(output_dir, "%(uploader|unknown)s_%(title).120s_%(id)s.%(ext)s"),
        "format": format_str,
        "merge_output_format": "mp4",
        "format_sort": ["res", "fps", "hdr:12", "vcodec", "acodec", "br"],
        "noplaylist": True,
        "restrictfilenames": True,
        "windowsfilenames": True,
        "quiet": False,
        "no_warnings": False,
        "retries": 10,
        "fragment_retries": 10,
        "extractor_retries": 3,
        "file_access_retries": 3,
        "socket_timeout": 30,
        "geo_bypass": True,
        "format_sort_force": True,
        "progress_hooks": [progress_hook],
        "http_headers": {
            "User-Agent": (
                "Mozilla/5.0 (Windows NT 10.0; Win64; x64) "
                "AppleWebKit/537.36 (KHTML, like Gecko) "
                "Chrome/131.0.0.0 Safari/537.36"
            ),
            "Accept-Language": "zh-CN,zh;q=0.9,en-US;q=0.8,en;q=0.7",
        },
    }

    max_attempts = 3
    last_error = None

    for attempt in range(max_attempts):
        options = dict(base_options)
        try:
            if cookie_file and os.path.exists(cookie_file):
                options["cookiefile"] = cookie_file

            # 重试时用 cookies_from_browser 从浏览器 SQLite 直读（比 cookiefile 更新鲜）
            if attempt > 0 and browser_profile and os.path.isdir(browser_profile):
                options.pop("cookiefile", None)
                options["cookies_from_browser"] = ("chrome", browser_profile, None, None)
                print(json.dumps({"type": "info", "message": "使用浏览器直读 cookie 重试"}), flush=True)

            if attempt > 0:
                print(json.dumps({"type": "retry", "attempt": attempt + 1, "message": "正在换下载策略重试"}), flush=True)
                options["format"] = "best/bv*+ba"
                options["http_headers"] = {
                    **options.get("http_headers", {}),
                    "Referer": "https://www.douyin.com/",
                }

            with yt_dlp.YoutubeDL(options) as ydl:
                info = ydl.extract_info(url, download=True)
                title = info.get("title", "") if info else ""
                resolution = ""
                if info:
                    if info.get("width") and info.get("height"):
                        resolution = f"{info['width']}x{info['height']}"
                    elif info.get("resolution"):
                        resolution = str(info["resolution"])
                print(json.dumps({
                    "type": "done",
                    "title": title,
                    "resolution": resolution,
                }), flush=True)
                sys.exit(0)
        except Exception as e:
            last_error = str(e)
            err_lower = last_error.lower()
            if "fresh cookies" in err_lower or "403" in err_lower:
                if attempt < max_attempts - 1:
                    continue
            print(json.dumps({"type": "error", "message": last_error, "attempt": attempt + 1}), flush=True)
            if "Unsupported URL" in last_error:
                break
            if attempt < max_attempts - 1:
                continue
            break

    print(json.dumps({"type": "error", "message": last_error or "未知错误"}), flush=True)
    sys.exit(1)

if __name__ == "__main__":
    main()
