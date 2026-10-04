$ErrorActionPreference = "Stop"
function Invoke-CDP($ws, $id, $method, $params) {
  $req = @{ id = $id; method = $method; params = $params } | ConvertTo-Json -Compress -Depth 8
  $reqBytes = [System.Text.Encoding]::UTF8.GetBytes($req)
  $buf = New-Object byte[] 262144
  $ws.SendAsync([ArraySegment[byte]]$reqBytes, [System.Net.WebSockets.WebSocketMessageType]::Text, $true, [System.Threading.CancellationToken]::None).Wait()
  while ($true) {
    $r = $ws.ReceiveAsync([ArraySegment[byte]]$buf, [System.Threading.CancellationToken]::None).Result
    $msg = [System.Text.Encoding]::UTF8.GetString($buf, 0, $r.Count)
    $obj = $msg | ConvertFrom-Json
    if ($obj.id -eq $id) { return $obj }
  }
}
$ws = [System.Net.WebSockets.ClientWebSocket]::new()
$ws.ConnectAsync([Uri]"ws://127.0.0.1:62914/devtools/page/EAA05FB118AE441D955B1B900117D588", [System.Threading.CancellationToken]::None).Wait()
$expr = 'JSON.stringify({scrW: screen.width, scrH: screen.height, avW: screen.availWidth, avH: screen.availHeight, dpr: devicePixelRatio, outW: window.outerWidth, outH: window.outerHeight, inW: window.innerWidth, inH: window.innerHeight, ua: navigator.userAgent, lang: navigator.language})'
$resp = Invoke-CDP $ws 1 "Runtime.evaluate" @{ expression = $expr; returnByValue = $true }
"VALUE: " + $resp.result.result.value
"RAW: " + ($resp | ConvertTo-Json -Compress -Depth 8)
$ws.Dispose()
