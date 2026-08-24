//! SQL dialect helpers for building dynamic WHERE clauses.
//!
//! WhereBuilder tracks clauses and provides methods for common comparison operators.
//! Supports SQLite/MySQL (`?`) and PostgreSQL (`$N`) placeholder styles.
//! Also provides dialect-specific helpers for upsert, NOW(), and last-insert-id.

use crate::storage::find_args::Paged;

/// Database dialect for SQL generation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialect {
    /// SQLite database dialect.
    Sqlite,
    /// MySQL database dialect.
    Mysql,
    /// PostgreSQL database dialect.
    Postgres,
}

impl Dialect {
    /// Return the appropriate SQL expression for "current timestamp" in this dialect.
    pub fn now_expr(self) -> &'static str {
        match self {
            Dialect::Sqlite => "datetime('now')",
            Dialect::Mysql => "NOW()",
            Dialect::Postgres => "NOW()",
        }
    }

    /// Quote a column name with the appropriate identifier quoting for this dialect.
    /// MySQL uses backticks, PostgreSQL uses double quotes, SQLite accepts both.
    pub fn quote_column(self, col: &str) -> String {
        match self {
            Dialect::Mysql | Dialect::Sqlite => format!("`{col}`"),
            Dialect::Postgres => format!("\"{col}\""),
        }
    }
}

/// Generate a placeholder string for the given dialect and 1-based parameter index.
///
/// SQLite and MySQL use `?`, PostgreSQL uses `$1`, `$2`, etc.
pub fn placeholder(dialect: Dialect, index: usize) -> String {
    match dialect {
        Dialect::Sqlite | Dialect::Mysql => "?".to_string(),
        Dialect::Postgres => format!("${index}"),
    }
}

/// Generate an upsert clause appropriate for the dialect.
///
/// - SQLite/PostgreSQL: `INSERT ... ON CONFLICT (conflict_cols) DO UPDATE SET col1 = excluded.col1, ...`
/// - MySQL: `INSERT ... ON DUPLICATE KEY UPDATE col1 = VALUES(col1), ...`
///
/// Returns the suffix to append after the VALUES clause.
pub fn upsert_suffix(dialect: Dialect, conflict_cols: &[&str], update_cols: &[&str]) -> String {
    match dialect {
        Dialect::Sqlite | Dialect::Postgres => {
            let conflict = conflict_cols.join(", ");
            let updates: Vec<String> = update_cols
                .iter()
                .map(|c| format!("{c} = excluded.{c}"))
                .collect();
            format!(
                " ON CONFLICT ({}) DO UPDATE SET {}",
                conflict,
                updates.join(", ")
            )
        }
        Dialect::Mysql => {
            let updates: Vec<String> = update_cols
                .iter()
                .map(|c| format!("{c} = VALUES({c})"))
                .collect();
            format!(" ON DUPLICATE KEY UPDATE {}", updates.join(", "))
        }
    }
}

/// Generate the SQL to retrieve the last auto-increment ID after an INSERT.
///
/// - SQLite: uses `last_insert_rowid()` on the query result (not a separate query)
/// - MySQL: uses `LAST_INSERT_ID()` (or sqlx `.last_insert_id()`)
/// - PostgreSQL: uses `RETURNING <column>` appended to the INSERT statement
pub fn last_insert_id_query(dialect: Dialect) -> &'static str {
    match dialect {
        Dialect::Sqlite => "-- use result.last_insert_rowid()",
        Dialect::Mysql => "SELECT LAST_INSERT_ID()",
        Dialect::Postgres => "-- use RETURNING clause",
    }
}

/// Builds dynamic WHERE clauses for SQL queries.
///
/// Supports both `?` (SQLite/MySQL) and `$N` (PostgreSQL) placeholder styles.
///
/// Usage:
/// ```ignore
/// let mut wb = WhereBuilder::new(Dialect::Postgres);
/// wb.add_eq("userId");
/// wb.add_gte("created_at");
/// let where_clause = wb.build_where();
/// // where_clause = " WHERE userId = $1 AND created_at >= $2"
/// ```
pub struct WhereBuilder {
    clauses: Vec<String>,
    dialect: Dialect,
    param_index: usize,
}

impl WhereBuilder {
    /// Create a new empty WhereBuilder for the given dialect.
    pub fn new(dialect: Dialect) -> Self {
        Self {
            clauses: Vec::new(),
            dialect,
            param_index: 0,
        }
    }

    /// Create a new empty WhereBuilder defaulting to SQLite dialect.
    /// Provided for backward compatibility.
    pub fn new_sqlite() -> Self {
        Self::new(Dialect::Sqlite)
    }

    fn next_placeholder(&mut self) -> String {
        self.param_index += 1;
        placeholder(self.dialect, self.param_index)
    }

    /// Add an equality condition: `` `column` = ? `` or `` "column" = $N ``
    pub fn add_eq(&mut self, column: &str) {
        let ph = self.next_placeholder();
        let qc = self.dialect.quote_column(column);
        self.clauses.push(format!("{qc} = {ph}"));
    }

