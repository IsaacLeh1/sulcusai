// SPDX-License-Identifier: AGPL-3.0-only
//! Live market prices for market_quote: stocks, funds, indexes, crypto,
//! currencies and commodities, from Yahoo Finance's public chart data (no
//! account or key). Names people use ("bitcoin", "S&P 500", "gold",
//! "EUR/USD", "Apple") become Yahoo's symbols.

use chrono::{TimeZone, Utc};
use serde_json::{json, Value};

/// One symbol's price now, and over a range when asked.
#[derive(Debug, Clone, PartialEq)]
pub struct Quote {
    pub symbol: String,
    pub name: String,
    pub currency: String,
    pub price: f64,
    pub previous_close: Option<f64>,
    pub day_high: Option<f64>,
    pub day_low: Option<f64>,
    pub year_high: Option<f64>,
    pub year_low: Option<f64>,
    /// When the price is from (seconds since 1970).
    pub time: i64,
    pub exchange: String,
    /// (time, close) over the range asked for.
    pub history: Vec<(i64, f64)>,
}

/// Yahoo's symbol for what people call a market ("bitcoin" → BTC-USD).
pub fn symbol_for(asked: &str) -> String {
    let a = asked.trim();
    let low = a.to_ascii_lowercase();
    let known = match low.as_str() {
        "bitcoin" | "btc" => "BTC-USD",
        "ethereum" | "eth" | "ether" => "ETH-USD",
        "solana" | "sol" => "SOL-USD",
        "dogecoin" | "doge" => "DOGE-USD",
        "xrp" | "ripple" => "XRP-USD",
        "cardano" | "ada" => "ADA-USD",
        "litecoin" | "ltc" => "LTC-USD",
        "s&p 500" | "s&p" | "sp500" | "s&p500" | "spx" => "^GSPC",
        "dow" | "dow jones" | "djia" => "^DJI",
        "nasdaq" | "nasdaq composite" => "^IXIC",
        "russell 2000" => "^RUT",
        "vix" => "^VIX",
        "ftse" | "ftse 100" => "^FTSE",
        "nikkei" | "nikkei 225" => "^N225",
        "dax" => "^GDAXI",
        "gold" => "GC=F",
        "silver" => "SI=F",
        "oil" | "crude" | "crude oil" | "wti" => "CL=F",
        "brent" => "BZ=F",
        "natural gas" => "NG=F",
        _ => "",
    };
    if !known.is_empty() {
        return known.into();
    }
    // Currency pairs: "EUR/USD", "eur to usd", "EURUSD".
    let pair = a.contains('/') || low.contains(" to ") || low.contains(" in ");
    let letters: String = low.replace(" to ", " ").replace(" in ", " ").chars().filter(|c| c.is_ascii_alphabetic()).collect();
    if letters.len() == 6 && (pair || (a.len() == 6 && a.chars().all(|c| c.is_ascii_uppercase()) && is_currency(&a[..3]) && is_currency(&a[3..]))) {
        let l = letters.to_ascii_uppercase();
        if is_currency(&l[..3]) && is_currency(&l[3..]) {
            return format!("{}{}=X", &l[..3], &l[3..]);
        }
    }
    // Crypto written like BTC/USD.
    if let Some((base, quote)) = a.split_once('/') {
        return format!("{}-{}", base.trim().to_ascii_uppercase(), quote.trim().to_ascii_uppercase());
    }
    a.to_ascii_uppercase()
}

fn is_currency(code: &str) -> bool {
    const CODES: &[&str] = &[
        "USD", "EUR", "GBP", "JPY", "CNY", "CAD", "AUD", "NZD", "CHF", "SEK", "NOK", "DKK", "MXN", "BRL", "INR", "KRW", "HKD", "SGD", "ZAR", "TRY", "PLN", "THB", "IDR", "PHP",
        "ILS", "AED", "SAR", "CZK", "HUF", "TWD", "MYR", "ARS", "CLP", "COP", "VND", "EGP", "NGN", "RUB",
    ];
    CODES.contains(&code.to_ascii_uppercase().as_str())
}

