<#
.SYNOPSIS
  DEVNET ONLY: proves the P2P chain relay end to end on one Windows machine.

.DESCRIPTION
  Starts two mock chain gateways (node/hashgram-devtools mock-gateway), two
  hashgram-node processes with the `relay` role and *different* operator
  keys pointed at them, then runs `hashgram-client wallet balance` with NO
  chain_api configured. The client must report the balance as
  "verified by 2 nodes" - every read cross-checked across two operators over
  /hashgram/rpc/1, no HTTP endpoint anywhere on the client side.

  With -Lie the second gateway answers balances one uhash too high; the
  client must then refuse the read ("nodes disagree") because no third node
  exists to break the tie, and must NOT print a balance.

  Also checks that the node's local API passthrough denies
  /v1/chain/cosmos/tx/v1beta1/simulate (the shared allow-list).

.PARAMETER Lie
  Make gateway 2 lie about balances.

.PARAMETER Keep
  Leave the processes running (for poking at with the client) and print how
  to stop them.

.EXAMPLE
  pwsh scripts/testnet/chain-relay-devnet.ps1
  pwsh scripts/testnet/chain-relay-devnet.ps1 -Lie
#>
[CmdletBinding()]
param(
    [switch]$Lie,
    [switch]$Keep,
    [string]$DevnetDir = ""
)

# Native tools write progress to stderr; that must not be an error here.
$ErrorActionPreference = "Continue"
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$bin = Join-Path $root "node\target\debug"
if ($DevnetDir -eq "") { $DevnetDir = Join-Path $root "devnet-relay" }

$node = Join-Path $bin "hashgram-node.exe"
$client = Join-Path $bin "hashgram-client.exe"
$gateway = Join-Path $bin "mock-gateway.exe"
foreach ($b in @($node, $client, $gateway)) {
    if (-not (Test-Path $b)) {
        throw "missing $b - run: cd node; cargo build -p hashgram-node -p hashgram-client -p hashgram-devtools"
    }
}

# A devnet genesis "hash": any 64 lowercase hex characters. It can never be
# mistaken for Mainnet - the devnet magic and chain id differ.
$genesis = "6465766e65742d72656c61792d6861726e657373000000000000000000000001"
$chainId = "hashgram-devnet-1"

