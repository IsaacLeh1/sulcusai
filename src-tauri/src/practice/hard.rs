// SPDX-License-Identifier: AGPL-3.0-only
//! Levels 5 and 6: tasks a good 8-14B model still gets wrong often enough
//! to learn from. Number puzzles with exact answers, keeping track of many
//! changes in a row, working days and time zones across midnight, and tool
//! requests whose details have to be worked out or need two calls.

use chrono::{Datelike, Duration, NaiveDate, Weekday};

use super::tasks::{clock, date_text, duration_text, thousands, Arg, Check, Rng, Want, DAYS, MONTHS, NAMES, NOUNS};

fn modpow(mut b: u64, mut e: u64, m: u64) -> u64 {
    let mut r = 1 % m;
    b %= m;
    while e > 0 {
        if e & 1 == 1 {
            r = r * b % m;
        }
        b = b * b % m;
        e >>= 1;
    }
    r
}

fn gcd(a: i64, b: i64) -> i64 {
    if b == 0 {
        a.abs()
    } else {
        gcd(b, a % b)
    }
}

fn choose(n: i64, k: i64) -> i64 {
    let k = k.min(n - k);
    (0..k).fold(1i64, |acc, i| acc * (n - i) / (i + 1))
}

// ---------- word problems ----------

pub fn math(level: u32, rng: &mut Rng) -> (String, i64) {
    if level <= 5 {
        match rng.range(0, 4) {
            0 => {
                let (b, k, m) = (rng.range(2, 9), rng.range(20, 300), rng.range(7, 60));
                (format!("What is the remainder when {b} to the power {k} is divided by {m}?"), modpow(b as u64, k as u64, m as u64) as i64)
            }
            1 => {
                let n = rng.range(10, 600) * 100;
                let a = rng.range(3, 13);
                let b = loop {
                    let b = rng.range(3, 13);
                    if b != a && a % b != 0 && b % a != 0 {
                        break b;
                    }
                };
                let l = a / gcd(a, b) * b;
                (format!("How many whole numbers from 1 to {} can be divided exactly by {a} or by {b}, but not by both?", thousands(n)), n / a + n / b - 2 * (n / l))
            }
            2 => {
                let n = rng.range(8, 22);
                let broken = rng.range(2, n - 1);
                let mut ways = vec![0i64; (n + 1) as usize];
                ways[0] = 1;
                for i in 1..=n {
                    if i == broken {
                        continue;
                    }
                    ways[i as usize] = (1..=3).filter(|s| i - s >= 0).map(|s| ways[(i - s) as usize]).sum();
                }
                (
                    format!("A staircase has {n} steps. You climb it taking 1, 2 or 3 steps at a time, but step {broken} is broken and can't be stepped on. In how many different ways can you get from the bottom to the top step?"),
                    ways[n as usize],
                )
            }
            3 => {
                let (hi, s) = (rng.range(10, 99) * 100, rng.range(5, 25));
                let count = (1..=hi).filter(|x| x.to_string().bytes().map(|d| (d - b'0') as i64).sum::<i64>() == s).count() as i64;
                (format!("How many whole numbers from 1 to {} have digits that add up to exactly {s}?", thousands(hi)), count)
            }
            _ => {
                let (p, q) = loop {
                    let (p, q) = (rng.range(2, 9) * 5, rng.range(1, 8) * 5);
                    if q < p {
                        break (p, q);
                    }
                };
                let x = (1..200).map(|k| k * q / gcd(p, q)).find(|x| x * p % q == 0 && *x >= 10).unwrap_or(q);
                (format!("How many litres of pure water must be added to {x} litres of a {p}% salt solution to make it a {q}% solution?"), x * p / q - x)
            }
        }
    } else {
        match rng.range(0, 5) {
            0 => {
                let n = rng.range(50, 600);
                let sum: i64 = (2..n).filter(|x| (2..*x).take_while(|d| d * d <= *x).all(|d| x % d != 0)).sum();
                (format!("What is the sum of all the prime numbers below {n}?"), sum)
            }
            1 => {
                let (a, b) = (rng.range(3, 7), rng.range(3, 7));
                let (c, d) = (rng.range(1, a - 1), rng.range(1, b - 1));
                (
                    format!("On a grid you walk from the corner (0, 0) to ({a}, {b}), one step at a time, only moving right (x + 1) or up (y + 1). How many different routes never pass through the point ({c}, {d})?"),
                    choose(a + b, a) - choose(c + d, c) * choose(a + b - c - d, a - c),
                )
            }
            2 => {
                let b = loop {
                    let b = rng.range(2, 19);
                    if b % 10 != 0 {
                        break b;
                    }
                };
                let k = rng.range(50, 2000);
                (format!("What are the last two digits of {b} to the power {k}? Give them as a number from 0 to 99."), modpow(b as u64, k as u64, 100) as i64)
            }
            3 => {
                let (m1, m2) = *rng.pick(&[(3, 5), (4, 7), (5, 9), (7, 11), (8, 9), (9, 13), (11, 12)]);
                let (r1, r2) = (rng.range(1, m1 - 1), rng.range(1, m2 - 1));
                let x = (1..=m1 * m2).find(|x| x % m1 == r1 && x % m2 == r2).unwrap_or(m1 * m2);
                (format!("What is the smallest positive whole number that leaves a remainder of {r1} when divided by {m1} and a remainder of {r2} when divided by {m2}?"), x)
            }
            4 => {
                let n = rng.range(100, 5000);
                let mut zeros = 0;
                let mut p = 5;
                while p <= n {
                    zeros += n / p;
                    p *= 5;
                }
                (format!("How many zeros are at the end of {} factorial ({}! = 1 × 2 × 3 × … × {})?", thousands(n), n, n), zeros)
            }
            _ => {
                let (m, n) = (rng.range(3, 12), rng.range(3, 12));
                (format!("A grid is {m} squares wide and {n} squares tall. How many rectangles of any size (squares included) can be found in it?"), m * (m + 1) / 2 * (n * (n + 1) / 2))
            }
        }
    }
}

