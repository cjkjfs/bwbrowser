#!/usr/bin/env python3
import paramiko, time

HOST = "47.93.197.114"
USER = "root"
PASS = "Aa850120"
TK_DIR = "/www/wwwroot/www.yacm.xin/tk"

ssh = paramiko.SSHClient()
ssh.set_missing_host_key_policy(paramiko.AutoAddPolicy())
ssh.connect(HOST, username=USER, password=PASS, timeout=15)
sftp = ssh.open_sftp()

# Upload check script
sftp.put(r'd:\bian\donutbrowser-latest\server\tk\_check_db.php', f'{TK_DIR}/_check_db.php')
time.sleep(0.5)

# Run it
stdin, stdout, stderr = ssh.exec_command(f"php {TK_DIR}/_check_db.php")
out = stdout.read().decode()
err = stderr.read().decode()
print(out)
if err:
    print(f"STDERR: {err}")

# Cleanup
ssh.exec_command(f"rm {TK_DIR}/_check_db.php")
sftp.close()
ssh.close()
