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
            Dialect::Mysql | Dialect::Sqlite => format!("`{}`", col),
            Dialect::Postgres => format!("\"{}\"", col),
        }
    }
}

/// Generate a placeholder string for the given dialect and 1-based parameter index.
///
/// SQLite and MySQL use `?`, PostgreSQL uses `$1`, `$2`, etc.
pub fn placeholder(dialect: Dialect, index: usize) -> String {
    match dialect {
        Dialect::Sqlite | Dialect::Mysql => "?".to_string(),
        Dialect::Postgres => format!("${}", index),
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
        self.clauses.push(format!("{} = {}", qc, ph));
    }

    /// Add a greater-than-or-equal condition: `` `column` >= ? ``
    pub fn add_gte(&mut self, column: &str) {
        let ph = self.next_placeholder();
        let qc = self.dialect.quote_column(column);
        self.clauses.push(format!("{} >= {}", qc, ph));
    }

    /// Add a less-than-or-equal condition: `` `column` <= ? ``
    #[allow(dead_code)]
    pub fn add_lte(&mut self, column: &str) {
        let ph = self.next_placeholder();
        let qc = self.dialect.quote_column(column);
        self.clauses.push(format!("{} <= {}", qc, ph));
    }

    /// Add a LIKE condition: `` `column` LIKE ? ``
    #[allow(dead_code)]
    pub fn add_like(&mut self, column: &str) {
        let ph = self.next_placeholder();
        let qc = self.dialect.quote_column(column);
        self.clauses.push(format!("{} LIKE {}", qc, ph));
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

    /// Build ORDER BY clause. Note: column should be pre-quoted if needed,
    /// or use `dialect.quote_column()`.
    pub fn build_order_by(column: &str, desc: bool) -> String {
        if desc {
            format!(" ORDER BY {} DESC", column)
        } else {
            format!(" ORDER BY {} ASC", column)
        }
    }

    /// Build LIMIT/OFFSET clause from Paged.
    pub fn build_limit_offset(paged: &Paged) -> String {
        format!(" LIMIT {} OFFSET {}", paged.limit, paged.offset)
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
}
