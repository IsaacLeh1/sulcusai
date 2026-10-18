// SPDX-License-Identifier: AGPL-3.0-only
//! A small JavaScript sandbox (QuickJS) for calculate and run_code: no
//! network, no way to change files, a time and memory limit. It can print,
//! read text files the caller allows, and draw charts.

use std::cell::RefCell;
use std::rc::Rc;
use std::time::{Duration, Instant};

use rquickjs::{Context, Function, Runtime};
use serde_json::Value;

const TIME_LIMIT: Duration = Duration::from_secs(10);
const MEMORY_LIMIT: usize = 256 * 1024 * 1024;
const OUTPUT_CHARS: usize = 12_000;
const CHARTS: usize = 4;

/// What the code did.
#[derive(Debug, Default)]
pub struct Ran {
    /// What it printed.
    pub output: String,
    /// Its last value, shown as text (None when it was undefined).
    pub value: Option<String>,
    pub error: Option<String>,
    /// Charts it drew (see `chart_of`).
    pub charts: Vec<Value>,
}

/// Helpers every run has (as plain global properties, so the code may
/// declare names of its own like `sum` without clashing).
const PRELUDE: &str = r#"(function () {
  var g = globalThis;
  ['abs','acos','asin','atan','atan2','cbrt','ceil','cos','cosh','exp','floor','hypot','log','log2','log10','max','min','pow','random','round','sign','sin','sinh','sqrt','tan','tanh','trunc','PI','E'].forEach(function (k) { g[k] = Math[k]; });
  g.ln = Math.log;
  g.sum = function (a) { return a.reduce(function (s, x) { return s + Number(x); }, 0); };
  g.mean = g.average = function (a) { return a.length ? g.sum(a) / a.length : NaN; };
  g.median = function (a) { var s = a.map(Number).sort(function (x, y) { return x - y; }), m = s.length >> 1; return s.length % 2 ? s[m] : (s[m - 1] + s[m]) / 2; };
  g.stdev = function (a) { var m = g.mean(a); return Math.sqrt(g.sum(a.map(function (x) { return (x - m) * (x - m); })) / (a.length - 1)); };
  g.factorial = function (n) { var r = 1; for (var i = 2; i <= n; i++) r *= i; return r; };
  g.round2 = function (x, d) { var p = Math.pow(10, d === undefined ? 2 : d); return Math.round(x * p) / p; };
  g.daysBetween = function (a, b) { return Math.round((new Date(b) - new Date(a)) / 86400000); };
  g.addDays = function (d, n) { var t = new Date(d); t.setDate(t.getDate() + n); return t.toDateString(); };
  // Compound interest: principal, yearly rate (5 = 5%), years, times a year.
  g.compound = function (p, rate, years, n) { n = n || 12; return p * Math.pow(1 + rate / 100 / n, n * years); };
  // A loan's monthly payment: principal, yearly rate (6.5 = 6.5%), years.
  g.payment = function (p, rate, years) { var r = rate / 100 / 12, n = years * 12; return r ? p * r / (1 - Math.pow(1 + r, -n)) : p / n; };
  g.parseCSV = function (text) {
    var rows = [], row = [], cell = '', q = false;
    for (var i = 0; i < text.length; i++) {
      var c = text[i];
      if (q) { if (c === '"') { if (text[i + 1] === '"') { cell += '"'; i++; } else q = false; } else cell += c; }
      else if (c === '"') q = true;
      else if (c === ',') { row.push(cell); cell = ''; }
      else if (c === '\n' || c === '\r') { if (c === '\r' && text[i + 1] === '\n') i++; row.push(cell); cell = ''; if (row.length > 1 || row[0] !== '') rows.push(row); row = []; }
      else cell += c;
    }
    if (cell !== '' || row.length) { row.push(cell); rows.push(row); }
    var head = rows.shift() || [];
    return rows.map(function (r) { var o = {}; head.forEach(function (h, j) { var v = r[j] === undefined ? '' : r[j], n = Number(v); o[h.trim()] = v.trim() !== '' && !isNaN(n) ? n : v; }); return o; });
  };
  g.readFile = function (path) { var r = JSON.parse(g.__read(String(path))); if (!r.ok) throw new Error(r.error); return r.text; };
  g.readCSV = function (path) { return g.parseCSV(g.readFile(path)); };
  g.readJSON = function (path) { return JSON.parse(g.readFile(path)); };
  var num = function (x) {
    if (Number.isInteger(x)) return String(x);
    var s = Number(x.toPrecision(12)).toString();
    return s;
  };
  var fmt = function (v, depth) {
    if (v === undefined) return 'undefined';
    if (typeof v === 'number') return num(v);
    if (typeof v === 'string') return v;
    if (typeof v === 'bigint') return v.toString();
    if (typeof v === 'function') return '[function ' + (v.name || '') + ']';
    if (v instanceof Date) return isNaN(v) ? 'Invalid Date' : v.toString();
    if (v instanceof Error) return v.name + ': ' + v.message;
    try { return JSON.stringify(v, function (k, x) { return typeof x === 'number' && !Number.isInteger(x) ? Number(x.toPrecision(12)) : typeof x === 'bigint' ? x.toString() : x; }, 2); } catch (e) { return String(v); }
  };
  g.__fmt = fmt;
  var print = function () { g.__out([].map.call(arguments, function (a) { return fmt(a); }).join(' ')); };
  g.print = print;
  g.console = { log: print, info: print, warn: print, error: print, debug: print, table: function (rows) { g.__out(fmt(rows)); } };
  // A chart in the chat: chart({ type: 'bar' | 'line' | 'area' | 'pie' | 'scatter', title, labels, series: [{ name, data }] })
  // or chart({ type, title, labels, values }).
  g.chart = function (spec) { return g.__chart(JSON.stringify(spec)); };
  g.__run = function (src) {
    try {
      var v = (0, eval)(src);
      return JSON.stringify({ ok: true, value: v === undefined ? null : fmt(v) });
    } catch (e) {
      var at = e && e.stack ? /<input>:(\d+)/.exec(String(e.stack)) : null;
      return JSON.stringify({ ok: false, error: (e && e.name ? e.name + ': ' + e.message : String(e)) + (at ? ' (line ' + at[1] + ')' : '') });
    }
  };
})();"#;

