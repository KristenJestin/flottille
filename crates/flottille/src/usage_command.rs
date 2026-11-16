//! `flottille usage`: what agents used, in weighted tokens, and the cost of a task that ends
//! merged (`docs/cli.md`, `docs/usage.md`).

use flottille_core::paths::Paths;
use jiff::civil::Date;
use serde_json::{Value, json};

use crate::client::Client;

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;

pub fn usage(
    paths: &Paths,
    project: Option<String>,
    since: Option<&str>,
    by: &str,
    json: bool,
) -> Result<()> {
    // The days are the daemon's: it says which day is today, not this command's time zone.
    let mut asked = json!({"project": project, "by": by});
    match period(since.unwrap_or("30d"))? {
        Period::Days(days) => asked["days"] = days.into(),
        Period::Since(date) => asked["since"] = date.to_string().into(),
    }
    let stats = Client::connect_or_start(paths)?.request("usage.stats", asked)?;
    if json {
        println!("{stats}");
    } else {
        print!("{}", usage_text(&stats));
    }
    Ok(())
}

#[derive(Debug, PartialEq)]
enum Period {
    /// The last days, today included.
    Days(u64),
    /// From this day to today.
    Since(Date),
}

/// What `--since` says: a date, or `N` days ending today.
fn period(since: &str) -> Result<Period> {
    if let Some(days) = since.strip_suffix('d')
        && let Ok(days) = days.parse::<u64>()
        && days >= 1
    {
        return Ok(Period::Days(days));
    }
    since.parse::<Date>().map(Period::Since).map_err(|_| {
        format!("--since takes a date (2026-11-01) or a number of days (30d), not `{since}`").into()
    })
}

/// A number of weighted tokens, short: `950`, `12.3k`, `4.5M`.
fn tokens(weighted: f64) -> String {
    match weighted {
        w if w >= 1_000_000.0 => format!("{:.1}M", w / 1_000_000.0),
        w if w >= 1_000.0 => format!("{:.1}k", w / 1_000.0),
        w => format!("{w:.0}"),
    }
}

fn share(value: &Value) -> String {
    format!("{:.0}%", value.as_f64().unwrap_or(0.0) * 100.0)
}

fn usage_text(stats: &Value) -> String {
    let total = &stats["total"];
    let tasks = &total["tasks"];
    let count = |outcome: &str| tasks[outcome]["count"].as_u64().unwrap_or(0);
    let mut out = format!(
        "Usage of {} from {} to {}, in weighted tokens \
         (input + 1.25 × cache write + 0.1 × cache read + 5 × output).\n\n",
        stats["project"]
            .as_str()
            .map_or("every project".to_owned(), |p| format!("project {p}")),
        stats["since"].as_str().unwrap_or("?"),
        stats["until"].as_str().unwrap_or("?"),
    );
    match tasks["cost_per_merged_task"].as_f64() {
        Some(cost) => out.push_str(&format!(
            "Cost per merged task: {} ({} merged, {} failed, finished in these days, each with \
             all of its turns).\n",
            tokens(cost),
            count("merged"),
            count("failed")
        )),
        None => out.push_str(&format!(
            "Cost per merged task: none merged ({} failed).\n",
            count("failed")
        )),
    }
    out.push_str(&format!(
        "Left out of it: {} in progress ({}), the turns in main ({}).\n",
        count("in_progress"),
        tokens(tasks["in_progress"]["weighted"].as_f64().unwrap_or(0.0)),
        tokens(tasks["main"].as_f64().unwrap_or(0.0)),
    ));
    if count("no_forge") > 0 {
        out.push_str(&format!(
            "Tasks without a forge: {} ({}), their outcome not known.\n",
            count("no_forge"),
            tokens(tasks["no_forge"]["weighted"].as_f64().unwrap_or(0.0)),
        ));
    }
    out.push_str(&format!(
        "\nTotal: {}, {} of the input read from the cache, {} rows (one per turn and model)",
        tokens(total["weighted"].as_f64().unwrap_or(0.0)),
        share(&total["cache_read_share"]),
        total["rows"],
    ));
    match total["missing"].as_u64().unwrap_or(0) {
        0 => out.push_str(".\n"),
        missing => out.push_str(&format!(" ({missing} missing a count, taken as 0).\n")),
    }
    let by = stats["by"][0].as_str().unwrap_or("?");
    out.push_str(&format!("\nBy {by}:\n"));
    for group in stats["groups"][by].as_array().into_iter().flatten() {
        let key = match &group["key"] {
            Value::String(key) if key.is_empty() => "(none)".to_owned(),
            Value::String(key) => key.clone(),
            _ => "(unknown)".to_owned(),
        };
        out.push_str(&format!(
            "  {key:<24} {:>8}  {:>4} from the cache",
            tokens(group["weighted"].as_f64().unwrap_or(0.0)),
            share(&group["cache_read_share"]),
        ));
        if let Some(cost) = group["tasks"]["cost_per_merged_task"].as_f64() {
            out.push_str(&format!("  {} per merged task", tokens(cost)));
        }
        out.push('\n');
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn since_takes_a_date_or_a_number_of_days_the_daemon_ends_today() {
        assert_eq!(period("7d").unwrap(), Period::Days(7));
        assert_eq!(
            period("2026-10-01").unwrap(),
            Period::Since("2026-10-01".parse().unwrap())
        );
        assert!(period("0d").is_err());
        assert!(period("last week").is_err());
    }

    #[test]
    fn the_text_says_the_cost_per_merged_task_then_each_group() {
        let task = |count: u64, weighted: f64| json!({"count": count, "weighted": weighted});
        let stats = json!({
            "project": null, "since": "2026-11-09", "until": "2026-11-15", "by": ["tuning"],
            "total": {"weighted": 4_500_000.0, "cache_read_share": 0.38, "rows": 120,
                "missing": 3, "tasks": {"merged": task(3, 3_000_000.0), "failed": task(1, 600_000.0),
                "in_progress": task(2, 12_300.0), "no_forge": task(1, 950.0), "main": 900.0,
                "cost_per_merged_task": 1_200_000.0}},
            "groups": {"tuning": [
                {"key": "", "weighted": 4_000_000.0, "cache_read_share": 0.4,
                    "tasks": {"cost_per_merged_task": 1_300_000.0}},
                {"key": "unchanged", "weighted": 500_000.0, "cache_read_share": 0.2,
                    "tasks": {"cost_per_merged_task": null}}
            ]}
        });

        let text = usage_text(&stats);

        assert_eq!(
            text,
            "Usage of every project from 2026-11-09 to 2026-11-15, in weighted tokens \
             (input + 1.25 × cache write + 0.1 × cache read + 5 × output).\n\
             \n\
             Cost per merged task: 1.2M (3 merged, 1 failed, finished in these days, each with \
             all of its turns).\n\
             Left out of it: 2 in progress (12.3k), the turns in main (900).\n\
             Tasks without a forge: 1 (950), their outcome not known.\n\
             \n\
             Total: 4.5M, 38% of the input read from the cache, 120 rows (one per turn and \
             model) (3 missing a count, taken as 0).\n\
             \n\
             By tuning:\n\
             \x20 (none)                       4.0M   40% from the cache  1.3M per merged task\n\
             \x20 unchanged                  500.0k   20% from the cache\n"
        );
    }
}
