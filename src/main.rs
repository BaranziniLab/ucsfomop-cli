use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use rand::Rng;
use regex::Regex;
use std::io::{self, Write};
use tiberius::{AuthMethod, Client, Config};
use tokio::net::TcpStream;
use tokio_util::compat::TokioAsyncWriteCompatExt;

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// ucsfomop-cli — Query the UCSF OMOP de-identified EHR database
///
/// Credentials are loaded from a .env file (CLINICAL_RECORDS_SERVER,
/// CLINICAL_RECORDS_DATABASE, CLINICAL_RECORDS_USERNAME,
/// CLINICAL_RECORDS_PASSWORD) or the corresponding environment variables.
#[derive(Parser, Debug)]
#[command(
    name = "ucsfomop",
    version = "0.1.0",
    author,
    about = "UCSF OMOP clinical database CLI",
    long_about = "\
ucsfomop-cli — UCSF OMOP Clinical Database CLI
===============================================

Connects to the UCSF OMOP de-identified EHR (OMOP_DEID) via SQL Server
using credentials stored in a .env file or environment variables.

ENVIRONMENT VARIABLES (loaded from .env automatically):
  CLINICAL_RECORDS_SERVER    SQL Server hostname
  CLINICAL_RECORDS_DATABASE  Database name
  CLINICAL_RECORDS_USERNAME  Login (supports DOMAIN\\user syntax)
  CLINICAL_RECORDS_PASSWORD  Password

COMMANDS:
  test-connection        Run a lightweight query to verify connectivity.

  list-clinical-tables   Print every table with its schema to stdout (JSON).

  query <SQL>            Execute a read-only SELECT query.
                           --output <file>   Save results to <file>.csv
                                             (default: <random8chars>.csv)
                           --stdio           Print CSV to stdout instead of
                                             saving a file.

EXAMPLES:
  ucsfomop test-connection
  ucsfomop list-clinical-tables
  ucsfomop query \"SELECT TOP 5 * FROM person\" --output person_sample
  ucsfomop query \"SELECT TOP 5 * FROM person\" --stdio

SAFETY:
  Write operations (INSERT, UPDATE, DELETE, DROP, ALTER, EXEC, …) are
  blocked at the client level and will be rejected before any network
  traffic is sent.
"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand, Debug)]
enum Commands {
    /// Run a lightweight query to verify the database connection
    TestConnection,

    /// List all base tables in the database with schema and table name
    ListClinicalTables,

    /// Execute a read-only SQL SELECT query and return results as CSV
    Query {
        /// SQL SELECT statement to execute (must be read-only)
        sql: String,

        /// Output filename without extension (default: random 8-char hash)
        #[arg(short, long)]
        output: Option<String>,

        /// Print CSV to stdout instead of saving to a file
        #[arg(long)]
        stdio: bool,
    },
}

// ---------------------------------------------------------------------------
// Query safety
// ---------------------------------------------------------------------------

fn is_write_query(query: &str) -> bool {
    let re = Regex::new(
        r"(?i)\b(MERGE|CREATE|SET|DELETE|REMOVE|ADD|INSERT|UPDATE|DROP|ALTER|TRUNCATE|GRANT|REVOKE|EXEC|EXECUTE|SP_)\b"
    ).unwrap();
    re.is_match(query)
}

