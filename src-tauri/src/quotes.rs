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
    parse_chart(&v, symbol)
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
}
