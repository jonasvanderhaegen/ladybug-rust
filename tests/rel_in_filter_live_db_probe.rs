//! Env-gated probe: run the IN-vs-OR rel-property filter comparison against a
//! REAL database directory (e.g. a copy of a skybox graph that exhibits the
//! #806 corruption live). Not part of the normal suite — set
//! `LBUG_REPRO_DB=/path/to/lbug.kuzu` and `LBUG_REPRO_NODE_ID` to run:
//!
//! ```sh
//! LBUG_REPRO_DB=/tmp/copy/lbug.kuzu \
//! LBUG_REPRO_NODE_ID='Class:src/Models/Role.php:Role' \
//! cargo test --test rel_in_filter_live_db_probe -- --ignored --nocapture
//! ```
//!
//! Run each probe in its OWN process (`-- --ignored --exact <name>`): the FTS
//! probes load a database extension process-wide, which would contaminate any
//! sibling test sharing the harness process. A batch run (`-- --ignored`) is
//! additionally serialized by `PROBE_STORE_LOCK`: all probes share the single
//! `LBUG_REPRO_DB` store path, and libtest's default parallel threads made
//! concurrent probes race the same database files (shadow-replay refusal on
//! read-only open, WAL unlink ENOENT, interleaved WAL records asserting
//! UNREACHABLE_CODE in wal_record.cpp) — skylence-be/ladybug#7.
//!
//! ENGINE IDENTITY (the lesson of 2026-08-06): build with `LBUG_VERSION=0.17.0`
//! so build.rs links the same prebuilt liblbug the skybox daemon ships (storage
//! version 41, May 2026 artifact, sha256 958dfa3a…). THAT build reproduces the
//! #806 IN corruption deterministically on a pristine production DB copy —
//! single Database, single Connection, no extensions, no daemon state. The
//! default `latest` prebuilt (storage version 43, Aug 2026) does NOT reproduce
//! it, and its first R/W open silently upgrades a v41 DB file to v43,
//! destroying the evidence. Take a fresh DB copy per engine build.

use std::collections::BTreeMap;
use std::sync::Mutex;

use lbug::{Connection, Database, SystemConfig, Value};

const IN_FILTER: &str =
    "r.type IN ['EXTENDS', 'IMPLEMENTS', 'USES_TRAIT', 'METHOD_OVERRIDES', 'METHOD_IMPLEMENTS']";
const OR_FILTER: &str = "(r.type = 'EXTENDS' OR r.type = 'IMPLEMENTS' OR r.type = 'USES_TRAIT' OR r.type = 'METHOD_OVERRIDES' OR r.type = 'METHOD_IMPLEMENTS')";

/// Serializes every probe in this file: they all open the one store named by
/// `LBUG_REPRO_DB`, so two probes on parallel libtest threads collide on the
/// same database files (skylence-be/ladybug#7). Poison-tolerant so one
/// panicking probe does not cascade into every later one.
static PROBE_STORE_LOCK: Mutex<()> = Mutex::new(());

fn probe_store_guard() -> std::sync::MutexGuard<'static, ()> {
    PROBE_STORE_LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

fn rows(conn: &Connection, node_id: &str, filter: &str) -> BTreeMap<(String, String), usize> {
    let q = format!(
        "MATCH (n {{id: '{node_id}'}})-[r:CodeRelation]->(x) WHERE {filter} RETURN x.name, r.type"
    );
    let result = conn.query(&q).expect("probe query must execute");
    let mut out: BTreeMap<(String, String), usize> = BTreeMap::new();
    for values in result {
        let target = match &values[0] {
            Value::String(s) => s.clone(),
            v => format!("{v:?}"),
        };
        let ty = match &values[1] {
            Value::String(s) => s.clone(),
            v => format!("{v:?}"),
        };
        *out.entry((target, ty)).or_insert(0) += 1;
    }
    out
}