fn validate_read_only(query: &str) -> Result<()> {
    let trimmed = query.trim().to_uppercase();
    let allowed = ["SELECT", "WITH", "DECLARE"];
    if !allowed.iter().any(|s| trimmed.starts_with(s)) {
        return Err(anyhow!(
            "Query rejected: only SELECT / WITH / DECLARE statements are permitted."
        ));
    }
    if is_write_query(query) {
        return Err(anyhow!(
            "Query rejected: write operations are not allowed (INSERT, UPDATE, DELETE, DROP, …)."
        ));
    }
    // Block stacked statements
    let stacked = Regex::new(r";\s*\w+").unwrap();
    if stacked.is_match(&trimmed) {
        return Err(anyhow!(
            "Query rejected: stacked statements (semicolon followed by another statement) are not allowed."
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Database connection
// ---------------------------------------------------------------------------

struct DbConfig {
    server: String,
    database: String,
    username: String,
    password: String,
}

fn load_db_config() -> Result<DbConfig> {
    // Load .env if present (silently ignore if missing)
    let _ = dotenvy::dotenv();

    let server = std::env::var("CLINICAL_RECORDS_SERVER")
        .context("CLINICAL_RECORDS_SERVER not set")?;
    let database = std::env::var("CLINICAL_RECORDS_DATABASE")
        .context("CLINICAL_RECORDS_DATABASE not set")?;
    let username = std::env::var("CLINICAL_RECORDS_USERNAME")
        .context("CLINICAL_RECORDS_USERNAME not set")?;
    let password = std::env::var("CLINICAL_RECORDS_PASSWORD")
        .context("CLINICAL_RECORDS_PASSWORD not set")?;

    Ok(DbConfig { server, database, username, password })
}

async fn connect(cfg: &DbConfig) -> Result<Client<tokio_util::compat::Compat<TcpStream>>> {
    let mut config = Config::new();
    config.host(&cfg.server);
    config.port(1433);
    config.database(&cfg.database);

    // Pass username as-is (DOMAIN\user format is accepted by sql_server auth on this server)
    config.authentication(AuthMethod::sql_server(&cfg.username, &cfg.password));

    // Use rustls (no native TLS dependency required on macOS)
    config.encryption(tiberius::EncryptionLevel::Required);
    config.trust_cert();

    let tcp = TcpStream::connect(config.get_addr())
        .await
        .context("TCP connection failed")?;
    tcp.set_nodelay(true)?;

    let client = Client::connect(config, tcp.compat_write())
        .await
        .context("TDS handshake failed")?;
    Ok(client)
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

async fn cmd_test_connection(cfg: &DbConfig) -> Result<()> {
    println!("Connecting to {}\\{}...", cfg.server, cfg.database);
    let mut client = connect(cfg).await?;
    let row = client
        .query("SELECT GETDATE() AS server_time", &[])
        .await?
        .into_row()
        .await?
        .ok_or_else(|| anyhow!("No row returned"))?;

    let ts: &str = row.get(0).unwrap_or("(null)");
    println!("Connection successful. Server time: {ts}");
    Ok(())
}

async fn cmd_list_tables(cfg: &DbConfig) -> Result<()> {
    let mut client = connect(cfg).await?;
    let sql = "\
        SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE \
        FROM INFORMATION_SCHEMA.TABLES \
        WHERE TABLE_TYPE = 'BASE TABLE' \
        ORDER BY TABLE_SCHEMA, TABLE_NAME";

    let rows = client.query(sql, &[]).await?.into_first_result().await?;

    // Print as a simple aligned table to stdout
    println!("{:<30} {:<50} {}", "SCHEMA", "TABLE_NAME", "TYPE");
    println!("{}", "-".repeat(90));
    for row in rows {
        let schema: &str = row.get(0).unwrap_or("");
        let name: &str = row.get(1).unwrap_or("");
        let ttype: &str = row.get(2).unwrap_or("");
        println!("{:<30} {:<50} {}", schema, name, ttype);
    }
    Ok(())
}

async fn cmd_query(
    cfg: &DbConfig,
    sql: &str,
    output: Option<String>,
    stdio: bool,
) -> Result<()> {
    validate_read_only(sql)?;

    let mut client = connect(cfg).await?;
    let result = client.query(sql, &[]).await?.into_first_result().await?;

    if result.is_empty() {
        eprintln!("Query returned no rows.");
        return Ok(());
    }

    // Build header from column metadata of first row
    let columns: Vec<String> = result[0]
        .columns()
        .iter()
        .map(|c| c.name().to_string())
        .collect();

    // Serialize to CSV in memory
    let mut buf: Vec<u8> = Vec::new();
    {
        let mut wtr = csv::Writer::from_writer(&mut buf);
        wtr.write_record(&columns)?;
        for row in &result {
            let fields: Vec<String> = (0..columns.len())
                .map(|i| {
                    // tiberius returns typed values; we cast everything to string
                    row.get::<&str, _>(i)
                        .map(|s| s.to_string())
                        .or_else(|| row.get::<i32, _>(i).map(|v| v.to_string()))
                        .or_else(|| row.get::<i64, _>(i).map(|v| v.to_string()))
                        .or_else(|| row.get::<f64, _>(i).map(|v| v.to_string()))
                        .or_else(|| row.get::<f32, _>(i).map(|v| v.to_string()))
                        .or_else(|| row.get::<bool, _>(i).map(|v| v.to_string()))
                        .unwrap_or_else(|| "NULL".to_string())
                })
                .collect();
            wtr.write_record(&fields)?;
        }
        wtr.flush()?;
    }

    if stdio {
        io::stdout().write_all(&buf)?;
    } else {
        let stem = output.unwrap_or_else(|| {
            let hash: String = rand::thread_rng()
                .sample_iter(&rand::distributions::Alphanumeric)
                .take(8)
                .map(char::from)
                .collect();
            hash
        });
        let path = if stem.ends_with(".csv") {
            stem
        } else {
            format!("{stem}.csv")
        };
        std::fs::write(&path, &buf)?;
        println!("Saved {} rows to {path}", result.len());
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

#[tokio::main]
async fn main() {
    let cli = Cli::parse();

    let cfg = match load_db_config() {
        Ok(c) => c,
        Err(e) => {
            eprintln!("Configuration error: {e}");
            std::process::exit(1);
        }
    };

    let result = match &cli.command {
        Commands::TestConnection => cmd_test_connection(&cfg).await,
        Commands::ListClinicalTables => cmd_list_tables(&cfg).await,
        Commands::Query { sql, output, stdio } => {
            cmd_query(&cfg, sql, output.clone(), *stdio).await
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
