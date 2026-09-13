//! DEVNET ONLY: a stand-in for `hashgramd`'s REST gateway.
//!
//! The P2P chain relay forwards allow-listed reads to whatever answers at
//! `chain_api`. On a developer machine without a chain node this binary
//! answers instead, with deterministic, height-stamped JSON, so two
//! `hashgram-node` processes can be pointed at two instances and a client
//! can be shown reading a balance "verified by 2 nodes" — or, with `--lie`
//! on one instance, shown catching a node that answers differently.
//!
//! Nothing here is a chain. It never validates a transaction, holds no
//! state beyond a height counter and the transactions it was handed, and
//! refuses to run on any port below 1024 so it cannot be mistaken for a
//! real gateway by a misconfigured node.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use axum::extract::{Path, Query, State};
use axum::http::{HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use clap::Parser;
use sha2::{Digest, Sha256};

#[derive(Parser)]
#[command(name = "mock-gateway", about = "DEVNET ONLY mock chain REST gateway")]
struct Cli {
    /// Port on 127.0.0.1 (>= 1024).
    #[arg(long, default_value_t = 31317)]
    port: u16,
    /// Chain id reported by node_info (a node cross-checks this at start).
    #[arg(long, default_value = "hashgram-devnet-1")]
    chain_id: String,
    /// Starting block height; advances once per `--block-secs`.
    #[arg(long, default_value_t = 1000)]
    height: u64,
    /// Seconds per block.
    #[arg(long, default_value_t = 5)]
    block_secs: u64,
    /// Answer balances one uhash too high: a node that lies.
    #[arg(long)]
    lie: bool,
    /// A device on chain, `hash1<address>=<64-hex ed25519 public key>`,
    /// repeatable. Served under `hashgram/identity/v1/{identity,devices}`
    /// so two clients can run the whole messaging flow (devices from the
    /// chain, key packages and mailboxes on store nodes) with no chain.
    #[arg(long = "device")]
    devices: Vec<String>,
    /// A registered `@username`, `<name>=hash1<address>`, repeatable.
    #[arg(long = "username")]
    usernames: Vec<String>,
}

struct App {
    chain_id: String,
    height: AtomicU64,
    lie: bool,
    txs: Mutex<Vec<(String, u64)>>,
    /// DEVNET ONLY identity registry: address → devices. Lets two
    /// unfunded test clients find each other's device keys (the SDK asks
    /// the chain for them) without a chain. Seeded from `--device` and
    /// filled through `POST /devnet/identity`.
    identities: Mutex<std::collections::BTreeMap<String, Vec<MockDevice>>>,
    /// DEVNET ONLY username registry: name → owner. Seeded from
    /// `--username` and filled through `POST /devnet/identity`.
    usernames: Mutex<std::collections::BTreeMap<String, String>>,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
struct MockDevice {
    device_id: String,
    /// Hex ed25519 public key.
    device_pubkey: String,
    #[serde(default)]
    label: String,
    #[serde(default)]
    platform: String,
    #[serde(default)]
    revoked: bool,
}

fn b64(bytes: &[u8]) -> String {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity(bytes.len().div_ceil(3) * 4);
    for chunk in bytes.chunks(3) {
        let b0 = chunk.first().copied().unwrap_or(0);
        let b1 = chunk.get(1).copied().unwrap_or(0);
        let b2 = chunk.get(2).copied().unwrap_or(0);
        let n = (u32::from(b0) << 16) | (u32::from(b1) << 8) | u32::from(b2);
        for i in 0..4 {
            if i <= chunk.len() {
                let idx = ((n >> (18 - 6 * i)) & 63) as usize;
                out.push(T.get(idx).copied().unwrap_or(b'A') as char);
            } else {
                out.push('=');
            }
        }
    }
    out
}

fn b64_decode(s: &str) -> Vec<u8> {
    const T: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = Vec::new();
    let mut buf = 0u32;
    let mut bits = 0;
    for c in s.bytes() {
        let c = match c {
            b'-' => b'+',
            b'_' => b'/',
            b'=' => break,
            other => other,
        };
        let Some(v) = T.iter().position(|t| *t == c) else {
            continue;
        };
        buf = (buf << 6) | v as u32;
        bits += 6;
        if bits >= 8 {
            bits -= 8;
            out.push(((buf >> bits) & 0xff) as u8);
        }
    }
    out
}

#[derive(serde::Deserialize)]
struct RegisterIdentity {
    address: String,
    devices: Vec<MockDevice>,
    #[serde(default)]
    username: String,
}

/// `POST /devnet/identity` — registers an address with its devices (and an
/// optional username). DEVNET ONLY; a real chain requires a signed
/// certificate and gas for this.
async fn devnet_identity(State(app): S, Json(body): Json<RegisterIdentity>) -> Response {
    if let Ok(mut ids) = app.identities.lock() {
        ids.insert(body.address.clone(), body.devices.clone());
    }
    if !body.username.is_empty() {
        if let Ok(mut u) = app.usernames.lock() {
            u.insert(body.username.to_lowercase(), body.address.clone());
        }
    }
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({ "registered": body.address }),
    )
}

fn device_json(root: &str, d: &MockDevice) -> serde_json::Value {
    serde_json::json!({
        "root_address": root,
        "device_id": d.device_id,
        "device_pubkey": b64(&hex::decode(&d.device_pubkey).unwrap_or_default()),
        "key_type": "KEY_TYPE_ED25519",
        "label": d.label,
        "platform": d.platform,
        "revoked": d.revoked,
        "added_height": "1",
    })
}

fn parse_pairs(items: &[String], what: &str) -> Vec<(String, String)> {
    items
        .iter()
        .filter_map(|s| {
            let (a, b) = s.split_once('=')?;
            let (a, b) = (a.trim(), b.trim());
            if a.is_empty() || b.is_empty() {
                eprintln!("mock-gateway: ignoring malformed --{what} {s:?}");
                return None;
            }
            Some((a.to_owned(), b.to_owned()))
        })
        .collect()
}

type S = State<Arc<App>>;

fn stamped(app: &App, status: StatusCode, body: serde_json::Value) -> Response {
    let mut r = (status, Json(body)).into_response();
    if let Ok(v) = HeaderValue::from_str(&app.height.load(Ordering::Relaxed).to_string()) {
        r.headers_mut()
            .insert("grpc-metadata-x-cosmos-block-height", v);
    }
    r
}

fn not_found(app: &App) -> Response {
    stamped(
        app,
        StatusCode::NOT_FOUND,
        serde_json::json!({ "code": 5, "message": "not found", "details": [] }),
    )
}

/// Deterministic balance per address: the mock has no ledger, so the
/// amount is a function of the address, stable across instances so two
/// honest mocks agree.
fn balance_for(address: &str, lie: bool) -> u128 {
    let h = Sha256::digest(address.as_bytes());
    let mut n = 0u128;
    for b in h.iter().take(6) {
        n = (n << 8) | u128::from(*b);
    }
    (n % 5_000_000_000_000) + 1_000_000 + u128::from(lie)
}

async fn node_info(State(app): S) -> Response {
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({
            "default_node_info": { "network": app.chain_id, "moniker": "mock-gateway" },
            "application_version": { "name": "mock-gateway", "version": "devnet" }
        }),
    )
}