$procs = @()
function Start-Bg([string]$exe, [string[]]$argv, [string]$log) {
    $p = Start-Process -FilePath $exe -ArgumentList $argv -PassThru -NoNewWindow `
        -RedirectStandardError $log -RedirectStandardOutput "$log.out"
    $script:procs += $p
    return $p
}
function Stop-All {
    foreach ($p in $script:procs) {
        try { if (-not $p.HasExited) { Stop-Process -Id $p.Id -Force } } catch {}
    }
}
function Wait-Http([string]$url, [int]$seconds) {
    $deadline = (Get-Date).AddSeconds($seconds)
    while ((Get-Date) -lt $deadline) {
        try {
            $r = Invoke-WebRequest -Uri $url -UseBasicParsing -TimeoutSec 2
            if ($r.StatusCode -ge 200 -and $r.StatusCode -lt 500) { return $true }
        } catch {}
        Start-Sleep -Milliseconds 300
    }
    return $false
}

$failures = 0
function Ok([string]$m) { Write-Host "  [ ok ] $m" }
function Bad([string]$m) { Write-Host "  [FAIL] $m"; $script:failures++ }

try {
    if (Test-Path $DevnetDir) { Remove-Item -Recurse -Force $DevnetDir }
    New-Item -ItemType Directory -Path $DevnetDir | Out-Null
    foreach ($d in @("n1", "n2", "client", "logs")) { New-Item -ItemType Directory -Path (Join-Path $DevnetDir $d) | Out-Null }
    $logs = Join-Path $DevnetDir "logs"

    Write-Host ""
    Write-Host "Chain relay devnet (DEVNET ONLY)  lie=$Lie"
    Write-Host "=========================================================================="

    # 1. Two gateways.
    $gw1 = 31317; $gw2 = 31318
    Start-Bg $gateway @("--port", "$gw1", "--chain-id", $chainId, "--height", "1000") (Join-Path $logs "gw1.log") | Out-Null
    $gw2Args = @("--port", "$gw2", "--chain-id", $chainId, "--height", "1001")
    if ($Lie) { $gw2Args += "--lie" }
    Start-Bg $gateway $gw2Args (Join-Path $logs "gw2.log") | Out-Null
    foreach ($port in @($gw1, $gw2)) {
        if (Wait-Http "http://127.0.0.1:$port/cosmos/base/tendermint/v1beta1/node_info" 10) { Ok "mock gateway on $port" } else { Bad "mock gateway on $port did not start" }
    }

    # 2. Two nodes with distinct operator keys (the handshake carries the
    #    operator address; cross-checking wants two different ones).
    $ports = @{ n1 = @{ p2p = 27671; api = 27672; metrics = 27673; gw = $gw1 }; n2 = @{ p2p = 27681; api = 27682; metrics = 27683; gw = $gw2 } }
    $peerIds = @{}
    foreach ($n in @("n1", "n2")) {
        $nodeHome = Join-Path $DevnetDir $n
        & $node operator-key --home $nodeHome 2>$null | Out-Null
        $peerIds[$n] = (& $node node-id --home $nodeHome 2>$null | Select-Object -Last 1).Trim()
        if ($peerIds[$n] -notmatch '^12D3Koo') { Bad "$n peer id: '$($peerIds[$n])'" }
    }
    $addr1quic = "/ip4/127.0.0.1/udp/$($ports.n1.p2p)/quic-v1/p2p/$($peerIds.n1)"
    $addr1tcp = "/ip4/127.0.0.1/tcp/$($ports.n1.p2p)/p2p/$($peerIds.n1)"
    $addr2quic = "/ip4/127.0.0.1/udp/$($ports.n2.p2p)/quic-v1/p2p/$($peerIds.n2)"
    $addr2tcp = "/ip4/127.0.0.1/tcp/$($ports.n2.p2p)/p2p/$($peerIds.n2)"

    foreach ($n in @("n1", "n2")) {
        $nodeHome = Join-Path $DevnetDir $n
        $p = $ports[$n]
        $boot = if ($n -eq "n2") { "bootstrap_peers = [`"$addr1quic`", `"$addr1tcp`"]" } else { "bootstrap_peers = []" }
        $toml = @"
