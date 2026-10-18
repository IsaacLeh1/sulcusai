// SPDX-License-Identifier: AGPL-3.0-only
//! calculate and run_code: exact math and a JavaScript sandbox, the way
//! ChatGPT's data analysis and Claude's analysis tool work, so a model never
//! does arithmetic, dates, counting or data in its head. The code runs on
//! this PC in QuickJS (`crate::jsrun`): no network, no changing files; it can
//! read text files in the shared folders, print, and draw charts that appear
//! in the chat.

use serde_json::{json, Value};

use super::{Ctx, Outcome};

pub fn calculate_params() -> Value {
    json!({ "type": "object", "required": ["expression"], "properties": {
        "expression": { "type": "string", "description": "The math to work out, e.g. \"(1250 * 0.07) / 12\", \"2^10\", \"15% of 2345\", \"sqrt(2) * PI\" or \"daysBetween('2026-10-17', '2026-12-25')\"." } } })
}

pub fn run_code_params() -> Value {
    json!({ "type": "object", "required": ["code"], "properties": {
        "code": { "type": "string", "description": "JavaScript to run. The last expression's value comes back, and so does anything you console.log. \
            Helpers: sum, mean, median, stdev, round2(x, digits), daysBetween(a, b), addDays(date, n), compound(principal, ratePercent, years), \
            payment(principal, ratePercent, years), readFile(path), readCSV(path) (rows as objects), readJSON(path), and \
            chart({ type: 'bar'|'line'|'area'|'pie'|'scatter', title, labels, values }) or chart({ type, title, labels, series: [{ name, data }] }) \
            to show a chart in the chat. No network." } } })
}

fn reader(ctx: &Ctx<'_>) -> impl Fn(&str) -> Result<String, String> + Send + 'static {
    let sandbox = ctx.sandbox.clone();
    move |path: &str| {
        if sandbox.is_empty() {
            return Err("No folders are shared, so there are no files to read.".into());
        }
        let p = sandbox.resolve(path)?;
        let meta = std::fs::metadata(&p).map_err(|_| format!("{path} doesn't exist."))?;
        if meta.len() > 10 * 1024 * 1024 {
            return Err(format!("{path} is over 10 MB."));
        }
        std::fs::read_to_string(&p).map_err(|_| format!("{path} isn't a text file (for Word, Excel or PDF files use read_document)."))
    }
}

pub async fn run(name: &str, args: &Value, ctx: &Ctx<'_>) -> Outcome {
    let read = reader(ctx);
    if name == "calculate" {
        let expr = args.get("expression").and_then(Value::as_str).map(str::trim).unwrap_or_default().to_string();
        if expr.is_empty() {
            return Outcome::error("Calculate", "Give the math to work out (expression).");
        }
        let js = crate::jsrun::as_js(&expr);
        let ran = tokio::task::spawn_blocking(move || crate::jsrun::run(&js, read)).await.unwrap_or_default();
        return match (ran.error, ran.value) {
            (None, Some(v)) => {
                let shown: String = expr.chars().take(80).collect();
                Outcome::ok(format!("{expr} = {v}\nUse this exact result in your answer."), format!("Calculated {shown} = {v}"), "text", Some(format!("{expr} = {v}")))
            }
            (Some(e), _) => Outcome::error("Couldn't calculate that", format!("{e}\nWrite it as math with numbers and + - * / ^ ( ), or use run_code for anything longer.")),
            (None, None) => Outcome::error("Couldn't calculate that", "It gave no result. Write it as one expression, e.g. (12 * 4) / 3."),
        };
    }
    let code = args.get("code").and_then(Value::as_str).unwrap_or_default().to_string();
    if code.trim().is_empty() {
        return Outcome::error("Run code", "Give the JavaScript to run (code).");
    }
    let ran = tokio::task::spawn_blocking(move || crate::jsrun::run(&code, read)).await.unwrap_or_default();
    let mut parts: Vec<String> = Vec::new();
    if !ran.output.is_empty() {
        parts.push(format!("Printed:\n{}", ran.output));
    }
    if let Some(v) = &ran.value {
        parts.push(format!("Result: {v}"));
    }
    if !ran.charts.is_empty() {
        parts.push(format!("{} chart{} shown in the chat; don't describe how to draw it.", ran.charts.len(), if ran.charts.len() == 1 { " is" } else { "s are" }));
    }
    let mut o = match &ran.error {
        Some(e) => {
            parts.push(format!("Error: {e}"));
            Outcome::error("The code stopped with an error", format!("{}\nFix the code and run it again.", parts.join("\n\n")))
        }
        None if parts.is_empty() => Outcome::ok("It ran and gave nothing back: end with the value you want, or console.log it.", "Ran code", "text", None),
        None => {
            let detail = parts.join("\n\n");
            Outcome::ok(format!("{detail}\n\nUse these exact results in your answer."), if ran.charts.is_empty() { "Ran code".to_string() } else { "Ran code and drew a chart".to_string() }, "text", Some(detail))
        }
    };
    if !ran.charts.is_empty() {
        o.meta["charts"] = json!(ran.charts);
    }
    o
}
