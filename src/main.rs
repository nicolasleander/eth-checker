use std::env;
use std::error::Error;
use std::io::{self, Write};
use std::sync::Arc;
use std::time::{Duration, Instant};

use futures::stream::{self, StreamExt};
use tokio::sync::Mutex;

use bip39::{Language, Mnemonic};
use chrono::Local;
use clap::Parser;
use colored::*;
use dotenv::dotenv;
use ethers::{
    prelude::*,
    signers::{coins_bip39::English, MnemonicBuilder},
    utils::format_ether,
};
use rand::{thread_rng, RngCore};
use rusqlite::{params, Connection};
use thousands::Separable;

#[derive(Parser, Debug)]
#[command(author, version, about, long_about = None)]
struct Args {
    /// Use predefined list instead of generating
    #[arg(short, long)]
    predefined: bool,

    /// Number of mnemonics to check
    #[arg(short, long, default_value_t = 30)]
    number: usize,

    /// Use local Geth node instead of Infura
    #[arg(short, long)]
    local: bool,

    /// Number of concurrent tasks
    #[arg(short = 't', long, default_value_t = num_cpus::get())]
    threads: usize,
}

#[derive(Debug)]
enum NodeType {
    Infura(String), // project_id
    Local,
}

#[derive(Debug)]
struct Config {
    node: NodeType,
    network: String,
}

pub struct ProgressTracker {
    start_time: Instant,
    total_items: usize,
    current_item: usize,
    last_update: Instant,
    update_interval: Duration,
    addresses_checked: usize,
    speed_ewma: f64, // Only keep EWMA for speed tracking
    last_check_time: Instant,
}

impl ProgressTracker {
    pub fn new(total_items: usize) -> Self {
        Self {
            start_time: Instant::now(),
            total_items,
            current_item: 0,
            last_update: Instant::now(),
            update_interval: Duration::from_millis(100),
            addresses_checked: 0,
            speed_ewma: 0.0,
            last_check_time: Instant::now(),
        }
    }

    pub fn update(&mut self, addresses_checked: usize) -> io::Result<()> {
        let now = Instant::now();
        let duration_since_last = now.duration_since(self.last_check_time).as_secs_f64();

        self.current_item += 1;
        self.addresses_checked = addresses_checked;

        // Update speed calculations
        if duration_since_last > 0.0 {
            let current_speed =
                self.addresses_checked as f64 / self.start_time.elapsed().as_secs_f64();
            const ALPHA: f64 = 0.1;
            self.speed_ewma = if self.speed_ewma == 0.0 {
                current_speed
            } else {
                self.speed_ewma * (1.0 - ALPHA) + current_speed * ALPHA
            };
        }

        self.last_check_time = now;

        if now.duration_since(self.last_update) >= self.update_interval {
            self.print_progress()?;
            self.last_update = now;
        }
        Ok(())
    }

    fn format_duration(duration: Duration) -> String {
        let hours = duration.as_secs() / 3600;
        let minutes = (duration.as_secs() % 3600) / 60;
        let seconds = duration.as_secs() % 60;
        format!("{:02}:{:02}:{:02}", hours, minutes, seconds)
    }

    fn format_speed(speed: f64) -> String {
        if speed >= 1000.0 {
            format!("{:.1}k/s", speed / 1000.0)
        } else {
            format!("{:.0}/s", speed)
        }
    }

    fn print_progress(&self) -> io::Result<()> {
        let elapsed = self.start_time.elapsed();
        let current_speed = self.addresses_checked as f64 / elapsed.as_secs_f64();

        // Calculate speed trend indicator using EWMA
        let speed_indicator = if current_speed > self.speed_ewma {
            "▲".green()
        } else if current_speed < self.speed_ewma {
            "▼".red()
        } else {
            "=".white()
        };

        // Calculate ETAs and percentages
        let progress_pct = (self.current_item as f64 / self.total_items as f64 * 100.0) as usize;
        let remaining_items = self.total_items - self.current_item;
        let eta = if self.speed_ewma > 0.0 {
            Duration::from_secs_f64(remaining_items as f64 / self.speed_ewma)
        } else {
            Duration::from_secs(0)
        };

        // Progress bar
        let bar_width = 30;
        let filled = (progress_pct as f64 * bar_width as f64 / 100.0) as usize;
        let progress_bar = format!(
            "{}{}",
            "█".repeat(filled).bright_green(),
            "▒".repeat(bar_width - filled).dimmed()
        );

        // Status indicators
        let status = if progress_pct >= 100 {
            "DONE".green()
        } else {
            "RUNNING".blue()
        };

        // Clear line and print progress
        print!("\r\x1B[K"); // Clear the current line
        print!(
            "{} {} │ {} {:>3}% │ {} {} │ {:>8} │ E: {} │ R: {} │ C: {}",
            "[STATUS]".blue(),
            status,
            progress_bar,
            progress_pct,
            Self::format_speed(current_speed),
            speed_indicator,
            format!("{}/{}", self.current_item, self.total_items).yellow(),
            Self::format_duration(elapsed).cyan(),
            Self::format_duration(eta).purple(),
            self.addresses_checked.separate_with_commas().bright_white(),
        );

        io::stdout().flush()
    }