async fn latest_block(State(app): S) -> Response {
    let h = app.height.load(Ordering::Relaxed);
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({ "block": { "header": { "height": h.to_string(), "chain_id": app.chain_id } } }),
    )
}

async fn balance_by_denom(
    State(app): S,
    Path(address): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let denom = q.get("denom").cloned().unwrap_or_else(|| "uhash".into());
    let amount = if denom == "uhash" {
        balance_for(&address, app.lie)
    } else {
        0
    };
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({ "balance": { "denom": denom, "amount": amount.to_string() } }),
    )
}

async fn balances(State(app): S, Path(address): Path<String>) -> Response {
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({
            "balances": [{ "denom": "uhash", "amount": balance_for(&address, app.lie).to_string() }],
            "pagination": { "next_key": null, "total": "1" }
        }),
    )
}

async fn account(State(app): S, Path(address): Path<String>) -> Response {
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({
            "account": {
                "@type": "/cosmos.auth.v1beta1.BaseAccount",
                "address": address,
                "pub_key": null,
                "account_number": "7",
                "sequence": "3"
            }
        }),
    )
}

async fn supply(State(app): S) -> Response {
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({ "amount": { "denom": "uhash", "amount": "1000000000000000" } }),
    )
}

async fn founder_params(State(app): S) -> Response {
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({ "params": { "share_bps": "100", "ceiling_bps": "100", "beneficiary": "hash1mockfounder" } }),
    )
}