/// Whether the text looks like a ticker already (AAPL, BRK-B, ^GSPC, EURUSD=X).
fn looks_like_symbol(s: &str) -> bool {
    !s.is_empty() && s.len() <= 12 && !s.contains(' ') && s.chars().all(|c| c.is_ascii_uppercase() || c.is_ascii_digit() || matches!(c, '.' | '-' | '^' | '='))
}

/// The interval Yahoo serves for a range.
fn interval_for(range: &str) -> &'static str {
    match range {
        "1d" => "5m",
        "5d" => "30m",
        "1mo" | "3mo" | "6mo" | "ytd" => "1d",
        "1y" | "2y" => "1wk",
        _ => "1mo",
    }
}

/// A range people say ("a month", "1 year", "ytd") as Yahoo's.
pub fn range_for(asked: &str) -> Option<&'static str> {
    let a = asked.trim().to_ascii_lowercase().replace(' ', "");
    Some(match a.as_str() {
        "" | "now" | "today" | "1d" | "day" | "1day" => return if a.is_empty() || a == "now" { None } else { Some("1d") },
        "5d" | "week" | "1w" | "1week" | "5days" => "5d",
        "1mo" | "1m" | "month" | "1month" => "1mo",
        "3mo" | "3m" | "3months" => "3mo",
        "6mo" | "6m" | "6months" => "6mo",
        "ytd" | "yeartodate" => "ytd",
        "1y" | "year" | "1year" | "12mo" => "1y",
        "2y" | "2years" => "2y",
        "5y" | "5years" => "5y",
        "10y" | "10years" => "10y",
        "max" | "all" => "max",
        _ => return None,
    })
}

pub async fn quote(client: &reqwest::Client, asked: &str, range: Option<&str>) -> Result<Quote, String> {
    let mut symbol = symbol_for(asked);
    if !looks_like_symbol(&symbol) || symbol.chars().filter(|c| c.is_ascii_alphabetic()).count() > 6 && !symbol.contains(['-', '=', '^', '.']) {
        symbol = lookup(client, asked).await?.unwrap_or(symbol);
    }
    match chart(client, &symbol, range).await {
        Ok(q) => Ok(q),
        // A company name that happened to look like a symbol ("NVIDIA").
        Err(e) if e.contains("isn't a symbol") => match lookup(client, asked).await? {
            Some(found) if found != symbol => chart(client, &found, range).await,
            _ => Err(e),
        },
        Err(e) => Err(e),
    }
}

/// A company, fund or coin's symbol from its name.
async fn lookup(client: &reqwest::Client, name: &str) -> Result<Option<String>, String> {
    let url = reqwest::Url::parse_with_params("https://query1.finance.yahoo.com/v1/finance/search", &[("q", name), ("quotesCount", "5"), ("newsCount", "0")]).map_err(|e| e.to_string())?;
    let v: Value = client.get(url).send().await.map_err(|e| format!("Couldn't reach Yahoo Finance ({}).", e.without_url()))?.json().await.map_err(|_| "Yahoo Finance's answer was unreadable.".to_string())?;
    Ok(v["quotes"].as_array().and_then(|qs| {
        // Stocks, funds and coins before futures and options.
        qs.iter().find(|q| matches!(q["quoteType"].as_str(), Some("EQUITY" | "ETF" | "MUTUALFUND" | "INDEX" | "CRYPTOCURRENCY" | "CURRENCY"))).or_else(|| qs.first())
    }).and_then(|q| q["symbol"].as_str()).map(str::to_string))
}

async fn chart(client: &reqwest::Client, symbol: &str, range: Option<&str>) -> Result<Quote, String> {
    let r = range.unwrap_or("1d");
    let mut url = reqwest::Url::parse("https://query1.finance.yahoo.com/v8/finance/chart/").map_err(|e| e.to_string())?;
    url.path_segments_mut().map_err(|_| "bad address".to_string())?.pop_if_empty().push(symbol);
    url.query_pairs_mut().append_pair("interval", interval_for(r)).append_pair("range", r);
    let res = client.get(url).send().await.map_err(|e| format!("Couldn't reach Yahoo Finance ({}).", e.without_url()))?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Err(format!("{symbol} isn't a symbol Yahoo Finance knows."));
    }
    if !res.status().is_success() {
        return Err(format!("Yahoo Finance answered {} for {symbol}.", res.status()));
    }
    let v: Value = res.json().await.map_err(|_| "Yahoo Finance's answer was unreadable.".to_string())?;
    let mut q = parse_chart(&v, symbol)?;
    // Over a longer range, Yahoo's chartPreviousClose is the close before the
    // range began (a month's move was told as the last day's: NVDA "-6.45%
    // since the last close" was -0.6%): the day before's close comes from
    // the daily closes instead (`daily`).
    if r != "1d" {
        q.previous_close = None;
    }
    Ok(q)
}

