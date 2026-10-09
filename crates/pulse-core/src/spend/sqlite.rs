// Ported from upstream Sources/Pulse/Usage/AgentSQLite.swift.
//! Read-only SQLite access for the agents that keep their records in a database.
//!
//! Open read-only and never write: `SQLITE_OPEN_READ_ONLY` will not create a missing file and
//! cannot modify an existing one, which matters because the database belongs to another
//! application that may be running. Pulse reads its journal in place and leaves it exactly as it
//! found it.
//!
//! A database whose schema does not match (no table, no column, an older version) simply yields
//! no rows. A failed `prepare` is an ordinary outcome here, not an error to surface, so a reader
//! built against one shape stays silent rather than failing against another.

use std::collections::HashSet;
use std::path::Path;

use rusqlite::types::ValueRef;
use rusqlite::{Connection, OpenFlags, Row};

/// Opens `path` read-only, or None when the file is missing or is not a database.
pub fn open(path: &Path) -> Option<Connection> {
    Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY | OpenFlags::SQLITE_OPEN_NO_MUTEX).ok()
}

/// Runs `sql` and calls `body` once per row. A statement that will not prepare yields nothing and
/// no error; this is how a schema mismatch reads as "no rows" rather than as a failure.
pub fn each(connection: &Connection, sql: &str, mut body: impl FnMut(&Row)) {
    let Ok(mut statement) = connection.prepare(sql) else { return };
    let Ok(mut rows) = statement.query([]) else { return };
    while let Ok(Some(row)) = rows.next() {
        body(row);
    }
}

/// A column as a string. NULL is None, never the empty string. A number is its decimal text, as
/// SQLite's own `sqlite3_column_text` gives it; a blob is read as UTF-8 where it is.
pub fn text(row: &Row, column: usize) -> Option<String> {
    match row.get_ref(column).ok()? {
        ValueRef::Text(bytes) | ValueRef::Blob(bytes) => std::str::from_utf8(bytes).ok().map(str::to_string),
        ValueRef::Integer(value) => Some(value.to_string()),
        ValueRef::Real(value) => Some(value.to_string()),
        ValueRef::Null => None,
    }
}

/// A column as an integer, the way `sqlite3_column_int64` reads it: a number as itself, text by
/// its leading digits, NULL as zero.
pub fn integer(row: &Row, column: usize) -> i64 {
    match row.get_ref(column) {
        Ok(ValueRef::Integer(value)) => value,
        Ok(ValueRef::Real(value)) => value as i64,
        Ok(ValueRef::Text(bytes)) => std::str::from_utf8(bytes).ok().and_then(|t| t.trim().parse::<f64>().ok()).map_or(0, |v| v as i64),
        _ => 0,
    }
}

/// The column names of a table; empty when there is no such table.
pub fn columns(connection: &Connection, table: &str) -> HashSet<String> {
    let mut names = HashSet::new();
    each(connection, &format!("PRAGMA table_info({table})"), |row| {
        if let Some(name) = text(row, 1) {
            names.insert(name);
        }
    });
    names
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A database file at `path` with the given statements run against it.
    pub fn database(path: &Path, statements: &[&str]) {
        let connection = Connection::open(path).unwrap();
        for sql in statements {
            connection.execute_batch(sql).unwrap();
        }
    }

    #[test]
    fn a_missing_file_is_not_created_and_a_schema_mismatch_yields_no_rows() {
        let root = tempfile::tempdir().unwrap();
        let missing = root.path().join("absent.db");
        assert!(open(&missing).is_none());
        assert!(!missing.exists(), "a read-only open must not create the file");

        let file = root.path().join("x.db");
        database(&file, &["CREATE TABLE t (a TEXT, b INTEGER)", "INSERT INTO t VALUES ('x', 7)", "INSERT INTO t VALUES (NULL, '12')"]);
        let connection = open(&file).unwrap();
        let mut rows = Vec::new();
        each(&connection, "SELECT a, b FROM t", |row| rows.push((text(row, 0), integer(row, 1))));
        assert_eq!(rows, [(Some("x".to_string()), 7), (None, 12)]);
        // No such table or column: nothing, and no panic.
        each(&connection, "SELECT * FROM nowhere", |_| panic!("no rows expected"));
        each(&connection, "SELECT nope FROM t", |_| panic!("no rows expected"));
        assert!(columns(&connection, "t").contains("b"));
        assert!(columns(&connection, "nowhere").is_empty());
    }
}