#[test]
#[ignore = "env-gated live-DB probe; set LBUG_REPRO_DB + LBUG_REPRO_NODE_ID"]
fn live_db_in_filter_matches_or_chain() {
    let _store_guard = probe_store_guard();
    let db_path = std::env::var("LBUG_REPRO_DB").expect("set LBUG_REPRO_DB");
    let node_id = std::env::var("LBUG_REPRO_NODE_ID").expect("set LBUG_REPRO_NODE_ID");
    let db = Database::new(&db_path, SystemConfig::default()).unwrap();
    let conn = Connection::new(&db).unwrap();
    let in_rows = rows(&conn, &node_id, IN_FILTER);
    let or_rows = rows(&conn, &node_id, OR_FILTER);
    println!("IN  rows: {in_rows:#?}");
    println!("OR  rows: {or_rows:#?}");
    assert_eq!(
        in_rows, or_rows,
        "rel-property IN-list diverges from the equivalent OR-chain on this DB (skybox#806)"
    );
}

/// Same probe with a SECOND Database open in the process (different path,
/// different schema), interleaving queries — the #189-family condition every
/// corrupted live observation shared (daemon with many Databases) and every
/// clean observation lacked (single-Database process).
#[test]
#[ignore = "env-gated live-DB probe; set LBUG_REPRO_DB + LBUG_REPRO_NODE_ID"]
fn live_db_in_filter_with_second_database_open() {
    let _store_guard = probe_store_guard();
    let db_path = std::env::var("LBUG_REPRO_DB").expect("set LBUG_REPRO_DB");
    let node_id = std::env::var("LBUG_REPRO_NODE_ID").expect("set LBUG_REPRO_NODE_ID");

    // Second database: tiny unrelated schema, kept open and queried first —
    // including its own rel-property IN filter to exercise whatever process-
    // global state IN evaluation touches.
    let dir = tempfile::tempdir().unwrap();
    let other = Database::new(dir.path().join("other.kz"), SystemConfig::default()).unwrap();
    let oconn = Connection::new(&other).unwrap();
    oconn.query("CREATE NODE TABLE A(id STRING, PRIMARY KEY(id))").unwrap();
    oconn.query("CREATE NODE TABLE B(id STRING, PRIMARY KEY(id))").unwrap();
    oconn
        .query("CREATE REL TABLE S(FROM A TO B, FROM A TO A, kind STRING)")
        .unwrap();
    oconn.query("CREATE (:A {id: 'a1'})").unwrap();
    oconn.query("CREATE (:A {id: 'a2'})").unwrap();
    oconn.query("CREATE (:B {id: 'b1'})").unwrap();
    oconn
        .query("MATCH (a:A {id: 'a1'}), (b:B {id: 'b1'}) CREATE (a)-[:S {kind: 'X'}]->(b)")
        .unwrap();
    oconn
        .query("MATCH (a:A {id: 'a1'}), (b:A {id: 'a2'}) CREATE (a)-[:S {kind: 'Y'}]->(b)")
        .unwrap();
    let _ = oconn
        .query("MATCH (a:A)-[r:S]->(x) WHERE r.kind IN ['Y', 'Z'] RETURN x.id, r.kind")
        .unwrap()
        .into_iter()
        .count();

    // Target database opened SECOND, probed while `other` stays alive.
    let db = Database::new(&db_path, SystemConfig::default()).unwrap();
    let conn = Connection::new(&db).unwrap();
    let in_rows = rows(&conn, &node_id, IN_FILTER);
    let or_rows = rows(&conn, &node_id, OR_FILTER);
    println!("IN  rows (2nd db open): {in_rows:#?}");
    println!("OR  rows (2nd db open): {or_rows:#?}");
    assert_eq!(
        in_rows, or_rows,
        "rel-property IN-list diverges from OR-chain with a second Database open (skybox#806/#799)"
    );
}