/// A year of daily closes and volumes (oldest first), for the averages and
/// momentum in `technicals`, and the close the day before.
pub async fn daily(client: &reqwest::Client, symbol: &str) -> Result<Vec<(f64, f64)>, String> {
    let mut url = reqwest::Url::parse("https://query1.finance.yahoo.com/v8/finance/chart/").map_err(|e| e.to_string())?;
    url.path_segments_mut().map_err(|_| "bad address".to_string())?.pop_if_empty().push(symbol);
    url.query_pairs_mut().append_pair("interval", "1d").append_pair("range", "1y");
    let v: Value = client.get(url).send().await.map_err(|e| format!("Couldn't reach Yahoo Finance ({}).", e.without_url()))?.json().await.map_err(|_| "Yahoo Finance's answer was unreadable.".to_string())?;
    Ok(parse_daily(&v))
}

pub fn parse_daily(v: &Value) -> Vec<(f64, f64)> {
    let q = &v["chart"]["result"][0]["indicators"]["quote"][0];
    let (closes, volumes) = (q["close"].as_array().cloned().unwrap_or_default(), q["volume"].as_array().cloned().unwrap_or_default());
    closes.iter().enumerate().filter_map(|(i, c)| Some((c.as_f64()?, volumes.get(i).and_then(Value::as_f64).unwrap_or(0.0)))).collect()
}

fn sma(v: &[f64], n: usize) -> Option<f64> {
    (n > 0 && v.len() >= n).then(|| v[v.len() - n..].iter().sum::<f64>() / n as f64)
}

/// The exponential average over `n`, from the n-th value on.
fn ema(v: &[f64], n: usize) -> Vec<f64> {
    let Some(seed) = (v.len() >= n).then(|| v[..n].iter().sum::<f64>() / n as f64) else { return Vec::new() };
    let k = 2.0 / (n as f64 + 1.0);
    v[n..].iter().fold(vec![seed], |mut out, x| {
        let last = *out.last().unwrap();
        out.push(last + k * (x - last));
        out
    })
}

/// Wilder's relative strength index over `n` days.
fn rsi(v: &[f64], n: usize) -> Option<f64> {
    if v.len() <= n {
        return None;
    }
    let moves: Vec<f64> = v.windows(2).map(|w| w[1] - w[0]).collect();
    let (mut up, mut down) = moves[..n].iter().fold((0.0, 0.0), |(u, d), m| if *m > 0.0 { (u + m, d) } else { (u, d - m) });
    up /= n as f64;
    down /= n as f64;
    for m in &moves[n..] {
        up = (up * (n as f64 - 1.0) + m.max(0.0)) / n as f64;
        down = (down * (n as f64 - 1.0) + (-m).max(0.0)) / n as f64;
    }
    Some(if down == 0.0 { 100.0 } else { 100.0 - 100.0 / (1.0 + up / down) })
}

/// MACD (12, 26, 9): the line, its signal, and the gap between them now and
/// the day before.
fn macd(v: &[f64]) -> Option<(f64, f64, f64, f64)> {
    let (fast, slow) = (ema(v, 12), ema(v, 26));
    // Both from the 26th close on.
    let line: Vec<f64> = slow.iter().enumerate().map(|(i, s)| fast[i + 14] - s).collect();
    let signal = ema(&line, 9);
    if signal.len() < 2 {
        return None;
    }
    let gap = |back: usize| line[line.len() - 1 - back] - signal[signal.len() - 1 - back];
    Some((*line.last()?, *signal.last()?, gap(0), gap(1)))
}

