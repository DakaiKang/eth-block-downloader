// src/bin/download_rw.rs
//
// Like main.rs, but fetches prestate with both diffMode:false and diffMode:true
// to distinguish read vs write per storage slot per transaction.
//
// Output JSON adds a "rw_sets" field alongside "prestate":
//   "rw_sets": [
//     { "txHash": "0x...", "reads": [{"address":"0x...","slot":"0x..."}], "writes": [...] },
//     ...
//   ]

use alloy_primitives::keccak256;
use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::{Block, BlockId, BlockTransactionsKind};
use serde_json::{json, Value};
use std::collections::HashSet;
use std::fs;
use std::time::Instant;
use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

#[tokio::main]
async fn main() -> Result<()> {
    let rpc_url = std::env::var("ETHEREUM_RPC_URL")
        .unwrap_or_else(|_| {
            println!("ETHEREUM_RPC_URL not set!");
            std::process::exit(1);
        });

    let blocks_to_download: Vec<u64> = vec![18581726];

    println!("RPC URL: {}", rpc_url);
    println!("Blocks to download: {}", blocks_to_download.len());

    fs::create_dir_all("test_data/blocks_rw")?;

    let provider = ProviderBuilder::new()
        .on_http(rpc_url.parse()?)
        .boxed();

    let multi_progress = MultiProgress::new();
    let overall_pb = multi_progress.add(ProgressBar::new(blocks_to_download.len() as u64));
    overall_pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg}\n{bar:40.cyan/blue} {pos}/{len} blocks [{elapsed_precise}]")
            .unwrap()
            .progress_chars("=>-")
    );
    overall_pb.set_message("Overall Progress");

    let mut total_size = 0u64;
    let mut successful = 0;
    let mut failed = 0;
    let start_time = Instant::now();

    for (idx, block_num) in blocks_to_download.iter().enumerate() {
        println!();
        println!("─────────────────────────────────────────────────────────");
        println!("Block {}/{}: #{}", idx + 1, blocks_to_download.len(), block_num);
        println!("─────────────────────────────────────────────────────────");

        let block_pb = multi_progress.add(ProgressBar::new(4));
        block_pb.set_style(
            ProgressStyle::default_bar()
                .template("  {msg:<30} [{bar:30.green}] {pos}/{len}")
                .unwrap()
                .progress_chars("█▓▒░ ")
        );

        match download_block(&provider, *block_num, &block_pb).await {
            Ok((_filename, size)) => {
                block_pb.finish_with_message(format!("Done ({:.2} MB)", size));
                total_size += (size * 1_000_000.0) as u64;
                successful += 1;
            }
            Err(e) => {
                block_pb.finish_with_message(format!("Failed: {}", e));
                eprintln!("Error details: {:?}", e);
                failed += 1;
            }
        }

        overall_pb.inc(1);
    }

    overall_pb.finish_with_message("All blocks processed");

    let elapsed = start_time.elapsed();
    println!();
    println!("Successful: {}, Failed: {}, Total size: {:.2} MB, Time: {:.1}s",
        successful, failed,
        total_size as f64 / 1_000_000.0,
        elapsed.as_secs_f64());

    Ok(())
}