// ---------- keeping track ----------

const COLOURS: &[&str] = &["red", "blue", "green", "yellow", "purple", "orange"];
const ORDINAL: &[&str] = &["first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth"];

/// A list changed step by step: the steps as words and the list after.
fn list_steps(rng: &mut Rng, steps: usize) -> (Vec<String>, Vec<String>, Vec<&'static str>) {
    let mut nouns = NOUNS.to_vec();
    rng.shuffle(&mut nouns);
    let start: Vec<&str> = nouns[..6].to_vec();
    let mut spare: Vec<&str> = nouns[6..].to_vec();
    let mut list = start.clone();
    let mut said = Vec::new();
    for _ in 0..steps {
        match rng.range(0, 5) {
            0 if list.len() >= 3 => {
                let n = list.len().min(ORDINAL.len());
                let i = rng.range(0, n as i64 - 1) as usize;
                let j = loop {
                    let j = rng.range(0, n as i64 - 1) as usize;
                    if j != i {
                        break j;
                    }
                };
                list.swap(i, j);
                said.push(format!("swap the {} and {} items", ORDINAL[i.min(j)], ORDINAL[i.max(j)]));
            }
            1 => {
                if let Some(last) = list.pop() {
                    list.insert(0, last);
                    said.push("move the last item to the front".to_string());
                }
            }
            2 if list.len() > 3 => {
                let i = rng.range(0, list.len() as i64 - 1) as usize;
                let gone = list.remove(i);
                said.push(format!("remove \"{gone}\""));
            }
            3 if !spare.is_empty() => {
                let new = spare.pop().unwrap_or("yarn");
                let i = rng.range(0, list.len() as i64 - 1) as usize;
                said.push(format!("insert \"{new}\" right after \"{}\"", list[i]));
                list.insert(i + 1, new);
            }
            _ => {
                list.reverse();
                said.push("reverse the whole list".to_string());
            }
        }
    }
    (said, list.iter().map(|s| s.to_string()).collect(), start)
}

