use anyhow::{anyhow, Context, Result};
use clap::{Parser, Subcommand};
use odbc_api::{
    buffers::TextRowSet, ConnectionOptions, Cursor, Environment, ResultSetMetadata,
};
use rand::Rng;
use regex::Regex;
use std::io::{self, Write as IoWrite};

// ---------------------------------------------------------------------------
// CLI definition
// ---------------------------------------------------------------------------

/// ucsfomop — Query the UCSF OMOP de-identified EHR database
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
(FreeTDS ODBC) using credentials stored in a .env file or environment
variables.

ENVIRONMENT VARIABLES (loaded from .env automatically):
  CLINICAL_RECORDS_SERVER    SQL Server hostname
  CLINICAL_RECORDS_DATABASE  Database name
  CLINICAL_RECORDS_USERNAME  Login (DOMAIN\\user format supported)
  CLINICAL_RECORDS_PASSWORD  Password

COMMANDS:
  test-connection        Run a lightweight query to verify connectivity.

  list-clinical-tables   Print all tables with schema/name to stdout.

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

fn validate_read_only(query: &str) -> Result<()> {
    let trimmed = query.trim().to_uppercase();
    let allowed = ["SELECT", "WITH", "DECLARE"];
    if !allowed.iter().any(|s| trimmed.starts_with(s)) {
        return Err(anyhow!(
            "Query rejected: only SELECT / WITH / DECLARE statements are permitted."
        ));
    }
    let write_re = Regex::new(
        r"(?i)\b(MERGE|CREATE|SET|DELETE|REMOVE|ADD|INSERT|UPDATE|DROP|ALTER|TRUNCATE|GRANT|REVOKE|EXEC|EXECUTE|SP_)\b"
    ).unwrap();
    if write_re.is_match(query) {
        return Err(anyhow!(
            "Query rejected: write operations are not allowed (INSERT, UPDATE, DELETE, DROP, …)."
        ));
    }
    // Block stacked statements
    if Regex::new(r";\s*\w+").unwrap().is_match(&trimmed) {
        return Err(anyhow!(
            "Query rejected: stacked statements are not allowed."
        ));
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Database config + connection string
// ---------------------------------------------------------------------------

struct DbConfig {
    server: String,
    database: String,
    username: String,
    password: String,
}

fn load_db_config() -> Result<DbConfig> {
    let _ = dotenvy::dotenv();
    Ok(DbConfig {
        server: std::env::var("CLINICAL_RECORDS_SERVER")
            .context("CLINICAL_RECORDS_SERVER not set")?,
        database: std::env::var("CLINICAL_RECORDS_DATABASE")
            .context("CLINICAL_RECORDS_DATABASE not set")?,
        username: std::env::var("CLINICAL_RECORDS_USERNAME")
            .context("CLINICAL_RECORDS_USERNAME not set")?,
        password: std::env::var("CLINICAL_RECORDS_PASSWORD")
            .context("CLINICAL_RECORDS_PASSWORD not set")?,
    })
}

/// Resolve the FreeTDS ODBC driver path.
/// Prefers a bundled copy next to the binary; falls back to homebrew or the
/// ODBC registry name "FreeTDS" if nothing else is found.
fn freetds_driver_path() -> String {
    // 1. Bundled: <exe>/../lib/ucsfomop/libtdsodbc.so
    if let Ok(exe) = std::env::current_exe() {
        if let Some(bin_dir) = exe.parent() {
            let bundled = bin_dir.join("../lib/ucsfomop/libtdsodbc.so");
            if let Ok(resolved) = bundled.canonicalize() {
                return resolved.to_string_lossy().into_owned();
            }
        }
    }
    // 2. Homebrew arm64
    if std::path::Path::new("/opt/homebrew/lib/libtdsodbc.so").exists() {
        return "/opt/homebrew/lib/libtdsodbc.so".to_string();
    }
    // 3. System ODBC registry name as last resort
    "FreeTDS".to_string()
}

fn conn_string(cfg: &DbConfig) -> String {
    let driver = freetds_driver_path();
    format!(
        "DRIVER={driver};SERVER={};PORT=1433;DATABASE={};UID={};PWD={};TDS_Version=7.4;Encrypt=yes;TrustServerCertificate=yes;",
        cfg.server, cfg.database, cfg.username, cfg.password
    )
}

// ---------------------------------------------------------------------------
// Helper: run a query and collect all rows as Vec<Vec<String>>
// ---------------------------------------------------------------------------

const BATCH_SIZE: usize = 1000;
const MAX_STR_LEN: usize = 4096;

fn fetch_all(
    env: &Environment,
    conn_str: &str,
    sql: &str,
) -> Result<(Vec<String>, Vec<Vec<String>>)> {
    let conn = env
        .connect_with_connection_string(conn_str, ConnectionOptions::default())
        .context("ODBC connection failed")?;

    match conn.execute(sql, ()).context("Query execution failed")? {
        None => Ok((vec![], vec![])),
        Some(mut cursor) => {
            let headers: Vec<String> = cursor
                .column_names()
                .context("Failed to read column names")?
                .collect::<std::result::Result<_, _>>()
                .context("Column name error")?;

            let mut buffers =
                TextRowSet::for_cursor(BATCH_SIZE, &mut cursor, Some(MAX_STR_LEN))
                    .context("Buffer allocation failed")?;
            let mut row_set = cursor
                .bind_buffer(&mut buffers)
                .context("Bind buffer failed")?;

            let mut rows: Vec<Vec<String>> = Vec::new();
            while let Some(batch) = row_set.fetch().context("Fetch failed")? {
                for row_idx in 0..batch.num_rows() {
                    let row: Vec<String> = (0..batch.num_cols())
                        .map(|col_idx| {
                            batch
                                .at(col_idx, row_idx)
                                .map(|b| {
                                    std::str::from_utf8(b)
                                        .unwrap_or("?")
                                        .to_string()
                                })
                                .unwrap_or_else(|| "NULL".to_string())
                        })
                        .collect();
                    rows.push(row);
                }
            }
            Ok((headers, rows))
        }
    }
}

// ---------------------------------------------------------------------------
// Write helpers
// ---------------------------------------------------------------------------

fn write_csv<W: IoWrite>(
    writer: W,
    headers: &[String],
    rows: &[Vec<String>],
) -> Result<()> {
    let mut wtr = csv::Writer::from_writer(writer);
    wtr.write_record(headers)?;
    for row in rows {
        wtr.write_record(row)?;
    }
    wtr.flush()?;
    Ok(())
}

fn random_stem() -> String {
    rand::thread_rng()
        .sample_iter(&rand::distributions::Alphanumeric)
        .take(8)
        .map(char::from)
        .collect()
}

// ---------------------------------------------------------------------------
// Commands
// ---------------------------------------------------------------------------

fn cmd_test_connection(env: &Environment, conn_str: &str) -> Result<()> {
    println!("Testing connection…");
    let (_, rows) = fetch_all(env, conn_str, "SELECT GETDATE() AS server_time")?;
    let ts = rows.first().and_then(|r| r.first()).map(|s| s.as_str()).unwrap_or("(none)");
    println!("Connection successful. Server time: {ts}");
    Ok(())
}

fn cmd_list_tables(env: &Environment, conn_str: &str) -> Result<()> {
    let sql = "\
        SELECT TABLE_SCHEMA, TABLE_NAME, TABLE_TYPE \
        FROM INFORMATION_SCHEMA.TABLES \
        WHERE TABLE_TYPE = 'BASE TABLE' \
        ORDER BY TABLE_SCHEMA, TABLE_NAME";

    let (_, rows) = fetch_all(env, conn_str, sql)?;
    println!("{:<30} {:<50} {}", "SCHEMA", "TABLE_NAME", "TYPE");
    println!("{}", "-".repeat(90));
    for row in &rows {
        println!(
            "{:<30} {:<50} {}",
            row.get(0).map(|s| s.as_str()).unwrap_or(""),
            row.get(1).map(|s| s.as_str()).unwrap_or(""),
            row.get(2).map(|s| s.as_str()).unwrap_or(""),
        );
    }
    println!("\n{} tables found.", rows.len());
    Ok(())
}

fn cmd_query(
    env: &Environment,
    conn_str: &str,
    sql: &str,
    output: Option<String>,
    stdio: bool,
) -> Result<()> {
    validate_read_only(sql)?;

    let (headers, rows) = fetch_all(env, conn_str, sql)?;
    if headers.is_empty() {
        eprintln!("Query returned no results.");
        return Ok(());
    }

    if stdio {
        write_csv(io::stdout(), &headers, &rows)?;
    } else {
        let stem = output.unwrap_or_else(random_stem);
        let path = if stem.ends_with(".csv") { stem } else { format!("{stem}.csv") };
        let file = std::fs::File::create(&path).context("Cannot create output file")?;
        write_csv(file, &headers, &rows)?;
        println!("Saved {} rows → {path}", rows.len());
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Entry point
// ---------------------------------------------------------------------------

fn main() {
    let cli = Cli::parse();

    let cfg = match load_db_config() {
        Ok(c) => c,
        Err(e) => { eprintln!("Configuration error: {e}"); std::process::exit(1); }
    };

    let env = match Environment::new() {
        Ok(e) => e,
        Err(e) => { eprintln!("ODBC environment error: {e}"); std::process::exit(1); }
    };

    let cs = conn_string(&cfg);

    let result = match &cli.command {
        Commands::TestConnection => cmd_test_connection(&env, &cs),
        Commands::ListClinicalTables => cmd_list_tables(&env, &cs),
        Commands::Query { sql, output, stdio } => {
            cmd_query(&env, &cs, sql, output.clone(), *stdio)
        }
    };

    if let Err(e) = result {
        eprintln!("Error: {e:#}");
        std::process::exit(1);
    }
}
