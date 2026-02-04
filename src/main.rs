// src/main.rs

use alloy_provider::{Provider, ProviderBuilder};
use alloy_rpc_types_eth::{Block, BlockId, BlockTransactionsKind};
use serde_json::{json, Value};
use std::fs;
use std::time::Instant;
use anyhow::Result;
use indicatif::{MultiProgress, ProgressBar, ProgressStyle};

#[tokio::main]
async fn main() -> Result<()> {
    // Configuration
    let rpc_url = std::env::var("ETHEREUM_RPC_URL")
        .unwrap_or_else(|_| {
            println!("⚠️  ETHEREUM_RPC_URL not set!");
            println!("Please set your Alchemy URL:");
            println!("export ETHEREUM_RPC_URL=\"https://eth-mainnet.g.alchemy.com/v2/YOUR_API_KEY\"");
            std::process::exit(1);
        });
    
    // Blocks to download
    let blocks_to_download = vec![
        46147,    // FRONTIER - simple block
        1150000,  // HOMESTEAD
        4370000,  // BYZANTIUM
        12965000, // LONDON - EIP-1559
        15537393, // MERGE - PoS
        19426587, // CANCUN - latest
    ];
    
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║       Ethereum Block Downloader (Alchemy + debug API)         ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();
    println!("RPC URL: {}", rpc_url);
    println!("Blocks to download: {}", blocks_to_download.len());
    println!();
    
    // Create output directory
    fs::create_dir_all("test_data/blocks")?;
    
    // Create provider
    let provider = ProviderBuilder::new()
        .on_http(rpc_url.parse()?)
        .boxed();
    
    // Test if debug API is available
    println!("Testing debug API availability...");
    match test_debug_api(&provider).await {
        Ok(true) => println!("✅ debug_traceBlockByNumber is available\n"),
        Ok(false) => {
            println!("❌ debug_traceBlockByNumber is NOT available");
            println!("\n📝 Alchemy Debug API requires:");
            println!("   - Growth plan ($49/month) or higher");
            println!("   - Or use QuickNode (free trial with debug API)");
            println!("\nAlternative: Use synthetic workloads for testing");
            return Ok(());
        }
        Err(e) => {
            println!("❌ Error testing debug API: {}", e);
            return Ok(());
        }
    }
    
    // Create progress bars
    let multi_progress = MultiProgress::new();
    let overall_pb = multi_progress.add(ProgressBar::new(blocks_to_download.len() as u64));
    overall_pb.set_style(
        ProgressStyle::default_bar()
            .template("{msg}\n{bar:40.cyan/blue} {pos}/{len} blocks [{elapsed_precise}]")
            .unwrap()
            .progress_chars("=>-")
    );
    overall_pb.set_message("Overall Progress");
    
    // Statistics
    let mut total_size = 0u64;
    let mut successful = 0;
    let mut failed = 0;
    let start_time = Instant::now();
    
    // Download each block
    for (idx, block_num) in blocks_to_download.iter().enumerate() {
        println!();
        println!("─────────────────────────────────────────────────────────");
        println!("Block {}/{}: #{}", idx + 1, blocks_to_download.len(), block_num);
        println!("─────────────────────────────────────────────────────────");
        
        // Create progress bar for current block
        let block_pb = multi_progress.add(ProgressBar::new(3));
        block_pb.set_style(
            ProgressStyle::default_bar()
                .template("  {msg:<30} [{bar:30.green}] {pos}/{len}")
                .unwrap()
                .progress_chars("█▓▒░ ")
        );
        
        match download_block_with_debug_trace(&provider, *block_num, &block_pb).await {
            Ok((filename, size)) => {
                block_pb.finish_with_message(format!("✓ Done ({:.2} MB)", size));
                total_size += (size * 1_000_000.0) as u64;
                successful += 1;
            }
            Err(e) => {
                block_pb.finish_with_message(format!("✗ Failed: {}", e));
                eprintln!("Error details: {:?}", e);
                failed += 1;
            }
        }
        
        overall_pb.inc(1);
    }
    
    overall_pb.finish_with_message("All blocks processed");
    
    // Final summary
    let elapsed = start_time.elapsed();
    println!();
    println!("╔════════════════════════════════════════════════════════════════╗");
    println!("║                      Download Summary                          ║");
    println!("╠════════════════════════════════════════════════════════════════╣");
    println!("║ Total blocks:      {:>3}                                       ║", blocks_to_download.len());
    println!("║ Successful:        {:>3}                                       ║", successful);
    println!("║ Failed:            {:>3}                                       ║", failed);
    println!("║ Total size:        {:>10.2} MB                              ║", total_size as f64 / 1_000_000.0);
    println!("║ Total time:        {:>3.1} seconds                            ║", elapsed.as_secs_f64());
    
    if successful > 0 {
        println!("║ Avg size/block:    {:>10.2} MB                              ║", 
                 (total_size as f64 / 1_000_000.0) / successful as f64);
        println!("║ Avg time/block:    {:>10.1} seconds                         ║", 
                 elapsed.as_secs_f64() / successful as f64);
    }
    
    println!("╠════════════════════════════════════════════════════════════════╣");
    println!("║ Output directory:  test_data/blocks/                          ║");
    println!("╚════════════════════════════════════════════════════════════════╝");
    println!();
    
    // Display file list
    if successful > 0 {
        println!("Downloaded files:");
        let mut entries: Vec<_> = fs::read_dir("test_data/blocks")?
            .filter_map(|e| e.ok())
            .collect();
        entries.sort_by_key(|e| e.file_name());
        
        for entry in entries {
            let metadata = entry.metadata()?;
            let size_mb = metadata.len() as f64 / 1_000_000.0;
            println!("  {} ({:.2} MB)", entry.file_name().to_string_lossy(), size_mb);
        }
    }
    
    println!();
    if successful > 0 {
        println!("✓ Done! You can now use these blocks for testing.");
    }
    if failed > 0 {
        println!("⚠ {} block(s) failed to download. Check the errors above.", failed);
    }
    
    Ok(())
}

