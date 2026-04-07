# ucsfomop-cli

A command-line tool for querying the UCSF OMOP de-identified electronic health records database (`OMOP_DEID`). Written in Rust. Ships as a self-contained macOS binary — no Homebrew, no Python, no system ODBC setup required.

---

## Contents

- [Installation](#installation)
- [Configuration](#configuration)
- [Commands](#commands)
  - [test-connection](#test-connection)
  - [list-clinical-tables](#list-clinical-tables)
  - [query](#query)
- [Safety](#safety)
- [Database Schema Overview](#database-schema-overview)
- [Example Workflows](#example-workflows)
- [Building from Source](#building-from-source)
- [Troubleshooting](#troubleshooting)

---

## Installation

The `dist/ucsfomop-bundle/` directory contains a fully self-contained bundle — all required native libraries are included and pre-patched with `@rpath`/`@loader_path` so nothing needs to be installed system-wide first.

```bash
git clone https://github.com/Broccolito/ucsfomop-cli
cd ucsfomop-cli/dist/ucsfomop-bundle
bash install.sh
```

The installer will place the binary at `/usr/local/bin/ucsfomop` (system-wide, requires sudo) or `~/.local/bin/ucsfomop` (user-only, no sudo) depending on your permissions.

Verify the installation:

```bash
ucsfomop --version
# ucsfomop 0.1.0
```

**Requirements:** macOS on Apple Silicon (arm64). No Homebrew or other package manager required.

---

## Configuration

`ucsfomop` reads credentials from a `.env` file in the **current working directory**, or from environment variables. Create a `.env` before running any commands:

```dotenv
# .env
CLINICAL_RECORDS_SERVER=QCDIDDWDB001.ucsfmedicalcenter.org
CLINICAL_RECORDS_DATABASE=OMOP_DEID
CLINICAL_RECORDS_USERNAME=CAMPUS\\YourUsername
CLINICAL_RECORDS_PASSWORD=YourPassword
```

> **Note on the backslash:** In the `.env` file, the domain backslash must be escaped as `\\` (e.g. `CAMPUS\\Wgu`). In your shell environment, use a single backslash.

Environment variables take precedence over the `.env` file and can be used directly:

```bash
export CLINICAL_RECORDS_USERNAME='CAMPUS\YourUsername'
export CLINICAL_RECORDS_PASSWORD='YourPassword'
ucsfomop test-connection
```

---

## Commands

### test-connection

Runs a lightweight `SELECT GETDATE()` query to verify that the credentials and network path to the server are valid.

```bash
ucsfomop test-connection
```

```
Testing connection…
Connection successful. Server time: 2026-04-06 19:09:16.283
```

Use this first whenever you are on a new machine or network to confirm access before running heavier queries.

---

### list-clinical-tables

Lists all base tables in `OMOP_DEID`, grouped by schema, with their schema name, table name, and type.

```bash
ucsfomop list-clinical-tables
```

```
SCHEMA                         TABLE_NAME                                         TYPE
------------------------------------------------------------------------------------------
omop                           care_site                                          BASE TABLE
omop                           cdm_source                                         BASE TABLE
omop                           cohort                                             BASE TABLE
omop                           condition_occurrence                               BASE TABLE
omop                           drug_exposure                                      BASE TABLE
omop                           measurement                                        BASE TABLE
omop                           observation                                        BASE TABLE
omop                           person                                             BASE TABLE
omop                           visit_occurrence                                   BASE TABLE
...
47 tables found.
```

Output is always written to stdout, making it easy to pipe or redirect:

```bash
ucsfomop list-clinical-tables > tables.txt
ucsfomop list-clinical-tables | grep condition
```

---

### query

Executes a read-only SQL `SELECT` query against `OMOP_DEID` and returns the results as CSV.

```bash
ucsfomop query "<SQL>"
```

**Flags:**

| Flag | Description |
|---|---|
| `--output <name>` | Save results to `<name>.csv`. If omitted, a random 8-character filename is used. |
| `--stdio` | Print CSV to stdout instead of saving a file. |

**Examples:**

Save to a named file:
```bash
ucsfomop query "SELECT TOP 100 * FROM omop.person" --output person_sample
# Saved 100 rows → person_sample.csv
```

Save to a random-named file (useful for quick one-offs):
```bash
ucsfomop query "SELECT COUNT(*) AS n FROM omop.visit_occurrence"
# Saved 1 rows → a3f9c2d1.csv
```

Print to stdout (pipe-friendly):
```bash
ucsfomop query "SELECT TOP 5 person_id, year_of_birth FROM omop.person" --stdio
```
```
person_id,year_of_birth
1,2000
2,1963
3,1982
4,1975
5,1991
```

Save to a specific directory:
```bash
ucsfomop query "SELECT * FROM omop.death" --output /tmp/death_records
```

---

## Safety

All queries are validated client-side before any network traffic is sent. The following are blocked:

| Blocked keyword | Reason |
|---|---|
| `INSERT`, `UPDATE`, `DELETE` | Row modification |
| `DROP`, `ALTER`, `TRUNCATE` | Schema modification |
| `CREATE` | Object creation |
| `EXEC`, `EXECUTE`, `SP_` | Stored procedure execution |
| `MERGE` | Combined insert/update/delete |
| `GRANT`, `REVOKE` | Permission changes |
| Stacked statements (`;` followed by another statement) | SQL injection guard |

Only queries beginning with `SELECT`, `WITH`, or `DECLARE` are permitted.

If a blocked keyword is detected, the query is rejected immediately with an error message:

```
Error: Query rejected: write operations are not allowed (INSERT, UPDATE, DELETE, DROP, …).
```

---

## Database Schema Overview

The database contains 47 tables in the `omop` schema, following the [OMOP CDM v5](https://ohdsi.github.io/CommonDataModel/) standard. Key tables:

| Table | Description |
|---|---|
| `omop.person` | One row per patient — demographics, birth year, gender, race, ethnicity |
| `omop.visit_occurrence` | Inpatient, outpatient, and ED encounters |
| `omop.condition_occurrence` | Diagnoses (ICD → SNOMED mapped via concept table) |
| `omop.drug_exposure` | Medication prescriptions and administrations |
| `omop.measurement` | Lab results and vitals |
| `omop.observation` | Clinical observations not captured elsewhere |
| `omop.procedure_occurrence` | Procedures performed |
| `omop.death` | Date and cause of death |
| `omop.concept` | Vocabulary lookup — maps concept IDs to names and domains |
| `omop.concept_ancestor` | Hierarchical relationships between concepts (useful for cohort queries) |
| `omop.condition_era` | Rolled-up condition periods derived from condition_occurrence |
| `omop.drug_era` | Rolled-up drug exposure periods |
| `omop.note` | Clinical notes (free text) |
| `omop.measurement_extension` | UCSF-specific measurement extensions |

For a full list run `ucsfomop list-clinical-tables`.

---

## Example Workflows

**Count patients with a diagnosis (using concept hierarchy):**
```sql
SELECT COUNT(DISTINCT co.person_id) AS patient_count
FROM omop.condition_occurrence co
JOIN omop.concept_ancestor ca ON co.condition_concept_id = ca.descendant_concept_id
WHERE ca.ancestor_concept_id = 374919  -- Multiple sclerosis
```
```bash
ucsfomop query "..." --output /tmp/ms_count
```

**Pull demographics for a cohort:**
```sql
SELECT p.person_id, p.year_of_birth, p.gender_concept_id,
       g.concept_name AS gender
FROM omop.person p
JOIN omop.concept g ON p.gender_concept_id = g.concept_id
```
```bash
ucsfomop query "..." --stdio | head -20
```

**Find all lab measurements for a concept:**
```sql
SELECT TOP 1000 m.person_id, m.measurement_date,
       c.concept_name, m.value_as_number, m.unit_source_value
FROM omop.measurement m
JOIN omop.concept c ON m.measurement_concept_id = c.concept_id
WHERE c.concept_name LIKE '%hemoglobin%'
ORDER BY m.measurement_date DESC
```
```bash
ucsfomop query "..." --output /tmp/hgb_labs
```

**Pipe into other tools:**
```bash
ucsfomop query "SELECT year_of_birth, COUNT(*) AS n FROM omop.person GROUP BY year_of_birth ORDER BY year_of_birth" --stdio | \
  python3 -c "import sys,csv; rows=list(csv.reader(sys.stdin)); [print(r) for r in rows[:5]]"
```

---

## Building from Source

Requires Rust (stable), Homebrew, and the build-time dependencies:

```bash
brew install freetds unixodbc openssl@3 libtool
```

Build and run:
```bash
cargo build --release
./target/release/ucsfomop --help
```

To rebuild the self-contained distribution bundle after code changes:
```bash
bash bundle.sh
```

This regenerates `dist/ucsfomop-bundle/` with the updated binary and re-patched dylibs. Commit the `dist/` directory so users can install without a Rust toolchain.

---

## Troubleshooting

**`Configuration error: CLINICAL_RECORDS_USERNAME not set`**
No `.env` file found in the current directory. Either `cd` to the directory containing your `.env`, or export the variables in your shell.

**`Login failed for user '...'`**
Credentials are incorrect, or you are not connected to the UCSF network / VPN. Confirm VPN is active and credentials match those used for other UCSF systems.

**`TCP connection failed` / `TDS handshake failed`**
The server is unreachable. Connect to the UCSF VPN and retry.

**`dyld: Library not loaded`**
The bundled libraries are missing from `~/.local/lib/ucsfomop/` or `/usr/local/lib/ucsfomop/`. Re-run `install.sh` from the bundle directory.

**`Query rejected: write operations are not allowed`**
The SQL contains a blocked keyword. Only `SELECT`, `WITH`, and `DECLARE` queries are permitted — see [Safety](#safety).

---

*Maintained by Wanjun Gu (wanjun.gu@ucsf.edu)*