/// What the daily closes say, worked out here so a model doesn't have to
/// (one made up a 20-day average and read an RSI of 29 as "buying
/// strength"): the averages and how far the price is from each, RSI and
/// MACD with what they mean, the last days' moves, volume, and the 20-day
/// closing range. None without enough days.
pub fn technicals(price: f64, days: &[(f64, f64)]) -> Option<String> {
    let closes: Vec<f64> = days.iter().map(|d| d.0).collect();
    if closes.len() < 21 {
        return None;
    }
    let from = |avg: f64| {
        let pct = (price - avg) / avg * 100.0;
        format!("price {:.1}% {}", pct.abs(), if pct >= 0.0 { "above" } else { "below" })
    };
    let mut parts = Vec::new();
    let averages: Vec<String> = [20, 50, 200].iter().filter_map(|&n| sma(&closes, n).map(|a| format!("{n}-day average {} ({})", money(a), from(a)))).collect();
    parts.push(averages.join(", "));
    if let Some(r) = rsi(&closes, 14) {
        let means = if r >= 70.0 {
            "overbought (70 or more)"
        } else if r <= 30.0 {
            "oversold (30 or less)"
        } else {
            "neutral (between 30 and 70)"
        };
        parts.push(format!("RSI(14) {r:.1}: {means}"));
    }
    if let Some((line, signal, gap, before)) = macd(&closes) {
        let way = if gap >= 0.0 { "above its signal line: momentum is rising" } else { "below its signal line: momentum is falling" };
        let crossed = if (gap >= 0.0) != (before >= 0.0) { ", and it crossed on the last day" } else { "" };
        parts.push(format!("MACD(12,26,9) {line:.2} vs signal {signal:.2}, {way}{crossed}"));
    }
    let moves: Vec<String> = closes.windows(2).rev().take(5).collect::<Vec<_>>().into_iter().rev().map(|w| format!("{:+.1}%", (w[1] - w[0]) / w[0] * 100.0)).collect();
    parts.push(format!("the last {} trading days, oldest first: {}", moves.len(), moves.join(", ")));
    let volumes: Vec<f64> = days.iter().map(|d| d.1).collect();
    if let (Some(&last), Some(avg)) = (volumes.last(), sma(&volumes[..volumes.len() - 1], 20)) {
        if last > 0.0 && avg > 0.0 {
            parts.push(format!("volume on the last day {:.1}× its 20-day average", last / avg));
        }
    }
    let recent = &closes[closes.len() - 20..];
    let (lo, hi) = recent.iter().fold((f64::INFINITY, f64::NEG_INFINITY), |(lo, hi), c| (lo.min(*c), hi.max(*c)));
    parts.push(format!("20-day closing range {}–{}", money(lo), money(hi)));
    Some(format!("Worked out from {} daily closes: {}.", closes.len(), parts.join("; ")))
}

pub fn parse_chart(v: &Value, symbol: &str) -> Result<Quote, String> {
    let r = &v["chart"]["result"][0];
    let m = &r["meta"];
    let price = m["regularMarketPrice"].as_f64().ok_or_else(|| format!("{symbol} isn't a symbol Yahoo Finance knows."))?;
    let num = |k: &str| m[k].as_f64();
    let times = r["timestamp"].as_array().cloned().unwrap_or_default();
    let closes = r["indicators"]["quote"][0]["close"].as_array().cloned().unwrap_or_default();
    let history: Vec<(i64, f64)> = times.iter().zip(closes.iter()).filter_map(|(t, c)| Some((t.as_i64()?, c.as_f64()?))).collect();
    Ok(Quote {
        symbol: m["symbol"].as_str().unwrap_or(symbol).to_string(),
        name: m["longName"].as_str().or_else(|| m["shortName"].as_str()).unwrap_or(symbol).to_string(),
        currency: m["currency"].as_str().unwrap_or("").to_string(),
        price,
        previous_close: num("chartPreviousClose").or_else(|| num("previousClose")),
        day_high: num("regularMarketDayHigh"),
        day_low: num("regularMarketDayLow"),
        year_high: num("fiftyTwoWeekHigh"),
        year_low: num("fiftyTwoWeekLow"),
        time: m["regularMarketTime"].as_i64().unwrap_or(0),
        exchange: m["fullExchangeName"].as_str().or_else(|| m["exchangeName"].as_str()).unwrap_or("").to_string(),
        history,
    })
}