async fn download_block(
    provider: &impl Provider,
    block_num: u64,
    pb: &ProgressBar,
) -> Result<(String, f64)> {
    // Step 1: block metadata
    pb.set_message("Fetching metadata...");
    pb.set_position(0);

    let block = provider
        .get_block(BlockId::number(block_num), BlockTransactionsKind::Full)
        .await?
        .ok_or_else(|| anyhow::anyhow!("Block not found"))?;

    let block: Block = serde_json::from_value(serde_json::to_value(&block)?)?;
    println!("  Transactions: {}", block.transactions.len());
    pb.inc(1);

    // Step 2: prestate — fetch full access set (diffMode:false) and write set (diffMode:true)
    pb.set_message("Fetching prestate...");
    let t0 = Instant::now();
    let block_hex = format!("0x{:x}", block_num);

    let prestate_full = fetch_prestate(provider, &block_hex, false).await?;
    let prestate_diff = fetch_prestate(provider, &block_hex, true).await?;
    let beneficiary = format!("{:?}", block.header.beneficiary).to_lowercase();
    let rw_sets = compute_rw_sets(&prestate_full, &prestate_diff, &beneficiary);

    println!("  RW sets: {} txs ({:.1}s)", rw_sets.as_array().map(|a| a.len()).unwrap_or(0), t0.elapsed().as_secs_f64());
    pb.inc(1);

    // Step 3: receipts for per-tx gas usage
    pb.set_message("Fetching receipts...");
    let receipts = provider
        .get_block_receipts(BlockId::number(block_num))
        .await?
        .unwrap_or_default();

    // Build txHash -> gasUsed map
    let gas_map: std::collections::HashMap<String, u128> = receipts.iter()
        .map(|r| (format!("{:?}", r.transaction_hash), r.gas_used))
        .collect();

    // Merge gasUsed into each rw_sets entry
    let rw_sets: Value = rw_sets.as_array()
        .map(|arr| {
            arr.iter().map(|entry| {
                let tx_hash = entry.get("txHash").and_then(|h| h.as_str()).unwrap_or("");
                let gas_used = gas_map.get(tx_hash).copied().unwrap_or(0);
                let mut e = entry.clone();
                e["gasUsed"] = json!(gas_used);
                e
            }).collect::<Vec<_>>()
        })
        .map(Value::Array)
        .unwrap_or(Value::Array(vec![]));

    pb.inc(1);

    // Step 4: save
    pb.set_message("Saving...");

    // Build rw_gas file: per-tx hashed key sets
    let rw_gas: Vec<Value> = rw_sets.as_array().unwrap_or(&vec![]).iter().map(|entry| {
        let empty = vec![];
        let reads  = hash_access_keys(entry.get("reads") .and_then(|v| v.as_array()).unwrap_or(&empty));
        let writes = hash_access_keys(entry.get("writes").and_then(|v| v.as_array()).unwrap_or(&empty));
        json!({
            "txHash":  entry.get("txHash").cloned().unwrap_or(Value::Null),
            "gasUsed": entry.get("gasUsed").cloned().unwrap_or(json!(0)),
            "reads":   reads,
            "writes":  writes,
        })
    }).collect();

    fs::create_dir_all("test_data/rw_gas")?;
    let rw_gas_filename = format!("test_data/rw_gas/rw_gas_{}.json", block_num);
    fs::write(&rw_gas_filename, serde_json::to_string_pretty(&Value::Array(rw_gas))?)?;

    let block_data = json!({
        "number": block_num,
        "timestamp": block.header.timestamp,
        "hash": block.header.hash,
        "parentHash": format!("{:?}", block.header.parent_hash),
        "beneficiary": format!("{:?}", block.header.beneficiary),
        "gasLimit": format!("0x{:x}", block.header.gas_limit),
        "difficulty": format!("0x{:x}", block.header.difficulty),
        "baseFeePerGas": block.header.base_fee_per_gas.map(|f| format!("0x{:x}", f)),
        "mixHash": format!("{:?}", block.header.mix_hash),
        "excessBlobGas": block.header.excess_blob_gas.map(|g| format!("0x{:x}", g)),
        "transactions": block.transactions,
        // Full prestate (pre-execution state of all touched accounts/slots)
        "prestate": prestate_full,
        // Per-tx read/write sets derived from prestate tracers
        // reads  = slots accessed but NOT modified
        // writes = slots that changed (present in diffMode:true post-state)
        "rw_sets": rw_sets,
    });

    let filename = format!("test_data/blocks_rw/block_{}.json", block_num);
    let json_string = serde_json::to_string_pretty(&block_data)?;
    fs::write(&filename, &json_string)?;

    let size_mb = json_string.len() as f64 / 1_000_000.0;
    pb.inc(1);

    Ok((filename, size_mb))
}

/// Fetch prestate for all txs in the block.
///
/// diffMode=false → per-tx pre-state of every accessed slot (reads + writes)
/// diffMode=true  → per-tx {pre, post} of only modified slots (writes)
///
/// Response is an array, one entry per tx, each entry may be wrapped as
/// {"txHash": "0x...", "result": <data>} depending on the provider.
async fn fetch_prestate(
    provider: &impl Provider,
    block_hex: &str,
    diff_mode: bool,
) -> Result<Value> {
    let tracer_opts = json!({
        "tracer": "prestateTracer",
        "tracerConfig": { "diffMode": diff_mode }
    });

    for method in ["debug_traceBlockByNumber", "debug_traceBlock"] {
        let result: Result<Value, _> = provider
            .raw_request(method.into(), vec![json!(block_hex), tracer_opts.clone()])
            .await;
        if let Ok(v) = result {
            return Ok(v);
        }
    }

    Err(anyhow::anyhow!("debug prestate API unavailable (need Alchemy Growth or equivalent)"))
}