/// Production-shaped FTS search session, exactly as skybox's `FtsIndexer`
/// runs it per search (skybox fts.rs): INSTALL + LOAD EXTENSION fts with
/// "already ..." errors swallowed, the SHOW_INDEXES existence guard, then a
/// real QUERY_FTS_INDEX against the persisted `class_fts` index.
fn fts_search_session(db: &Database) {
    let conn = Connection::new(db).unwrap();
    for q in &["INSTALL fts", "LOAD EXTENSION fts"] {
        match conn.query(q) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string().to_ascii_lowercase();
                assert!(
                    msg.contains("already loaded")
                        || msg.contains("already installed")
                        || msg.contains("already exists"),
                    "FTS extension setup failed on '{q}': {e}"
                );
            }
        }
    }
    let shown = conn
        .query("CALL SHOW_INDEXES() RETURN *")
        .expect("SHOW_INDEXES must execute")
        .into_iter()
        .count();
    let hits = conn
        .query(
            "CALL QUERY_FTS_INDEX('Class', 'class_fts', 'role', conjunctive := false) \
             RETURN node.id AS id, score ORDER BY score DESC LIMIT 10",
        )
        .expect("QUERY_FTS_INDEX must execute (is class_fts persisted in this DB copy?)")
        .into_iter()
        .count();
    println!("FTS session: {shown} indexes shown, {hits} fts hits");
}

/// Ingredient probe: the per-search FTS session (extension load + index
/// query) on the SAME Database before the IN/OR probe — the session every
/// corrupted daemon observation shared (skybox#799/#806 prime suspect:
/// LOAD EXTENSION mutating process-global registries).
#[test]
#[ignore = "env-gated live-DB probe; set LBUG_REPRO_DB + LBUG_REPRO_NODE_ID; run with --exact"]
fn live_db_in_filter_after_fts_search_session() {
    let _store_guard = probe_store_guard();
    let db_path = std::env::var("LBUG_REPRO_DB").expect("set LBUG_REPRO_DB");
    let node_id = std::env::var("LBUG_REPRO_NODE_ID").expect("set LBUG_REPRO_NODE_ID");
    let db = Database::new(&db_path, SystemConfig::default()).unwrap();

    let baseline_or = rows(&Connection::new(&db).unwrap(), &node_id, OR_FILTER);
    fts_search_session(&db);

    let conn = Connection::new(&db).unwrap();
    let in_rows = rows(&conn, &node_id, IN_FILTER);
    let or_rows = rows(&conn, &node_id, OR_FILTER);
    println!("OR  rows (pre-FTS):  {baseline_or:#?}");
    println!("IN  rows (post-FTS): {in_rows:#?}");
    println!("OR  rows (post-FTS): {or_rows:#?}");
    assert_eq!(
        or_rows, baseline_or,
        "OR-chain result changed after an FTS session on the same Database (skybox#799/#806)"
    );
    assert_eq!(
        in_rows, or_rows,
        "rel-property IN-list diverges from OR-chain after an FTS session (skybox#799/#806)"
    );
}

