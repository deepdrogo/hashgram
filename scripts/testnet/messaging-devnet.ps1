<#
.SYNOPSIS
  DEVNET ONLY: proves end-to-end encrypted messaging on one Windows machine.

.DESCRIPTION
  Starts two mock chain gateways that know two devices (Alice's and Bob's)
  and two @usernames, two hashgram-node processes with the `relay` and
  `store` roles pointed at them, then drives two hashgram-client profiles
  through the whole path a desktop user takes:

    1. each device publishes its MLS key packages to the store nodes;
    2. Alice resolves Bob's devices through the P2P chain relay (no HTTP on
       the client), creates the MLS group, delivers the Welcome and a text
       message into Bob's mailbox on the store nodes;
    3. Bob fetches his mailbox, joins the group and decrypts the text;
    4. Bob replies; Alice receives it;
    5. a second sync on each side is quiet (duplicates from two store nodes
       are recognised, not re-delivered).

  Everything runs over the real libp2p swarm on loopback (TCP and QUIC),
  which is also what exercises the Windows dial path.

.PARAMETER Keep
  Leave the processes running and print how to stop them.

.EXAMPLE
  powershell -File scripts/testnet/messaging-devnet.ps1
#>
[CmdletBinding()]
param(
    [switch]$Keep,
    [string]$DevnetDir = ""
)

$ErrorActionPreference = "Continue"
# A script error is a failed check, not a pass with a stack trace above it.
trap { Bad "script error: $_"; continue }
$root = Resolve-Path (Join-Path $PSScriptRoot "..\..")
$bin = Join-Path $root "node\target\debug"
if ($DevnetDir -eq "") { $DevnetDir = Join-Path $root "devnet-messaging" }

$node = Join-Path $bin "hashgram-node.exe"
$client = Join-Path $bin "hashgram-client.exe"
$gateway = Join-Path $bin "mock-gateway.exe"
foreach ($b in @($node, $client, $gateway)) {
    if (-not (Test-Path $b)) {
        throw "missing $b - run: cd node; cargo build -p hashgram-node -p hashgram-client -p hashgram-devtools"
    }
}

$genesis = "6465766e65742d6d6573736167696e672d6861726e6573730000000000000001"
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
# Runs the client for one profile and returns every line it printed.
function Client([string]$profileDir, [string[]]$argv) {
    $env:HASHGRAM_CLIENT_HOME = $profileDir
    $out = & $client @argv 2>&1 | ForEach-Object { "$_" }
    return $out
}

$failures = 0
function Ok([string]$m) { Write-Host "  [ ok ] $m" }
function Bad([string]$m) { Write-Host "  [FAIL] $m"; $script:failures++ }

