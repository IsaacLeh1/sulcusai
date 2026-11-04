// SPDX-License-Identifier: AGPL-3.0-only
//! Spreadsheet analysis with a plan the person approves first: the plan lists
//! each step in words, and only an approved plan runs. Steps only read the
//! table; results go to a new sheet, never over the original cells.

use serde::{Deserialize, Serialize};

#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
#[serde(tag = "step", rename_all = "snake_case")]
pub enum Step {
    /// Count, sum, average, smallest and largest of a numeric column.
    Summary { column: String },
    /// Totals of one column for each value of another (a pivot).
    GroupTotal { by: String, column: String },
    /// Rows where a column holds a value, any case.
    Filter { column: String, contains: String },
}

#[derive(Serialize, Debug, PartialEq)]
pub struct Table {
    pub headers: Vec<String>,
    pub rows: Vec<Vec<String>>,
}

impl Step {
    pub fn describe(&self) -> String {
        match self {
            Step::Summary { column } => format!("Count, total, average, smallest and largest of \"{column}\""),
            Step::GroupTotal { by, column } => format!("Total of \"{column}\" for each \"{by}\""),
            Step::Filter { column, contains } => format!("Rows where \"{column}\" contains \"{contains}\""),
        }
    }
}

/// The plan as words, with any step naming a column the table doesn't have.
pub fn check_plan(headers: &[String], steps: &[Step]) -> Result<Vec<String>, String> {
    let has = |c: &str| headers.iter().any(|h| h.eq_ignore_ascii_case(c));
    for s in steps {
        let cols: Vec<&str> = match s {
            Step::Summary { column } | Step::Filter { column, .. } => vec![column],
            Step::GroupTotal { by, column } => vec![by, column],
        };
        if let Some(c) = cols.into_iter().find(|c| !has(c)) {
            return Err(format!("The table has no column called \"{c}\"."));
        }
    }
    Ok(steps.iter().map(Step::describe).collect())
}

fn col(headers: &[String], name: &str) -> usize {
    headers.iter().position(|h| h.eq_ignore_ascii_case(name)).unwrap_or(0)
}

fn num(s: &str) -> Option<f64> {
    s.trim().trim_start_matches(['$', '€', '£']).replace(',', "").parse().ok()
}

fn fmt(x: f64) -> String {
    let r = (x * 100.0).round() / 100.0;
    if r.fract() == 0.0 {
        format!("{}", r as i64)
    } else {
        format!("{r}")
    }
}

/// Runs an approved plan; each step gives one result table.
pub fn run(headers: &[String], rows: &[Vec<String>], steps: &[Step]) -> Result<Vec<(String, Table)>, String> {
    check_plan(headers, steps)?;
    let cell = |r: &Vec<String>, i: usize| r.get(i).cloned().unwrap_or_default();
    Ok(steps
        .iter()
        .map(|s| {
            let t = match s {
                Step::Summary { column } => {
                    let i = col(headers, column);
                    let v: Vec<f64> = rows.iter().filter_map(|r| num(&cell(r, i))).collect();
                    let sum: f64 = v.iter().sum();
                    let row = if v.is_empty() {
                        vec!["0".into(), "0".into(), "".into(), "".into(), "".into()]
                    } else {
                        vec![
                            v.len().to_string(),
                            fmt(sum),
                            fmt(sum / v.len() as f64),
                            fmt(v.iter().cloned().fold(f64::INFINITY, f64::min)),
                            fmt(v.iter().cloned().fold(f64::NEG_INFINITY, f64::max)),
                        ]
                    };
                    Table { headers: ["Count", "Total", "Average", "Smallest", "Largest"].map(String::from).to_vec(), rows: vec![row] }
                }
                Step::GroupTotal { by, column } => {
                    let (b, c) = (col(headers, by), col(headers, column));
                    let mut totals: Vec<(String, f64)> = vec![];
                    for r in rows {
                        let k = cell(r, b);
                        let x = num(&cell(r, c)).unwrap_or(0.0);
                        match totals.iter_mut().find(|(n, _)| *n == k) {
                            Some(t) => t.1 += x,
                            None => totals.push((k, x)),
                        }
                    }
                    totals.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                    Table { headers: vec![by.clone(), format!("Total {column}")], rows: totals.into_iter().map(|(k, x)| vec![k, fmt(x)]).collect() }
                }
                Step::Filter { column, contains } => {
                    let i = col(headers, column);
                    let needle = contains.to_lowercase();
                    Table { headers: headers.to_vec(), rows: rows.iter().filter(|r| cell(r, i).to_lowercase().contains(&needle)).cloned().collect() }
                }
            };
            (s.describe(), t)
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn data() -> (Vec<String>, Vec<Vec<String>>) {
        let h = vec!["Item".into(), "Room".into(), "Cost".into()];
        let r = vec![
            vec!["Sofa".into(), "Living".into(), "$1,200".into()],
            vec!["Lamp".into(), "Living".into(), "40.5".into()],
            vec!["Bed".into(), "Bedroom".into(), "800".into()],
        ];
        (h, r)
    }

    #[test]
    fn a_plan_names_missing_columns_before_running() {
        let (h, _) = data();
        assert!(check_plan(&h, &[Step::Summary { column: "Price".into() }]).unwrap_err().contains("Price"));
        assert_eq!(check_plan(&h, &[Step::Summary { column: "cost".into() }]).unwrap().len(), 1);
    }

    #[test]
    fn steps_compute() {
        let (h, r) = data();
        let out = run(
            &h,
            &r,
            &[
                Step::Summary { column: "Cost".into() },
                Step::GroupTotal { by: "Room".into(), column: "Cost".into() },
                Step::Filter { column: "Item".into(), contains: "LA".into() },
            ],
        )
        .unwrap();
        assert_eq!(out[0].1.rows[0], vec!["3", "2040.5", "680.17", "40.5", "1200"]);
        assert_eq!(out[1].1.rows[0], vec!["Living", "1240.5"]);
        assert_eq!(out[2].1.rows.len(), 1);
        let json = serde_json::to_string(&Step::GroupTotal { by: "a".into(), column: "b".into() }).unwrap();
        assert!(json.contains("\"step\":\"group_total\""));
    }
}
