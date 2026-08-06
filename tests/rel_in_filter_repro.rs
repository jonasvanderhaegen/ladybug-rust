//! Standalone reproduction attempt for the rel-property IN-list corruption
//! observed live on skybox graphs (skylence-be/binary-skybox#806, 2026-08-06).
//!
//! Live facts (raw Cypher on lbug 0.17.0, no application code): on a node
//! whose outgoing edges span multiple pairs of one multi-pair rel table,
//! `WHERE r.type IN [...]` returned the correct ROW COUNT but value-misaligned
//! rows — a type outside the list appeared (duplicated) and rows matching the
//! filter vanished — while the equivalent OR-chain returned the exact correct
//! set. IN on NODE properties was unaffected.
//!
//! Fixture ingredients mirrored from production: mixed-pair layout (the
//! minimal 3-pair shape alone did NOT reproduce — first falsification), the
//! production property mix (type STRING, confidence DOUBLE, reason STRING),
//! a couple hundred edges on the source node, an explicit CHECKPOINT, and a
//! close + reopen before querying. If these tests are GREEN on a version, the
//! corruption either needs further ingredients (bulk COPY loading, the full
//! 26-pair CodeRelation schema, FTS-index sessions) or is fixed there; the
//! live graphs remain the authoritative repro either way.

use std::collections::BTreeSet;
use std::path::Path;

use lbug::{Connection, Database, SystemConfig, Value};

const HERITAGE_FILTER_IN: &str =
    "r.type IN ['EXTENDS', 'IMPLEMENTS', 'USES_TRAIT', 'METHOD_OVERRIDES', 'METHOD_IMPLEMENTS']";
const HERITAGE_FILTER_OR: &str = "(r.type = 'EXTENDS' OR r.type = 'IMPLEMENTS' OR r.type = 'USES_TRAIT' OR r.type = 'METHOD_OVERRIDES' OR r.type = 'METHOD_IMPLEMENTS')";

fn build_fixture(db_path: &Path) {
    let db = Database::new(db_path, SystemConfig::default()).unwrap();
    let conn = Connection::new(&db).unwrap();
    for ddl in [
        "CREATE NODE TABLE Cls(id STRING, PRIMARY KEY(id))",
        "CREATE NODE TABLE Meth(id STRING, PRIMARY KEY(id))",
        "CREATE NODE TABLE Tr(id STRING, PRIMARY KEY(id))",
        "CREATE REL TABLE R(FROM Cls TO Meth, FROM Cls TO Cls, FROM Cls TO Tr, type STRING, confidence DOUBLE, reason STRING)",
    ] {
        conn.query(ddl).expect(ddl);
    }
    conn.query("CREATE (:Cls {id: 'c_src'})").unwrap();
    conn.query("CREATE (:Cls {id: 'c_base'})").unwrap();
    conn.query("CREATE (:Cls {id: 'c_contract'})").unwrap();
    for i in 1..=120 {
        conn.query(&format!("CREATE (:Meth {{id: 'm{i}'}})")).unwrap();
    }
    for i in 1..=3 {
        conn.query(&format!("CREATE (:Tr {{id: 't{i}'}})")).unwrap();
    }
    // Edges from c_src across three pairs, with the production property mix
    // so value misalignment has sibling columns to bleed across.
    for i in 1..=120 {
        conn.query(&format!(
            "MATCH (a:Cls {{id: 'c_src'}}), (b:Meth {{id: 'm{i}'}}) CREATE (a)-[:R {{type: 'HAS_METHOD', confidence: 1.0, reason: 'has-method'}}]->(b)"
        ))
        .unwrap();
    }
    conn.query(
        "MATCH (a:Cls {id: 'c_src'}), (b:Cls {id: 'c_base'}) CREATE (a)-[:R {type: 'EXTENDS', confidence: 1.0, reason: 'heritage'}]->(b)",
    )
    .unwrap();
    conn.query(
        "MATCH (a:Cls {id: 'c_src'}), (b:Cls {id: 'c_contract'}) CREATE (a)-[:R {type: 'IMPLEMENTS', confidence: 1.0, reason: 'heritage'}]->(b)",
    )
    .unwrap();
    for i in 1..=3 {
        conn.query(&format!(
            "MATCH (a:Cls {{id: 'c_src'}}), (b:Tr {{id: 't{i}'}}) CREATE (a)-[:R {{type: 'USES_TRAIT', confidence: 1.0, reason: 'heritage'}}]->(b)"
        ))
        .unwrap();
    }
    conn.query("CHECKPOINT").unwrap();
    // db + conn drop here: the queries below run on a REOPENED database, like
    // the production read path.
}

fn filtered_rows(conn: &Connection, filter: &str) -> (usize, BTreeSet<(String, String)>) {
    let q =
        format!("MATCH (n:Cls {{id: 'c_src'}})-[r:R]->(x) WHERE {filter} RETURN x.id, r.type");
    let result = conn.query(&q).expect("filter query must execute");
    let mut rows = BTreeSet::new();
    let mut raw_count = 0usize;
    for values in result {
        raw_count += 1;
        let target = match &values[0] {
            Value::String(s) => s.clone(),
            v => format!("{v:?}"),
        };
        let ty = match &values[1] {
            Value::String(s) => s.clone(),
            v => format!("{v:?}"),
        };
        rows.insert((target, ty));
    }
    (raw_count, rows)
}

fn expected() -> BTreeSet<(String, String)> {
    let mut e = BTreeSet::new();
    e.insert(("c_base".to_owned(), "EXTENDS".to_owned()));
    e.insert(("c_contract".to_owned(), "IMPLEMENTS".to_owned()));
    e.insert(("t1".to_owned(), "USES_TRAIT".to_owned()));
    e.insert(("t2".to_owned(), "USES_TRAIT".to_owned()));
    e.insert(("t3".to_owned(), "USES_TRAIT".to_owned()));
    e
}

/// The OR-chain control: must always return the exact correct set.
#[test]
fn or_chain_filter_returns_correct_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.kz");
    build_fixture(&db_path);
    let db = Database::new(&db_path, SystemConfig::default()).unwrap();
    let conn = Connection::new(&db).unwrap();
    let (raw, rows) = filtered_rows(&conn, HERITAGE_FILTER_OR);
    assert_eq!(rows, expected());
    assert_eq!(raw, 5, "no duplicated rows expected");
}

/// The IN-list form: semantically identical to the OR-chain; corrupted on the
/// live 0.17.0 graphs (phantom out-of-list rows, dropped matching rows).
/// RED here is the reproduction #806 wants.
#[test]
fn in_list_filter_returns_correct_rows() {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("t.kz");
    build_fixture(&db_path);
    let db = Database::new(&db_path, SystemConfig::default()).unwrap();
    let conn = Connection::new(&db).unwrap();
    let (raw, rows) = filtered_rows(&conn, HERITAGE_FILTER_IN);
    assert_eq!(
        rows,
        expected(),
        "rel-property IN-list must match the equivalent OR-chain (skybox#806)"
    );
    assert_eq!(raw, 5, "no duplicated rows expected");
}
