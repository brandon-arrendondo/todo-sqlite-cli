use std::path::Path;

use chrono::{Duration, NaiveDate, Utc};
use rusqlite::params;
use serde::Serialize;
use serde_json::json;

use crate::db;
use crate::error::{system, user, CliResult};

#[derive(Serialize)]
struct Bucket {
    date: String,
    backlog: i64,
    in_progress: i64,
    done: i64,
    rejected: i64,
}

pub fn run(
    db_path: &Path,
    json_flag: bool,
    since: Option<&str>,
    until: Option<&str>,
    bucket: &str,
    fmt: &str,
) -> CliResult<()> {
    let conn = db::open(db_path)?;
    if !db::is_initialized(&conn) {
        return Err(user(
            "database is not initialized; run `todo-sqlite-cli init` first",
        ));
    }

    let step_days: i64 = match bucket {
        "day" => 1,
        "week" => 7,
        other => {
            return Err(user(format!(
                "invalid --bucket '{other}' (expected day|week)"
            )))
        }
    };

    let since_date = match since {
        Some(s) => parse_date_only(s)?,
        None => {
            let earliest: Option<String> = conn
                .query_row("SELECT MIN(created_at) FROM tasks", [], |r| r.get(0))
                .map_err(|e| system(format!("query failed: {e}")))?;
            match earliest {
                Some(s) => parse_date_only(s.split('T').next().unwrap_or(&s))?,
                None => Utc::now().date_naive(),
            }
        }
    };
    let until_date = match until {
        Some(s) => parse_date_only(s)?,
        None => Utc::now().date_naive(),
    };

    if since_date > until_date {
        return Err(user(format!(
            "--since ({since_date}) must not be after --until ({until_date})"
        )));
    }

    // Bucket boundary dates: since_date, since_date+step, ... always ending
    // on until_date even when the step doesn't land on it exactly.
    let mut dates: Vec<NaiveDate> = Vec::new();
    let mut d = since_date;
    while d < until_date {
        dates.push(d);
        d += Duration::days(step_days);
    }
    dates.push(until_date);

    let mut stmt = conn
        .prepare(
            "SELECT \
               SUM(CASE WHEN created_at <= ?1 AND (started_at IS NULL OR started_at > ?1) AND (completed_at IS NULL OR completed_at > ?1) THEN 1 ELSE 0 END), \
               SUM(CASE WHEN started_at IS NOT NULL AND started_at <= ?1 AND (completed_at IS NULL OR completed_at > ?1) THEN 1 ELSE 0 END), \
               SUM(CASE WHEN completed_at IS NOT NULL AND completed_at <= ?1 AND status = 'done' THEN 1 ELSE 0 END), \
               SUM(CASE WHEN completed_at IS NOT NULL AND completed_at <= ?1 AND status = 'rejected' THEN 1 ELSE 0 END) \
             FROM tasks WHERE created_at <= ?1",
        )
        .map_err(|e| system(format!("prepare failed: {e}")))?;

    let mut buckets: Vec<Bucket> = Vec::new();
    for d in &dates {
        // End-of-day boundary. Stored timestamps are RFC3339 with a 'Z'
        // suffix at second precision, so lexicographic string comparison
        // against this boundary matches chronological comparison.
        let boundary = format!("{d}T23:59:59Z");
        let (backlog, in_progress, done, rejected): (
            Option<i64>,
            Option<i64>,
            Option<i64>,
            Option<i64>,
        ) = stmt
            .query_row(params![boundary], |r| {
                Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?))
            })
            .map_err(|e| system(format!("query failed: {e}")))?;
        buckets.push(Bucket {
            date: d.to_string(),
            backlog: backlog.unwrap_or(0),
            in_progress: in_progress.unwrap_or(0),
            done: done.unwrap_or(0),
            rejected: rejected.unwrap_or(0),
        });
    }

    // `--format` is the source of truth; `--json` is shorthand for `json`
    // when `--format` is left at its default (mirrors `list`'s convention).
    let effective_fmt = if fmt != "ascii" {
        fmt
    } else if json_flag {
        "json"
    } else {
        "ascii"
    };

    match effective_fmt {
        "ascii" => print_ascii(&buckets),
        "csv" => print_csv(&buckets),
        "json" => {
            let v = json!({ "buckets": buckets });
            println!("{}", serde_json::to_string(&v).unwrap());
        }
        other => {
            return Err(user(format!(
                "invalid --format '{other}' (expected ascii|csv|json)"
            )))
        }
    }
    Ok(())
}

fn parse_date_only(s: &str) -> CliResult<NaiveDate> {
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        return Ok(d);
    }
    if let Ok(dt) = chrono::DateTime::parse_from_rfc3339(s) {
        return Ok(dt.with_timezone(&Utc).date_naive());
    }
    Err(user(format!(
        "invalid date '{s}' (expected YYYY-MM-DD or RFC3339)"
    )))
}

fn print_ascii(buckets: &[Bucket]) {
    for b in buckets {
        println!(
            "{}  backlog={}  in_progress={}  done={}  rejected={}",
            b.date, b.backlog, b.in_progress, b.done, b.rejected
        );
    }
}

fn print_csv(buckets: &[Bucket]) {
    println!("date,backlog,in_progress,done,rejected");
    for b in buckets {
        println!(
            "{},{},{},{},{}",
            b.date, b.backlog, b.in_progress, b.done, b.rejected
        );
    }
}
