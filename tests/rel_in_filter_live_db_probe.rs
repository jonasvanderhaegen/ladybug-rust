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

use std::collections::BTreeMap;

use lbug::{Connection, Database, SystemConfig, Value};

const IN_FILTER: &str =
    "r.type IN ['EXTENDS', 'IMPLEMENTS', 'USES_TRAIT', 'METHOD_OVERRIDES', 'METHOD_IMPLEMENTS']";
const OR_FILTER: &str = "(r.type = 'EXTENDS' OR r.type = 'IMPLEMENTS' OR r.type = 'USES_TRAIT' OR r.type = 'METHOD_OVERRIDES' OR r.type = 'METHOD_IMPLEMENTS')";

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