async fn hashgram_any(
    State(app): S,
    Path(rest): Path<String>,
    Query(q): Query<std::collections::HashMap<String, String>>,
) -> Response {
    // DEVNET identity registry.
    if let Some(addr) = rest.strip_prefix("identity/v1/devices/") {
        let devices = app
            .identities
            .lock()
            .ok()
            .and_then(|m| m.get(addr).cloned());
        return match devices {
            Some(d) => stamped(
                &app,
                StatusCode::OK,
                serde_json::json!({ "devices": d.iter().map(|x| device_json(addr, x)).collect::<Vec<_>>() }),
            ),
            None => stamped(&app, StatusCode::OK, serde_json::json!({ "devices": [] })),
        };
    }
    if let Some(addr) = rest.strip_prefix("identity/v1/identity/") {
        let active = app
            .identities
            .lock()
            .ok()
            .and_then(|m| m.get(addr).map(|d| d.iter().filter(|x| !x.revoked).count()));
        return match active {
            Some(n) => stamped(
                &app,
                StatusCode::OK,
                serde_json::json!({
                    "found": true,
                    "identity": {
                        "address": addr,
                        "root_pubkey": "",
                        "root_key_type": "KEY_TYPE_ED25519",
                        "rotation_count": 0,
                        "created_height": "1",
                        "revoked": false
                    },
                    "active_devices": n
                }),
            ),
            None => not_found(&app),
        };
    }
    if rest == "identity/v1/resolve_device_key" {
        let key = q
            .get("device_pubkey")
            .map(|k| hex::encode(b64_decode(k)))
            .unwrap_or_default();
        let found = app.identities.lock().ok().and_then(|m| {
            m.iter().find_map(|(addr, devs)| {
                devs.iter()
                    .find(|d| d.device_pubkey.eq_ignore_ascii_case(&key))
                    .map(|d| (addr.clone(), d.clone()))
            })
        });
        return match found {
            Some((addr, d)) => stamped(
                &app,
                StatusCode::OK,
                serde_json::json!({ "found": true, "root_address": addr, "device": device_json(&addr, &d) }),
            ),
            None => stamped(&app, StatusCode::OK, serde_json::json!({ "found": false })),
        };
    }
    // Username queries answer with the same shapes as `x/username`
    // (`proto/hashgram/username/v1/query.proto`): lookup is 200 with
    // `found`, reverse is `registrations[]`, availability carries a reason.
    if let Some(name) = rest.strip_prefix("username/v1/lookup/") {
        let name = name.to_lowercase();
        let owner = app
            .usernames
            .lock()
            .ok()
            .and_then(|u| u.get(&name).cloned());
        let body = match owner {
            Some(o) => serde_json::json!({
                "found": true,
                "registration": { "name": name, "owner": o, "skeleton": name, "registered_height": "1", "expiry_height": "7884000" },
                "normalized": name
            }),
            None => serde_json::json!({
                "found": false,
                "registration": { "name": "", "owner": "", "skeleton": "", "registered_height": "0", "expiry_height": "0" },
                "normalized": name
            }),
        };
        return stamped(&app, StatusCode::OK, body);
    }
    if let Some(addr) = rest.strip_prefix("username/v1/reverse/") {
        let regs: Vec<serde_json::Value> = app
            .usernames
            .lock()
            .ok()
            .map(|u| {
                u.iter()
                    .filter(|(_, o)| o.as_str() == addr)
                    .map(|(n, o)| serde_json::json!({ "name": n, "owner": o, "skeleton": n, "registered_height": "1", "expiry_height": "7884000" }))
                    .collect()
            })
            .unwrap_or_default();
        return stamped(&app, StatusCode::OK, serde_json::json!({ "registrations": regs }));
    }
    if let Some(name) = rest.strip_prefix("username/v1/availability/") {
        let name = name.to_lowercase();
        let taken = app
            .usernames
            .lock()
            .ok()
            .map(|u| u.contains_key(&name))
            .unwrap_or(false);
        let reason = if taken {
            "taken"
        } else if name.len() < 3 {
            "too_short"
        } else if name.len() > 32 {
            "too_long"
        } else if !name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_') {
            "invalid"
        } else {
            ""
        };
        return stamped(
            &app,
            StatusCode::OK,
            serde_json::json!({
                "available": reason.is_empty(),
                "normalized": name,
                "skeleton": name,
                "reason": reason,
                "conflicting_name": if taken { name.clone() } else { String::new() }
            }),
        );
    }
    if rest.starts_with("serviceproof/v1/provider/")
        || rest.starts_with("serviceproof/v1/rewards/")
        || rest.starts_with("serviceproof/v1/assignments/")
    {
        return not_found(&app);
    }
    if rest == "serviceproof/v1/providers" {
        return stamped(
            &app,
            StatusCode::OK,
            serde_json::json!({ "providers": [], "pagination": { "next_key": null, "total": "0" } }),
        );
    }
    // Enough of the Hashgram modules for screens to render "empty" honestly.
    let body = match rest.as_str() {
        "username/v1/params" => {
            serde_json::json!({ "params": { "registration_fee": { "denom": "uhash", "amount": "1000000" }, "validity_blocks": "7884000", "grace_blocks": "648000", "min_length": "3", "max_length": "32" } })
        }
        "serviceproof/v1/epoch/current" => {
            serde_json::json!({ "epoch": { "number": "42", "start_height": "907200", "end_height": "928800" } })
        }
        "network/v1/info" => {
            serde_json::json!({ "info": { "chain_id": app.chain_id, "network_id": "hashgram-devnet" } })
        }
        _ => return not_found(&app),
    };
    stamped(&app, StatusCode::OK, body)
}