    pub fn finish(&self) -> io::Result<()> {
        let elapsed = self.start_time.elapsed();
        let avg_speed = self.addresses_checked as f64 / elapsed.as_secs_f64();

        println!("\n\n{}", "Final Statistics:".bright_yellow());
        println!("{}", "=".repeat(50).dimmed());
        println!(
            "{}: {}",
            "Total Runtime".bright_blue(),
            Self::format_duration(elapsed).bright_white()
        );
        println!(
            "{}: {}",
            "Average Speed".bright_blue(),
            format!("{}/s", (avg_speed as usize).separate_with_commas()).bright_white()
        );
        println!(
            "{}: {}",
            "Total Checked".bright_blue(),
            self.addresses_checked.separate_with_commas().bright_white()
        );
        println!("{}", "=".repeat(50).dimmed());
        Ok(())
    }
}

impl Config {
    fn from_env(use_local: bool) -> Result<Self, Box<dyn Error>> {
        dotenv().ok();

        let network = env::var("NETWORK").unwrap_or_else(|_| "mainnet".to_string());

        let node = if use_local {
            NodeType::Local
        } else {
            let project_id = env::var("INFURA_PROJECT_ID").map_err(|_| {
                "INFURA_PROJECT_ID is required in .env file when not using local node"
            })?;
            NodeType::Infura(project_id)
        };

        Ok(Config { node, network })
    }

    fn get_provider_url(&self) -> String {
        match &self.node {
            NodeType::Infura(project_id) => {
                format!("https://{}.infura.io/v3/{}", self.network, project_id)
            }
            NodeType::Local => {
                // Default local Geth RPC endpoint
                "http://127.0.0.1:8545".to_string()
            }
        }
    }
}

const PREDEFINED_MNEMONICS: &[&str] = &[
    "abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon abandon about",
    // ... [your other predefined mnemonics] ...
];

fn generate_mnemonic() -> Result<String, Box<dyn Error>> {
    let mut entropy = [0u8; 16];
    thread_rng().fill_bytes(&mut entropy);
    let mnemonic = Mnemonic::from_entropy_in(Language::English, &entropy)?;
    Ok(mnemonic.to_string())
}

fn get_mnemonics(args: &Args) -> Result<Vec<String>, Box<dyn Error>> {
    if args.predefined {
        Ok(PREDEFINED_MNEMONICS
            .iter()
            .map(|&s| s.to_string())
            .collect())
    } else {
        let mut mnemonics = Vec::new();
        for _ in 0..args.number {
            mnemonics.push(generate_mnemonic()?);
        }
        Ok(mnemonics)
    }
}

const BIP44_PATH: &str = "m/44'/60'/0'/0/0";

fn setup_database() -> Result<Connection, Box<dyn Error>> {
    let conn = Connection::open("eth_checker.db")?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS scans (
            id INTEGER PRIMARY KEY,
            start_time TEXT NOT NULL,
            end_time TEXT,
            total_checked INTEGER NOT NULL,
            total_found INTEGER NOT NULL,
            generation_type TEXT NOT NULL,
            node_type TEXT NOT NULL
        )",
        [],
    )?;

    conn.execute(
        "CREATE TABLE IF NOT EXISTS checks (
            id INTEGER PRIMARY KEY,
            scan_id INTEGER NOT NULL,
            mnemonic TEXT NOT NULL,
            address TEXT NOT NULL,
            private_key TEXT NOT NULL,
            balance REAL NOT NULL,
            execution_time_ms INTEGER NOT NULL,
            checked_at TEXT NOT NULL,
            success BOOLEAN NOT NULL,
            error_message TEXT,
            FOREIGN KEY(scan_id) REFERENCES scans(id)
        )",
        [],
    )?;

    Ok(conn)
}

async fn generate_address_from_mnemonic(
    mnemonic: &str,
) -> Result<(Address, String), Box<dyn Error>> {
    // Validate mnemonic first
    let _ = Mnemonic::parse_in_normalized(Language::English, mnemonic)?;

    let wallet = MnemonicBuilder::<English>::default()
        .phrase(mnemonic)
        .derivation_path(BIP44_PATH)?
        .build()?;

    let private_key = hex::encode(wallet.signer().to_bytes());
    Ok((wallet.address(), private_key))
}

async fn check_balance(
    provider: &Provider<Http>,
    address: Address,
) -> Result<U256, Box<dyn Error>> {
    Ok(provider.get_balance(address, None).await?)
}

