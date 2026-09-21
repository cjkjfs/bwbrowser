#!/usr/bin/env python3
"""
批量将 bwbrowser 重命名为 bwbrowser
"""
import os, re

ROOT = r'd:\bian\donutbrowser-latest'

REPLACEMENTS = [
    ('BWBROWSER', 'BWBROWSER'),
    ('Bwbrowser', 'Bwbrowser'),
    ('bwbrowser', 'bwbrowser'),
]

SKIP_DIRS = {'node_modules', '.git', 'dist', 'build', '.next', 'target', '.driver', '.turbo', 'out', '.cache', '__pycache__', '.svelte-kit'}
SKIP_EXTS = {'.png', '.jpg', '.jpeg', '.gif', '.ico', '.webp', '.bmp', '.svg', '.woff', '.woff2', '.ttf', '.eot', '.otf', '.map', '.lock', '.sum', '.bin', '.exe', '.dll', '.so', '.dylib', '.a', '.lib', '.pdf', '.doc', '.docx', '.xls', '.xlsx', '.ppt', '.pptx', '.zip', '.tar', '.gz', '.tgz', '.rar', '.7z', '.mp3', '.mp4', '.wav', '.avi', '.mov', '.pyc'}

def should_skip(filepath):
    ext = os.path.splitext(filepath)[1].lower()
    if ext in SKIP_EXTS:
        return True
    # Skip tsconfig.tsbuildinfo (binary-like)
    if filepath.endswith('.tsbuildinfo'):
        return True
    return False

def replace_content(content):
    for old, new in REPLACEMENTS:
        content = content.replace(old, new)
    return content

def main():
    print("=== [1/2] 替换文件内容 ===")
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
    print(f"  修改文件数: {modified}")

    print("\n=== [2/2] 重命名文件和目录 ===")
    renamed = 0
    # Collect rename targets (depth-first to avoid path issues)
    targets = []
    for dirpath, dirnames, filenames in os.walk(ROOT, topdown=False):
        if any(d in SKIP_DIRS for d in dirpath.split(os.sep)):
            continue
        for name in filenames + dirnames:
            if 'bwbrowser' in name.lower():
                old_path = os.path.join(dirpath, name)
                new_name = replace_content(name)
                if new_name != name:
                    new_path = os.path.join(dirpath, new_name)
                    targets.append((old_path, new_path))
    # Execute renames
    for old_path, new_path in targets:
        try:
            if os.path.exists(old_path) and not os.path.exists(new_path):
                os.rename(old_path, new_path)
                renamed += 1
                print(f"  {os.path.relpath(old_path, ROOT)} -> {os.path.basename(new_path)}")
        except Exception as e:
            print(f"  FAILED: {old_path}: {e}")
    print(f"  重命名数: {renamed}")
    print("\n完成!")

if __name__ == '__main__':
    main()
