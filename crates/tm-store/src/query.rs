//! Compiles a resolved filter to one parameterized query over the projection.
//! Every field and set term is an `IN` or `NOT IN` subquery, the form that
//! stays correct for negation and will compose under `OR`.

use rusqlite::types::Value as Sql;
use tm_core::filter::{Condition, Literal, Resolved, Sort, SortTarget, Target};

/// The collation every store connection registers for comparing people.
pub(crate) const FOLD: &str = "tm_fold";

/// SQL and its parameters, built side by side so placeholders stay in step.
#[derive(Default)]
struct Builder {
    sql: String,
    params: Vec<Sql>,
}

impl Builder {
    fn push(&mut self, sql: &str) {
        self.sql.push_str(sql);
    }

    fn param(&mut self, value: Sql) {
        self.params.push(value);
        self.sql.push('?');
    }

    /// `(?, ?, …)` for `values`.
    fn list(&mut self, values: &[Literal]) {
        self.push("(");
        for (i, value) in values.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            self.param(sql_value(value));
        }
        self.push(")");
    }

    /// `(VALUES (?, ?), …)` for (type, Status) pairs.
    fn pairs(&mut self, pairs: &[(String, String)]) {
        self.push("(VALUES ");
        for (i, (issue_type, status)) in pairs.iter().enumerate() {
            if i > 0 {
                self.push(", ");
            }
            self.push("(");
            self.param(Sql::Text(issue_type.clone()));
            self.push(", ");
            self.param(Sql::Text(status.clone()));
            self.push(")");
        }
        self.push(")");
    }
}

fn sql_value(value: &Literal) -> Sql {
    match value {
        Literal::Text(text) | Literal::Name(text) => Sql::Text(text.clone()),
        Literal::Number(number) => Sql::Real(*number),
        Literal::Integer(integer) => Sql::Integer(*integer),
        Literal::Issue(id) => Sql::Blob(id.as_bytes().to_vec()),
    }
}

/// The query for `resolved`: matching Issue ids in the requested order, with
/// creation and id breaking ties so pages are stable.
pub(crate) fn compile(resolved: &Resolved) -> (String, Vec<Sql>) {
    let mut b = Builder::default();
    b.push("SELECT id FROM issues WHERE ");
    if resolved.archived_named {
        b.push("1");
    } else {
        b.push("archived = 0");
    }
    for condition in &resolved.conditions {
        b.push(" AND ");
        condition_sql(&mut b, condition);
    }
    b.push(" ORDER BY ");
    if resolved.sorts.is_empty() {
        b.push("updated_ms DESC, ");
    }
    for sort in &resolved.sorts {
        sort_sql(&mut b, sort);
        b.push(", ");
    }
    b.push("created_ms, id");
    (b.sql, b.params)
}

/// The ready queue's ids: `todo`'s Issues, unarchived and held by no Claim
/// unlapsed at `now_ms`, that no Issue matching `open` blocks, most urgent
/// first and then oldest.
pub(crate) fn ready(
    todo: &Resolved,
    open: &Resolved,
    blocks: &str,
    now_ms: i64,
) -> (String, Vec<Sql>) {
    let mut b = Builder::default();
    b.push("SELECT id FROM issues WHERE archived = 0");
    for condition in &todo.conditions {
        b.push(" AND ");
        condition_sql(&mut b, condition);
    }
    b.push(" AND id NOT IN (SELECT issue_id FROM claims WHERE expires_ms IS NULL OR expires_ms > ");
    b.param(Sql::Integer(now_ms));
    b.push(")");
    b.push(" AND id NOT IN (SELECT dst FROM links WHERE type = ");
    b.param(Sql::Text(blocks.to_owned()));
    b.push(" AND src IN ");
    open_ids(&mut b, open);
    b.push(") ORDER BY priority DESC, created_ms, id");
    (b.sql, b.params)
}

/// `blocks` Links whose both ends match `open`, as (src, dst). The unary `+`
/// keeps the ends out of index lookups, so the `blocks` Links are scanned
/// once; otherwise the planner probes every pair of open Issues.
pub(crate) fn open_blocks(open: &Resolved, blocks: &str) -> (String, Vec<Sql>) {
    let mut b = Builder::default();
    b.push("SELECT src, dst FROM links WHERE type = ");
    b.param(Sql::Text(blocks.to_owned()));
    b.push(" AND +src IN ");
    open_ids(&mut b, open);
    b.push(" AND +dst IN ");
    open_ids(&mut b, open);
    (b.sql, b.params)
}

/// `blocks` Links whose blocker is open, whatever the Issue it blocks.
pub(crate) fn blocks_from_open(open: &Resolved, blocks: &str) -> (String, Vec<Sql>) {
    let mut b = Builder::default();
    b.push("SELECT src, dst FROM links WHERE type = ");
    b.param(Sql::Text(blocks.to_owned()));
    b.push(" AND +src IN ");
    open_ids(&mut b, open);
    (b.sql, b.params)
}

/// `(SELECT id FROM issues WHERE …)`: unarchived Issues matching `open`. Its
/// own scope, so the conditions' bare column names read this `issues`.
fn open_ids(b: &mut Builder, open: &Resolved) {
    b.push("(SELECT id FROM issues WHERE archived = 0");
    for condition in &open.conditions {
        b.push(" AND ");
        condition_sql(b, condition);
    }
    b.push(")");
}