fn money(x: f64) -> String {
    if x.abs() >= 1000.0 {
        format!("{x:.2}")
    } else if x.abs() >= 1.0 {
        format!("{x:.2}")
    } else {
        format!("{x:.6}").trim_end_matches('0').to_string()
    }
}

/// A quote in words for the model; `range` adds how it moved.
pub fn describe(q: &Quote, range: Option<&str>) -> String {
    let when = Utc.timestamp_opt(q.time, 0).single().map(|t| t.with_timezone(&chrono::Local).format("%b %-d, %-I:%M %p").to_string()).unwrap_or_default();
    let mut s = format!("{} ({}): {} {}", q.name, q.symbol, money(q.price), q.currency);
    if let Some(prev) = q.previous_close.filter(|p| *p != 0.0) {
        let ch = q.price - prev;
        let pct = ch / prev * 100.0;
        s.push_str(&format!(", {}{} ({}{:.2}%) since the last close of {}", if ch >= 0.0 { "+" } else { "" }, money(ch), if pct >= 0.0 { "+" } else { "" }, pct, money(prev)));
    }
    if !when.is_empty() {
        s.push_str(&format!(", as of {when} (this PC's time)"));
    }
    if let (Some(lo), Some(hi)) = (q.day_low, q.day_high) {
        s.push_str(&format!(". Day range {}–{}", money(lo), money(hi)));
    }
    if let (Some(lo), Some(hi)) = (q.year_low, q.year_high) {
        s.push_str(&format!(", 52-week range {}–{}", money(lo), money(hi)));
    }
    if !q.exchange.is_empty() {
        s.push_str(&format!(" ({})", q.exchange));
    }
    s.push('.');
    if let (Some(r), Some(first), Some(last)) = (range.filter(|r| *r != "1d"), q.history.first(), q.history.last()) {
        let lo = q.history.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
        let hi = q.history.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
        let pct = (last.1 - first.1) / first.1 * 100.0;
        s.push_str(&format!(" Over {r}: from {} to {} ({}{pct:.1}%), low {}, high {}.", money(first.1), money(last.1), if pct >= 0.0 { "+" } else { "" }, money(lo), money(hi)));
    }
    s
}