/// A chart spec checked and trimmed to what the chat draws.
pub fn chart_of(raw: &str) -> Result<Value, String> {
    let v: Value = serde_json::from_str(raw).map_err(|_| "chart() takes an object.".to_string())?;
    let kind = v.get("type").and_then(Value::as_str).unwrap_or("bar").to_ascii_lowercase();
    if !matches!(kind.as_str(), "bar" | "line" | "area" | "pie" | "scatter") {
        return Err(format!("chart type {kind:?} isn't one of bar, line, area, pie, scatter."));
    }
    let text = |k: &str| v.get(k).and_then(Value::as_str).map(|s| s.chars().take(80).collect::<String>());
    let labels: Vec<String> = v.get("labels").and_then(Value::as_array).map(|a| a.iter().take(500).map(|l| match l { Value::String(s) => s.chars().take(40).collect(), other => other.to_string() }).collect()).unwrap_or_default();
    let nums = |a: &Value| -> Vec<Value> { a.as_array().map(|a| a.iter().take(2000).map(|x| x.as_f64().filter(|f| f.is_finite()).map_or(Value::Null, |f| serde_json::json!(f))).collect()).unwrap_or_default() };
    let mut series: Vec<Value> = Vec::new();
    if kind == "scatter" {
        // Points as [x, y] pairs or {x, y}.
        let pts = |a: &Value| -> Vec<Value> {
            a.as_array().map(|a| a.iter().take(2000).filter_map(|p| {
                let (x, y) = match p { Value::Array(xy) if xy.len() >= 2 => (xy[0].as_f64()?, xy[1].as_f64()?), Value::Object(_) => (p.get("x")?.as_f64()?, p.get("y")?.as_f64()?), _ => return None };
                (x.is_finite() && y.is_finite()).then(|| serde_json::json!([x, y]))
            }).collect()).unwrap_or_default()
        };
        if let Some(list) = v.get("series").and_then(Value::as_array) {
            for s in list.iter().take(6) {
                series.push(serde_json::json!({ "name": s.get("name").and_then(Value::as_str).unwrap_or(""), "points": pts(s.get("data").or_else(|| s.get("points")).unwrap_or(&Value::Null)) }));
            }
        } else if let Some(p) = v.get("points").or_else(|| v.get("data")) {
            series.push(serde_json::json!({ "name": "", "points": pts(p) }));
        }
        if series.iter().all(|s| s["points"].as_array().is_none_or(Vec::is_empty)) {
            return Err("A scatter chart needs points: series: [{ data: [[x, y], ...] }].".into());
        }
    } else {
        if let Some(list) = v.get("series").and_then(Value::as_array) {
            for s in list.iter().take(6) {
                let data = match s { Value::Array(_) => nums(s), _ => nums(s.get("data").or_else(|| s.get("values")).unwrap_or(&Value::Null)) };
                series.push(serde_json::json!({ "name": s.get("name").and_then(Value::as_str).unwrap_or(""), "data": data }));
            }
        } else if let Some(d) = v.get("values").or_else(|| v.get("data")) {
            series.push(serde_json::json!({ "name": text("name").unwrap_or_default(), "data": nums(d) }));
        }
        if series.iter().all(|s| s["data"].as_array().is_none_or(Vec::is_empty)) {
            return Err("A chart needs numbers: labels: [...] and values: [...] (or series: [{ name, data }]).".into());
        }
    }
    Ok(serde_json::json!({
        "type": kind, "title": text("title").unwrap_or_default(), "x_label": text("x_label").or_else(|| text("xLabel")).unwrap_or_default(),
        "y_label": text("y_label").or_else(|| text("yLabel")).unwrap_or_default(), "labels": labels, "series": series,
    }))
}