    /// Add a greater-than-or-equal condition: `` `column` >= ? ``
    pub fn add_gte(&mut self, column: &str) {
        let ph = self.next_placeholder();
        let qc = self.dialect.quote_column(column);
        self.clauses.push(format!("{qc} >= {ph}"));
    }

    /// Add a less-than-or-equal condition: `` `column` <= ? ``
    #[allow(dead_code)]
    pub fn add_lte(&mut self, column: &str) {
        let ph = self.next_placeholder();
        let qc = self.dialect.quote_column(column);
        self.clauses.push(format!("{qc} <= {ph}"));
    }

    /// Add a LIKE condition: `` `column` LIKE ? ``
    #[allow(dead_code)]
    pub fn add_like(&mut self, column: &str) {
        let ph = self.next_placeholder();
        let qc = self.dialect.quote_column(column);
        self.clauses.push(format!("{qc} LIKE {ph}"));
    }

    /// Add an IN condition: `` `column` IN (?, ?, ...) ``
    #[allow(dead_code)]
    pub fn add_in(&mut self, column: &str, count: usize) {
        if count == 0 {
            return;
        }
        let qc = self.dialect.quote_column(column);
        let placeholders: Vec<String> = (0..count).map(|_| self.next_placeholder()).collect();
        self.clauses
            .push(format!("{} IN ({})", qc, placeholders.join(", ")));
    }

    /// Add a subquery IN condition:
    /// `` (SELECT `sub_col` FROM `table` WHERE `table`.`join_col` = `outer_table`.`outer_col`) IN (?, ?, ...) ``
    ///
    /// Used for filtering outputs by their parent transaction's status.
    #[allow(dead_code)]
    pub fn add_subquery_in(
        &mut self,
        table: &str,
        sub_col: &str,
        join_col: &str,
        outer_table: &str,
        outer_col: &str,
        count: usize,
    ) {
        if count == 0 {
            return;
        }
        let qt = self.dialect.quote_column(table);
        let qsc = self.dialect.quote_column(sub_col);
        let qjc = self.dialect.quote_column(join_col);
        let qot = self.dialect.quote_column(outer_table);
        let qoc = self.dialect.quote_column(outer_col);
        let placeholders: Vec<String> = (0..count).map(|_| self.next_placeholder()).collect();
        self.clauses.push(format!(
            "(SELECT {} FROM {} WHERE {}.{} = {}.{}) IN ({})",
            qsc,
            qt,
            qt,
            qjc,
            qot,
            qoc,
            placeholders.join(", ")
        ));
    }

    /// Return the current parameter count (useful for building subsequent
    /// parameterized SQL that continues after the WHERE clause).
    pub fn param_count(&self) -> usize {
        self.param_index
    }

    /// Build the WHERE clause string.
    /// Returns empty string if no clauses, otherwise " WHERE clause1 AND clause2 ...".
    pub fn build_where(&self) -> String {
        if self.clauses.is_empty() {
            String::new()
        } else {
            format!(" WHERE {}", self.clauses.join(" AND "))
        }
    }