pub fn careful(level: u32, rng: &mut Rng) -> (String, Want) {
    match (level, rng.range(0, 2)) {
        (5, 0) | (6, 0) => {
            let (said, end, start) = list_steps(rng, if level == 5 { 6 } else { 10 });
            (
                format!("Start with this list: {}. Then, in order: {}. What is the list now, from first to last?", start.join(", "), said.join("; ")),
                Want::Words(end),
            )
        }
        (5, _) => {
            let words: Vec<&str> = (0..rng.range(25, 35)).map(|_| *rng.pick(super::tasks::FILLER)).collect();
            let text = words.join(" ");
            let letter = *rng.pick(&['e', 'a', 's', 't', 'o', 'r']);
            (format!("How many times does the letter \"{letter}\" appear in this text? \"{text}\""), Want::Number(text.chars().filter(|c| *c == letter).count() as f64))
        }
        (_, 1) => {
            let mut people = NAMES.to_vec();
            rng.shuffle(&mut people);
            let people: Vec<&str> = people[..5].to_vec();
            let mut has: Vec<&str> = COLOURS[..5].to_vec();
            rng.shuffle(&mut has);
            let start: Vec<String> = people.iter().zip(&has).map(|(p, c)| format!("{p} has a {c} ball")).collect();
            let mut swaps = Vec::new();
            for _ in 0..rng.range(8, 12) {
                let i = rng.range(0, 4) as usize;
                let j = loop {
                    let j = rng.range(0, 4) as usize;
                    if j != i {
                        break j;
                    }
                };
                has.swap(i, j);
                swaps.push(format!("{} and {} swap balls", people[i], people[j]));
            }
            let who = rng.range(0, 4) as usize;
            (format!("{}. Then: {}. What colour is {}'s ball at the end?", start.join(", "), swaps.join(", then "), people[who]), Want::Word(has[who].to_string()))
        }
        _ => {
            let start = rng.range(5, 40);
            let mut v = start;
            let mut said = Vec::new();
            for _ in 0..rng.range(8, 11) {
                match rng.range(0, 4) {
                    0 => {
                        let k = rng.range(3, 19);
                        v += k;
                        said.push(format!("add {k}"));
                    }
                    1 if v < 500 => {
                        v *= 2;
                        said.push("double it".to_string());
                    }
                    2 => {
                        // Never below zero.
                        let k = rng.range(2, 15).min(v);
                        v -= k;
                        said.push(format!("subtract {k}"));
                    }
                    3 => {
                        v = if v % 2 == 0 { v + 3 } else { v - 1 };
                        said.push("if it's even add 3, otherwise subtract 1".to_string());
                    }
                    _ => {
                        v = v.div_euclid(2);
                        said.push("halve it, rounding down".to_string());
                    }
                }
            }
            (format!("A counter starts at {start}. Then, in order: {}. What number does it show at the end?", said.join("; ")), Want::Number(v as f64))
        }
    }
}

// ---------- dates and times ----------

pub fn time(level: u32, rng: &mut Rng) -> (String, Want) {
    let year_start = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap();
    if level <= 5 {
        if rng.chance(0.6) {
            let mut start = year_start + Duration::days(rng.range(0, 330));
            while matches!(start.weekday(), Weekday::Sat | Weekday::Sun) {
                start += Duration::days(1);
            }
            let n = rng.range(8, 45);
            let mut day = start;
            let mut left = n - 1;
            while left > 0 {
                day += Duration::days(1);
                if !matches!(day.weekday(), Weekday::Sat | Weekday::Sun) {
                    left -= 1;
                }
            }
            (
                format!(
                    "A job starts on {}, {}. It takes {n} working days, counting the first day, and work only happens Monday to Friday. On what date does it finish?",
                    DAYS[start.weekday().num_days_from_monday() as usize],
                    date_text(start)
                ),
                Want::Date(day.year(), day.month(), day.day(), false),
            )
        } else {
            let a = year_start + Duration::days(rng.range(0, 250));
            let b = a + Duration::days(rng.range(30, 110));
            let wd = rng.range(0, 6) as u32;
            let count = (0..=(b - a).num_days()).filter(|k| (a + Duration::days(*k)).weekday().num_days_from_monday() == wd).count();
            (format!("How many {}s are there from {} to {}, counting both dates?", DAYS[wd as usize], date_text(a), date_text(b)), Want::Number(count as f64))
        }
    } else if rng.chance(0.6) {
        let day = year_start + Duration::days(rng.range(0, 364));
        let leave = rng.range(0, 287) as i64 * 5;
        let fly = rng.range(30, 200) * 5;
        let shift = loop {
            let s = rng.range(-8, 11);
            if s != 0 {
                break s;
            }
        };
        let land = leave + fly + shift * 60;
        let land_day = day + Duration::days(land.div_euclid(1440));
        let at = land.rem_euclid(1440) as u32;
        (
            format!(
                "A flight leaves at {} on {} (local time) and is in the air for {}. The clock where it lands is {} hours {} the clock where it took off. What are the local date and time when it lands?",
                clock(leave as u32),
                date_text(day),
                duration_text(fly as u32),
                shift.abs(),
                if shift > 0 { "ahead of" } else { "behind" }
            ),
            Want::DateTime(land_day.year(), land_day.month(), land_day.day(), at),
        )
    } else {
        let d = NaiveDate::from_ymd_opt(rng.range(1990, 2026) as i32, 1, 1).unwrap() + Duration::days(rng.range(0, 364));
        (
            format!("{} was a Friday. What day of the week was {}?", date_text(year_start), date_text(d)),
            Want::Word(DAYS[d.weekday().num_days_from_monday() as usize].to_lowercase()),
        )
    }
}

