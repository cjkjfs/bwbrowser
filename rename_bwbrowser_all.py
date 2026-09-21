#!/usr/bin/env python3
"""
批量替换所有剩余的 donut 引用为 bwbrowser
包括：文件内容、文件名、目录名
"""
import os, re

ROOT = r'd:\bian\donutbrowser-latest'

# Replace donut -> bwbrowser, Donut -> Bwbrowser, DONUT -> BWBROWSER
# But NOT donutbrowser (already handled) or donut-sync (directory)
REPLACEMENTS = [
    ('DONUT', 'BWBROWSER'),
    ('Donut', 'Bwbrowser'),
    ('donut', 'bwbrowser'),
]

SKIP_DIRS = {'node_modules', '.git', 'dist', 'build', '.next', 'target', '.driver', '.turbo', 'out', '.cache', '__pycache__', '.svelte-kit'}
SKIP_EXTS = {'.png', '.jpg', '.jpeg', '.gif', '.ico', '.webp', '.bmp', '.svg', '.woff', '.woff2', '.ttf', '.eot', '.otf', '.map', '.lock', '.sum', '.bin', '.exe', '.dll', '.so', '.dylib', '.a', '.lib', '.pdf', '.doc', '.docx', '.xls', '.xlsx', '.ppt', '.pptx', '.zip', '.tar', '.gz', '.tgz', '.rar', '.7z', '.mp3', '.mp4', '.wav', '.avi', '.mov', '.pyc', '.tsbuildinfo'}
SKIP_FILES = {'AGENTS.md', 'CLAUDE.md', 'CONTRIBUTING.md', 'LICENSE', 'SECURITY.md', 'CODE_OF_CONDUCT.md', 'dev-log.txt', 'flake.nix'}
# Skip temp scripts
SKIP_FILE_PATTERNS = [r'^rename_.*\.py$', r'^check_.*\.py$', r'^deploy_.*\.py$', r'^init_db\.py$']

def should_skip(filepath):
    fname = os.path.basename(filepath)
    if fname in SKIP_FILES:
        return True
    for pat in SKIP_FILE_PATTERNS:
        if re.match(pat, fname):
            return True
    ext = os.path.splitext(filepath)[1].lower()
    if ext in SKIP_EXTS:
        return True
    return False

def replace_content(content):
    for old, new in REPLACEMENTS:
        content = content.replace(old, new)
    return content

def main():
    print("替换所有剩余 donut -> bwbrowser...")
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

    # Rename files with 'donut' in name
    print("\n重命名文件...")
    renamed = 0
    targets = []
    for dirpath, dirnames, filenames in os.walk(ROOT):
        dirnames[:] = [d for d in dirnames if d not in SKIP_DIRS]
        for name in filenames + dirnames:
            if 'donut' in name.lower():
                old_path = os.path.join(dirpath, name)
                new_name = replace_content(name)
                if new_name != name:
                    targets.append((old_path, new_name))
    for old_path, new_name in targets:
        new_path = os.path.join(os.path.dirname(old_path), new_name)
        try:
            if os.path.exists(old_path) and not os.path.exists(new_path):
                os.rename(old_path, new_path)
                renamed += 1
                print(f"  {os.path.relpath(old_path, ROOT)} -> {new_name}")
        except Exception as e:
            print(f"  FAILED: {old_path}: {e}")
    print(f"重命名数: {renamed}")

    # Rename directories with 'donut' in name (depth-first)
    print("\n重命名目录...")
    dir_targets = []
    for dirpath, dirnames, filenames in os.walk(ROOT, topdown=False):
        if any(d in SKIP_DIRS for d in dirpath.split(os.sep)):
            continue
        for dname in dirnames:
            if 'donut' in dname.lower():
                old_path = os.path.join(dirpath, dname)
                new_name = replace_content(dname)
                if new_name != dname:
                    dir_targets.append((old_path, os.path.join(dirpath, new_name)))
    dir_renamed = 0
    for old_path, new_path in dir_targets:
        try:
            if os.path.exists(old_path) and not os.path.exists(new_path):
                os.rename(old_path, new_path)
                dir_renamed += 1
                print(f"  {os.path.relpath(old_path, ROOT)} -> {os.path.basename(new_path)}")
        except Exception as e:
            print(f"  FAILED: {old_path}: {e}")
    print(f"目录重命名数: {dir_renamed}")

    print("\n完成!")

if __name__ == '__main__':
    main()