/// Runs JavaScript. `read` gives a file's text by the path the code names (or says why not).
pub fn run(code: &str, read: impl Fn(&str) -> Result<String, String> + 'static) -> Ran {
    let mut ran = Ran::default();
    let rt = match Runtime::new() {
        Ok(rt) => rt,
        Err(e) => {
            ran.error = Some(format!("Couldn't start JavaScript: {e}"));
            return ran;
        }
    };
    rt.set_memory_limit(MEMORY_LIMIT);
    rt.set_max_stack_size(1024 * 1024);
    let deadline = Instant::now() + TIME_LIMIT;
    rt.set_interrupt_handler(Some(Box::new(move || Instant::now() > deadline)));
    let ctx = match Context::full(&rt) {
        Ok(c) => c,
        Err(e) => {
            ran.error = Some(format!("Couldn't start JavaScript: {e}"));
            return ran;
        }
    };
    let out = Rc::new(RefCell::new(String::new()));
    let charts: Rc<RefCell<Vec<Value>>> = Rc::default();
    let result: Result<String, String> = ctx.with(|ctx| {
        let g = ctx.globals();
        let o = out.clone();
        let print = Function::new(ctx.clone(), move |line: String| {
            let mut o = o.borrow_mut();
            if o.chars().count() < OUTPUT_CHARS {
                o.push_str(&line);
                o.push('\n');
            }
        });
        let c = charts.clone();
        let chart = Function::new(ctx.clone(), move |spec: String| -> String {
            let mut c = c.borrow_mut();
            if c.len() >= CHARTS {
                return format!("Only {CHARTS} charts a run.");
            }
            match chart_of(&spec) {
                Ok(v) => {
                    c.push(v);
                    format!("Chart {} drawn.", c.len())
                }
                Err(e) => format!("No chart: {e}"),
            }
        });
        let file = Function::new(ctx.clone(), move |path: String| -> String {
            match read(&path) {
                Ok(text) => serde_json::json!({ "ok": true, "text": text }).to_string(),
                Err(e) => serde_json::json!({ "ok": false, "error": e }).to_string(),
            }
        });
        let setup = (|| -> rquickjs::Result<()> {
            g.set("__out", print?)?;
            g.set("__chart", chart?)?;
            g.set("__read", file?)?;
            ctx.eval::<(), _>(PRELUDE)?;
            Ok(())
        })();
        if let Err(e) = setup {
            return Err(format!("Couldn't start JavaScript: {e}"));
        }
        let runner: Function = g.get("__run").map_err(|e| e.to_string())?;
        match runner.call::<_, String>((code.to_string(),)) {
            Ok(s) => Ok(s),
            Err(_) if Instant::now() > deadline => Err(format!("It ran longer than {} seconds and was stopped (an endless loop?).", TIME_LIMIT.as_secs())),
            Err(e) => {
                let caught = ctx.catch();
                let msg = caught.as_object().and_then(|o| o.get::<_, String>("message").ok()).unwrap_or_else(|| e.to_string());
                Err(msg)
            }
        }
    });
    // Promises the code started (there's no network, so they settle at once).
    while rt.is_job_pending() && Instant::now() < deadline {
        if rt.execute_pending_job().is_err() {
            break;
        }
    }
    ran.output = out.borrow().trim_end().to_string();
    ran.charts = charts.borrow().clone();
    match result.and_then(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string())) {
        Ok(v) if v["ok"] == true => ran.value = v["value"].as_str().map(str::to_string),
        Ok(v) => ran.error = Some(v["error"].as_str().unwrap_or("It failed.").to_string()),
        Err(e) => ran.error = Some(e),
    }
    ran
}