/// Test if debug API is available
async fn test_debug_api(provider: &impl Provider) -> Result<bool> {
    // Try multiple debug API methods
    let methods = vec![
        ("debug_traceBlockByNumber", vec![json!("0xb443"), json!({"tracer": "prestateTracer"})]),
        ("debug_traceBlock", vec![json!("0xb443"), json!({"tracer": "prestateTracer"})]),
    ];
    
    for (method, params) in methods {
        let result: Result<Value, _> = provider  // ← 添加类型标注
            .raw_request(method.into(), params)
            .await;
        
        match result {
            Ok(_) => {
                println!("✅ Using method: {}", method);
                return Ok(true);
            }
            Err(e) => {
                let err_str = format!("{:?}", e);
                if !err_str.contains("Unsupported method") && !err_str.contains("Method not found") {
                    continue;
                }
            }
        }
    }
    
    Ok(false)
}

/// Download block using debug API
async fn download_block_with_debug_trace(
    provider: &impl Provider,
    block_num: u64,
    pb: &ProgressBar,
) -> Result<(String, f64)> {
    // Step 1: Get block metadata
    pb.set_message("Fetching metadata...");
    pb.set_position(0);
    
    let block = provider
        .get_block(
            BlockId::number(block_num),
            BlockTransactionsKind::Full
        )
        .await?
        .ok_or_else(|| anyhow::anyhow!("Block not found"))?;
    
    // Convert to concrete type
    let block: Block = serde_json::from_value(serde_json::to_value(&block)?)?;
    
    let tx_count = block.transactions.len();
    println!("  Transactions: {}", tx_count);
    
    pb.inc(1);
    
    // Step 2: Get prestate
    pb.set_message("Fetching prestate...");
    
    let prestate_start = Instant::now();
    let block_hex = format!("0x{:x}", block_num);
    let prestate = try_get_prestate(provider, &block_hex).await?;
    
    let prestate_time = prestate_start.elapsed();
    let account_count = prestate.as_object().map(|o| o.len()).unwrap_or(0);
    
    println!("  Accounts: {} (fetched in {:.1}s)", account_count, prestate_time.as_secs_f64());
    
    pb.inc(1);
    
    // Step 3: Save file with ALL block environment fields
    pb.set_message("Saving to disk...");
    
    let block_data = json!({
        "number": block_num,
        "timestamp": block.header.timestamp,
        "hash": block.header.hash,
        
        // Add block environment fields ← 添加这些！
        "beneficiary": format!("{:?}", block.header.beneficiary),
        "gasLimit": format!("0x{:x}", block.header.gas_limit),
        "difficulty": format!("0x{:x}", block.header.difficulty),
        "baseFeePerGas": block.header.base_fee_per_gas
            .map(|fee| format!("0x{:x}", fee)),
        
        "transactions": block.transactions,
        "prestate": prestate,
    });
    
    let filename = format!("test_data/blocks/block_{}.json", block_num);
    let json_string = serde_json::to_string_pretty(&block_data)?;
    fs::write(&filename, &json_string)?;
    
    let size_mb = json_string.len() as f64 / 1_000_000.0;
    
    pb.inc(1);
    
    Ok((filename, size_mb))
}

/// Try different debug API methods to get prestate
async fn try_get_prestate(
    provider: &impl Provider,
    block_hex: &str,
) -> Result<Value> {
    // Method 1: debug_traceBlockByNumber with prestateTracer
    let result: Result<Value, _> = provider  // ← 添加类型标注
        .raw_request(
            "debug_traceBlockByNumber".into(),
            vec![
                json!(block_hex),
                json!({
                    "tracer": "prestateTracer",
                    "tracerConfig": {
                        "diffMode": false
                    }
                })
            ]
        )
        .await;
    
    if let Ok(prestate) = result {
        return Ok(prestate);
    }
    
    // Method 2: debug_traceBlock
    let result: Result<Value, _> = provider  // ← 添加类型标注
        .raw_request(
            "debug_traceBlock".into(),
            vec![
                json!(block_hex),
                json!({
                    "tracer": "prestateTracer",
                    "tracerConfig": {
                        "diffMode": false
                    }
                })
            ]
        )
        .await;
    
    if let Ok(prestate) = result {
        return Ok(prestate);
    }
    
    // Method 3: debug_traceBlockByNumber with different params
    let result: Result<Value, _> = provider  // ← 添加类型标注
        .raw_request(
            "debug_traceBlockByNumber".into(),
            vec![
                json!(block_hex),
                json!({"tracer": "prestateTracer"})
            ]
        )
        .await;
    
    if let Ok(prestate) = result {
        return Ok(prestate);
    }
    
    Err(anyhow::anyhow!("All debug API methods failed"))
}