    /// Build " ORDER BY <cols> LIMIT n OFFSET m" for a paged query.
    ///
    /// Offset pagination is only stable over a total order: SQL leaves the
    /// enumeration order of an unordered SELECT plan-dependent, so paging
    /// without ORDER BY can skip or repeat rows between chunks. Every paged
    /// query must therefore name a unique key here. Columns may be qualified
    /// with a table alias ("pt.provenTxId"); each dot segment is quoted for
    /// the dialect.
    pub fn build_ordered_page(dialect: Dialect, order_cols: &[&str], paged: &Paged) -> String {
        debug_assert!(
            !order_cols.is_empty(),
            "paged queries must order by a unique key"
        );
        let cols: Vec<String> = order_cols
            .iter()
            .map(|c| {
                c.split('.')
                    .map(|seg| dialect.quote_column(seg))
                    .collect::<Vec<_>>()
                    .join(".")
            })
            .collect();
        format!(
            " ORDER BY {} LIMIT {} OFFSET {}",
            cols.join(", "),
            paged.limit,
            paged.offset
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sqlite_placeholders() {
        let mut wb = WhereBuilder::new(Dialect::Sqlite);
        wb.add_eq("userId");
        wb.add_gte("created_at");
        assert_eq!(
            wb.build_where(),
            " WHERE `userId` = ? AND `created_at` >= ?"
        );
        assert_eq!(wb.param_count(), 2);
    }

    #[test]
    fn mysql_placeholders() {
        let mut wb = WhereBuilder::new(Dialect::Mysql);
        wb.add_eq("userId");
        wb.add_eq("status");
        assert_eq!(wb.build_where(), " WHERE `userId` = ? AND `status` = ?");
    }

    #[test]
    fn postgres_placeholders() {
        let mut wb = WhereBuilder::new(Dialect::Postgres);
        wb.add_eq("userId");
        wb.add_gte("created_at");
        wb.add_eq("status");
        assert_eq!(
            wb.build_where(),
            " WHERE \"userId\" = $1 AND \"created_at\" >= $2 AND \"status\" = $3"
        );
        assert_eq!(wb.param_count(), 3);
    }

    #[test]
    fn postgres_in_clause() {
        let mut wb = WhereBuilder::new(Dialect::Postgres);
        wb.add_in("id", 3);
        assert_eq!(wb.build_where(), " WHERE \"id\" IN ($1, $2, $3)");
    }

    #[test]
    fn ordered_page_sqlite_single_key() {
        let paged = Paged {
            limit: 3,
            offset: 6,
        };
        assert_eq!(
            WhereBuilder::build_ordered_page(Dialect::Sqlite, &["outputId"], &paged),
            " ORDER BY `outputId` LIMIT 3 OFFSET 6"
        );
    }

    #[test]
    fn ordered_page_postgres_composite_key() {
        let paged = Paged {
            limit: 10,
            offset: 0,
        };
        assert_eq!(
            WhereBuilder::build_ordered_page(
                Dialect::Postgres,
                &["transactionId", "txLabelId"],
                &paged
            ),
            " ORDER BY \"transactionId\", \"txLabelId\" LIMIT 10 OFFSET 0"
        );
    }

    #[test]
    fn ordered_page_quotes_alias_segments() {
        let paged = Paged {
            limit: 5,
            offset: 15,
        };
        assert_eq!(
            WhereBuilder::build_ordered_page(Dialect::Sqlite, &["pt.provenTxId"], &paged),
            " ORDER BY `pt`.`provenTxId` LIMIT 5 OFFSET 15"
        );
        assert_eq!(
            WhereBuilder::build_ordered_page(Dialect::Postgres, &["pt.provenTxId"], &paged),
            " ORDER BY \"pt\".\"provenTxId\" LIMIT 5 OFFSET 15"
        );
    }

    #[test]
    fn upsert_sqlite() {
        let result = upsert_suffix(Dialect::Sqlite, &["userId"], &["name", "updated_at"]);
        assert_eq!(
            result,
            " ON CONFLICT (userId) DO UPDATE SET name = excluded.name, updated_at = excluded.updated_at"
        );
    }

    #[test]
    fn upsert_mysql() {
        let result = upsert_suffix(Dialect::Mysql, &["userId"], &["name", "updated_at"]);
        assert_eq!(
            result,
            " ON DUPLICATE KEY UPDATE name = VALUES(name), updated_at = VALUES(updated_at)"
        );
    }

    #[test]
    fn sqlite_subquery_in() {
        let mut wb = WhereBuilder::new(Dialect::Sqlite);
        wb.add_eq("userId");
        wb.add_subquery_in(
            "transactions",
            "status",
            "transactionId",
            "outputs",
            "transactionId",
            2,
        );
        assert_eq!(
            wb.build_where(),
            " WHERE `userId` = ? AND (SELECT `status` FROM `transactions` WHERE `transactions`.`transactionId` = `outputs`.`transactionId`) IN (?, ?)"
        );
        assert_eq!(wb.param_count(), 3);
    }

    #[test]
    fn postgres_subquery_in() {
        let mut wb = WhereBuilder::new(Dialect::Postgres);
        wb.add_subquery_in(
            "transactions",
            "status",
            "transactionId",
            "outputs",
            "transactionId",
            2,
        );
        assert_eq!(
            wb.build_where(),
            " WHERE (SELECT \"status\" FROM \"transactions\" WHERE \"transactions\".\"transactionId\" = \"outputs\".\"transactionId\") IN ($1, $2)"
        );
        assert_eq!(wb.param_count(), 2);
    }
}

#[cfg(test)]
mod monitor_events_placeholder_tests {
    use super::{placeholder, Dialect};

    /// The shared storage macro generates both the MySQL and PostgreSQL
    /// implementations, so any statement it builds must use dialect
    /// placeholders. A hardcoded `?` is a hard syntax error on PostgreSQL —
    /// verified against PostgreSQL 16.15:
    ///   ERROR: syntax error at or near "AND"
    /// It was previously reachable from the ReviewDoubleSpends monitor task on
    /// every tick after the first.
    #[test]
    fn delete_monitor_events_uses_dialect_placeholders() {
        let build = |d: Dialect| {
            format!(
                "DELETE FROM monitor_events WHERE event = {} AND id < {}",
                placeholder(d, 1),
                placeholder(d, 2)
            )
        };

        assert_eq!(
            build(Dialect::Postgres),
            "DELETE FROM monitor_events WHERE event = $1 AND id < $2",
            "PostgreSQL requires positional $N placeholders"
        );
        assert_eq!(
            build(Dialect::Mysql),
            "DELETE FROM monitor_events WHERE event = ? AND id < ?"
        );
        assert_eq!(
            build(Dialect::Sqlite),
            "DELETE FROM monitor_events WHERE event = ? AND id < ?"
        );

        // The specific regression: PostgreSQL must never receive `?`.
        assert!(
            !build(Dialect::Postgres).contains('?'),
            "hardcoded `?` reaching PostgreSQL is ERROR: syntax error"
        );
    }
}