/// A math expression the way people write it, as JavaScript: ^ for powers,
/// × ÷ −, thousands separators, "15% of 80".
pub fn as_js(expr: &str) -> String {
    let mut s = expr.trim().trim_end_matches(['=', '?']).trim().to_string();
    for (from, to) in [("×", "*"), ("÷", "/"), ("−", "-"), ("–", "-"), ("^", "**"), ("√", "sqrt")] {
        s = s.replace(from, to);
    }
    let thousands = regex_lite(r"(\d),(\d{3})\b");
    while thousands.is_match(&s) {
        s = thousands.replace_all(&s, "$1$2").into_owned();
    }
    s = regex_lite(r"(?i)(\d+(?:\.\d+)?)\s*(?:%|percent)\s*of\s*").replace_all(&s, "($1/100)*").into_owned();
    s = regex_lite(r"(\d+(?:\.\d+)?)\s*%").replace_all(&s, "($1/100)").into_owned();
    s = regex_lite(r"(\d)\s*[xX]\s*(\d)").replace_all(&s, "$1*$2").into_owned();
    s
}

fn regex_lite(p: &str) -> regex::Regex {
    regex::Regex::new(p).unwrap()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn value(code: &str) -> String {
        let r = run(code, |p| if p == "data.csv" { Ok("item,price,qty\n\"Pens, blue\",2.5,4\nPaper,10,3\n".into()) } else { Err(format!("{p} isn't in a shared folder.")) });
        assert_eq!(r.error, None, "{code}");
        r.value.unwrap_or_default()
    }

    #[test]
    fn math_dates_and_text_come_out_exact() {
        assert_eq!(value("0.1 + 0.2"), "0.3");
        assert_eq!(value("payment(300000, 6.5, 30)"), "1896.20407048");
        assert_eq!(value("daysBetween('2026-10-17', '2026-12-25')"), "69");
        assert_eq!(value("'strawberry'.split('').filter(c => c === 'r').length"), "3");
        // Its own names may match the helpers'.
        assert_eq!(value("const sum = 5; sum + 1"), "6");
        assert_eq!(value("readCSV('data.csv').map(r => r.price * r.qty)"), "[\n  10,\n  30\n]");
        assert_eq!(value("readCSV('data.csv')[0].item"), "Pens, blue");
        for (said, js, out) in [("2^10", "2**10", "1024"), ("15% of 80", "(15/100)*80", "12"), ("1,234,567 × 2", "1234567 * 2", "2469134"), ("√(16) + 3 x 4", "sqrt(16) + 3*4", "16")] {
            assert_eq!(as_js(said), js);
            assert_eq!(value(js), out);
        }
    }

    #[test]
    fn prints_charts_and_errors() {
        let r = run("for (let i = 1; i <= 3; i++) console.log('row', i, { n: i / 3 }); chart({ type: 'bar', title: 'Sales', labels: ['Jan', 'Feb'], values: [3, 5] })", |_| Err("no".into()));
        assert!(r.output.starts_with("row 1 {\n  \"n\": 0.333333333333\n}"), "{}", r.output);
        assert_eq!(r.charts.len(), 1);
        assert_eq!(r.charts[0]["series"][0]["data"], serde_json::json!([3.0, 5.0]));
        let r = run("undefinedThing + 1", |_| Err("no".into()));
        assert_eq!(r.error.as_deref(), Some("ReferenceError: undefinedThing is not defined (line 1)"));
        let r = run("readFile('../secret.txt')", |p| Err(format!("{p} isn't in a shared folder.")));
        assert_eq!(r.error.as_deref(), Some("Error: ../secret.txt isn't in a shared folder. (line 1)"));
        assert!(chart_of(r#"{"type":"radar","values":[1]}"#).is_err());
        assert!(chart_of(r#"{"type":"line","labels":["a"]}"#).is_err(), "no numbers");
    }

    #[test]
    fn endless_or_greedy_code_is_stopped() {
        let r = run("let a = []; while (true) a.push('x'.repeat(1000000));", |_| Err("no".into()));
        assert!(r.error.unwrap().contains("out of memory"));
        let started = Instant::now();
        let r = run("while (true) {}", |_| Err("no".into()));
        assert!(r.error.unwrap().contains("stopped"));
        assert!(started.elapsed() < TIME_LIMIT + Duration::from_secs(3));
    }
}
