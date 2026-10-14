#!/usr/bin/env python3
# -*- coding: utf-8 -*-
# 一次性补丁：create_account 查重加 platform 条件（同账号名可存在于不同平台）
import io

p = '/www/wwwroot/www.yacm.xin/tk/bwbrowser_sync.php'
s = io.open(p, encoding='utf-8').read()

old_sql = 'SELECT id FROM tiktok_accounts WHERE account_name = ? AND company_id = ? AND is_deleted = 0'
new_sql = 'SELECT id FROM tiktok_accounts WHERE account_name = ? AND company_id = ? AND platform = ? AND is_deleted = 0'
assert s.count(old_sql) == 1, 'sql not unique: %d' % s.count(old_sql)
s = s.replace(old_sql, new_sql)

old_exec = "$stmt->execute([$account_name, $user['company_id']]);"
new_exec = "$stmt->execute([$account_name, $user['company_id'], $platform]);"
assert s.count(old_exec) == 1, 'exec not unique: %d' % s.count(old_exec)
s = s.replace(old_exec, new_exec)

old_msg = "'message' => '账号名称已存在'"
new_msg = "'message' => '该账号名称在此平台已存在，请勿重复'"
assert s.count(old_msg) == 1, 'msg not unique: %d' % s.count(old_msg)
s = s.replace(old_msg, new_msg)

io.open(p, 'w', encoding='utf-8').write(s)
print('PATCHED')