async fn check_addresses(args: Args) -> Result<(), Box<dyn Error>> {
    let config = Config::from_env(args.local)?;
    let provider_url = config.get_provider_url();
    let provider = Provider::<Http>::try_from(provider_url)?;
    let provider = Arc::new(provider);
    let conn = Arc::new(Mutex::new(setup_database()?));

    let mnemonics = get_mnemonics(&args)?;
    let generation_type = if args.predefined {
        "predefined"
    } else {
        "generated"
    };
    let node_type = if args.local { "local" } else { "infura" };

    println!("\n[+] Starting ETH balance checker");
    println!("[+] Configuration:");
    println!("    Network: {}", config.network);
    println!("    Node Type: {}", node_type);
    println!("    Path: {} (BIP44)", BIP44_PATH);
    println!("    Mode: {}", generation_type);
    println!("    Mnemonics to check: {}", mnemonics.len());
    println!("    Concurrent tasks: {}\n", args.threads);

    let start_time = Local::now().to_string();
    conn.lock().await.execute(
        "INSERT INTO scans (start_time, total_checked, total_found, generation_type, node_type) 
         VALUES (?1, 0, 0, ?2, ?3)",
        params![start_time, generation_type, node_type],
    )?;
    let scan_id = conn.lock().await.last_insert_rowid();

    let progress = Arc::new(Mutex::new(ProgressTracker::new(mnemonics.len())));
    let check_count = Arc::new(Mutex::new(0));
    let found_count = Arc::new(Mutex::new(0));

    // Process mnemonics in parallel
    stream::iter(mnemonics)
        .map(|mnemonic| {
            let provider = Arc::clone(&provider);
            let conn = Arc::clone(&conn);
            let progress = Arc::clone(&progress);
            let check_count = Arc::clone(&check_count);
            let found_count = Arc::clone(&found_count);
            let scan_id = scan_id;

            async move {
                let check_start = Instant::now();
                let check_time = Local::now().to_string();

                let result = match generate_address_from_mnemonic(&mnemonic).await {
                    Ok((address, private_key)) => match check_balance(&provider, address).await {
                        Ok(balance) => {
                            let execution_time = check_start.elapsed().as_millis() as i64;
                            let balance_eth = format_ether(balance).parse::<f64>().unwrap_or(0.0);

                            if balance_eth > 0.0 {
                                let mut found = found_count.lock().await;
                                *found += 1;
                                println!("\n[!] Found balance!");
                                println!("    Mnemonic: {}", mnemonic);
                                println!("    Address: {}", address);
                                println!("    Private Key: 0x{}", private_key);
                                println!("    Balance: {} ETH", balance_eth);
                                println!("    Check time: {}ms\n", execution_time);
                            }

                            conn.lock().await.execute(
                                "INSERT INTO checks (
                                    scan_id, mnemonic, address, private_key, balance,
                                    execution_time_ms, checked_at, success, error_message
                                ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                                params![
                                    scan_id,
                                    mnemonic,
                                    address.to_string(),
                                    private_key,
                                    balance_eth,
                                    execution_time,
                                    check_time,
                                    true,
                                    Option::<String>::None
                                ],
                            )?;
                            Ok(())
                        }
                        Err(e) => Err(format!("Balance check error: {}", e)),
                    },
                    Err(e) => Err(format!("Invalid mnemonic: {}", e)),
                };

                // Update progress
                let mut count = check_count.lock().await;
                *count += 1;
                progress.lock().await.update(*count)?;

                if let Err(error_msg) = result {
                    conn.lock().await.execute(
                        "INSERT INTO checks (
                            scan_id, mnemonic, address, private_key, balance,
                            execution_time_ms, checked_at, success, error_message
                        ) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                        params![scan_id, mnemonic, "", "", 0.0, 0, check_time, false, error_msg],
                    )?;
                }

                conn.lock().await.execute(
                    "UPDATE scans SET total_checked = ?1, total_found = ?2 WHERE id = ?3",
                    params![*count as i64, *found_count.lock().await as i64, scan_id],
                )?;

                Ok::<_, Box<dyn Error>>(())
            }
        })
        .buffer_unordered(args.threads) // Process in parallel with specified number of threads
        .collect::<Vec<_>>()
        .await;

    progress.lock().await.finish()?;

    let final_count = *check_count.lock().await;
    let final_found = *found_count.lock().await;

    conn.lock().await.execute(
        "UPDATE scans SET 
            end_time = ?1,
            total_checked = ?2,
            total_found = ?3
        WHERE id = ?4",
        params![
            Local::now().to_string(),
            final_count as i64,
            final_found as i64,
            scan_id
        ],
    )?;

    println!("\n[+] Scan complete!");
    println!("[+] Total mnemonics checked: {}", final_count);
    println!("[+] Total addresses with balance: {}", final_found);
    println!("[+] Results saved in eth_checker.db");

    Ok(())
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn Error>> {
    let args = Args::parse();
    check_addresses(args).await
}
