#!/usr/bin/env python3
"""
1. 删除旧的 bwbrowser_sync.php (原 simprint_sync.php, 不被 Rust 代码使用)
2. 将 donut_sync.php 重命名为 bwbrowser_sync.php
3. 替换文件内容中所有 donut -> bwbrowser
4. 更新 Rust 代码中的 API URL
"""
import os, shutil

ROOT = r'd:\bian\donutbrowser-latest'
TK_DIR = os.path.join(ROOT, 'server', 'tk')

# Step 1: Delete old bwbrowser_sync.php (renamed from simprint_sync.php)
old_bwbrowser_sync = os.path.join(TK_DIR, 'bwbrowser_sync.php')
if os.path.exists(old_bwbrowser_sync):
    os.remove(old_bwbrowser_sync)
    print("已删除旧的 bwbrowser_sync.php (原 simprint_sync.php)")

# Step 2: Rename donut_sync.php -> bwbrowser_sync.php
donut_sync = os.path.join(TK_DIR, 'donut_sync.php')
new_bwbrowser_sync = os.path.join(TK_DIR, 'bwbrowser_sync.php')
if os.path.exists(donut_sync):
    # Read content, replace donut -> bwbrowser
    with open(donut_sync, 'r', encoding='utf-8-sig') as f:
        content = f.read()
    # Replace donut_ function prefixes and variables
    content = content.replace('DONUT_LOG', 'BWBROWSER_LOG')
    content = content.replace('donut_log', 'bwbrowser_log')
    content = content.replace('donut_get_user', 'bwbrowser_get_user')
    content = content.replace('donut_is_super_admin', 'bwbrowser_is_super_admin')
    content = content.replace('donut_is_manager', 'bwbrowser_is_manager')
    content = content.replace('donut_get_module_perms', 'bwbrowser_get_module_perms')
    content = content.replace('donut_check_perm', 'bwbrowser_check_perm')
    content = content.replace("donut-1.0.0", "bwbrowser-1.0.0")
    content = content.replace("donut-unknown", "bwbrowser-unknown")
    content = content.replace("donut_sync.log", "bwbrowser_sync.log")
    # Write to new file
    with open(new_bwbrowser_sync, 'w', encoding='utf-8') as f:
        f.write(content)
    # Delete old file
    os.remove(donut_sync)
    print(f"已重命名 donut_sync.php -> bwbrowser_sync.php (含内容替换)")

# Step 3: Update API URL in Rust code
rust_file = os.path.join(ROOT, 'src-tauri', 'src', 'bwbrowser_cloud.rs')
with open(rust_file, 'r', encoding='utf-8') as f:
    content = f.read()

old_url = 'https://www.yacm.xin/tk/donut_sync.php'
new_url = 'https://www.yacm.xin/tk/bwbrowser_sync.php'
content = content.replace(old_url, new_url)
# Also fix the comment
content = content.replace('bwbrowser_sync.php 的账号密码登录系统', 'bwbrowser_sync.php 的账号密码登录系统')
content = content.replace('https://www.yacm.xin/bwbrowser_sync.php', 'https://www.yacm.xin/tk/bwbrowser_sync.php')

with open(rust_file, 'w', encoding='utf-8') as f:
    f.write(content)
print(f"已更新 Rust API URL: {new_url}")

# Step 4: Also rename any backup files
for f in os.listdir(TK_DIR):
    if 'donut_sync' in f and f != 'donut_sync.php':
        old_path = os.path.join(TK_DIR, f)
        new_name = f.replace('donut_sync', 'bwbrowser_sync')
        new_path = os.path.join(TK_DIR, new_name)
        if not os.path.exists(new_path):
            os.rename(old_path, new_path)
            print(f"  重命名备份: {f} -> {new_name}")

print("\n完成!")