/// A line chart of the range, for the chat.
pub fn chart_of(q: &Quote, range: &str) -> Option<Value> {
    if q.history.len() < 2 {
        return None;
    }
    // At most about 120 points.
    let step = q.history.len().div_ceil(120);
    let pts: Vec<&(i64, f64)> = q.history.iter().step_by(step).collect();
    let fmt = if matches!(range, "1d" | "5d") { "%b %-d %-I:%M %p" } else { "%b %-d, %Y" };
    let labels: Vec<String> = pts.iter().map(|(t, _)| Utc.timestamp_opt(*t, 0).single().map(|d| d.with_timezone(&chrono::Local).format(fmt).to_string()).unwrap_or_default()).collect();
    let data: Vec<f64> = pts.iter().map(|(_, c)| (c * 100.0).round() / 100.0).collect();
    Some(json!({ "type": "line", "title": format!("{} ({}), {range}", q.name, q.symbol), "x_label": "", "y_label": q.currency, "labels": labels, "series": [{ "name": q.symbol, "data": data }] }))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn names_become_symbols() {
        assert_eq!(symbol_for("bitcoin"), "BTC-USD");
        assert_eq!(symbol_for("S&P 500"), "^GSPC");
        assert_eq!(symbol_for("gold"), "GC=F");
        assert_eq!(symbol_for("EUR/USD"), "EURUSD=X");
        assert_eq!(symbol_for("usd to jpy"), "USDJPY=X");
        assert_eq!(symbol_for("GBPUSD"), "GBPUSD=X");
        assert_eq!(symbol_for("aapl"), "AAPL");
        assert_eq!(symbol_for("BTC/EUR"), "BTC-EUR");
        assert!(looks_like_symbol("BRK-B") && looks_like_symbol("^GSPC") && !looks_like_symbol("APPLE INC"));
        assert_eq!(range_for("a month"), None);
        assert_eq!(range_for("1 year"), Some("1y"));
        assert_eq!(range_for(""), None);
        assert_eq!(range_for("ytd"), Some("ytd"));
    }

    #[test]
    fn a_chart_answer_reads_as_a_quote() {
        let v = json!({ "chart": { "result": [{ "meta": { "symbol": "AAPL", "longName": "Apple Inc.", "currency": "USD", "regularMarketPrice": 343.46, "chartPreviousClose": 342.0,
            "regularMarketDayHigh": 347.82, "regularMarketDayLow": 340.46, "fiftyTwoWeekHigh": 348.15, "fiftyTwoWeekLow": 243.42, "regularMarketTime": 1792180801, "fullExchangeName": "NasdaqGS" },
            "timestamp": [1, 2, 3], "indicators": { "quote": [{ "close": [340.0, null, 343.46] }] } }] } });
        let q = parse_chart(&v, "AAPL").unwrap();
        assert_eq!(q.history, vec![(1, 340.0), (3, 343.46)]);
        let s = describe(&q, None);
        assert!(s.starts_with("Apple Inc. (AAPL): 343.46 USD, +1.46 (+0.43%) since the last close of 342.00"), "{s}");
        assert!(s.contains("52-week range 243.42–348.15") && s.contains("NasdaqGS"));
        assert!(describe(&q, Some("1mo")).contains("Over 1mo: from 340.00 to 343.46 (+1.0%)"));
        assert!(chart_of(&q, "1mo").is_some());
        assert!(parse_chart(&json!({ "chart": { "result": null } }), "ZZZZ").unwrap_err().contains("isn't a symbol"));
    }

    #[test]
    fn the_figures_are_worked_out_and_said_plainly() {
        let rising: Vec<f64> = (1..=60).map(f64::from).collect();
        assert_eq!(sma(&rising, 20), Some(50.5));
        assert_eq!(rsi(&rising, 14), Some(100.0));
        let (line, _, gap, _) = macd(&rising).unwrap();
        assert!(line > 0.0 && gap.abs() < 1e-9, "a steady rise: MACD above zero, on its signal");
        assert_eq!(macd(&rising[..34]), None, "too few days");
        // RSI by hand: 14 moves of +1 and -1 in turn, then a +1: Wilder's
        // averages 0.5 and 0.5, then (0.5*13+1)/14 and (0.5*13)/14.
        let zigzag: Vec<f64> = (0..16).map(|i| if i % 2 == 0 { 10.0 } else { 11.0 }).collect();
        let r = rsi(&zigzag, 14).unwrap();
        assert!((r - 53.57).abs() < 0.01, "{r}");

        // Down from 160 for 60 days: oversold, under every average.
        let days: Vec<(f64, f64)> = (0..60).map(|i| (160.0 - i as f64, if i == 59 { 3e6 } else { 1e6 })).collect();
        let s = technicals(101.0, &days).unwrap();
        assert!(s.starts_with("Worked out from 60 daily closes: 20-day average 110.50 (price 8.6% below), 50-day average 125.50 (price 19.5% below);"), "{s}");
        assert!(!s.contains("200-day"), "not enough days for it: {s}");
        assert!(s.contains("RSI(14) 0.0: oversold (30 or less)"), "{s}");
        assert!(s.contains("below its signal line: momentum is falling") || s.contains("above its signal line"), "{s}");
        assert!(s.contains("the last 5 trading days, oldest first: -0.9%, -1.0%, -1.0%, -1.0%, -1.0%"), "{s}");
        assert!(s.contains("volume on the last day 3.0× its 20-day average"), "{s}");
        assert!(s.contains("20-day closing range 101.00–120.00"), "{s}");
        assert_eq!(technicals(100.0, &days[..20]), None);

        let v = json!({ "chart": { "result": [{ "indicators": { "quote": [{ "close": [1.0, null, 3.0], "volume": [10, 20, null] }] } }] } });
        assert_eq!(parse_daily(&v), vec![(1.0, 10.0), (3.0, 0.0)]);
    }
}