async fn broadcast(State(app): S, Json(body): Json<serde_json::Value>) -> Response {
    let Some(tx) = body.get("tx_bytes").and_then(|v| v.as_str()) else {
        return stamped(
            &app,
            StatusCode::BAD_REQUEST,
            serde_json::json!({ "code": 3, "message": "tx_bytes missing" }),
        );
    };
    let hash = hex::encode_upper(Sha256::digest(tx.as_bytes()));
    let h = app.height.load(Ordering::Relaxed) + 1;
    if let Ok(mut txs) = app.txs.lock() {
        txs.push((hash.clone(), h));
    }
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({ "tx_response": { "txhash": hash, "code": 0, "height": "0", "raw_log": "", "gas_used": "0" } }),
    )
}

async fn simulate(State(app): S) -> Response {
    // A relay must never forward this; reaching it means the allow-list
    // failed. Answer anyway so the local API test can prove the denial
    // happened at the node, not here.
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({ "gas_info": { "gas_wanted": "0", "gas_used": "123456" } }),
    )
}

async fn tx_by_hash(State(app): S, Path(hash): Path<String>) -> Response {
    let found = app.txs.lock().ok().and_then(|t| {
        t.iter()
            .find(|(h, _)| h.eq_ignore_ascii_case(&hash))
            .cloned()
    });
    match found {
        Some((h, height)) if app.height.load(Ordering::Relaxed) >= height => stamped(
            &app,
            StatusCode::OK,
            serde_json::json!({ "tx_response": { "txhash": h, "code": 0, "height": height.to_string(), "raw_log": "", "gas_used": "91234" } }),
        ),
        _ => not_found(&app),
    }
}

