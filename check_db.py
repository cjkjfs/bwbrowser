#!/usr/bin/env python3
"""
通过 SSH 检查服务器数据库中的用户和配置
"""
import paramiko

HOST = "47.93.197.114"
USER = "root"
PASS = "Aa850120"
TK_DIR = "/www/wwwroot/www.yacm.xin/tk"

def main():
    print(f"连接到 {HOST}...")
    ssh = paramiko.SSHClient()
    ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
    ssh.connect(HOST, username=USER, password=PASS, timeout=15)
    print("SSH 连接成功!\n")

    # Read config.php to get DB credentials
    sftp = ssh.open_sftp()
    try:
        with sftp.open(f"{TK_DIR}/config.php", 'r') as f:
            config_content = f.read().decode()
        print("=== config.php ===")
        print(config_content[:500])
        print("...")
    except:
        print("无法读取 config.php")

    # Check users table
    print("\n=== 检查 users 表 ===")
    stdin, stdout, stderr = ssh.exec_command(
        f"cd {TK_DIR} && php -r \""
        "include 'config.php';"
        "$stmt = $pdo->query('SELECT id, username, role, company_id FROM users LIMIT 20');"
        "while($row = $stmt->fetch()){{"
        "echo implode(' | ', $row).PHP_EOL;"
        "}}\""
    )
    out = stdout.read().decode()
    err = stderr.read().decode()
    if out:
        print(out)
    if err:
        print(f"Error: {err}")

    # Check if bwbrowser_proxies table exists
    print("\n=== 检查 bwbrowser_proxies 表 ===")
    stdin, stdout, stderr = ssh.exec_command(
        f"cd {TK_DIR} && php -r \""
        "include 'config.php';"
        "$stmt = $pdo->query('SHOW TABLES LIKE \"bwbrowser_proxies\"');"
        "if($stmt->fetch()){{echo 'bwbrowser_proxies 表存在'.PHP_EOL;}}"
        "else{{echo 'bwbrowser_proxies 表不存在'.PHP_EOL;}}"
        "$stmt2 = $pdo->query('SELECT COUNT(*) as cnt FROM bwbrowser_proxies');"
        "$cnt = $stmt2->fetch();"
        "echo '代理数量: '.$cnt['cnt'].PHP_EOL;"
        "\""
    )
    out = stdout.read().decode()
    err = stderr.read().decode()
    if out:
        print(out)
    if err:
        print(f"Error: {err}")

    # Check bwbrowser_envs table
    print("\n=== 检查 bwbrowser_envs 表 ===")
    stdin, stdout, stderr = ssh.exec_command(
        f"cd {TK_DIR} && php -r \""
        "include 'config.php';"
        "$stmt = $pdo->query('SHOW TABLES LIKE \"bwbrowser_envs\"');"
        "if($stmt->fetch()){{echo 'bwbrowser_envs 表存在'.PHP_EOL;}}"
        "else{{echo 'bwbrowser_envs 表不存在'.PHP_EOL;}}"
        "$stmt2 = $pdo->query('SELECT COUNT(*) as cnt FROM bwbrowser_envs');"
        "$cnt = $stmt2->fetch();"
        "echo '环境数量: '.$cnt['cnt'].PHP_EOL;"
        "\""
    )
    out = stdout.read().decode()
    err = stderr.read().decode()
    if out:
        print(out)
    if err:
        print(f"Error: {err}")

    # Check tiktok_accounts table for bwbrowser accounts
    print("\n=== 检查 tiktok_accounts 表 ===")
    stdin, stdout, stderr = ssh.exec_command(
        f"cd {TK_DIR} && php -r \""
        "include 'config.php';"
        "$stmt = $pdo->query('SELECT COUNT(*) as cnt FROM tiktok_accounts WHERE is_bwbrowser=1');"
        "$cnt = $stmt->fetch();"
        "echo 'bwbrowser 账号数量: '.$cnt['cnt'].PHP_EOL;"
        "\""
    )
    out = stdout.read().decode()
    err = stderr.read().decode()
    if out:
        print(out)
    if err:
        print(f"Error: {err}")

    sftp.close()
    ssh.close()
    print("\n检查完成!")

if __name__ == '__main__':
    main()
