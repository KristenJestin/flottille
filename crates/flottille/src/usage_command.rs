//! `flottille usage`: what agents used, in weighted tokens, and the cost of a task that ends
//! merged (`docs/cli.md`, `docs/usage.md`).

use flottille_core::paths::Paths;
use jiff::Zoned;
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
    let today = Zoned::now().date();
    let since = first_day(since.unwrap_or("30d"), today)?;
    let stats = Client::connect_or_start(paths)?.request(
        "usage.stats",
        json!({"project": project, "since": since.to_string(), "until": today.to_string(),
            "by": by}),
    )?;
    if json {
        println!("{stats}");
    } else {
        print!("{}", usage_text(&stats));
    }
    Ok(())
}

/// The first day of `--since`: a date, or `N` days ending today.
fn first_day(since: &str, today: Date) -> Result<Date> {
    if let Some(days) = since.strip_suffix('d')
        && let Ok(days) = days.parse::<i64>()
        && days >= 1
    {
        return Ok(today.checked_sub(jiff::Span::new().days(days - 1))?);
    }
    since.parse::<Date>().map_err(|_| {
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
            "Cost per merged task: {} ({} merged, {} failed).\n",
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
            "Tasks of projects without a forge: {} ({}), their outcome not known.\n",
            count("no_forge"),
            tokens(tasks["no_forge"]["weighted"].as_f64().unwrap_or(0.0)),
        ));
    }
    out.push_str(&format!(
        "\nTotal: {}, {} of it cache reads, {} turns",
        tokens(total["weighted"].as_f64().unwrap_or(0.0)),
        share(&total["cache_read_share"]),
        total["turns"],
    ));
    match total["missing"].as_u64().unwrap_or(0) {
        0 => out.push_str(".\n"),
        missing => out.push_str(&format!(" ({missing} missing a count, taken as 0).\n")),
    }
    out.push_str(&format!("\nBy {}:\n", stats["by"].as_str().unwrap_or("?")));
    for group in stats["groups"].as_array().into_iter().flatten() {
        let key = match &group["key"] {
            Value::String(key) if key.is_empty() => "(none)".to_owned(),
            Value::String(key) => key.clone(),
            _ => "(unknown)".to_owned(),
        };
        out.push_str(&format!(
            "  {key:<24} {:>8}  {:>4} cache reads",
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
    fn since_takes_a_date_or_a_number_of_days_ending_today() {
        let today: Date = "2026-11-15".parse().unwrap();
        assert_eq!(first_day("7d", today).unwrap().to_string(), "2026-11-09");
        assert_eq!(first_day("1d", today).unwrap(), today);
        assert_eq!(
            first_day("2026-10-01", today).unwrap().to_string(),
            "2026-10-01"
        );
        assert!(first_day("0d", today).is_err());
        assert!(first_day("last week", today).is_err());
    }

    #[test]
    fn the_text_says_the_cost_per_merged_task_then_each_group() {
        let task = |count: u64, weighted: f64| json!({"count": count, "weighted": weighted});
        let stats = json!({
            "project": null, "since": "2026-11-09", "until": "2026-11-15", "by": "tuning",
            "total": {"weighted": 4_500_000.0, "cache_read_share": 0.38, "turns": 120,
                "missing": 3, "tasks": {"merged": task(3, 3_000_000.0), "failed": task(1, 600_000.0),
                "in_progress": task(2, 12_300.0), "no_forge": task(1, 950.0), "main": 900.0,
                "cost_per_merged_task": 1_200_000.0}},
            "groups": [
                {"key": "", "weighted": 4_000_000.0, "cache_read_share": 0.4,
                    "tasks": {"cost_per_merged_task": 1_300_000.0}},
                {"key": "unchanged", "weighted": 500_000.0, "cache_read_share": 0.2,
                    "tasks": {"cost_per_merged_task": null}}
            ]
        });

        let text = usage_text(&stats);

        assert_eq!(
            text,
            "Usage of every project from 2026-11-09 to 2026-11-15, in weighted tokens \
             (input + 1.25 × cache write + 0.1 × cache read + 5 × output).\n\
             \n\
             Cost per merged task: 1.2M (3 merged, 1 failed).\n\
             Left out of it: 2 in progress (12.3k), the turns in main (900).\n\
             Tasks of projects without a forge: 1 (950), their outcome not known.\n\
             \n\
             Total: 4.5M, 38% of it cache reads, 120 turns (3 missing a count, taken as 0).\n\
             \n\
             By tuning:\n\
             \x20 (none)                       4.0M   40% cache reads  1.3M per merged task\n\
             \x20 unchanged                  500.0k   20% cache reads\n"
        );
    }
}
