#!/bin/bash
cd /www/wwwroot/www.yacm.xin/tk

# Backup
cp bwbrowser_sync.php bwbrowser_sync.php.bak2.$(date +%Y%m%d%H%M%S)

python3 << 'PYEOF'
import re

with open('bwbrowser_sync.php', 'r', encoding='utf-8') as f:
    content = f.read()

# Fix 1: Add require_once for dingtalk_helper.php after config.php require
# Find the require_once __DIR__ . '/config.php' line in bwbrowser_sync.php
old_require = "require_once __DIR__ . '/config.php';"
new_require = "require_once __DIR__ . '/config.php';\nrequire_once __DIR__ . '/dingtalk_helper.php';"

if old_require in content:
    content = content.replace(old_require, new_require, 1)
    print("Fix 1: Added dingtalk_helper.php require - OK")
else:
    print("Fix 1: WARNING - require line not found")

# Fix 2: Add attendance fetch before the array_map in list_accounts
# Find the $result = array_map line in list_accounts
old_map_start = """            $result = array_map(function($a) use ($user, $pdo) {
                // 获取归属人名字
                $owner_name = '';
                if (!empty($a['owner_id'])) {
                    try {
                        $stmt = $pdo->prepare("SELECT real_name, username FROM users WHERE id = ?");
                        $stmt->execute([$a['owner_id']]);
                        $u = $stmt->fetch();
                        if ($u) $owner_name = $u['real_name'] ?: $u['username'];
                    } catch (Exception $e) {}
                }"""

new_map_start = """            // 获取今日钉钉考勤状态
            $attendance_data = [];
            try {
                $attendance_data = dingtalk_get_today_attendance($pdo, $user['company_id'] ?? 1);
            } catch (Exception $e) {
                bwbrowser_log("list_accounts: attendance fetch failed: " . $e->getMessage(), 'WARN');
            }

            $result = array_map(function($a) use ($user, $pdo, $attendance_data) {
                // 获取归属人名字
                $owner_name = '';
                $attendance_checked = false;
                if (!empty($a['owner_id'])) {
                    try {
                        $stmt = $pdo->prepare("SELECT real_name, username FROM users WHERE id = ?");
                        $stmt->execute([$a['owner_id']]);
                        $u = $stmt->fetch();
                        if ($u) {
                            $owner_name = $u['real_name'] ?: $u['username'];
                            // 检查考勤状态
                            $attendance_checked = isset($attendance_data[$owner_name]) && $attendance_data[$owner_name] === true;
                        }
                    } catch (Exception $e) {}
                }"""

if old_map_start in content:
    content = content.replace(old_map_start, new_map_start)
    print("Fix 2: Added attendance fetch and owner_name lookup - OK")
else:
    print("Fix 2: WARNING - array_map block not found")

# Fix 3: Add attendance_checked to the return array
old_return = """                    'owner_name' => $owner_name,"""
new_return = """                    'owner_name' => $owner_name,
                    'attendance_checked' => $attendance_checked,"""

if old_return in content:
    content = content.replace(old_return, new_return, 1)
    print("Fix 3: Added attendance_checked field - OK")
else:
    print("Fix 3: WARNING - owner_name return line not found")

with open('bwbrowser_sync.php', 'w', encoding='utf-8') as f:
    f.write(content)

print("All fixes applied!")
PYEOF

chown www:www bwbrowser_sync.php
echo "=== Done ==="