/// The full daemon anatomy (skybox#799): a read-only Database (the cypher
/// pool analog — the corrupted live observations came through it) beside a
/// SECOND R/W Database on the SAME path running the FTS session (the
/// FtsIndexer analog). Same-path double-open is the #189 constraint the
/// daemon violates. Probes the read-only side both while the writer is
/// alive and after it drops.
#[test]
#[ignore = "env-gated live-DB probe; set LBUG_REPRO_DB + LBUG_REPRO_NODE_ID; run with --exact"]
fn live_db_in_filter_readonly_beside_same_path_fts_writer() {
    let _store_guard = probe_store_guard();
    let db_path = std::env::var("LBUG_REPRO_DB").expect("set LBUG_REPRO_DB");
    let node_id = std::env::var("LBUG_REPRO_NODE_ID").expect("set LBUG_REPRO_NODE_ID");

    let writer = Database::new(&db_path, SystemConfig::default()).unwrap();
    fts_search_session(&writer);

    let ro = Database::new(&db_path, SystemConfig::default().read_only(true))
        .expect("read-only open beside a live same-path R/W Database (daemon shape) failed");
    let conn = Connection::new(&ro).unwrap();
    let in_alive = rows(&conn, &node_id, IN_FILTER);
    let or_alive = rows(&conn, &node_id, OR_FILTER);
    println!("IN  rows (writer alive): {in_alive:#?}");
    println!("OR  rows (writer alive): {or_alive:#?}");

    drop(writer);
    let conn2 = Connection::new(&ro).unwrap();
    let in_after = rows(&conn2, &node_id, IN_FILTER);
    let or_after = rows(&conn2, &node_id, OR_FILTER);
    println!("IN  rows (writer dropped): {in_after:#?}");
    println!("OR  rows (writer dropped): {or_after:#?}");

    assert_eq!(
        in_alive, or_alive,
        "IN diverges from OR on the read-only Database while a same-path FTS writer is alive (skybox#799/#806)"
    );
    assert_eq!(
        in_after, or_after,
        "IN diverges from OR on the read-only Database after the same-path FTS writer dropped (skybox#799/#806)"
    );
}

/// Production-shaped semantic-search session, as skybox's `semantic_search`
/// runs it per query (semantic.rs): its own R/W `Database` open, INSTALL +
/// LOAD EXTENSION VECTOR (idempotent), SHOW_INDEXES guard, then a real
/// QUERY_VECTOR_INDEX against the persisted `code_embedding_idx`.
fn vector_search_session(db: &Database, dims: usize) {
    let conn = Connection::new(db).unwrap();
    for q in &["INSTALL VECTOR", "LOAD EXTENSION VECTOR"] {
        match conn.query(q) {
            Ok(_) => {}
            Err(e) => {
                let msg = e.to_string().to_ascii_lowercase();
                assert!(
                    msg.contains("already loaded")
                        || msg.contains("already installed")
                        || msg.contains("already exists"),
                    "VECTOR extension setup failed on '{q}': {e}"
                );
            }
        }
    }
    let _shown = conn
        .query("CALL SHOW_INDEXES() RETURN *")
        .expect("SHOW_INDEXES must execute")
        .into_iter()
        .count();
    let qv = vec!["0.1"; dims].join(", ");
    let hits = conn
        .query(&format!(
            "CALL QUERY_VECTOR_INDEX('CodeEmbedding', 'code_embedding_idx', \
             CAST([{qv}] AS FLOAT[{dims}]), 5) \
             YIELD node, distance RETURN node.id, distance"
        ))
        .expect("QUERY_VECTOR_INDEX must execute (is code_embedding_idx persisted in this DB copy?)")
        .into_iter()
        .count();
    println!("VECTOR session: {hits} hits");
}