// ---------- tools ----------

const SPOKEN: &[(&str, u32)] = &[("quarter past two in the afternoon", 14 * 60 + 15), ("half past nine in the morning", 9 * 60 + 30), ("ten to six in the evening", 17 * 60 + 50), ("noon", 12 * 60)];

/// A tool task: what's asked, the check, and the tools it needs.
pub fn tools(level: u32, rng: &mut Rng) -> (String, Check, Vec<&'static str>) {
    let s = |x: &str| x.to_string();
    let call = |name: &str, args: Vec<(&str, Arg)>| (name.to_string(), args.into_iter().map(|(k, v)| (k.to_string(), v)).collect::<Vec<_>>());
    let has = |xs: &[&str]| Arg::Has(xs.iter().map(|x| x.to_string()).collect());
    let iso = |d: NaiveDate| Arg::Text(d.format("%Y-%m-%d").to_string());
    let hhmm = |m: u32| Arg::Text(format!("{:02}:{:02}", m / 60, m % 60));
    if level <= 5 {
        match rng.range(0, 4) {
            0 => {
                let now = rng.range(9 * 12, 19 * 12) as u32 * 5;
                let leaves = now + rng.range(8, 30) as u32 * 5;
                let before = rng.range(1, 4) as u32 * 5;
                let (name, args) = call("set_timer", vec![("minutes", Arg::Number((leaves - now - before) as f64))]);
                (format!("It's {} now and my train leaves at {}. Set a timer to go off {before} minutes before it leaves.", clock(now), clock(leaves)), Check::Call { name, args }, vec!["set_timer"])
            }
            1 => {
                let people = rng.range(2, 6);
                let share = rng.range(15, 90);
                let (cur, code) = *rng.pick(&[("euros", "EUR"), ("British pounds", "GBP"), ("Canadian dollars", "CAD")]);
                let (name, args) = call("convert_currency", vec![("amount", Arg::Number(share as f64)), ("from", Arg::Text(s(code))), ("to", Arg::Text(s("USD")))]);
                (
                    format!("Dinner came to {} {cur}, split equally between {people} of us. Convert my share into US dollars.", share * people),
                    Check::Call { name, args },
                    vec!["convert_currency"],
                )
            }
            2 => {
                let title = *rng.pick(&["Quarterly review", "Dentist", "Book club"]);
                let month = rng.range(1, 12) as u32;
                let wd = rng.range(0, 4) as u32;
                let first_next = if month == 12 { NaiveDate::from_ymd_opt(2028, 1, 1) } else { NaiveDate::from_ymd_opt(2027, month + 1, 1) }.unwrap();
                let mut day = first_next - Duration::days(1);
                while day.weekday().num_days_from_monday() != wd {
                    day -= Duration::days(1);
                }
                let (spoken, at) = *rng.pick(SPOKEN);
                let (name, args) = call("add_event", vec![("title", has(&[title])), ("date", iso(day)), ("time", hhmm(at))]);
                (
                    format!("Book \"{title}\" for the last {} of {} 2027, at {spoken}.", DAYS[wd as usize], MONTHS[month as usize - 1]),
                    Check::Call { name, args },
                    vec!["add_event"],
                )
            }
            3 => {
                let who = *rng.pick(&["Priya", "Alex", "Jordan"]);
                let was = rng.range(9 * 4, 16 * 4) as u32 * 15;
                let by = rng.range(1, 6) as u32 * 15;
                let new = clock(was + by);
                let key = new.split(' ').next().unwrap_or("").to_string();
                let (name, args) = call("send_message", vec![("to", has(&[who])), ("text", has(&[key.as_str()]))]);
                (format!("Text {who} that the meeting at {} is pushed back by {by} minutes, and give the new time.", clock(was)), Check::Call { name, args }, vec!["send_message"])
            }
            _ => {
                let costs: Vec<(&str, i64)> = vec![("rent", rng.range(600, 1600)), ("food", rng.range(150, 500)), ("travel", rng.range(30, 200)), ("phone", rng.range(15, 70))];
                let total: i64 = costs.iter().map(|c| c.1).sum();
                let list = costs.iter().map(|(k, v)| format!("{k} {v}")).collect::<Vec<_>>().join(", ");
                let (name, args) = call("create_note", vec![("title", Arg::Text(s("Budget"))), ("text", Arg::HasNumber(total as f64))]);
                (format!("Save a note called \"Budget\" with the total of these monthly costs: {list}."), Check::Call { name, args }, vec!["create_note"])
            }
        }
    } else {
        match rng.range(0, 3) {
            0 => {
                let mut cities = ["Paris", "Tokyo", "Nairobi", "Lima", "Oslo", "Toronto"].to_vec();
                rng.shuffle(&mut cities);
                let (a, b) = (cities[0], cities[1]);
                (
                    format!("Is it warmer in {a} or in {b} right now?"),
                    Check::Calls(vec![call("get_weather", vec![("city", has(&[a]))]), call("get_weather", vec![("city", has(&[b]))])]),
                    vec!["get_weather"],
                )
            }
            1 => {
                let (spoken, m) = *rng.pick(&[("an hour and a quarter", 75), ("forty-five minutes", 45), ("two hours and twenty minutes", 140)]);
                let (title, text, key) = *rng.pick(&[("Oven", "oven on at 180 degrees", "180"), ("Parking", "level 3, bay 42", "42")]);
                (
                    format!("Start a timer for {spoken}, and save a note called \"{title}\" that says {text}."),
                    Check::Calls(vec![call("set_timer", vec![("minutes", Arg::Number(m as f64))]), call("create_note", vec![("title", Arg::Text(s(title))), ("text", has(&[key]))])]),
                    vec!["set_timer", "create_note"],
                )
            }
            2 => {
                let title = *rng.pick(&["Team lunch", "Car service"]);
                let day = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap() + Duration::days(rng.range(0, 364));
                let at = rng.range(8 * 4, 18 * 4) as u32 * 15;
                let who = *rng.pick(&["Mom", "Alex", "Priya"]);
                (
                    format!("Put \"{title}\" on my calendar for {} at {}, then text {who} to say it's booked.", date_text(day), clock(at)),
                    Check::Calls(vec![
                        call("add_event", vec![("title", has(&[title])), ("date", iso(day)), ("time", hhmm(at))]),
                        call("send_message", vec![("to", has(&[who]))]),
                    ]),
                    vec!["add_event", "send_message"],
                )
            }
            _ => {
                let (p, n) = (*rng.pick(&[10, 15, 20, 25]), rng.range(4, 30) * 4);
                (
                    format!("Don't look anything up or use a tool, just tell me: what is {p}% of {n}?"),
                    Check::NoCall { says: vec![super::tasks::number_text(p as f64 * n as f64 / 100.0)] },
                    vec![],
                )
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn number_helpers() {
        assert_eq!(modpow(7, 222, 10), 9);
        assert_eq!(choose(6, 3), 20);
        assert_eq!(gcd(12, 18), 6);
    }

    #[test]
    fn hard_tasks_have_sensible_answers() {
        let mut rng = Rng::new(21);
        for level in [5, 6] {
            for _ in 0..300 {
                let (q, a) = math(level, &mut rng);
                assert!(a >= 0, "{q} → {a}");
                let (q, _) = careful(level, &mut rng);
                assert!(!q.contains('{'), "{q}");
                let (_, _) = time(level, &mut rng);
                let (q, check, needs) = tools(level, &mut rng);
                if let Check::Call { args, .. } = &check {
                    if let Some((_, Arg::Number(n))) = args.iter().find(|(k, _)| k == "minutes") {
                        assert!(*n > 0.0, "{q}");
                    }
                }
                assert!(needs.len() <= 2);
            }
        }
    }
}
