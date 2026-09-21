#!/usr/bin/env python3
"""
批量将 donut-proxy 重命名为 bwbrowser-proxy
和 donut_proxy 重命名为 bwbrowser_proxy
"""
import os, re

ROOT = r'd:\bian\donutbrowser-latest'

REPLACEMENTS = [
    ('donut-proxy', 'bwbrowser-proxy'),
    ('donut_proxy', 'bwbrowser_proxy'),
    ('Donut-proxy', 'Bwbrowser-proxy'),
    ('Donut_proxy', 'Bwbrowser_proxy'),
    ('DONUT-PROXY', 'BWBROWSER-PROXY'),
    ('DONUT_PROXY', 'BWBROWSER_PROXY'),
]

SKIP_DIRS = {'node_modules', '.git', 'dist', 'build', '.next', 'target', '.driver', '.turbo', 'out', '.cache', '__pycache__', '.svelte-kit'}
SKIP_EXTS = {'.png', '.jpg', '.jpeg', '.gif', '.ico', '.webp', '.bmp', '.svg', '.woff', '.woff2', '.ttf', '.eot', '.otf', '.map', '.lock', '.sum', '.bin', '.exe', '.dll', '.so', '.dylib', '.a', '.lib', '.pdf', '.doc', '.docx', '.xls', '.xlsx', '.ppt', '.pptx', '.zip', '.tar', '.gz', '.tgz', '.rar', '.7z', '.mp3', '.mp4', '.wav', '.avi', '.mov', '.pyc', '.tsbuildinfo'}

# Files to skip (documentation that references upstream project)
SKIP_FILES = {'AGENTS.md', 'CLAUDE.md', 'CONTRIBUTING.md', 'LICENSE', 'SECURITY.md', 'CODE_OF_CONDUCT.md'}

def should_skip(filepath):
    fname = os.path.basename(filepath)
    if fname in SKIP_FILES:
        return True
    ext = os.path.splitext(filepath)[1].lower()
    if ext in SKIP_EXTS:
        return True
    if filepath.endswith('.tsbuildinfo'):
        return True
    # Skip the rename scripts themselves
    if 'rename' in fname.lower() and fname.endswith('.py'):
        return True
    return False

def replace_content(content):
    for old, new in REPLACEMENTS:
        content = content.replace(old, new)
    return content

def main():
    print("替换 donut-proxy -> bwbrowser-proxy...")
    modified = 0
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for fname in filenames:
            fpath = os.path.join(dirpath, fname)
            if should_skip(fpath):
                continue
            try:
                with open(fpath, 'r', encoding='utf-8') as f:
                    content = f.read()
            except:
                continue
            new_content = replace_content(content)
            if new_content != content:
                with open(fpath, 'w', encoding='utf-8') as f:
                    f.write(new_content)
                modified += 1
                print(f"  {os.path.relpath(fpath, ROOT)}")
    print(f"\n修改文件数: {modified}")

    # Rename binary files in binaries/ directory
    bin_dir = os.path.join(ROOT, 'src-tauri', 'binaries')
    if os.path.isdir(bin_dir):
        print("\n重命名 binaries/ 目录中的文件...")
        for f in os.listdir(bin_dir):
            if 'donut-proxy' in f or 'donut_proxy' in f:
                old_path = os.path.join(bin_dir, f)
                new_name = replace_content(f)
                new_path = os.path.join(bin_dir, new_name)
                if not os.path.exists(new_path):
                    os.rename(old_path, new_path)
                    print(f"  {f} -> {new_name}")

    print("\n完成!")

if __name__ == '__main__':
    main()