async fn txs_list(State(app): S) -> Response {
    stamped(
        &app,
        StatusCode::OK,
        serde_json::json!({ "txs": [], "tx_responses": [], "pagination": { "next_key": null, "total": "0" } }),
    )
}

async fn fallback(State(app): S) -> Response {
    not_found(&app)
}

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();
    let cli = Cli::parse();
    if cli.port < 1024 {
        eprintln!("mock-gateway: refusing to bind a privileged port; this is a DEVNET tool");
        std::process::exit(2);
    }
    // Seed the DEVNET registries from the command line; `POST
    // /devnet/identity` can add more while running.
    let mut identities: std::collections::BTreeMap<String, Vec<MockDevice>> = Default::default();
    for (address, hex_key) in parse_pairs(&cli.devices, "device") {
        match hex::decode(&hex_key) {
            Ok(pubkey) if pubkey.len() == 32 => {
                let list = identities.entry(address).or_default();
                let i = list.len();
                list.push(MockDevice {
                    device_id: format!("mock-device-{i}"),
                    device_pubkey: hex::encode(pubkey),
                    label: format!("mock {i}"),
                    platform: "devnet".into(),
                    revoked: false,
                });
            }
            _ => {
                eprintln!("mock-gateway: --device {address}: key is not 32 bytes of hex; ignored");
            }
        }
    }
    let usernames: std::collections::BTreeMap<String, String> = parse_pairs(&cli.usernames, "username")
        .into_iter()
        .map(|(n, a)| (n.to_lowercase(), a))
        .collect();
    let app = Arc::new(App {
        chain_id: cli.chain_id.clone(),
        height: AtomicU64::new(cli.height),
        lie: cli.lie,
        txs: Mutex::new(Vec::new()),
        identities: Mutex::new(identities),
        usernames: Mutex::new(usernames),
    });
    {
        let app = app.clone();
        let secs = cli.block_secs.max(1);
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(secs));
            loop {
                tick.tick().await;
                app.height.fetch_add(1, Ordering::Relaxed);
            }
        });
    }
    let router = Router::new()
        .route("/cosmos/base/tendermint/v1beta1/node_info", get(node_info))
        .route(
            "/cosmos/base/tendermint/v1beta1/blocks/latest",
            get(latest_block),
        )
        .route(
            "/cosmos/bank/v1beta1/balances/{address}/by_denom",
            get(balance_by_denom),
        )
        .route("/cosmos/bank/v1beta1/balances/{address}", get(balances))
        .route("/cosmos/bank/v1beta1/supply/by_denom", get(supply))
        .route("/cosmos/auth/v1beta1/accounts/{address}", get(account))
        .route("/cosmos/tx/v1beta1/txs", get(txs_list).post(broadcast))
        .route("/cosmos/tx/v1beta1/simulate", post(simulate))
        .route("/cosmos/tx/v1beta1/txs/{hash}", get(tx_by_hash))
        .route("/hashgram/founder/v1/params", get(founder_params))
        .route("/devnet/identity", post(devnet_identity))
        .route("/hashgram/{*rest}", get(hashgram_any))
        .fallback(fallback)
        .with_state(app);
    let addr = format!("127.0.0.1:{}", cli.port);
    let listener = match tokio::net::TcpListener::bind(&addr).await {
        Ok(l) => l,
        Err(e) => {
            eprintln!("mock-gateway: cannot bind {addr}: {e}");
            std::process::exit(1);
        }
    };
    tracing::info!(
        addr,
        chain_id = cli.chain_id,
        lie = cli.lie,
        "DEVNET ONLY mock gateway listening"
    );
    if let Err(e) = axum::serve(listener, router).await {
        eprintln!("mock-gateway: {e}");
        std::process::exit(1);
    }
}
