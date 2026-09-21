$log = "C:\Users\Administrator\AppData\Local\BwBrowserDev\bwbrowser_debug.log"
$lines = Get-Content $log -Tail 500
$filtered = $lines | Where-Object { $_ -match "Wayfern-TZ|Launch.*fingerprint|FP-DEBUG|saved fingerprint|Successfully applied|setFingerprint succeeded|profile saved|has_fingerprint|migrating_payload|randomize" }
foreach ($line in $filtered) {
    $short = $line.Substring(0, [Math]::Min($line.Length, 200))
    Write-Output $short
}
