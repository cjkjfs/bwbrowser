#!/usr/bin/env python3
"""
通过 SSH 部署 PHP 文件到 VPS
"""
import paramiko
import os
import sys

HOST = "47.93.197.114"
USER = "root"
PASS = "Aa850120"
WEB_ROOT = "/www/wwwroot/www.yacm.xin"
TK_DIR = f"{WEB_ROOT}/tk"

LOCAL_TK = r"d:\bian\donutbrowser-latest\server\tk"

def main():
    print(f"连接到 {HOST}...")
    ssh = paramiko.SSHClient()
    ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    try:
        ssh.connect(HOST, username=USER, password=PASS, timeout=15)
        print("SSH 连接成功!")
    except Exception as e:
        print(f"SSH 连接失败: {e}")
        sys.exit(1)

    # Create SFTP client
    sftp = ssh.open_sftp()

    # Ensure tk directory exists
    ssh.exec_command(f"mkdir -p {TK_DIR}")
    import time; time.sleep(1)

    # List PHP files to upload (only main ones, not backups)
    files_to_upload = [
        'bwbrowser_sync.php',
        'bwbrowser_accounts.php',
        'bwbrowser_cloud.php',
        'bwbrowser_updates.php',
        'bwbrowser_update_admin.php',
        'login.php',
        'nav.php',
        'register_company.php',
        'update_manage.php',
    ]

    # Check for config.php locally
    config_local = os.path.join(LOCAL_TK, 'config.php')
    if os.path.exists(config_local):
        files_to_upload.append('config.php')

    print(f"\n上传 {len(files_to_upload)} 个文件到 {TK_DIR}:")

    for fname in files_to_upload:
        local_path = os.path.join(LOCAL_TK, fname)
        remote_path = f"{TK_DIR}/{fname}"
        if os.path.exists(local_path):
            try:
                sftp.put(local_path, remote_path)
                # Set permissions
                ssh.exec_command(f"chmod 644 {remote_path}")
                print(f"  OK: {fname}")
            except Exception as e:
                print(f"  FAILED: {fname}: {e}")
        else:
            print(f"  SKIP (not found): {fname}")

    # Check if config.php exists on server
    try:
        sftp.stat(f"{TK_DIR}/config.php")
        print("\n服务器上 config.php 已存在")
    except FileNotFoundError:
        print(f"\n警告: 服务器上 {TK_DIR}/config.php 不存在!")
        print("需要手动创建 config.php (数据库连接配置)")

    # Also upload the logs and uploads directories if they don't exist
    ssh.exec_command(f"mkdir -p {TK_DIR}/logs {TK_DIR}/uploads/bwbrowser")
    time.sleep(1)
    ssh.exec_command(f"chown -R www:www {TK_DIR}/logs {TK_DIR}/uploads")
    time.sleep(1)

    # Verify deployment
    print("\n验证部署:")
    stdin, stdout, stderr = ssh.exec_command(f"ls -la {TK_DIR}/*.php 2>/dev/null | head -20")
    result = stdout.read().decode()
    print(result)

    # Test the API
    print("\n测试 API 连接:")
    stdin, stdout, stderr = ssh.exec_command(
        f"curl -s -X POST https://www.yacm.xin/tk/bwbrowser_sync.php "
        f"-d 'action=login&username=test&password=test' 2>/dev/null | head -200"
    )
    result = stdout.read().decode()
    print(f"  响应: {result[:200]}")

    sftp.close()
    ssh.close()
    print("\n部署完成!")

if __name__ == '__main__':
    main()