fn condition_sql(b: &mut Builder, condition: &Condition) {
    match condition {
        Condition::Match {
            target,
            values,
            negated,
        } => {
            let collate = if matches!(values.first(), Some(Literal::Name(_))) {
                format!(" COLLATE {FOLD}")
            } else {
                String::new()
            };
            match target {
                // A NULL column fails both IN and NOT IN, so an unset value is
                // matched explicitly by negation.
                Target::Column(column) if *negated => {
                    b.push(&format!("({column} IS NULL OR {column}{collate} NOT IN "));
                    b.list(values);
                    b.push(")");
                }
                Target::Column(column) | Target::IntColumn(column) => {
                    b.push(&format!(
                        "{column}{collate} {}IN ",
                        if *negated { "NOT " } else { "" }
                    ));
                    b.list(values);
                }
                Target::SchemeValue(field) => {
                    let column = match values.first() {
                        Some(Literal::Text(_) | Literal::Name(_)) => "txt",
                        _ => "num",
                    };
                    member_of(b, "field_values", field, *negated, |b| {
                        b.push(&format!(" AND {column}{collate} IN "));
                        b.list(values);
                    });
                }
                Target::Set(field) => member_of(b, "set_members", field, *negated, |b| {
                    b.push(" AND elem IN ");
                    b.list(values);
                }),
            }
        }
        Condition::Present { target, present } => match target {
            Target::Column(column) => {
                b.push(&format!(
                    "{column} IS {}NULL",
                    if *present { "NOT " } else { "" }
                ));
            }
            // Only priority reaches here, whose rank 0 (`none`) is its unset value.
            Target::IntColumn(column) => {
                b.push(&format!("{column} {} 0", if *present { "<>" } else { "=" }));
            }
            Target::SchemeValue(field) => member_of(b, "field_values", field, !present, |_| {}),
            Target::Set(field) => member_of(b, "set_members", field, !present, |_| {}),
        },
        Condition::Category {
            pairs,
            known,
            include_unknown,
            negated,
        } => {
            // coalesce: an Issue with no Status has no category, so it is
            // outside every category rather than unknown to the comparison.
            b.push(if *negated {
                "NOT coalesce("
            } else {
                "coalesce("
            });
            if pairs.is_empty() {
                b.push("0");
            } else {
                b.push("(type, status) IN ");
                b.pairs(pairs);
            }
            if *include_unknown && known.is_empty() {
                b.push(" OR status IS NOT NULL");
            } else if *include_unknown {
                b.push(" OR (status IS NOT NULL AND (type IS NULL OR (type, status) NOT IN ");
                b.pairs(known);
                b.push("))");
            }
            b.push(", 0)");
        }
    }
}

/// `id [NOT] IN (SELECT issue_id FROM table WHERE field = ? …)`. Both tables
/// declare `issue_id NOT NULL`, which `NOT IN` needs to be correct.
fn member_of(
    b: &mut Builder,
    table: &str,
    field: &str,
    negated: bool,
    rest: impl FnOnce(&mut Builder),
) {
    b.push(&format!(
        "id {}IN (SELECT issue_id FROM {table} WHERE field = ",
        if negated { "NOT " } else { "" }
    ));
    b.param(Sql::Text(field.to_owned()));
    rest(b);
    b.push(")");
}

fn sort_sql(b: &mut Builder, sort: &Sort) {
    let direction = if sort.descending { "DESC" } else { "ASC" };
    let value = |b: &mut Builder, column: &str, field: &str| {
        b.push(&format!(
            "(SELECT {column} FROM field_values WHERE issue_id = issues.id AND field = "
        ));
        b.param(Sql::Text(field.to_owned()));
        b.push(")");
    };
    // Unset values sort last in either direction.
    match &sort.target {
        SortTarget::Column(column) => {
            b.push(&format!("{column} IS NULL, {column} {direction}"));
        }
        SortTarget::Rank(column) => {
            b.push(&format!("{column} = 0, {column} {direction}"));
        }
        SortTarget::Name(column) => {
            b.push(&format!(
                "{column} IS NULL, {column} COLLATE {FOLD} {direction}"
            ));
        }
        SortTarget::SchemeNumber(field)
        | SortTarget::SchemeText(field)
        | SortTarget::SchemeName(field) => {
            let (column, collate) = match sort.target {
                SortTarget::SchemeNumber(_) => ("num", String::new()),
                SortTarget::SchemeName(_) => ("txt", format!(" COLLATE {FOLD}")),
                _ => ("txt", String::new()),
            };
            value(b, column, field);
            b.push(" IS NULL, ");
            value(b, column, field);
            b.push(&format!("{collate} {direction}"));
        }
        SortTarget::SchemeOrder { field, order } => {
            value(b, "txt", field);
            b.push(" IS NULL");
            // A CASE needs at least one WHEN; with no options there is
            // nothing to order by beyond whether a value is set.
            if !order.is_empty() {
                b.push(", CASE ");
                value(b, "txt", field);
                for (rank, option) in order.iter().enumerate() {
                    b.push(" WHEN ");
                    b.param(Sql::Text(option.clone()));
                    b.push(&format!(" THEN {rank}"));
                }
                b.push(&format!(" ELSE {} END {direction}", order.len()));
            }
        }
    }
}