/// Every remaining in-process daemon ingredient at once (skybox#799/#806):
/// a long-lived read-only pool Database, repeated per-query same-path R/W
/// Database churn running VECTOR + FTS sessions (hybrid-style shared open on
/// even rounds, two separate same-path opens on odd rounds), rel-property IN
/// query history on the churned connections, and a throwaway unrelated DB
/// with its own rel-property IN per round. Red here bisects down to the
/// triggering ingredient; clean here means the corruption needs state this
/// harness cannot build in-process (uptime, concurrency, query history) and
/// the next lane is C++ instrumentation.
#[test]
#[ignore = "env-gated live-DB probe; set LBUG_REPRO_DB + LBUG_REPRO_NODE_ID; run with --exact"]
fn live_db_in_filter_daemon_anatomy_kitchen_sink() {
    let _store_guard = probe_store_guard();
    let db_path = std::env::var("LBUG_REPRO_DB").expect("set LBUG_REPRO_DB");
    let node_id = std::env::var("LBUG_REPRO_NODE_ID").expect("set LBUG_REPRO_NODE_ID");
    let dims: usize = std::fs::read_to_string(
        std::path::Path::new(&db_path).with_file_name("embedding_dims"),
    )
    .expect("read embedding_dims beside the DB copy")
    .trim()
    .parse()
    .expect("embedding_dims must be a number");

    // Long-lived read-only Database — the cypher pool analog, the surface the
    // live corruption was observed through. Stays open across all churn.
    let ro = Database::new(&db_path, SystemConfig::default().read_only(true)).unwrap();
    let baseline_or = rows(&Connection::new(&ro).unwrap(), &node_id, OR_FILTER);

    for i in 0..8 {
        let rw = Database::new(&db_path, SystemConfig::default()).unwrap();
        vector_search_session(&rw, dims);
        if i % 2 == 0 {
            // Hybrid-style: FTS on the same Database as the vector leg.
            fts_search_session(&rw);
            let _ = rows(&Connection::new(&rw).unwrap(), &node_id, IN_FILTER);
        } else {
            // Separate FtsIndexer-style open: two same-path R/W Databases alive.
            let rw2 = Database::new(&db_path, SystemConfig::default()).unwrap();
            fts_search_session(&rw2);
            let _ = rows(&Connection::new(&rw2).unwrap(), &node_id, IN_FILTER);
        }

        // Cross-repo diversity: throwaway unrelated DB, own rel-property IN.
        let dir = tempfile::tempdir().unwrap();
        let other = Database::new(dir.path().join("other.kz"), SystemConfig::default()).unwrap();
        let oconn = Connection::new(&other).unwrap();
        oconn.query("CREATE NODE TABLE A(id STRING, PRIMARY KEY(id))").unwrap();
        oconn.query("CREATE REL TABLE S(FROM A TO A, kind STRING)").unwrap();
        oconn.query("CREATE (:A {id: 'a1'})").unwrap();
        oconn.query("CREATE (:A {id: 'a2'})").unwrap();
        oconn
            .query("MATCH (a:A {id: 'a1'}), (b:A {id: 'a2'}) CREATE (a)-[:S {kind: 'Y'}]->(b)")
            .unwrap();
        let _ = oconn
            .query("MATCH (a:A)-[r:S]->(x) WHERE r.kind IN ['Y', 'Z'] RETURN x.id")
            .unwrap()
            .into_iter()
            .count();
        println!("churn round {i} done");
    }

    // Probe the long-lived read-only pool first, then a fresh R/W open.
    let conn = Connection::new(&ro).unwrap();
    let in_ro = rows(&conn, &node_id, IN_FILTER);
    let or_ro = rows(&conn, &node_id, OR_FILTER);
    println!("IN  rows (pool after churn): {in_ro:#?}");
    println!("OR  rows (pool after churn): {or_ro:#?}");

    let rw = Database::new(&db_path, SystemConfig::default()).unwrap();
    let conn_rw = Connection::new(&rw).unwrap();
    let in_rw = rows(&conn_rw, &node_id, IN_FILTER);
    let or_rw = rows(&conn_rw, &node_id, OR_FILTER);
    println!("IN  rows (fresh R/W after churn): {in_rw:#?}");
    println!("OR  rows (fresh R/W after churn): {or_rw:#?}");

    assert_eq!(
        or_ro, baseline_or,
        "OR-chain result changed on the pool Database after daemon-anatomy churn (skybox#799/#806)"
    );
    assert_eq!(
        in_ro, or_ro,
        "rel-property IN-list diverges from OR-chain on the pool Database after daemon-anatomy churn (skybox#799/#806)"
    );
    assert_eq!(
        in_rw, or_rw,
        "rel-property IN-list diverges from OR-chain on a fresh R/W open after daemon-anatomy churn (skybox#799/#806)"
    );
}