network = "devnet"
genesis_hash = "$genesis"
roles = ["relay"]
moniker = "$n"
listen_addr = "127.0.0.1"
listen_port = $($p.p2p)
api_addr = "127.0.0.1:$($p.api)"
metrics_addr = "127.0.0.1:$($p.metrics)"
chain_api = "http://127.0.0.1:$($p.gw)"
peerstore_path = "$(($nodeHome -replace '\\','/'))/peerstore.json"
data_dir = "$(($nodeHome -replace '\\','/'))"
min_peers = 1
$boot
"@
        Set-Content -Path (Join-Path $nodeHome "node.toml") -Value $toml -Encoding ascii
        Start-Bg $node @("run", "--home", $nodeHome, "--config", (Join-Path $nodeHome "node.toml")) (Join-Path $logs "$n.log") | Out-Null
    }
    foreach ($n in @("n1", "n2")) {
        if (Wait-Http "http://127.0.0.1:$($ports[$n].api)/v1/health" 20) { Ok "$n up (peer $($peerIds[$n]))" } else { Bad "$n did not start; see $logs\$n.log" }
    }
    Start-Sleep -Seconds 2
    $n1log = Get-Content (Join-Path $logs "n1.log") -Raw
    if ($n1log -match "chain relay enabled") { Ok "n1 enabled the chain relay" } else { Bad "n1 did not enable the chain relay" }

    # 3. The shared allow-list at the local API: simulate is denied at the node.
    try {
        Invoke-WebRequest -Uri "http://127.0.0.1:$($ports.n1.api)/v1/chain/cosmos/tx/v1beta1/simulate" -UseBasicParsing -TimeoutSec 5 | Out-Null
        Bad "local API forwarded /cosmos/tx/v1beta1/simulate"
    } catch {
        $code = $_.Exception.Response.StatusCode.value__
        if ($code -eq 400) { Ok "local API denies cosmos/tx/v1beta1/simulate (400)" } else { Bad "local API simulate: unexpected status $code" }
    }
    try {
        $r = Invoke-WebRequest -Uri "http://127.0.0.1:$($ports.n1.api)/v1/chain/cosmos/bank/v1beta1/supply/by_denom?denom=uhash" -UseBasicParsing -TimeoutSec 5
        if ($r.Content -match '1000000000000000') { Ok "local API forwards an allow-listed read" } else { Bad "local API read returned unexpected body" }
    } catch { Bad "local API allow-listed read failed: $_" }

    # 4. The client, with NO chain_api, bootstrapping to both nodes.
    $env:HASHGRAM_CLIENT_HOME = Join-Path $DevnetDir "client"
    $env:HASHGRAM_PASSPHRASE = "devnet-only"
    $env:HASHGRAM_LIGHT_KDF = "true"
    & $client configure --network devnet --genesis-hash $genesis --bootstrap $addr1quic --bootstrap $addr1tcp --bootstrap $addr2quic --bootstrap $addr2tcp 2>&1 | Out-String | Write-Verbose
    $profile = Get-Content (Join-Path $env:HASHGRAM_CLIENT_HOME "profile.toml") -Raw
    if ($profile -match 'chain_api = ""') { Ok "client profile has no chain_api" } else { Bad "client profile unexpectedly has a chain_api" }

    $peersOut = & $client net peers 2>$null
    $seen = @($peersOut | Where-Object { $_ -match '^12D3Koo' }).Count
    if ($seen -eq 2) { Ok "client verified both nodes: `n$($peersOut -join "`n")" } else { Bad "client saw $seen verified peers:`n$peersOut" }

    $target = "hash13t8v5nnghrvgcuuqcrt9k5wyhtqwq7fl3ynjpy"
    $balRaw = & $client --json wallet balance $target 2>&1
    $balText = ($balRaw | Out-String)
    if ($Lie) {
        if ($balText -match "nodes disagree") { Ok "lying node caught: client refused the read ('nodes disagree')" } else { Bad "expected 'nodes disagree', got:`n$balText" }
        if ($balText -notmatch '"uhash"') { Ok "no balance was printed from disputed answers" } else { Bad "a balance was printed despite the dispute" }
    } else {
        try {
            $json = ($balRaw | Where-Object { $_ -is [string] } | Out-String) | ConvertFrom-Json
            if ($json.transport -eq "p2p chain relay") { Ok "transport: $($json.transport)" } else { Bad "transport: $($json.transport)" }
            if ($json.verified_by -eq 2 -and $json.agreed -eq $true) { Ok "balance $($json.hash) HASH verified by 2 nodes, agreed" } else { Bad "verified_by=$($json.verified_by) agreed=$($json.agreed)" }
            if ($json.single_operator -eq $false) { Ok "two distinct operators" } else { Bad "single_operator=$($json.single_operator)" }
        } catch { Bad "balance output was not JSON:`n$balText" }
    }

    # 5. Metrics: the relay counted the requests.
    $mr = Invoke-WebRequest -Uri "http://127.0.0.1:$($ports.n1.api)/metrics" -UseBasicParsing -TimeoutSec 5
    # OpenMetrics is served as application/openmetrics-text, which
    # Invoke-WebRequest hands back as bytes.
    $m = if ($mr.Content -is [byte[]]) { [Text.Encoding]::UTF8.GetString($mr.Content) } else { [string]$mr.Content }
    if ($m -match 'hashgram_node_chain_relay_requests_total\{kind="Query"\} [1-9]') { Ok "n1 metrics count chain relay queries" } else { Bad "n1 metrics show no chain relay queries" }
}
finally {
    if ($Keep) {
        Write-Host ""
        Write-Host "left running (pids: $(($procs | ForEach-Object { $_.Id }) -join ', ')). Stop with:"
        Write-Host "  Stop-Process -Id $(($procs | ForEach-Object { $_.Id }) -join ',')"
    } else {
        Stop-All
    }
}

Write-Host ""
if ($failures -eq 0) { Write-Host "chain relay devnet: all checks passed"; exit 0 } else { Write-Host "chain relay devnet: $failures check(s) failed (logs in $DevnetDir\logs)"; exit 1 }