/// Compute per-tx read/write slot sets from two prestate responses.
///
/// Entry format expected from provider (Alchemy / geth):
///   diffMode=false array element: {"txHash":"0x...", "result": {addr: {storage: {slot: val}}}}
///   diffMode=true  array element: {"txHash":"0x...", "result": {"pre": {...}, "post": {addr: {storage: {slot: val}}}}}
///
/// write set = slots present in post-state  (these were modified)
/// read  set = slots in full access set that are NOT in write set
fn compute_rw_sets(full: &Value, diff: &Value, beneficiary: &str) -> Value {
    let empty = vec![];
    let full_txs = full.as_array().unwrap_or(&empty);
    let diff_txs = diff.as_array().unwrap_or(&empty);

    full_txs.iter().zip(diff_txs.iter()).map(|(full_tx, diff_tx)| {
        let tx_hash = full_tx.get("txHash")
            .or_else(|| diff_tx.get("txHash"))
            .cloned()
            .unwrap_or(Value::Null);

        // Unwrap the "result" wrapper if present
        let full_state = full_tx.get("result").unwrap_or(full_tx);
        let diff_result = diff_tx.get("result").unwrap_or(diff_tx);
        let post_state = diff_result.get("post").unwrap_or(&Value::Null);

        // Build write set from post-state: storage slots + balance
        // We use slot="balance" as a sentinel for the account balance field.
        let mut write_set: HashSet<(String, String)> = HashSet::new();
        let mut writes: Vec<Value> = Vec::new();

        if let Some(addrs) = post_state.as_object() {
            for (addr, acct) in addrs {
                if acct.get("balance").is_some() && addr.to_lowercase() != beneficiary {
                    write_set.insert((addr.clone(), "balance".to_string()));
                    writes.push(json!({"address": addr, "slot": "balance"}));
                }
                if let Some(slots) = acct.get("storage").and_then(|s| s.as_object()) {
                    for slot in slots.keys() {
                        write_set.insert((addr.clone(), slot.clone()));
                        writes.push(json!({"address": addr, "slot": slot}));
                    }
                }
            }
        }

        // Build read set: full access set minus write set, covering storage slots + balance
        let mut reads: Vec<Value> = Vec::new();

        if let Some(addrs) = full_state.as_object() {
            for (addr, acct) in addrs {
                if acct.get("balance").is_some()
                    && addr.to_lowercase() != beneficiary
                    && !write_set.contains(&(addr.clone(), "balance".to_string()))
                {
                    reads.push(json!({"address": addr, "slot": "balance"}));
                }
                if let Some(slots) = acct.get("storage").and_then(|s| s.as_object()) {
                    for slot in slots.keys() {
                        if !write_set.contains(&(addr.clone(), slot.clone())) {
                            reads.push(json!({"address": addr, "slot": slot}));
                        }
                    }
                }
            }
        }

        json!({
            "txHash": tx_hash,
            "reads":  reads,
            "writes": writes,
        })
    }).collect()
}

/// Hash each (address, slot) pair into a single 32-byte key:
///   storage slot  → keccak256(addr_20bytes || slot_32bytes)
///   balance       → keccak256(addr_20bytes)
///
/// Returns a sorted, deduplicated array of "0x..."-prefixed hex strings.
fn hash_access_keys(entries: &[Value]) -> Value {
    let mut keys: Vec<String> = entries.iter().filter_map(|entry| {
        let addr_hex = entry.get("address").and_then(|v| v.as_str()).unwrap_or("");
        let slot_str = entry.get("slot")   .and_then(|v| v.as_str()).unwrap_or("");

        // Strip "0x" prefix and decode address (20 bytes)
        let addr_bytes = hex::decode(addr_hex.trim_start_matches("0x")).ok()?;
        if addr_bytes.len() != 20 { return None; }

        let hash = if slot_str == "balance" {
            // balance: hash just the address
            keccak256(&addr_bytes)
        } else {
            // storage slot: hash address || slot (32 bytes)
            let slot_bytes = hex::decode(slot_str.trim_start_matches("0x")).ok()?;
            if slot_bytes.len() != 32 { return None; }
            let mut buf = [0u8; 52];
            buf[..20].copy_from_slice(&addr_bytes);
            buf[20..].copy_from_slice(&slot_bytes);
            keccak256(&buf)
        };

        Some(format!("0x{}", hex::encode(hash.as_slice())))
    }).collect();

    keys.sort();
    keys.dedup();
    Value::Array(keys.into_iter().map(Value::String).collect())
}
