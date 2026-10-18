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
/// Most of an error message kept (the prelude keeps 2,000, then the line).
const ERROR_CHARS: usize = 2_100;
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
  // Math in x: an expression as text ('x**2 - 5*x + 6') or a function.
  var fn = function (expr, v) { return typeof expr === 'function' ? expr : new Function(v || 'x', 'return (' + expr + ');'); };
  var tidy = function (r) { r = Number(r.toPrecision(10)); return Math.abs(r) < 1e-10 ? 0 : r; };
  // The real values of v that make expr zero (sign changes and touching roots, in -1e6..1e6).
  g.solve = function (expr, v) {
    var f = fn(expr, v), roots = [];
    var add = function (r) { r = tidy(r); if (!roots.some(function (q) { return Math.abs(q - r) <= 1e-7 * Math.max(1, Math.abs(r)); })) roots.push(r); };
    var bisect = function (a, b, fa) {
      for (var i = 0; i < 200; i++) {
        var m = (a + b) / 2, fm = f(m);
        if (fm === 0 || b - a < 1e-14 * Math.max(1, Math.abs(m))) return m;
        if ((fm < 0) === (fa < 0)) { a = m; fa = fm; } else b = m;
      }
      return (a + b) / 2;
    };
    var scan = function (lo, hi, n) {
      var step = (hi - lo) / n, x0 = lo, f0 = f(x0);
      for (var i = 1; i <= n; i++) {
        var x1 = lo + i * step, f1 = f(x1);
        if (isFinite(f0) && isFinite(f1)) {
          if (Math.abs(f0) < 1e-12) add(x0);
          else if ((f0 < 0) !== (f1 < 0) && Math.abs(f1) > 1e-12) { var r = bisect(x0, x1, f0); if (Math.abs(f(r)) < 1e-6 * Math.max(1, Math.abs(f0), Math.abs(f1))) add(r); }
        }
        x0 = x1; f0 = f1;
      }
      if (isFinite(f0) && Math.abs(f0) < 1e-12) add(x0);
    };
    scan(-10, 10, 20000); scan(-1000, 1000, 200000); scan(-1e6, 1e6, 200000);
    roots.sort(function (a, b) { return a - b; });
    return roots.length ? roots : 'no real solution';
  };
  // The area under expr from a to b (Simpson's rule).
  g.integrate = function (expr, a, b, v) {
    var f = fn(expr, v), n = 20000, h = (b - a) / n, s = f(a) + f(b);
    for (var i = 1; i < n; i++) s += f(a + i * h) * (i % 2 ? 4 : 2);
    return tidy(s * h / 3);
  };
  // The slope of expr at a point.
  g.derivative = function (expr, at, v) {
    var f = fn(expr, v), h = 1e-4 * Math.max(1, Math.abs(at));
    return tidy((f(at - 2 * h) - 8 * f(at - h) + 8 * f(at + h) - f(at + 2 * h)) / (12 * h));
  };
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
  // Text going back to the app is cut here first (it keeps 12,000 characters;
  // a character may take two units here), so a huge string isn't copied out whole.
  var clip = function (s, max) {
    s = String(s);
    max = max || 24002;
    if (s.length <= max) return s;
    s = s.slice(0, max);
    var c = s.charCodeAt(s.length - 1);
    return (c >= 0xD800 && c <= 0xDBFF ? s.slice(0, -1) : s) + '…';
  };
  var print = function () { g.__out(clip([].map.call(arguments, function (a) { return fmt(a); }).join(' '))); };
  g.print = print;
  g.console = { log: print, info: print, warn: print, error: print, debug: print, table: function (rows) { g.__out(clip(fmt(rows))); } };
  // A chart in the chat: chart({ type: 'bar' | 'line' | 'area' | 'pie' | 'scatter', title, labels, series: [{ name, data }] })
  // or chart({ type, title, labels, values }).
  g.chart = function (spec) { return g.__chart(JSON.stringify(spec)); };
  g.__run = function (src) {
    try {
      var v = (0, eval)(src);
      return JSON.stringify({ ok: true, value: v === undefined ? null : clip(fmt(v)) });
    } catch (e) {
      var at = e && e.stack ? /<input>:(\d+)/.exec(String(e.stack)) : null;
      return JSON.stringify({ ok: false, error: clip(e && e.name ? e.name + ': ' + e.message : String(e), 2000) + (at ? ' (line ' + at[1] + ')' : '') });
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
    let out: Rc<RefCell<Printed>> = Rc::default();
    let charts: Rc<RefCell<Vec<Value>>> = Rc::default();
    let result: Result<String, String> = ctx.with(|ctx| {
        let g = ctx.globals();
        let o = out.clone();
        let print = Function::new(ctx.clone(), move |line: String| o.borrow_mut().push(&line));
        let c = charts.clone();
        let chart = Function::new(ctx.clone(), move |spec: String| -> String {
            let mut c = c.borrow_mut();
            if c.len() >= CHARTS {
                return format!("Only {CHARTS} charts a run.");
            }
            match chart_of(&spec) {
                Ok(v) => {
                    c.push(v);
                    // Not "Chart 1": models wrote "[Chart 1]" into their reply.
                    "It's shown in the chat.".to_string()
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
    ran.output = out.borrow().text.trim_end().to_string();
    ran.charts = charts.borrow().clone();
    match result.and_then(|s| serde_json::from_str::<Value>(&s).map_err(|e| e.to_string())) {
        Ok(v) if v["ok"] == true => ran.value = v["value"].as_str().map(|s| clip(s, OUTPUT_CHARS)),
        Ok(v) => ran.error = Some(clip(v["error"].as_str().unwrap_or("It failed."), ERROR_CHARS)),
        Err(e) => ran.error = Some(clip(&e, ERROR_CHARS)),
    }
    ran
}

/// What the code printed: up to OUTPUT_CHARS in all. Each print is cut to
/// what's left, so one huge print (or very many) never piles up.
#[derive(Default)]
struct Printed {
    text: String,
    chars: usize,
    cut: bool,
}

impl Printed {
    fn push(&mut self, line: &str) {
        if self.cut {
            return;
        }
        let room = OUTPUT_CHARS.saturating_sub(self.chars);
        if room == 0 || line.chars().nth(room).is_some() {
            self.text.extend(line.chars().take(room));
            self.text.push_str("\n[The rest of the output was cut.]");
            self.cut = true;
        } else {
            self.text.push_str(line);
            self.text.push('\n');
            self.chars += line.chars().count() + 1;
        }
    }
}

/// Text cut to `max` characters, saying so.
fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((at, _)) => format!("{}… [cut: it was longer than {max} characters]", &s[..at]),
        None => s.to_string(),
    }
}

/// A math expression the way people write it, as JavaScript: ^ for powers,
/// × ÷ −, thousands separators, "15% of 80".
pub fn as_js(expr: &str) -> String {
    let mut s = expr.trim().trim_end_matches(['=', '?']).trim().to_string();
    for (from, to) in [("×", "*"), ("÷", "/"), ("−", "-"), ("–", "-"), ("^", "**"), ("√", "sqrt")] {
        s = s.replace(from, to);
    }
    s = without_thousands(&s);
    s = regex_lite(r"(?i)(\d+(?:\.\d+)?)\s*(?:%|percent)\s*of\s*").replace_all(&s, "($1/100)*").into_owned();
    s = regex_lite(r"(\d+(?:\.\d+)?)\s*%").replace_all(&s, "($1/100)").into_owned();
    s = regex_lite(r"(\d)\s*[xX]\s*(\d)").replace_all(&s, "$1*$2").into_owned();
    // "integral of x^2 from 0 to 3", "derivative of x^3 at 2".
    if let Some(c) = regex_lite(r"(?i)^(?:what\s+is\s+)?(?:the\s+)?(?:definite\s+)?integral\s+of\s+(.+?)\s+from\s+(\S+)\s+to\s+(\S+?)\s*$").captures(&s) {
        s = format!("integrate({}, {}, {})", &c[1], &c[2], &c[3]);
    } else if let Some(c) = regex_lite(r"(?i)^(?:what\s+is\s+)?(?:the\s+)?derivative\s+of\s+(.+?)\s+at\s+(?:[a-z]\s*=\s*)?(\S+?)\s*$").captures(&s) {
        s = format!("derivative({}, {})", &c[1], &c[2]);
    }
    s = implied_times(&s);
    s = quote_first_arg(&s);
    equation(&s).unwrap_or(s)
}

/// "1,234,567" → "1234567", but not the commas between a function's
/// arguments: max(120,450) stays two numbers.
fn without_thousands(s: &str) -> String {
    let chars: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len());
    // For each open bracket: whether it's a function call's.
    let mut calls: Vec<bool> = Vec::new();
    for (i, &c) in chars.iter().enumerate() {
        match c {
            '(' | '[' => calls.push(c == '(' && i > 0 && (chars[i - 1].is_alphanumeric() || chars[i - 1] == '_')),
            ')' | ']' => {
                calls.pop();
            }
            ',' if !calls.last().copied().unwrap_or(false) => {
                let digit_before = i > 0 && chars[i - 1].is_ascii_digit();
                let three_after = chars.get(i + 1..i + 4).is_some_and(|d| d.iter().all(char::is_ascii_digit)) && !chars.get(i + 4).is_some_and(char::is_ascii_digit);
                if digit_before && three_after {
                    continue;
                }
            }
            _ => {}
        }
        out.push(c);
    }
    out
}

/// Names that aren't unknowns in an equation.
const KNOWN: &[&str] = &[
    "abs", "acos", "asin", "atan", "atan2", "cbrt", "ceil", "cos", "cosh", "exp", "floor", "hypot", "log", "log2", "log10", "max", "min", "pow", "random", "round", "sign", "sin", "sinh",
    "sqrt", "tan", "tanh", "trunc", "PI", "E", "ln", "Math", "Infinity", "NaN", "sum", "mean", "average", "median", "stdev", "factorial", "round2", "daysBetween", "addDays", "compound",
    "payment", "solve", "integrate", "derivative",
];

/// "5x" as 5*x, "2(3+4)" as 2*(3+4), ")(" as ")*(" — but not inside names
/// ("log10(") or numbers ("1e5"), or in quotes.
fn implied_times(s: &str) -> String {
    let c: Vec<char> = s.chars().collect();
    let mut out = String::with_capacity(s.len() + 8);
    let (mut quote, mut in_name, mut in_number) = (None::<char>, false, false);
    for i in 0..c.len() {
        let ch = c[i];
        out.push(ch);
        if let Some(q) = quote {
            if ch == q {
                quote = None;
            }
            continue;
        }
        if ch == '\'' || ch == '"' {
            quote = Some(ch);
            in_name = false;
            in_number = false;
            continue;
        }
        if in_name {
            in_name = ch.is_ascii_alphanumeric() || ch == '_';
        } else if ch.is_ascii_alphabetic() || ch == '_' {
            in_name = true;
        }
        in_number = !in_name && (ch.is_ascii_digit() || (ch == '.' && in_number));
        let Some(&next) = c.get(i + 1) else { continue };
        let sci = (next == 'e' || next == 'E') && c.get(i + 2).is_some_and(|d| d.is_ascii_digit() || ((*d == '-' || *d == '+') && c.get(i + 3).is_some_and(char::is_ascii_digit)));
        if ((in_number && !sci) || ch == ')') && (next.is_ascii_alphabetic() || next == '_' || next == '(') {
            out.push('*');
        }
    }
    out
}

/// solve(x^2 - 4, …), integrate(x^2, 0, 3): the expression as text, as the helpers take it.
fn quote_first_arg(s: &str) -> String {
    let re = regex_lite(r"\b(solve|integrate|derivative)\(\s*");
    let mut out = String::new();
    let mut rest = s;
    while let Some(m) = re.find(rest) {
        out.push_str(&rest[..m.end()]);
        rest = &rest[m.end()..];
        if rest.starts_with(['\'', '"']) {
            continue;
        }
        let mut depth = 0i32;
        let end = rest.char_indices().find(|&(_, ch)| match ch {
            '(' => { depth += 1; false }
            ')' if depth == 0 => true,
            ')' => { depth -= 1; false }
            ',' => depth == 0,
            _ => false,
        }).map_or(rest.len(), |(i, _)| i);
        let arg = rest[..end].trim();
        let arg = match split_equation(arg) {
            Some((l, r)) => format!("({l}) - ({r})"),
            None => arg.to_string(),
        };
        out.push_str(&format!("'{}'", arg.replace('\'', "")));
        rest = &rest[end..];
    }
    out.push_str(rest);
    out
}

/// "lhs = rhs" (one plain =, not ==, <=, >=, != or =>), outside quotes.
fn split_equation(s: &str) -> Option<(String, String)> {
    let c: Vec<char> = s.chars().collect();
    let mut quote = None::<char>;
    let mut at = Vec::new();
    for i in 0..c.len() {
        match (quote, c[i]) {
            (Some(q), ch) if ch == q => quote = None,
            (Some(_), _) => {}
            (None, '\'' | '"') => quote = Some(c[i]),
            (None, '=') => {
                let before = i.checked_sub(1).map(|j| c[j]);
                let after = c.get(i + 1).copied();
                if !matches!(before, Some('=' | '<' | '>' | '!')) && !matches!(after, Some('=' | '>')) {
                    at.push(i);
                }
            }
            _ => {}
        }
    }
    let [i] = at[..] else { return None };
    let (l, r): (String, String) = (c[..i].iter().collect(), c[i + 1..].iter().collect());
    (!l.trim().is_empty() && !r.trim().is_empty()).then(|| (l.trim().to_string(), r.trim().to_string()))
}

/// An equation in one unknown ("x**2 - 5*x + 6 = 0") as solve('…', 'x'); one
/// with no unknowns ("2+2 = 4") as a check.
fn equation(s: &str) -> Option<String> {
    let (l, r) = split_equation(s)?;
    let names = regex_lite(r"[A-Za-z_]\w*");
    let outside_quotes = regex_lite(r#"'[^']*'|"[^"]*""#).replace_all(s, "");
    let mut unknown: Vec<&str> = Vec::new();
    for m in names.find_iter(&outside_quotes) {
        let calls = outside_quotes[m.end()..].trim_start().starts_with('(');
        if !calls && !KNOWN.contains(&m.as_str()) && !unknown.contains(&m.as_str()) {
            unknown.push(m.as_str());
        }
    }
    match unknown[..] {
        [] => Some(format!("({l}) === ({r})")),
        [v] => Some(format!("solve('({}) - ({})', '{v}')", l.replace('\'', ""), r.replace('\'', ""))),
        _ => None,
    }
}

/// The unknown an expression from `as_js` solves for, if it's an equation.
pub fn solving_for(js: &str) -> Option<String> {
    regex_lite(r"^solve\('.*', '([A-Za-z_]\w*)'\)$").captures(js).map(|c| c[1].to_string())
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
        // A function's arguments aren't one big number; a grouped one still is.
        assert_eq!(without_thousands("max(120,450)"), "max(120,450)");
        assert_eq!(without_thousands("(2,000 + 3) * 1,500"), "(2000 + 3) * 1500");
        assert_eq!(without_thousands("round2(1,234.5,2)"), "round2(1,234.5,2)");
    }

    #[test]
    fn equations_integrals_and_derivatives() {
        let calc = |said: &str| {
            let js = as_js(said);
            let r = run(&js, |_| Err("no".into()));
            assert_eq!(r.error, None, "{said} → {js}");
            (js, r.value.unwrap_or_default())
        };
        assert_eq!(as_js("x^2 - 5x + 6 = 0"), "solve('(x**2 - 5*x + 6) - (0)', 'x')");
        assert_eq!(calc("x^2 - 5x + 6 = 0").1, "[\n  2,\n  3\n]");
        assert_eq!(solving_for(&as_js("x^2 - 5x + 6 = 0")).as_deref(), Some("x"));
        assert_eq!(calc("(x - 2)^2 = 0").1, "[\n  2\n]", "a touching root");
        assert_eq!(calc("2y + 3 = 11").1, "[\n  4\n]");
        assert_eq!(calc("x^2 + 1 = 0").1, "no real solution");
        assert_eq!(calc("solve(x^2 - 2 = 0)").1, "[\n  -1.414213562,\n  1.414213562\n]");
        assert_eq!(calc("integral of x^2 from 0 to 3").1, "9");
        assert_eq!(calc("integrate(sin(x), 0, PI)").1, "2");
        assert_eq!(calc("derivative of x^3 at 2").1, "12");
        assert_eq!(calc("2+2 = 4").1, "true");
        // Implied multiplication, but not inside names or numbers.
        assert_eq!(as_js("2(3+4)"), "2*(3+4)");
        assert_eq!(calc("log10(1000) + 1e3").1, "1003");
        assert_eq!(calc("3sqrt(16)").1, "12");
        assert_eq!(as_js("daysBetween('2026-10-17', '2026-12-25')"), "daysBetween('2026-10-17', '2026-12-25')");
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
    fn huge_prints_and_values_are_cut() {
        // One huge print, then many: the output stays small and says it was cut.
        let r = run("print('a'.repeat(5000000)); for (let i = 0; i < 100000; i++) print('more'); 'b'.repeat(3000000)", |_| Err("no".into()));
        assert_eq!(r.error, None);
        assert!(r.output.chars().count() < OUTPUT_CHARS + 100 && r.output.ends_with("[The rest of the output was cut.]"), "{}", r.output.len());
        let v = r.value.unwrap();
        assert!(v.chars().count() < OUTPUT_CHARS + 100 && v.contains("[cut:"), "{}", v.len());
        // Many small prints fill it exactly as before.
        let r = run("for (let i = 0; i < 3; i++) print('row', i)", |_| Err("no".into()));
        assert_eq!(r.output, "row 0\nrow 1\nrow 2");
        let r = run("throw new Error('x'.repeat(1000000))", |_| Err("no".into()));
        let e = r.error.unwrap();
        assert!(e.starts_with("Error: xxx") && e.chars().count() <= ERROR_CHARS + 60, "{}", e.len());
        let mut p = Printed::default();
        p.push(&"é".repeat(OUTPUT_CHARS));
        p.push("");
        assert!(p.cut && p.text.starts_with("éé"));
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