try {
    if (Test-Path $DevnetDir) { Remove-Item -Recurse -Force $DevnetDir }
    New-Item -ItemType Directory -Path $DevnetDir | Out-Null
    foreach ($d in @("n1", "n2", "alice", "bob", "logs")) { New-Item -ItemType Directory -Path (Join-Path $DevnetDir $d) | Out-Null }
    $logs = Join-Path $DevnetDir "logs"

    Write-Host ""
    Write-Host "Messaging devnet (DEVNET ONLY)"
    Write-Host "=========================================================================="

    # 0. Two accounts, offline: the mock gateway is told their device keys,
    #    which is what MsgCreateIdentity would have put on a real chain.
    $env:HASHGRAM_PASSPHRASE = "devnet-only"
    $env:HASHGRAM_LIGHT_KDF = "true"
    $who = @{}
    foreach ($n in @("alice", "bob")) {
        $profileDir = Join-Path $DevnetDir $n
        Client $profileDir @("configure", "--network", "devnet", "--genesis-hash", $genesis) | Out-Null
        Client $profileDir @("identity", "create", "--offline", "--device-id", "$n-pc", "--platform", "windows") | Out-Null
        $show = (Client $profileDir @("--json", "identity", "show")) -join "`n"
        try {
            $j = $show | ConvertFrom-Json
            $who[$n] = @{ home = $profileDir; address = $j.address; device = $j.device_pubkey }
            if ($j.address -match '^hash1' -and $j.device_pubkey.Length -eq 64) { Ok "$n = $($j.address) device $($j.device_pubkey.Substring(0,8))..." } else { Bad "$n identity: $show" }
        } catch { Bad "$n identity show was not JSON:`n$show" }
    }

    # 1. Two gateways that know both devices and both usernames.
    $gw1 = 31417; $gw2 = 31418
    $registry = @(
        "--device", "$($who.alice.address)=$($who.alice.device)",
        "--device", "$($who.bob.address)=$($who.bob.device)",
        "--username", "alice=$($who.alice.address)",
        "--username", "bob=$($who.bob.address)"
    )
    Start-Bg $gateway (@("--port", "$gw1", "--chain-id", $chainId, "--height", "1000") + $registry) (Join-Path $logs "gw1.log") | Out-Null
    Start-Bg $gateway (@("--port", "$gw2", "--chain-id", $chainId, "--height", "1001") + $registry) (Join-Path $logs "gw2.log") | Out-Null
    foreach ($port in @($gw1, $gw2)) {
        if (Wait-Http "http://127.0.0.1:$port/cosmos/base/tendermint/v1beta1/node_info" 10) { Ok "mock gateway on $port" } else { Bad "mock gateway on $port did not start" }
    }
    try {
        $r = Invoke-WebRequest -Uri "http://127.0.0.1:$gw1/hashgram/identity/v1/devices/$($who.bob.address)" -UseBasicParsing -TimeoutSec 5
        if ($r.Content -match 'device_pubkey') { Ok "gateway serves Bob's device from the registry" } else { Bad "gateway devices answer: $($r.Content)" }
    } catch { Bad "gateway devices read failed: $_" }

    # 2. Two nodes: relay (chain reads) + store (mailboxes, key packages).
    $ports = @{ n1 = @{ p2p = 27771; api = 27772; metrics = 27773; gw = $gw1 }; n2 = @{ p2p = 27781; api = 27782; metrics = 27783; gw = $gw2 } }
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
roles = ["relay", "store"]
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
    if ($n1log -match "store services enabled") { Ok "n1 serves the store role" } else { Bad "n1 did not enable store services" }
    if ($n1log -match "chain relay enabled") { Ok "n1 enabled the chain relay" } else { Bad "n1 did not enable the chain relay" }

    # 3. Point both clients at the nodes (no chain_api anywhere).
    foreach ($n in @("alice", "bob")) {
        Client $who[$n].home @("configure", "--network", "devnet", "--genesis-hash", $genesis, "--bootstrap", $addr1quic, "--bootstrap", $addr1tcp, "--bootstrap", $addr2quic, "--bootstrap", $addr2tcp) | Out-Null
    }

    # 4. Key packages: each device registers its mailbox with the stores.
    foreach ($n in @("alice", "bob")) {
        $out = (Client $who[$n].home @("identity", "publish-keys")) -join "`n"
        if ($out -match "published to (\d+) store node") {
            if ([int]$Matches[1] -ge 1) { Ok "$n published key packages to $($Matches[1]) store node(s)" } else { Bad "$n published to 0 stores:`n$out" }
        } else { Bad "$n publish-keys:`n$out" }
    }

    # 5. Alice -> Bob. Bob's devices come from the chain through the relay.
    $out = (Client $who.alice.home @("message", "send", $who.bob.address, "hello bob, from alice")) -join "`n"
    if ($out -match "sent [0-9a-f]+ in conversation ([0-9a-f]+)") { $gid = $Matches[1]; Ok "alice sent into conversation $($gid.Substring(0,12))..." } else { Bad "alice send:`n$out" }

    $out = (Client $who.bob.home @("message", "receive")) -join "`n"
    if ($out -match "hello bob, from alice" -and $out -match [regex]::Escape($who.alice.address)) { Ok "bob received and decrypted alice's text" } else { Bad "bob receive:`n$out" }

    # 6. Bob -> Alice, in the same conversation.
    $out = (Client $who.bob.home @("message", "send", $who.alice.address, "hi alice, bob here")) -join "`n"
    if ($out -match "sent [0-9a-f]+ in conversation $gid") { Ok "bob replied in the same conversation" } elseif ($out -match "sent [0-9a-f]+ in conversation") { Ok "bob replied (new conversation id)" } else { Bad "bob send:`n$out" }

    $out = (Client $who.alice.home @("message", "receive")) -join "`n"
    if ($out -match "hi alice, bob here") { Ok "alice received bob's reply" } else { Bad "alice receive:`n$out" }

    # 7. Quiet second syncs: two store nodes hold every envelope twice.
    foreach ($n in @("alice", "bob")) {
        $out = (Client $who[$n].home @("message", "receive")) -join "`n"
        if ($out -match "no new messages") { Ok "${n}: second sync is quiet (duplicates recognised)" } else { Bad "$n second sync:`n$out" }
    }

    # 8. Conversations list both ways.
    $out = (Client $who.bob.home @("message", "list")) -join "`n"
    if ($out -match [regex]::Escape($who.alice.address)) { Ok "bob lists the conversation with alice" } else { Bad "bob list:`n$out" }
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
if ($failures -eq 0) { Write-Host "messaging devnet: all checks passed"; exit 0 } else { Write-Host "messaging devnet: $failures check(s) failed (logs in $DevnetDir\logs)"; exit 1 }

