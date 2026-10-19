// SPDX-License-Identifier: AGPL-3.0-only
//! Practice tasks, made up fresh from a seed. Each comes with a check that
//! says whether a reply got it right, so no other model has to judge: sums
//! with one right answer, careful counting and spelling, dates and times,
//! replies in a set shape (bullets, word counts, JSON), and picking the
//! right tool with the right details.
//!
//! Each skill has four levels, from one-step tasks a small model gets right
//! to ones a good 4B model still often gets wrong (big numbers, leap years,
//! exact word counts, tool details that need working out), so practice
//! always has something to learn from.

use std::sync::LazyLock;

use chrono::{Datelike, Duration, NaiveDate};
use regex::Regex;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Skill {
    Code,
    Writing,
    Research,
    Math,
    Careful,
    Time,
    Format,
    Tools,
    /// Finding things in the person's own code with the file tools.
    Project,
}

impl Skill {
    pub const ALL: [Skill; 9] = [Skill::Code, Skill::Writing, Skill::Research, Skill::Math, Skill::Careful, Skill::Time, Skill::Format, Skill::Tools, Skill::Project];

    pub fn label(self) -> &'static str {
        match self {
            Skill::Code => "coding",
            Skill::Writing => "writing",
            Skill::Research => "research from sources",
            Skill::Math => "word problems",
            Skill::Careful => "careful counting",
            Skill::Time => "dates and times",
            Skill::Format => "following instructions",
            Skill::Tools => "using tools",
            Skill::Project => "your code",
        }
    }
}

/// Levels of difficulty, 1 to this.
pub const LEVELS: u32 = 6;

/// A small, fast random source (splitmix64): the same seed makes the same
/// tasks, so a test can be given again.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed)
    }

    pub fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// From `lo` to `hi`, both included.
    pub fn range(&mut self, lo: i64, hi: i64) -> i64 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next() % (hi - lo + 1) as u64) as i64
    }

    pub fn pick<'a, T>(&mut self, xs: &'a [T]) -> &'a T {
        &xs[self.next() as usize % xs.len()]
    }

    pub fn chance(&mut self, p: f64) -> bool {
        (self.next() >> 11) as f64 / (1u64 << 53) as f64 <= p
    }

    pub fn shuffle<T>(&mut self, xs: &mut [T]) {
        for i in (1..xs.len()).rev() {
            xs.swap(i, self.next() as usize % (i + 1));
        }
    }
}

pub struct Task {
    pub skill: Skill,
    pub level: u32,
    pub user: String,
    /// Tools offered (OpenAI style), for the tool tasks.
    pub tools: Option<Value>,
    pub check: Check,
}

pub enum Check {
    /// A last line "Answer: …" with this answer.
    Answer(Want),
    /// The reply keeps to every one of these.
    Format(Vec<Rule>),
    /// Exactly one call of this tool, with these details.
    Call { name: String, args: Vec<(String, Arg)> },
    /// Exactly these calls, in any order.
    Calls(Vec<(String, Vec<(String, Arg)>)>),
    /// No tool call, and an answer saying one of these.
    NoCall { says: Vec<String> },
    /// A JavaScript function that gives the reference's answers (`want`, as
    /// JSON) on these cases (a JSON array of argument lists).
    Code { name: String, cases: String, want: Vec<String> },
    /// Writing that keeps each of these facts (any of the ways it may be
    /// written) within the limits; with a source, it may not have numbers
    /// the source doesn't.
    Facts { must: Vec<Vec<String>>, source: Option<String>, max_words: Option<usize>, max_sentence_words: Option<usize>, sentences: Option<usize>, last_line: Option<String> },
    /// Exactly this text.
    Exact(String),
    /// An answer from the sources citing all of `sources` and nothing but
    /// those and `may` (no answer: the sources don't say). `not`: words that
    /// mustn't be in it.
    Cited { want: Option<Want>, sources: Vec<usize>, may: Vec<usize>, not: Vec<String> },
    /// An answer about the person's code, found with the file tools and
    /// checked against the scan of its folders.
    Look(super::yourcode::Look),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Want {
    Number(f64),
    /// One word (or a single letter), compared by its letters alone.
    Word(String),
    /// Words in this order.
    Words(Vec<String>),
    /// A time of day, in minutes after midnight.
    Time(u32),
    /// Year, month and day; whether the answer must say the year.
    Date(i32, u32, u32, bool),
    /// Year, month, day and time of day (minutes after midnight).
    DateTime(i32, u32, u32, u32),
    Numbers(Vec<f64>),
}

#[derive(Debug, Clone, PartialEq)]
pub enum Arg {
    /// The same text, ignoring case and spaces around it.
    Text(String),
    /// Text containing all of these, ignoring case.
    Has(Vec<String>),
    Number(f64),
    /// Text with this number in it (1,355 or 1355).
    HasNumber(f64),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Rule {
    Bullets(usize),
    MaxWords(usize),
    Lowercase,
    Caps,
    Json(&'static [&'static str]),
    EndWith(&'static str),
    StartWith(&'static str),
    Include(&'static str, usize),
    NoCommas,
    Paragraphs(usize),
    Title,
    ExactWords(usize),
    Sentences(usize),
    JsonList(usize),
    Quoted,
    /// This many bullets of exactly this many words each.
    BulletWords(usize, usize),
    /// Lines whose first letters spell the word.
    Acrostic(&'static str),
    EverySentence(&'static str),
    /// No word longer than this many letters.
    ShortWords(usize),
    NoLetter(char),
}

pub(crate) const SYSTEM: &str = "You are a helpful assistant.";
const ANSWER_LINE: &str = "Work it out step by step, then give the final answer on its own last line as: Answer: <answer>";

impl Task {
    /// The answer, said in words, for a retry that's told it (the model then
    /// has to show how to get there).
    pub fn hint(&self) -> Option<String> {
        match &self.check {
            Check::Answer(w) => Some(want_text(w)),
            _ => None,
        }
    }
}

/// An answer as it would be written.
pub fn want_text(w: &Want) -> String {
    match w {
        Want::Number(n) => number_text(*n),
        Want::Word(s) => s.clone(),
        Want::Words(ws) => ws.join(" "),
        Want::Time(m) => clock(*m),
        Want::Date(y, mo, d, _) => format!("{} {d}, {y}", MONTHS[*mo as usize - 1]),
        Want::DateTime(y, mo, d, t) => format!("{} {d}, {y}, {}", MONTHS[*mo as usize - 1], clock(*t)),
        Want::Numbers(xs) => xs.iter().map(|x| number_text(*x)).collect::<Vec<_>>().join(", "),
    }
}

pub(super) fn number_text(n: f64) -> String {
    if n.fract() == 0.0 {
        format!("{}", n as i64)
    } else {
        format!("{}", (n * 1000.0).round() / 1000.0)
    }
}

/// 1234567 → "1,234,567".
pub(super) fn thousands(n: i64) -> String {
    let digits = n.abs().to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().enumerate() {
        if i > 0 && (digits.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    if n < 0 {
        format!("-{out}")
    } else {
        out
    }
}

/// A task of this skill at this level (1 to `LEVELS`).
pub fn make(skill: Skill, level: u32, rng: &mut Rng) -> Task {
    let level = level.clamp(1, LEVELS);
    let mut t = match skill {
        Skill::Code => super::coding::task(level, rng),
        Skill::Writing => super::writing::task(level, rng),
        Skill::Research => super::research::task(level, rng),
        Skill::Math => math(level, rng),
        Skill::Careful => careful(level, rng),
        Skill::Time => time(level, rng),
        Skill::Format => format_task(level, rng),
        Skill::Tools => tools_task(level, rng),
        Skill::Project => unreachable!("tasks about the person's code are made from its scan (yourcode::task)"),
    };
    t.level = level;
    t
}

fn answer_task(skill: Skill, question: String, want: Want) -> Task {
    Task { skill, level: 1, user: format!("{question}\n\n{ANSWER_LINE}"), tools: None, check: Check::Answer(want) }
}

// ---------- word problems ----------

pub(super) const NAMES: &[&str] = &["Maya", "Leo", "Priya", "Sam", "Ana", "Kenji", "Zara", "Omar", "Lucy", "Diego", "Mei", "Noah", "Ines", "Tariq"];

fn two_names(rng: &mut Rng) -> (&'static str, &'static str) {
    let a = *rng.pick(NAMES);
    loop {
        let b = *rng.pick(NAMES);
        if b != a {
            return (a, b);
        }
    }
}

fn math(level: u32, rng: &mut Rng) -> Task {
    let (q, a) = match level {
        1 => math_basic(rng, 1),
        2 => math_basic(rng, 10),
        3 if rng.chance(0.7) => math_multi(rng, 1),
        3 => math_basic(rng, 100),
        4 if rng.chance(0.6) => math_big(rng),
        4 => math_multi(rng, 10),
        _ => super::hard::math(level, rng),
    };
    answer_task(Skill::Math, q, Want::Number(a as f64))
}

/// One- or two-step problems; `s` makes the numbers bigger.
fn math_basic(rng: &mut Rng, s: i64) -> (String, i64) {
    let n = *rng.pick(NAMES);
    match rng.range(0, 11) {
        0 => {
            let (a, p, b, q) = (rng.range(2, 6), rng.range(2, 9) * s, rng.range(2, 8), rng.range(1, 5) * s);
            let total = a * p + b * q;
            let bill = [20, 50, 100, 200, 500, 1000, 2000, 5000, 10000].into_iter().find(|b| *b > total).unwrap_or(10000);
            (
                format!("{n} buys {a} notebooks at ${} each and {b} pens at ${} each, paying with ${}. How much change, in dollars, should {n} get back?", thousands(p), thousands(q), thousands(bill)),
                bill - total,
            )
        }
        1 => {
            let (v, w, m) = (rng.range(4, 12) * 10, *rng.pick(&[48, 60, 72, 84, 96]), *rng.pick(&[15, 20, 30, 40, 45]));
            let h = rng.range(1, 4) + if s > 1 { rng.range(1, 6) } else { 0 };
            (format!("A train travels at {v} km/h for {h} hours, then at {w} km/h for {m} minutes. How many kilometres does it travel in total?"), v * h + w * m / 60)
        }
        2 => {
            let (p, d, c) = (rng.range(3, 15) * 20 * s, *rng.pick(&[10, 15, 20, 25, 30, 40, 50]), rng.range(1, 3) * 5 * s);
            (format!("A jacket costs ${}. It's {d}% off, and then a ${} coupon is taken off the sale price. What is the final price in dollars?", thousands(p), thousands(c)), p - p * d / 100 - c)
        }
        3 => {
            let (f, e) = (rng.range(3, 7), rng.range(6, 15) * s);
            let (k, g) = (rng.range(1, f - 1), rng.range(1, e - 3));
            (
                format!("{} stickers are shared equally among {f} friends, and the {k} left over go back in the box. Then each friend gives {g} of theirs to a younger cousin. How many stickers does each friend have now?", thousands(f * e + k)),
                e - g,
            )
        }
        4 => {
            let (a, other) = two_names(rng);
            let (b, x, y) = (rng.range(5, 30), rng.range(2, 12), rng.range(2, 10));
            (format!("{a} is {x} years older than {other}. In {y} years, their ages will add up to {}. How old is {other} now?", 2 * b + x + 2 * y), b)
        }
        5 => {
            let (a, b, h) = (rng.range(12, 40) * s, rng.range(10, 35) * s, rng.range(5, 10));
            let stop = rng.range(1, h - 2);
            (
                format!("Machine A makes {} parts an hour and machine B makes {} parts an hour. Both are switched on for {h} hours, but B is stopped for repairs for {stop} of those hours. How many parts do they make altogether?", thousands(a), thousands(b)),
                a * h + b * (h - stop),
            )
        }
        6 => {
            let start = rng.range(40, 120) * s;
            let (a, d, c) = (rng.range(10 * s, start / 2), rng.range(2, 6), rng.range(12, 30) * s);
            let b = rng.range(5 * s, (start - a + d * c).min(60 * s));
            (
                format!(
                    "A shop starts the day with {} apples. It sells {} in the morning, then a delivery brings {d} crates of {} apples each, and it sells {} more in the afternoon. How many apples are left at closing?",
                    thousands(start),
                    thousands(a),
                    thousands(c),
                    thousands(b)
                ),
                start - a + d * c - b,
            )
        }
        7 => {
            let (l, w, g, c) = (rng.range(8, 30) * s, rng.range(5, 20) * s, rng.range(1, 3), rng.range(3, 12));
            (format!("A rectangular field is {} m long and {} m wide. Fencing costs ${c} a metre, and a {g} m gateway is left without fencing. How much does the fencing cost in dollars?", thousands(l), thousands(w)), (2 * (l + w) - g) * c)
        }
        8 => {
            let (have, per) = (rng.range(0, 20) * 5 * s, rng.range(3, 15) * 5 * s);
            let goal = have + rng.range(per * 2, per * 12);
            (format!("{n} has ${} and saves ${} every week. After how many weeks will {n} have at least ${} for the first time?", thousands(have), thousands(per), thousands(goal)), (goal - have + per - 1) / per)
        }
        9 => {
            let (people, k) = (*rng.pick(&[2, 3, 4, 5, 6]), rng.range(20, 90) * s);
            let m = loop {
                let m = rng.range(2, 12);
                if m != people {
                    break m;
                }
            };
            (format!("A recipe for {people} people uses {} grams of flour. How many grams are needed for {m} people?", thousands(people * k)), k * m)
        }
        10 => {
            let p = *rng.pick(&[10, 20, 25, 40, 50]);
            let size = (0..40).map(|_| rng.range(20, 60) * s).find(|z| z * p % 100 == 0 && (z - z * p / 100) % 3 == 0).unwrap_or(60 * s);
            (format!("A school has {} students. {p}% of them walk to school, and a third of the others take the bus. How many students take the bus?", thousands(size)), (size - size * p / 100) / 3)
        }
        _ => {
            let (h, d, r) = (rng.range(3, 9), rng.range(2, 6) * if s > 1 { 5 } else { 1 }, rng.range(12, 30));
            let t = rng.range(1, h * d * r / 10) * 5;
            (format!("{n} works {h} hours a day for {d} days at ${r} an hour, and spends ${} on travel to get there. How much of the pay is left, in dollars?", thousands(t)), h * d * r - t)
        }
    }
}

/// Problems with several steps to keep track of.
fn math_multi(rng: &mut Rng, s: i64) -> (String, i64) {
    match rng.range(0, 5) {
        0 => {
            let (p, a, b) = (rng.range(2, 9) * 10_000 * s, *rng.pick(&[5, 10, 15, 20, 25]), *rng.pick(&[5, 10, 15, 20]));
            (format!("A town of {} people grows by {a}% in one year, then shrinks by {b}% the next year. How many people live there after the two years?", thousands(p)), p * (100 + a) * (100 - b) / 10_000)
        }
        1 => {
            let (a, b) = *rng.pick(&[(2, 3), (3, 6), (4, 12), (2, 6), (6, 12), (3, 12), (4, 6), (5, 20), (6, 10)]);
            (format!("Pipe A fills a tank in {a} hours and pipe B fills it in {b} hours. With both pipes open, how many minutes does it take to fill the tank?"), 60 * a * b / (a + b))
        }
        2 => {
            let (older, younger) = two_names(rng);
            let (b, k) = (rng.range(4, 15), rng.range(3, 6));
            (format!("{older} is {k} times as old as {younger}. In {} years, {older} will be twice as old as {younger}. How old is {older} now?", b * (k - 2)), k * b)
        }
        3 => {
            let (v, w, t) = (rng.range(6, 12) * 10, rng.range(5, 10) * 10, *rng.pick(&[30, 60, 90, 120, 150, 180]));
            (format!("Two trains start {} km apart and travel toward each other, one at {v} km/h and the other at {w} km/h. After how many minutes do they meet?", thousands((v + w) * t / 60)), t)
        }
        4 => {
            let n = *rng.pick(NAMES);
            let tests = rng.range(3, 4);
            let scores: Vec<i64> = (0..tests).map(|_| rng.range(60, 98)).collect();
            let sum: i64 = scores.iter().sum();
            let need = (60..=100).find(|x| (sum + x) % (tests + 1) == 0 && rng.chance(0.5)).or_else(|| (60..=100).find(|x| (sum + x) % (tests + 1) == 0)).unwrap_or(80);
            let list = scores.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
            (format!("{n} scored {list} on {tests} tests. What score on the next test makes the average of all {} tests exactly {}?", tests + 1, (sum + need) / (tests + 1)), need)
        }
        _ => {
            let n = rng.range(20, 60) * s;
            let c = rng.range(4, 15);
            let k = rng.range(n / 2, n - 5);
            let p = rng.range(c + 4, c + 20) * 2;
            (format!("A stall buys {} mugs at ${c} each. It sells {} of them at ${p} each and the rest at half that price. What is the profit in dollars?", thousands(n), thousands(k)), k * p + (n - k) * p / 2 - n * c)
        }
    }
}

/// Big numbers to multiply and count with care.
fn math_big(rng: &mut Rng) -> (String, i64) {
    match rng.range(0, 3) {
        0 => {
            let (b, i, d, r) = (rng.range(1200, 9800), rng.range(12, 48), rng.range(7, 31), rng.range(15, 95));
            (
                format!("A warehouse ships {} boxes a day, but {r} of them are sent back each day. Every box holds {i} items. How many items stay delivered over {d} days?", thousands(b)),
                (b - r) * i * d,
            )
        }
        1 => {
            let k = rng.range(6, 19);
            let lo = rng.range(100, 900);
            let hi = lo + rng.range(500, 5000);
            (format!("How many whole numbers from {lo} to {}, including both, can be divided exactly by {k}?", thousands(hi)), hi / k - (lo - 1) / k)
        }
        2 => {
            let n = rng.range(20, 200);
            (format!("What is the sum of all the even numbers from 2 to {}, including both?", 2 * n), n * (n + 1))
        }
        _ => {
            let who = *rng.pick(NAMES);
            let (a, b, c) = (rng.range(2, 9), rng.range(2, 12), rng.range(3, 20));
            let (p1, p2, p3) = (rng.range(30, 90) * 20, rng.range(8, 30) * 20, rng.range(1, 3) * 20);
            let d = *rng.pick(&[5, 10, 15, 20]);
            let total = a * p1 + b * p2 + c * p3;
            (
                format!(
                    "{who} orders {a} laptops at ${} each, {b} monitors at ${} each and {c} cables at ${p3} each, and gets {d}% off the whole order. How much does {who} pay, in dollars?",
                    thousands(p1),
                    thousands(p2)
                ),
                total * (100 - d) / 100,
            )
        }
    }
}

// ---------- careful counting ----------

const WORDS: &[&str] = &[
    "strawberry", "mississippi", "bookkeeper", "banana", "committee", "occurrence", "parallel", "raspberry", "assessment", "necessary", "possession", "balloon", "coffee", "engineer",
    "letter", "peppermint", "tomorrow", "referee", "success", "address", "cinnamon", "grammar", "embarrass", "pineapple", "tennessee", "giraffe", "hippopotamus", "broccoli",
];
const LONG_WORDS: &[&str] = &[
    "uncharacteristically", "incomprehensibilities", "electroencephalograph", "counterrevolutionaries", "indistinguishable", "responsibilities", "misinterpretation",
    "disproportionately", "parallelogram", "accommodation", "unconsciousness", "interrelationships", "overstimulation", "mispronunciation", "telecommunications",
];
const FRUITS: &[&str] = &["apple", "banana", "cherry", "grape", "mango", "pear", "peach", "plum", "kiwi", "lemon", "orange", "melon", "apricot", "papaya"];
const NOT_FRUITS: &[&str] = &["chair", "carrot", "pencil", "onion", "hammer", "potato", "lamp", "spoon", "cabbage", "guitar", "brick", "sock", "candle", "bucket"];
pub(super) const FILLER: &[&str] = &[
    "the", "old", "river", "ran", "past", "a", "quiet", "village", "where", "children", "played", "near", "tall", "green", "trees", "every", "summer", "morning", "and", "evening", "while",
    "birds", "sang", "softly", "over", "small", "bright", "houses", "stones", "slowly", "sheep", "sleepy", "fields",
];
pub(super) const NOUNS: &[&str] = &[
    "apple", "river", "candle", "zebra", "marble", "hammer", "violin", "garden", "pocket", "ladder", "tunnel", "biscuit", "orbit", "feather", "quartz", "nectar", "willow", "yarn",
    "jacket", "kettle", "lantern", "meadow", "button", "dolphin",
];
const ORDINALS: &[&str] = &["first", "second", "third", "fourth", "fifth", "sixth", "seventh", "eighth", "ninth", "tenth", "eleventh", "twelfth"];

fn sentence(rng: &mut Rng, lo: i64, hi: i64) -> Vec<&'static str> {
    (0..rng.range(lo, hi)).map(|_| *rng.pick(FILLER)).collect()
}

fn capitalized(words: &[&str]) -> String {
    let mut s = words.join(" ");
    if let Some(f) = s.get_mut(0..1) {
        f.make_ascii_uppercase();
    }
    s
}

fn list(xs: &[f64]) -> String {
    xs.iter().map(|x| number_text(*x)).collect::<Vec<_>>().join(", ")
}

/// Numbers with the same whole part and different numbers of decimals
/// (4.9, 4.09, 4.089), shown as written.
fn lookalike_decimals(rng: &mut Rng, count: usize) -> Vec<(String, f64)> {
    let whole = rng.range(1, 20);
    let mut out: Vec<(String, f64)> = Vec::new();
    for _ in 0..count * 10 {
        let places = rng.range(1, 3) as usize;
        let digits = rng.range(1, 10i64.pow(places as u32) - 1);
        let text = format!("{whole}.{digits:0places$}");
        let value: f64 = text.parse().unwrap_or(0.0);
        if !text.ends_with('0') && !out.iter().any(|(_, v)| (*v - value).abs() < 1e-9) {
            out.push((text, value));
            if out.len() == count {
                break;
            }
        }
    }
    out
}

fn careful(level: u32, rng: &mut Rng) -> Task {
    let (q, want) = match level {
        1 => careful_basic(rng),
        2 => careful_longer(rng),
        3 => careful_sentences(rng),
        4 => careful_hard(rng),
        _ => super::hard::careful(level, rng),
    };
    answer_task(Skill::Careful, q, want)
}

fn letter_count(rng: &mut Rng, text: &str, what: &str) -> (String, Want) {
    let letters: Vec<char> = text.chars().filter(|c| c.is_alphabetic()).collect();
    let repeated: Vec<char> = letters.iter().copied().filter(|c| letters.iter().filter(|x| *x == c).count() > 1).collect();
    let l = if !repeated.is_empty() && rng.chance(0.85) { *rng.pick(&repeated) } else { *rng.pick(&letters) };
    (format!("How many times does the letter \"{l}\" appear in {what} \"{text}\"?"), Want::Number(letters.iter().filter(|c| **c == l).count() as f64))
}

fn careful_basic(rng: &mut Rng) -> (String, Want) {
    match rng.range(0, 7) {
        0 => {
            let w = *rng.pick(WORDS);
            letter_count(rng, w, "the word")
        }
        1 => {
            let xs = lookalike_decimals(rng, 2);
            let best = xs.iter().map(|x| x.1).fold(f64::MIN, f64::max);
            (format!("Which number is larger: {} or {}?", xs[0].0, xs[1].0), Want::Number(best))
        }
        2 => {
            let mut xs: Vec<f64> = (0..6).map(|_| rng.range(1, 999) as f64).collect();
            let shown = list(&xs);
            xs.sort_by(f64::total_cmp);
            (format!("Sort these numbers from smallest to largest: {shown}"), Want::Numbers(xs))
        }
        3 => {
            let words = sentence(rng, 7, 14);
            (format!("How many words are in this sentence? \"{}.\"", capitalized(&words)), Want::Number(words.len() as f64))
        }
        4 => {
            let w = *rng.pick(WORDS);
            (format!("Spell the word \"{w}\" backwards."), Want::Word(w.chars().rev().collect()))
        }
        5 => {
            let w = *rng.pick(WORDS);
            let i = rng.range(0, (w.chars().count().min(8) - 1) as i64) as usize;
            (format!("What is the {} letter of the word \"{w}\"?", ORDINALS[i]), Want::Word(w.chars().nth(i).unwrap_or('a').to_string()))
        }
        6 => {
            let (mut fruits, mut others) = (FRUITS.to_vec(), NOT_FRUITS.to_vec());
            rng.shuffle(&mut fruits);
            rng.shuffle(&mut others);
            let mut items: Vec<&str> = fruits[..rng.range(2, 6) as usize].to_vec();
            items.extend_from_slice(&others[..rng.range(2, 5) as usize]);
            rng.shuffle(&mut items);
            let count = items.iter().filter(|i| FRUITS.contains(i)).count();
            (format!("How many of these are fruits: {}?", items.join(", ")), Want::Number(count as f64))
        }
        _ => {
            let xs: Vec<i64> = (0..8).map(|_| rng.range(10, 99)).collect();
            let even = xs.iter().filter(|x| *x % 2 == 0).count();
            (format!("How many of these numbers are even: {}?", xs.iter().map(i64::to_string).collect::<Vec<_>>().join(", ")), Want::Number(even as f64))
        }
    }
}

fn careful_longer(rng: &mut Rng) -> (String, Want) {
    match rng.range(0, 5) {
        0 => {
            let w = *rng.pick(LONG_WORDS);
            letter_count(rng, w, "the word")
        }
        1 => {
            let xs = lookalike_decimals(rng, 3);
            let best = xs.iter().map(|x| x.1).fold(f64::MIN, f64::max);
            (format!("Which is the largest: {}, {} or {}?", xs[0].0, xs[1].0, xs[2].0), Want::Number(best))
        }
        2 => {
            let mut xs: Vec<f64> = (0..8).map(|_| rng.range(-99, 999) as f64).collect();
            let shown = list(&xs);
            xs.sort_by(f64::total_cmp);
            (format!("Sort these numbers from smallest to largest: {shown}"), Want::Numbers(xs))
        }
        3 => {
            let words = sentence(rng, 15, 24);
            (format!("How many words are in this sentence? \"{}.\"", capitalized(&words)), Want::Number(words.len() as f64))
        }
        4 => {
            let w = *rng.pick(LONG_WORDS);
            (format!("Spell the word \"{w}\" backwards."), Want::Word(w.chars().rev().collect()))
        }
        _ => {
            let w = *rng.pick(LONG_WORDS);
            let i = rng.range(4, (w.chars().count().min(ORDINALS.len()) - 1) as i64) as usize;
            (format!("What is the {} letter of the word \"{w}\"?", ORDINALS[i]), Want::Word(w.chars().nth(i).unwrap_or('a').to_string()))
        }
    }
}

fn careful_sentences(rng: &mut Rng) -> (String, Want) {
    match rng.range(0, 4) {
        0 => {
            let s = sentence(rng, 8, 12).join(" ");
            letter_count(rng, &s, "this sentence:")
        }
        1 => {
            let mut xs: Vec<f64> = (0..8).map(|_| rng.range(-99, 99) as f64 / 10.0).collect();
            let shown = list(&xs);
            xs.sort_by(f64::total_cmp);
            (format!("Sort these numbers from smallest to largest: {shown}"), Want::Numbers(xs))
        }
        2 => {
            let words = sentence(rng, 10, 16);
            let first = words[rng.range(0, words.len() as i64 - 1) as usize].chars().next().unwrap_or('s');
            let count = words.iter().filter(|w| w.starts_with(first)).count();
            (format!("How many words in this sentence start with the letter \"{first}\"? \"{}.\"", capitalized(&words)), Want::Number(count as f64))
        }
        3 => {
            let words = sentence(rng, 8, 14);
            let i = rng.range(2, (words.len().min(ORDINALS.len()) - 1) as i64) as usize;
            (format!("What is the {} word in this sentence? \"{}.\"", ORDINALS[i], capitalized(&words)), Want::Word(words[i].to_string()))
        }
        _ => {
            let mut xs: Vec<i64> = Vec::new();
            while xs.len() < 8 {
                let x = rng.range(10, 999);
                if !xs.contains(&x) {
                    xs.push(x);
                }
            }
            let shown = xs.iter().map(i64::to_string).collect::<Vec<_>>().join(", ");
            xs.sort_unstable_by(|a, b| b.cmp(a));
            (format!("What is the third largest of these numbers: {shown}?"), Want::Number(xs[2] as f64))
        }
    }
}

fn careful_hard(rng: &mut Rng) -> (String, Want) {
    match rng.range(0, 4) {
        0 => {
            let words = sentence(rng, 6, 9);
            let back: Vec<String> = words.iter().rev().map(|w| w.to_string()).collect();
            (format!("Write the words of this sentence in reverse order: \"{}.\"", capitalized(&words)), Want::Words(back))
        }
        1 => {
            let mut nouns = NOUNS.to_vec();
            rng.shuffle(&mut nouns);
            let mut pick: Vec<&str> = nouns[..7].to_vec();
            let shown = pick.join(", ");
            pick.sort_unstable();
            (format!("Put these words in alphabetical order: {shown}"), Want::Words(pick.iter().map(|w| w.to_string()).collect()))
        }
        2 => {
            let words = sentence(rng, 3, 5);
            let phrase = words.join(" ");
            let vowels = phrase.chars().filter(|c| "aeiou".contains(*c)).count();
            (format!("How many vowels (a, e, i, o, u) are in the phrase \"{phrase}\"?"), Want::Number(vowels as f64))
        }
        3 => {
            let xs = lookalike_decimals(rng, 5);
            let least = xs.iter().map(|x| x.1).fold(f64::MAX, f64::min);
            (format!("Which is the smallest of these numbers: {}?", xs.iter().map(|x| x.0.as_str()).collect::<Vec<_>>().join(", ")), Want::Number(least))
        }
        _ => {
            let words = sentence(rng, 4, 6);
            let phrase = words.join(" ");
            (format!("How many letters are in the phrase \"{phrase}\", not counting the spaces?"), Want::Number(phrase.chars().filter(|c| c.is_alphabetic()).count() as f64))
        }
    }
}

// ---------- dates and times ----------

pub(super) const DAYS: &[&str] = &["Monday", "Tuesday", "Wednesday", "Thursday", "Friday", "Saturday", "Sunday"];
pub(super) const MONTHS: &[&str] = &["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];

/// "7:45 PM".
pub(super) fn clock(minutes: u32) -> String {
    let m = minutes % 1440;
    let (h, mm) = (m / 60, m % 60);
    let h12 = if h % 12 == 0 { 12 } else { h % 12 };
    format!("{h12}:{mm:02} {}", if h < 12 { "AM" } else { "PM" })
}

/// "March 3, 2027".
pub(super) fn date_text(d: NaiveDate) -> String {
    format!("{} {}, {}", MONTHS[d.month0() as usize], d.day(), d.year())
}

fn day_in(rng: &mut Rng, year: i32, from_day: i64, to_day: i64) -> NaiveDate {
    NaiveDate::from_ymd_opt(year, 1, 1).unwrap() + Duration::days(rng.range(from_day, to_day))
}

fn weekday(d: NaiveDate) -> String {
    DAYS[d.weekday().num_days_from_monday() as usize].to_lowercase()
}

pub(super) fn duration_text(minutes: u32) -> String {
    let (h, m) = (minutes / 60, minutes % 60);
    match (h, m) {
        (0, m) => format!("{m} minutes"),
        (1, 0) => "1 hour".into(),
        (h, 0) => format!("{h} hours"),
        (1, m) => format!("1 hour {m} minutes"),
        (h, m) => format!("{h} hours {m} minutes"),
    }
}

fn time(level: u32, rng: &mut Rng) -> Task {
    let (q, want) = match level {
        1 => time_basic(rng),
        2 => time_longer(rng),
        3 => time_calendar(rng),
        4 => time_hard(rng),
        _ => super::hard::time(level, rng),
    };
    answer_task(Skill::Time, q, want)
}

fn time_basic(rng: &mut Rng) -> (String, Want) {
    match rng.range(0, 5) {
        0 => {
            let (d, n) = (rng.range(0, 6) as usize, rng.range(3, 100) as usize);
            (format!("Today is {}. What day of the week will it be in {n} days?", DAYS[d]), Want::Word(DAYS[(d + n) % 7].to_lowercase()))
        }
        1 => {
            let (d, n) = (rng.range(0, 6) as usize, rng.range(3, 60) as usize);
            (format!("Today is {}. What day of the week was it {n} days ago?", DAYS[d]), Want::Word(DAYS[(d + 7 - n % 7) % 7].to_lowercase()))
        }
        2 => {
            let start = rng.range(8 * 12, 19 * 12) as u32 * 5;
            let length = rng.range(14, 40) as u32 * 5;
            (format!("A film starts at {} and runs for {}. What time does it end?", clock(start), duration_text(length)), Want::Time((start + length) % 1440))
        }
        3 => {
            let start = rng.range(5 * 12, 11 * 12) as u32 * 5;
            let length = rng.range(30, 120) as u32 * 5;
            (format!("A shift runs from {} to {}. How many minutes long is it?", clock(start), clock(start + length)), Want::Number(length as f64))
        }
        4 => {
            let day = day_in(rng, 2027, 0, 300);
            let then = day + Duration::days(rng.range(5, 60));
            (format!("What is the date {} days after {}?", (then - day).num_days(), date_text(day)), Want::Date(then.year(), then.month(), then.day(), false))
        }
        _ => {
            let (h, m) = (rng.range(1, 5), rng.range(1, 59));
            (format!("How many seconds are there in {h} hours and {m} minutes?"), Want::Number((h * 3600 + m * 60) as f64))
        }
    }
}

fn time_longer(rng: &mut Rng) -> (String, Want) {
    match rng.range(0, 4) {
        0 => {
            let (d, n) = (rng.range(0, 6) as usize, rng.range(100, 1000) as usize);
            (format!("Today is {}. What day of the week will it be in {n} days?", DAYS[d]), Want::Word(DAYS[(d + n) % 7].to_lowercase()))
        }
        1 => {
            let day = day_in(rng, 2027, 0, 364);
            let then = day + Duration::days(rng.range(60, 400));
            (format!("What is the date {} days after {}? Give the full date with the year.", (then - day).num_days(), date_text(day)), Want::Date(then.year(), then.month(), then.day(), true))
        }
        2 => {
            let start = rng.range(21 * 12, 23 * 12 + 11) as u32 * 5;
            let length = rng.range(60, 108) as u32 * 5;
            (format!("A night shift runs from {} to {} the next morning. How many minutes long is it?", clock(start), clock(start + length)), Want::Number(length as f64))
        }
        3 => {
            let start = rng.range(21 * 12, 23 * 12 + 6) as u32 * 5;
            let length = rng.range(19, 47) as u32 * 5;
            (format!("A late film starts at {} and runs for {}. What time does it end?", clock(start), duration_text(length)), Want::Time((start + length) % 1440))
        }
        _ => {
            let (d, h) = (rng.range(2, 9), rng.range(1, 23));
            (format!("How many minutes are there in {d} days and {h} hours?"), Want::Number((d * 1440 + h * 60) as f64))
        }
    }
}

fn time_calendar(rng: &mut Rng) -> (String, Want) {
    let anchor = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap();
    match rng.range(0, 3) {
        0 => {
            let d = day_in(rng, 2027, 1, 364);
            (format!("{} was a {}. What day of the week is {}?", date_text(anchor), DAYS[anchor.weekday().num_days_from_monday() as usize], date_text(d)), Want::Word(weekday(d)))
        }
        1 => {
            let a = day_in(rng, 2027, 0, 300);
            let b = a + Duration::days(rng.range(20, 64));
            (format!("How many days after {} is {}?", date_text(a), date_text(b)), Want::Number((b - a).num_days() as f64))
        }
        2 => {
            let h = rng.range(2, 11) as u32;
            let t = rng.range(0, 287) as u32 * 5;
            if rng.chance(0.5) {
                (format!("Port Ellis is {h} hours ahead of Marrow Bay. When it is {} in Marrow Bay, what time is it in Port Ellis?", clock(t)), Want::Time((t + h * 60) % 1440))
            } else {
                (format!("Port Ellis is {h} hours behind Marrow Bay. When it is {} in Marrow Bay, what time is it in Port Ellis?", clock(t)), Want::Time((t + 1440 - h * 60) % 1440))
            }
        }
        _ => {
            // 2028 is a leap year: February has 29 days.
            let day = day_in(rng, 2028, 30, 58);
            let then = day + Duration::days(rng.range(5, 30));
            (format!("What is the date {} days after {}?", (then - day).num_days(), date_text(day)), Want::Date(then.year(), then.month(), then.day(), false))
        }
    }
}

fn time_hard(rng: &mut Rng) -> (String, Want) {
    let anchor = NaiveDate::from_ymd_opt(2027, 1, 1).unwrap();
    match rng.range(0, 3) {
        0 => {
            let a = day_in(rng, 2027, 150, 360);
            let b = day_in(rng, 2028, 0, 180);
            (format!("How many days after {} is {}?", date_text(a), date_text(b)), Want::Number((b - a).num_days() as f64))
        }
        1 => {
            let start = rng.range(5 * 12, 9 * 12) as u32 * 5;
            let legs: Vec<u32> = (0..3).map(|_| rng.range(12, 40) as u32 * 5).collect();
            let stop = rng.range(2, 8) as u32 * 5;
            let end = start + legs.iter().sum::<u32>() + 2 * stop;
            (
                format!(
                    "A trip has three legs taking {}, {} and {}, with a {stop}-minute stop between legs. Leaving at {}, what time do you arrive?",
                    duration_text(legs[0]),
                    duration_text(legs[1]),
                    duration_text(legs[2]),
                    clock(start)
                ),
                Want::Time(end % 1440),
            )
        }
        2 => {
            let day = day_in(rng, 2026, 0, 364);
            let then = day + Duration::days(rng.range(500, 1500));
            (format!("What is the date {} days after {}? Give the full date with the year.", thousands((then - day).num_days()), date_text(day)), Want::Date(then.year(), then.month(), then.day(), true))
        }
        _ => {
            let d = day_in(rng, 2028, 60, 365);
            (format!("{} was a {}. What day of the week is {}?", date_text(anchor), DAYS[anchor.weekday().num_days_from_monday() as usize], date_text(d)), Want::Word(weekday(d)))
        }
    }
}

// ---------- following instructions ----------

const TOPICS: &[&str] = &[
    "why sleep matters", "how to make a cup of tea", "the water cycle", "tips for saving money", "why the sky is blue", "how bees make honey", "the benefits of walking",
    "how to start learning a language", "what a volcano is", "how to care for a houseplant", "why recycling helps", "how rainbows form", "the history of the bicycle",
    "how to prepare for a job interview", "how the moon affects tides", "what photosynthesis is", "how to plan a small garden", "why reading is useful",
];
const KEYS: &[&[&str]] = &[&["title", "summary"], &["tip", "reason"], &["answer", "example"], &["title", "summary", "tip"]];
const ENDINGS: &[&str] = &["Hope this helps!", "That's all.", "Thanks for reading."];
const OPENINGS: &[&str] = &["Sure", "Absolutely", "Here", "Okay"];
const INCLUDES: &[&str] = &["simple", "important", "daily", "careful"];

impl Rule {
    fn instruction(self) -> String {
        // Words for the model, not the window: String::from keeps them out of the translation tables.
        match self {
            Rule::Bullets(n) => format!("Use exactly {n} bullet points, each line starting with \"- \", and nothing else."),
            Rule::MaxWords(n) => format!("Use fewer than {n} words."),
            Rule::Lowercase => String::from("Write everything in lowercase letters only."),
            Rule::Caps => String::from("Write everything in capital letters."),
            Rule::Json(keys) => format!("Reply with only a JSON object that has exactly these keys: {}. No code fences and no other text.", keys.join(", ")),
            Rule::EndWith(p) => format!("End your reply with the exact words \"{p}\""),
            Rule::StartWith(w) => format!("Start your reply with the word \"{w}\"."),
            Rule::Include(w, k) => format!("Use the word \"{w}\" at least {k} times."),
            Rule::NoCommas => String::from("Don't use any commas."),
            Rule::Paragraphs(n) => format!("Write exactly {n} paragraphs, separated by a blank line."),
            Rule::Title => String::from("Begin with a title wrapped in double angle brackets, like <<My title>>."),
            Rule::ExactWords(n) => format!("Use exactly {n} words."),
            Rule::Sentences(n) => format!("Write exactly {n} sentences."),
            Rule::JsonList(n) => format!("Reply with only a JSON object with one key, \"items\", holding a list of exactly {n} short strings. No code fences and no other text."),
            Rule::Quoted => String::from("Wrap your whole reply in double quotation marks."),
            Rule::BulletWords(n, k) => format!("Use exactly {n} bullet points, each line starting with \"- \" and each exactly {k} words long, and nothing else."),
            Rule::Acrostic(w) => format!("Write exactly {} lines whose first letters spell {w}, one letter per line, and nothing else.", w.len()),
            Rule::EverySentence(w) => format!("Every sentence must contain the word \"{w}\"."),
            Rule::ShortWords(k) => format!("Use no word longer than {k} letters."),
            Rule::NoLetter(c) => format!("Don't use the letter \"{c}\" anywhere."),
        }
    }

    fn kind(self) -> u8 {
        match self {
            Rule::Bullets(_) => 0,
            Rule::MaxWords(_) => 1,
            Rule::Lowercase => 2,
            Rule::Caps => 3,
            Rule::Json(_) => 4,
            Rule::EndWith(_) => 5,
            Rule::StartWith(_) => 6,
            Rule::Include(..) => 7,
            Rule::NoCommas => 8,
            Rule::Paragraphs(_) => 9,
            Rule::Title => 10,
            Rule::ExactWords(_) => 11,
            Rule::Sentences(_) => 12,
            Rule::JsonList(_) => 13,
            Rule::Quoted => 14,
            Rule::BulletWords(..) => 15,
            Rule::Acrostic(_) => 16,
            Rule::EverySentence(_) => 17,
            Rule::ShortWords(_) => 18,
            Rule::NoLetter(_) => 19,
        }
    }

    /// The two can't both be kept (or would make an odd request).
    fn clashes(self, other: Rule) -> bool {
        let (a, b) = (self.kind().min(other.kind()), self.kind().max(other.kind()));
        a == b
            || [4, 13].contains(&a)
            || [4, 13].contains(&b)
            || matches!(
                (a, b),
                (0, 5) | (0, 6) | (0, 9) | (0, 10) | (0, 12) | (0, 14) | (1, 11) | (2, 3) | (2, 5) | (2, 6) | (2, 10) | (3, 5) | (3, 6) | (3, 10) | (5, 14) | (6, 10) | (6, 14) | (9, 10) | (9, 14) | (10, 11) | (10, 12) | (10, 14)
            )
            // The harder rules shape the whole reply, so most of the others clash.
            || (b == 15 && [0, 5, 6, 9, 10, 11, 12, 14].contains(&a))
            || (b == 16 && [0, 5, 6, 9, 10, 12, 14, 15].contains(&a))
            || (b == 17 && [0, 10, 15, 16].contains(&a))
            || (b == 18 && [6, 7].contains(&a))
            || (b == 19 && [5, 6, 7, 16, 17].contains(&a))
    }

    fn holds(self, text: &str) -> bool {
        let t = text.trim();
        let lines: Vec<&str> = t.lines().map(str::trim).filter(|l| !l.is_empty()).collect();
        let words = t.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count();
        match self {
            Rule::Bullets(n) => lines.len() == n && lines.iter().all(|l| l.starts_with("- ") || l.starts_with("* ") || l.starts_with("• ")),
            Rule::MaxWords(n) => words < n,
            Rule::Lowercase => !t.chars().any(char::is_uppercase),
            Rule::Caps => !t.chars().any(char::is_lowercase),
            Rule::Json(keys) => serde_json::from_str::<Value>(t).ok().and_then(|v| v.as_object().cloned()).is_some_and(|o| {
                o.len() == keys.len() && keys.iter().all(|k| o.get(*k).and_then(Value::as_str).is_some_and(|s| !s.trim().is_empty()))
            }),
            Rule::EndWith(p) => t.ends_with(p),
            Rule::StartWith(w) => t.split(|c: char| !c.is_alphanumeric()).next() == Some(w),
            Rule::Include(w, k) => t.to_lowercase().split(|c: char| !c.is_alphanumeric()).filter(|x| *x == w).count() >= k,
            Rule::NoCommas => !t.contains(','),
            Rule::Paragraphs(n) => PARAGRAPH_BREAK.split(t).filter(|p| !p.trim().is_empty()).count() == n,
            Rule::Title => lines.first().is_some_and(|l| TITLE.is_match(l)),
            Rule::ExactWords(n) => words == n,
            Rule::Sentences(n) => SENTENCE_END.split(t.trim_matches(|c| c == '"' || c == '“' || c == '”')).filter(|s| s.chars().any(char::is_alphanumeric)).count() == n,
            Rule::JsonList(n) => serde_json::from_str::<Value>(t).ok().and_then(|v| v.as_object().cloned()).is_some_and(|o| {
                o.len() == 1 && o.get("items").and_then(Value::as_array).is_some_and(|xs| xs.len() == n && xs.iter().all(|x| x.as_str().is_some_and(|s| !s.trim().is_empty())))
            }),
            Rule::BulletWords(n, k) => {
                lines.len() == n
                    && lines.iter().all(|l| {
                        let body = l.strip_prefix("- ").or_else(|| l.strip_prefix("* ")).or_else(|| l.strip_prefix("• "));
                        body.is_some_and(|b| b.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count() == k)
                    })
            }
            Rule::Acrostic(w) => {
                lines.len() == w.len() && lines.iter().zip(w.chars()).all(|(l, c)| l.chars().find(|x| x.is_alphabetic()).is_some_and(|x| x.eq_ignore_ascii_case(&c)))
            }
            Rule::EverySentence(w) => {
                let parts: Vec<&str> = SENTENCE_END.split(t).filter(|s| s.chars().any(char::is_alphanumeric)).collect();
                !parts.is_empty() && parts.iter().all(|s| s.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|x| x == w))
            }
            Rule::ShortWords(k) => t.split(|c: char| !c.is_alphabetic()).all(|w| w.chars().count() <= k),
            Rule::NoLetter(c) => !t.to_lowercase().contains(c),
            Rule::Quoted => (t.starts_with('"') && t.ends_with('"') || t.starts_with('“') && t.ends_with('”')) && t.chars().count() > 2,
        }
    }
}

static PARAGRAPH_BREAK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\n\s*\n").unwrap());
static TITLE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^<<[^<>]+>>$").unwrap());
static SENTENCE_END: LazyLock<Regex> = LazyLock::new(|| Regex::new(r#"[.!?]+["”]?(\s+|$)"#).unwrap());

fn rule(rng: &mut Rng, level: u32) -> Rule {
    let kinds = match level {
        1 | 2 => 11,
        3 | 4 => 15,
        _ => 20,
    };
    match rng.range(0, kinds - 1) {
        0 => Rule::Bullets(rng.range(3, 5) as usize),
        1 => Rule::MaxWords(*rng.pick(&[30, 45, 60])),
        2 => Rule::Lowercase,
        3 => Rule::Caps,
        4 => Rule::Json(*rng.pick(KEYS)),
        5 => Rule::EndWith(*rng.pick(ENDINGS)),
        6 => Rule::StartWith(*rng.pick(OPENINGS)),
        7 => Rule::Include(*rng.pick(INCLUDES), rng.range(2, 3) as usize),
        8 => Rule::NoCommas,
        9 => Rule::Paragraphs(rng.range(2, 3) as usize),
        10 => Rule::Title,
        11 => Rule::ExactWords(rng.range(15, 40) as usize),
        12 => Rule::Sentences(rng.range(2, 5) as usize),
        13 => Rule::JsonList(rng.range(3, 6) as usize),
        14 => Rule::Quoted,
        15 => Rule::BulletWords(rng.range(3, 5) as usize, rng.range(5, 9) as usize),
        16 => Rule::Acrostic(*rng.pick(&["PLANT", "RIVER", "LIGHT", "STONE", "HONEY", "CLOUD"])),
        17 => Rule::EverySentence(*rng.pick(&["water", "light", "home", "time"])),
        18 => Rule::ShortWords(rng.range(6, 7) as usize),
        _ => Rule::NoLetter(*rng.pick(&['a', 't', 'o'])),
    }
}

fn format_task(level: u32, rng: &mut Rng) -> Task {
    let want = match level {
        1..=3 => level as usize,
        4 | 5 => 3,
        _ => 4,
    };
    // Level 4 always counts something exactly; 5 and 6 start with a rule
    // that shapes the whole reply (6: no letter "e" or an acrostic).
    let first = match level {
        4 => match rng.range(0, 2) {
            0 => Rule::ExactWords(rng.range(15, 40) as usize),
            1 => Rule::Sentences(rng.range(3, 6) as usize),
            _ => Rule::Bullets(rng.range(4, 6) as usize),
        },
        5 => match rng.range(0, 2) {
            0 => Rule::BulletWords(rng.range(3, 5) as usize, rng.range(5, 9) as usize),
            1 => Rule::EverySentence(*rng.pick(&["water", "light", "home", "time"])),
            _ => Rule::ShortWords(rng.range(6, 7) as usize),
        },
        6 => {
            if rng.chance(0.5) {
                Rule::NoLetter('e')
            } else {
                Rule::Acrostic(*rng.pick(&["PLANT", "RIVER", "LIGHT", "STONE", "HONEY", "CLOUD"]))
            }
        }
        _ => rule(rng, level),
    };
    let mut rules = vec![first];
    for _ in 0..40 {
        if rules.len() >= want {
            break;
        }
        let r = rule(rng, level);
        if rules.iter().all(|x| !x.clashes(r)) {
            rules.push(r);
        }
    }
    let asks = rules.iter().map(|r| r.instruction()).collect::<Vec<_>>().join(" ");
    Task { skill: Skill::Format, level, user: format!("Write a short explanation of {}. {asks}", rng.pick(TOPICS)), tools: None, check: Check::Format(rules) }
}

// ---------- using tools ----------

fn tool(name: &str, description: &str, properties: Value, required: &[&str]) -> Value {
    json!({ "type": "function", "function": { "name": name, "description": description, "parameters": { "type": "object", "properties": properties, "required": required } } })
}

fn all_tools() -> Vec<Value> {
    let s = |d: &str| json!({ "type": "string", "description": d });
    vec![
        tool("get_weather", "Get the current weather for a city.", json!({ "city": s("The city name") }), &["city"]),
        tool("set_timer", "Start a countdown timer.", json!({ "minutes": { "type": "integer", "description": "Length in minutes" }, "label": s("What it's for") }), &["minutes"]),
        tool("create_note", "Save a note.", json!({ "title": s("The note's title"), "text": s("What the note says") }), &["title", "text"]),
        tool("web_search", "Search the web.", json!({ "query": s("What to search for") }), &["query"]),
        tool(
            "convert_currency",
            "Convert an amount of money between currencies.",
            json!({ "amount": { "type": "number" }, "from": s("Three-letter currency code, like USD"), "to": s("Three-letter currency code, like EUR") }),
            &["amount", "from", "to"],
        ),
        tool("send_message", "Send a text message to a contact.", json!({ "to": s("The contact's name"), "text": s("The message") }), &["to", "text"]),
        tool("add_event", "Add an event to the calendar.", json!({ "title": s("The event"), "date": s("YYYY-MM-DD"), "time": s("HH:MM, 24-hour clock") }), &["title", "date"]),
    ]
}

fn tool_name(t: &Value) -> &str {
    t["function"]["name"].as_str().unwrap_or("")
}

const CITIES: &[&str] = &["Paris", "Tokyo", "Nairobi", "Lima", "Oslo", "Toronto", "Cairo", "Sydney", "Denver", "Mumbai"];
const CURRENCIES: &[(&str, &str)] = &[("US dollars", "USD"), ("euros", "EUR"), ("Japanese yen", "JPY"), ("British pounds", "GBP"), ("Canadian dollars", "CAD")];
const PLAIN_QUESTIONS: &[(&str, &[&str])] = &[
    ("What is the capital of Japan?", &["tokyo"]),
    ("How many legs does a spider have?", &["8", "eight"]),
    ("What colour do you get by mixing blue and yellow?", &["green"]),
    ("Translate \"thank you\" into Spanish.", &["gracias"]),
    ("What is 12 times 12?", &["144"]),
    ("Who wrote Romeo and Juliet?", &["shakespeare"]),
    ("What gas do plants take in from the air?", &["carbon dioxide", "co2", "co₂"]),
    ("What is the largest planet in the solar system?", &["jupiter"]),
    ("How many days are there in a leap year?", &["366"]),
    ("What is the freezing point of water in Celsius?", &["0", "zero"]),
];
/// Questions that sound like a tool's job but need none.
const TRAPS: &[(&str, &[&str])] = &[
    ("What is the French word for weather?", &["météo", "meteo", "temps"]),
    ("How many minutes are there in 3 hours?", &["180"]),
    ("Which is worth more, a euro or a euro cent?", &["euro"]),
    ("Spell the word \"calendar\" backwards.", &["radnelac"]),
    ("What does the abbreviation USD stand for?", &["united states dollar", "us dollar", "u.s. dollar"]),
    ("Is a note in music something you write down or something you hear? Answer in one short sentence.", &["hear", "sound"]),
];
/// Spoken lengths of time, in minutes.
const SPOKEN_TIMES: &[(&str, u32)] = &[
    ("an hour and a half", 90),
    ("two and a half hours", 150),
    ("a quarter of an hour", 15),
    ("half an hour", 30),
    ("three quarters of an hour", 45),
    ("an hour and ten minutes", 70),
];
/// Spoken times of day, in minutes after midnight.
const SPOKEN_CLOCK: &[(&str, u32)] = &[
    ("half past seven in the evening", 19 * 60 + 30),
    ("quarter to nine in the morning", 8 * 60 + 45),
    ("noon", 12 * 60),
    ("ten past four in the afternoon", 16 * 60 + 10),
    ("quarter past six in the evening", 18 * 60 + 15),
    ("half past eleven in the morning", 11 * 60 + 30),
];

fn tools_task(level: u32, rng: &mut Rng) -> Task {
    if level >= 5 {
        let (user, check, needs) = super::hard::tools(level, rng);
        return Task { skill: Skill::Tools, level, user, tools: Some(offer(rng, level, &needs)), check };
    }
    let s = |x: &str| x.to_string();
    let has = |xs: &[&str]| Arg::Has(xs.iter().map(|x| x.to_string()).collect());
    let call = |name: &str, args: Vec<(&str, Arg)>| Check::Call { name: name.to_string(), args: args.into_iter().map(|(k, v)| (k.to_string(), v)).collect() };
    let hhmm = |m: u32| Arg::Text(format!("{:02}:{:02}", m / 60, m % 60));
    let iso = |d: NaiveDate| Arg::Text(d.format("%Y-%m-%d").to_string());
    let (user, check) = match rng.range(0, 7) {
        0 => {
            let c = *rng.pick(CITIES);
            let asks: &[&str] = if level == 1 {
                &["What's the weather like in {c} right now?", "Do I need an umbrella in {c} today?", "Is it cold in {c} at the moment?", "Check the weather in {c} for me."]
            } else {
                &["I'm flying to {c} tomorrow morning. What's it like there right now?", "My sister lives in {c}. Is it raining where she is?", "Should I pack a coat for {c}? Check the conditions there now."]
            };
            (rng.pick(asks).replace("{c}", c), call("get_weather", vec![("city", has(&[c]))]))
        }
        1 => {
            let thing = *rng.pick(&["pasta", "laundry", "bread", "cookies", "tea"]);
            let (ask, minutes) = match level {
                1 => {
                    let (h, m) = (rng.range(0, 2) as u32, *rng.pick(&[5, 10, 15, 20, 25, 30, 45]));
                    let ask = if h == 0 { format!("Set a timer for {m} minutes for the {thing}.") } else { format!("Start a timer for {} for the {thing}.", duration_text(h * 60 + m)) };
                    (ask, h * 60 + m)
                }
                2 => {
                    let (spoken, m) = *rng.pick(SPOKEN_TIMES);
                    (format!("Set a timer for {spoken}, it's for the {thing}."), m)
                }
                3 => {
                    let now = rng.range(9 * 12, 20 * 12) as u32 * 5;
                    let m = rng.range(2, 34) as u32 * 5;
                    (format!("It's {} now. Set a timer that goes off at {} for the {thing}.", clock(now), clock(now + m)), m)
                }
                _ => {
                    if rng.chance(0.5) {
                        let (h, m) = (rng.range(1, 3) as u32, rng.range(1, 10) as u32 * 5);
                        (format!("Set a timer for {h} hour{} minus {m} minutes.", if h == 1 { "" } else { "s" }), h * 60 - m)
                    } else {
                        let (n, k) = (rng.range(2, 5) as u32, rng.range(8, 16) as u32);
                        (format!("I'm baking {n} trays of cookies one after another, {k} minutes each. Set one timer for the whole lot."), n * k)
                    }
                }
            };
            (ask, call("set_timer", vec![("minutes", Arg::Number(minutes as f64))]))
        }
        2 => {
            let notes: &[(&str, &str, &[&str])] =
                &[("Groceries", "milk, eggs and rice", &["milk", "eggs", "rice"]), ("Gate code", "4719", &["4719"]), ("Gift ideas", "a scarf for Sam", &["scarf", "sam"]), ("Book list", "The Hobbit", &["hobbit"])];
            let (title, text, keys) = *rng.pick(notes);
            let ask = if level == 1 {
                format!("Save a note called \"{title}\" that says: {text}")
            } else if level == 2 {
                format!("Remember this for me under \"{title}\": {text}")
            } else {
                format!("Okay, so the thing I keep forgetting is {text}. Can you jot that down? Call it \"{title}\".")
            };
            (ask, call("create_note", vec![("title", Arg::Text(s(title))), ("text", has(keys))]))
        }
        3 => {
            let (ask, key) = if level == 1 {
                let (topic, key) = *rng.pick(&[("the Mars rover", "mars"), ("electric bike prices", "electric bike"), ("the World Cup final", "world cup"), ("solar panel grants", "solar")]);
                (if rng.chance(0.5) { format!("Look up the latest news about {topic}.") } else { format!("Search the web for {topic}.") }, key)
            } else {
                let (event, key) = *rng.pick(&[("Tour de France", "tour de france"), ("Nobel Prize in Physics", "nobel"), ("Eurovision Song Contest", "eurovision"), ("Super Bowl", "super bowl")]);
                (format!("Find out who won the most recent {event}."), key)
            };
            (ask, call("web_search", vec![("query", has(&[key]))]))
        }
        4 => {
            let (from, to) = loop {
                let (a, b) = (*rng.pick(CURRENCIES), *rng.pick(CURRENCIES));
                if a.1 != b.1 {
                    break (a, b);
                }
            };
            // The hardest level counts dollars: into something else.
            let to = if level >= 4 && to.1 == "USD" { CURRENCIES[1] } else { to };
            let (ask, amount, code) = match level {
                1 => {
                    let amount = rng.range(5, 500);
                    (format!("How much is {amount} {} in {}?", from.0, to.0), amount, from.1)
                }
                2 => {
                    let (word, code) = *rng.pick(&[("bucks", "USD"), ("quid", "GBP")]);
                    let amount = rng.range(5, 300);
                    let to = if to.1 == code { CURRENCIES[1] } else { to };
                    return Task {
                        skill: Skill::Tools,
                        level,
                        user: format!("What's {amount} {word} in {}?", to.0),
                        tools: Some(offer(rng, level, &["convert_currency"])),
                        check: call("convert_currency", vec![("amount", Arg::Number(amount as f64)), ("from", Arg::Text(s(code))), ("to", Arg::Text(s(to.1)))]),
                    };
                }
                3 => {
                    let (n, each) = (rng.range(2, 6), rng.range(12, 95));
                    (format!("I'm buying {n} tickets at {each} {} each. What's the total in {}?", from.0, to.0), n * each, from.1)
                }
                _ => {
                    let (count, word, note) = (rng.range(2, 4), *rng.pick(&[("twenty", 20), ("fifty", 50), ("ten", 10)]), *rng.pick(&[("five", 5), ("ten", 10)]));
                    let counted = ["", "", "two", "three", "four"][count as usize];
                    let amount = count * word.1 + note.1;
                    (format!("I have {counted} {}-dollar bills and a {}. How much is that in {}?", word.0, note.0, to.0), amount, "USD")
                }
            };
            (ask, call("convert_currency", vec![("amount", Arg::Number(amount as f64)), ("from", Arg::Text(s(code))), ("to", Arg::Text(s(to.1)))]))
        }
        5 => {
            let who = *rng.pick(&["Mom", "Alex", "Jordan", "Priya"]);
            if level == 1 {
                let late = (rng.range(1, 6) * 5).to_string();
                (format!("Text {who} that I'll be {late} minutes late."), call("send_message", vec![("to", has(&[who])), ("text", has(&[late.as_str()]))]))
            } else {
                let from = rng.range(9 * 2, 16 * 2) as u32 * 30;
                let to = from + rng.range(1, 4) as u32 * 30;
                let new = clock(to);
                let key = new.split(' ').next().unwrap_or("").to_string();
                (format!("Let {who} know the meeting moved from {} to {new}.", clock(from)), call("send_message", vec![("to", has(&[who])), ("text", has(&[key.as_str()]))]))
            }
        }
        6 => {
            let title = *rng.pick(&["Dentist", "Team lunch", "Piano lesson", "Car service"]);
            match level {
                1 => {
                    let day = day_in(rng, 2027, 0, 364);
                    let at = rng.range(8 * 4, 19 * 4) as u32 * 15;
                    (format!("Put \"{title}\" on my calendar for {} at {}.", date_text(day), clock(at)), call("add_event", vec![("title", has(&[title])), ("date", iso(day)), ("time", hhmm(at))]))
                }
                2 => {
                    let today = day_in(rng, 2027, 0, 360);
                    let at = rng.range(8 * 4, 19 * 4) as u32 * 15;
                    let day = today + Duration::days(2);
                    (
                        format!("Today is {}, {}. Book \"{title}\" for the day after tomorrow at {}.", DAYS[today.weekday().num_days_from_monday() as usize], date_text(today), clock(at)),
                        call("add_event", vec![("title", has(&[title])), ("date", iso(day)), ("time", hhmm(at))]),
                    )
                }
                3 => {
                    let today = day_in(rng, 2027, 0, 340);
                    let n = rng.range(3, 20);
                    let (spoken, at) = *rng.pick(SPOKEN_CLOCK);
                    let day = today + Duration::days(n);
                    (format!("Today is {}. Put \"{title}\" on my calendar in {n} days at {spoken}.", date_text(today)), call("add_event", vec![("title", has(&[title])), ("date", iso(day)), ("time", hhmm(at))]))
                }
                _ => {
                    let base = NaiveDate::from_ymd_opt(2027, *rng.pick(&[1, 2, 3, 4, 6, 8, 9, 11, 12]), 1).unwrap();
                    let last = (base + Duration::days(32)).with_day(1).unwrap() - Duration::days(1);
                    let start = last - Duration::days(rng.range(0, 3));
                    let n = rng.range(2, 6);
                    let (spoken, at) = *rng.pick(SPOKEN_CLOCK);
                    let day = start + Duration::days(n);
                    (format!("My appointment is {n} days after {}, at {spoken}. Add \"{title}\" to my calendar.", date_text(start)), call("add_event", vec![("title", has(&[title])), ("date", iso(day)), ("time", hhmm(at))]))
                }
            }
        }
        _ => {
            let (q, says) = if level >= 3 && rng.chance(0.6) { *rng.pick(TRAPS) } else { *rng.pick(PLAIN_QUESTIONS) };
            (s(q), Check::NoCall { says: says.iter().map(|x| x.to_string()).collect() })
        }
    };
    let right = match &check {
        Check::Call { name, .. } => vec![name.clone()],
        _ => Vec::new(),
    };
    let right: Vec<&str> = right.iter().map(String::as_str).collect();
    Task { skill: Skill::Tools, level, user, tools: Some(offer(rng, level, &right)), check }
}

/// The tools a task offers, in any order: the ones it needs and others, 3
/// up to level 2 and 4 after (more wouldn't fit a 512-token training
/// example with Qwen's tool instructions).
fn offer(rng: &mut Rng, level: u32, right: &[&str]) -> Value {
    let count = if level <= 2 { 3 } else { 4 };
    let mut all = all_tools();
    rng.shuffle(&mut all);
    let mut picked: Vec<Value> = all.iter().filter(|t| right.contains(&tool_name(t))).cloned().collect();
    picked.extend(all.into_iter().filter(|t| !right.contains(&tool_name(t))).take(count.max(picked.len()) - picked.len()));
    rng.shuffle(&mut picked);
    Value::Array(picked)
}

// ---------- reading a reply ----------

/// What a reply said, its thinking left out.
#[derive(Debug, Default)]
pub struct Reply {
    pub text: String,
    pub calls: Vec<(String, Value)>,
}

static XML_CALL: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<function=([^>\s]+)>(.*?)</function>").unwrap());
static XML_PARAM: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?s)<parameter=([^>\s]+)>\s*(.*?)\s*</parameter>").unwrap());

/// Reads a model's raw output: its thinking dropped (none if it never
/// finished thinking), tool calls (`<tool_call>` with JSON inside, or the
/// `<function=…>` form) taken out.
pub fn read(raw: &str) -> Option<Reply> {
    let after = match raw.rfind("</think>") {
        Some(i) => &raw[i + "</think>".len()..],
        None if raw.contains("<think>") => return None,
        None => raw,
    };
    let mut reply = Reply::default();
    let mut text = after;
    if let Some(i) = after.find("<tool_call>") {
        text = &after[..i];
        for part in after[i..].split("<tool_call>").skip(1) {
            let body = part.split("</tool_call>").next().unwrap_or("").trim();
            if body.starts_with('{') {
                let v: Value = serde_json::Deserializer::from_str(body).into_iter::<Value>().next()?.ok()?;
                let name = v["name"].as_str()?.to_string();
                let args = match &v["arguments"] {
                    Value::String(s) => serde_json::from_str(s).ok()?,
                    other => other.clone(),
                };
                reply.calls.push((name, args));
            } else if let Some(c) = XML_CALL.captures(body) {
                let mut args = serde_json::Map::new();
                for p in XML_PARAM.captures_iter(&c[2]) {
                    let v = serde_json::from_str::<Value>(&p[2]).ok().filter(|v| !v.is_string()).unwrap_or_else(|| Value::String(p[2].to_string()));
                    args.insert(p[1].to_string(), v);
                }
                reply.calls.push((c[1].to_string(), Value::Object(args)));
            } else {
                return None;
            }
        }
    }
    reply.text = text.trim().to_string();
    Some(reply)
}

static ANSWER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)answer\s*:\s*(.+)$").unwrap());
static NUMBER: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"-?\d+(?:\.\d+)?").unwrap());
static THOUSANDS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d),(\d{3})").unwrap());
static CLOCK: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)(\d{1,2}):(\d{2})\s*([ap])?\.?\s*m?\b").unwrap());
static ISO_DATE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(\d{4})-(\d{2})-(\d{2})").unwrap());
static YEAR: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b(?:19|20)\d{2}\b").unwrap());

/// The last "Answer: …" line's answer, markdown and quotes taken off.
fn final_answer(text: &str) -> Option<String> {
    let line = text.lines().rev().map(|l| l.replace(['*', '_', '#', '`'], "")).find(|l| ANSWER.is_match(l))?;
    let a = ANSWER.captures(&line)?[1].trim().trim_matches(|c: char| matches!(c, '"' | '\'' | '“' | '”' | '.' | ' ')).to_string();
    let a = a.trim_start_matches("\\boxed{").trim_end_matches('}').trim().to_string();
    (!a.is_empty()).then_some(a)
}

fn numbers_in(s: &str) -> Vec<f64> {
    let mut plain = s.replace('−', "-");
    loop {
        let next = THOUSANDS.replace_all(&plain, "$1$2").to_string();
        if next == plain {
            break;
        }
        plain = next;
    }
    NUMBER.find_iter(&plain).filter_map(|m| m.as_str().parse().ok()).collect()
}

fn letters(s: &str) -> String {
    s.chars().filter(|c| c.is_alphabetic()).flat_map(char::to_lowercase).collect()
}

fn words_of(s: &str) -> Vec<String> {
    s.split(|c: char| !c.is_alphabetic()).filter(|w| !w.is_empty()).map(str::to_lowercase).collect()
}

fn answer_matches(want: &Want, a: &str) -> bool {
    match want {
        Want::Number(n) => numbers_in(a).first().is_some_and(|x| (x - n).abs() < 1e-6),
        Want::Word(w) => letters(a) == letters(w) || a.split_whitespace().last().is_some_and(|last| letters(last) == letters(w)),
        Want::Words(ws) => words_of(a) == *ws,
        Want::Numbers(xs) => {
            let got = numbers_in(a);
            got.len() == xs.len() && got.iter().zip(xs).all(|(g, x)| (g - x).abs() < 1e-6)
        }
        Want::Time(m) => CLOCK.captures(a).is_some_and(|c| {
            let (h, mm): (u32, u32) = (c[1].parse().unwrap_or(99), c[2].parse().unwrap_or(99));
            let h = match c.get(3).map(|x| x.as_str().to_ascii_lowercase()) {
                Some(p) if (1..=12).contains(&h) => h % 12 + if p == "p" { 12 } else { 0 },
                Some(_) => 99,
                None => h,
            };
            h * 60 + mm == *m
        }),
        Want::DateTime(year, month, day, minutes) => {
            let clock_at = CLOCK.find(a).map(|m| m.start()).unwrap_or(a.len());
            answer_matches(&Want::Time(*minutes), a) && (answer_matches(&Want::Date(*year, *month, *day, false), &a[..clock_at]) || answer_matches(&Want::Date(*year, *month, *day, false), &a[CLOCK.find(a).map_or(a.len(), |m| m.end())..]))
        }
        Want::Date(year, month, day, need_year) => {
            if let Some(c) = ISO_DATE.captures(a) {
                return c[1].parse::<i32>().ok() == Some(*year) && c[2].parse::<u32>().ok() == Some(*month) && c[3].parse::<u32>().ok() == Some(*day);
            }
            let lower = a.to_lowercase();
            let years: Vec<i32> = YEAR.find_iter(&lower).filter_map(|m| m.as_str().parse().ok()).collect();
            if years.iter().any(|y| y != year) || (*need_year && years.is_empty()) {
                return false;
            }
            let rest = YEAR.replace_all(&lower, "");
            let named = MONTHS.iter().position(|m| rest.contains(&m.to_lowercase()) || rest.split(|c: char| !c.is_alphabetic()).any(|w| w == m[..3].to_lowercase()));
            named.is_some_and(|i| i as u32 + 1 == *month) && numbers_in(&rest).first().is_some_and(|d| *d == *day as f64)
        }
    }
}

fn arg_matches(want: &Arg, got: Option<&Value>) -> bool {
    let Some(got) = got else { return false };
    let text = match got {
        Value::String(s) => s.clone(),
        other => other.to_string(),
    };
    match want {
        Arg::Text(t) => text.trim().eq_ignore_ascii_case(t.trim()),
        Arg::Has(parts) => parts.iter().all(|p| text.to_lowercase().contains(&p.to_lowercase())),
        Arg::Number(n) => text.trim().parse::<f64>().is_ok_and(|x| (x - n).abs() < 1e-6),
        Arg::HasNumber(n) => numbers_in(&text).iter().any(|x| (x - n).abs() < 1e-6),
    }
}

/// Each wanted call matches a different call made, in any order.
fn calls_match(want: &[(String, Vec<(String, Arg)>)], made: &[(String, Value)]) -> bool {
    if want.len() != made.len() {
        return false;
    }
    let mut used = vec![false; made.len()];
    want.iter().all(|(name, args)| {
        let found = made.iter().enumerate().find(|(i, (n, a))| !used[*i] && n == name && args.iter().all(|(k, w)| arg_matches(w, a.get(k))));
        found.map(|(i, _)| used[i] = true).is_some()
    })
}

/// Whether this reply gets the task right.
pub fn passes(task: &Task, reply: &Reply) -> bool {
    match &task.check {
        Check::Answer(want) => reply.calls.is_empty() && final_answer(&reply.text).is_some_and(|a| answer_matches(want, &a)),
        Check::Format(rules) => reply.calls.is_empty() && !reply.text.is_empty() && rules.iter().all(|r| r.holds(&reply.text)),
        Check::Call { name, args } => {
            reply.calls.len() == 1 && reply.calls[0].0 == *name && args.iter().all(|(k, want)| arg_matches(want, reply.calls[0].1.get(k)))
        }
        Check::NoCall { says } => {
            let lower = reply.text.to_lowercase();
            reply.calls.is_empty() && says.iter().any(|s| lower.contains(s))
        }
        Check::Calls(want) => calls_match(want, &reply.calls),
        Check::Code { name, cases, want } => reply.calls.is_empty() && super::coding::passes(name, cases, want, &reply.text),
        Check::Facts { must, source, max_words, max_sentence_words, sentences, last_line } => {
            reply.calls.is_empty() && facts_hold(&reply.text, must, source.as_deref(), *max_words, *max_sentence_words, *sentences, last_line.as_deref())
        }
        Check::Exact(want) => reply.calls.is_empty() && plain_text(&reply.text) == plain_text(want),
        Check::Cited { want, sources, may, not } => reply.calls.is_empty() && cited(&reply.text, want.as_ref(), sources, may, not),
        Check::Look(look) => reply.calls.is_empty() && look.passes(&reply.text),
    }
}

/// Text as written, give or take quotes around it, a code fence, curly
/// apostrophes and spacing.
fn plain_text(s: &str) -> String {
    let t = s.trim().trim_start_matches("```text").trim_start_matches("```").trim_end_matches("```").trim();
    let t = t.trim_matches(|c| c == '"' || c == '“' || c == '”').replace(['’', '‘'], "'");
    t.split_whitespace().collect::<Vec<_>>().join(" ")
}

/// A fact (one way of writing it) is in the text: a number among its
/// numbers (so 45 isn't found in 2,450), anything else as words.
fn fact_in(lower: &str, numbers: &[f64], variant: &str) -> bool {
    let v = variant.to_lowercase();
    if !v.is_empty() && v.chars().all(|c| c.is_ascii_digit() || c == ',') {
        let n: f64 = v.replace(',', "").parse().unwrap_or(f64::NAN);
        numbers.iter().any(|x| (x - n).abs() < 1e-9)
    } else {
        lower.contains(&v)
    }
}

#[allow(clippy::too_many_arguments)]
fn facts_hold(reply: &str, must: &[Vec<String>], source: Option<&str>, max_words: Option<usize>, max_sentence_words: Option<usize>, sentences: Option<usize>, last_line: Option<&str>) -> bool {
    let t = reply.trim().trim_matches(|c| c == '"' || c == '“' || c == '”').trim();
    if t.is_empty() {
        return false;
    }
    let lower = t.to_lowercase();
    let numbers = numbers_in(t);
    if !must.iter().all(|group| group.iter().any(|v| fact_in(&lower, &numbers, v))) {
        return false;
    }
    if let Some(src) = source {
        let known = numbers_in(src);
        if numbers.iter().any(|n| !known.iter().any(|k| (k - n).abs() < 1e-9)) {
            return false;
        }
    }
    let count = |s: &str| s.split_whitespace().filter(|w| w.chars().any(char::is_alphanumeric)).count();
    if max_words.is_some_and(|m| count(t) > m) {
        return false;
    }
    let parts: Vec<&str> = SENTENCE_END.split(t).filter(|s| s.chars().any(char::is_alphanumeric)).collect();
    if max_sentence_words.is_some_and(|m| parts.iter().any(|s| count(s) > m)) || sentences.is_some_and(|n| parts.len() != n) {
        return false;
    }
    last_line.is_none_or(|me| t.lines().rev().find(|l| !l.trim().is_empty()).is_some_and(|l| l.to_lowercase().contains(&me.to_lowercase())))
}

static CITE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\[(\d+(?:\s*,\s*\d+)*)\]").unwrap());

/// "Not in the sources", said some way.
fn says_unknown(s: &str) -> bool {
    let l = s.to_lowercase();
    ["not in the source", "don't say", "do not say", "doesn't say", "does not say", "not mentioned", "no information", "not stated", "not given"].iter().any(|p| l.contains(p))
}

/// The answer line gives the answer and cites exactly the sources used.
fn cited(text: &str, want: Option<&Want>, sources: &[usize], may: &[usize], not: &[String]) -> bool {
    let Some(a) = final_answer(text) else { return false };
    let mut cites: Vec<usize> = CITE.captures_iter(&a).flat_map(|c| c[1].split(',').filter_map(|n| n.trim().parse().ok()).collect::<Vec<_>>()).collect();
    cites.sort_unstable();
    cites.dedup();
    let bare = CITE.replace_all(&a, "").trim().to_string();
    match want {
        None => cites.is_empty() && says_unknown(&bare),
        Some(w) => {
            let right = match w {
                Want::Word(x) => {
                    let words = words_of(&bare);
                    words.contains(x) && !not.iter().any(|n| words.contains(n))
                }
                other => answer_matches(other, &bare),
            };
            let all = sources.iter().all(|s| cites.contains(s));
            let only = cites.iter().all(|c| sources.contains(c) || may.contains(c));
            right && all && only && !says_unknown(&bare)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn reply(text: &str) -> Reply {
        read(text).unwrap()
    }

    #[test]
    fn the_same_seed_makes_the_same_tasks() {
        // Tasks about the person's code come from a scan (yourcode's tests).
        for skill in Skill::ALL.into_iter().filter(|s| *s != Skill::Project) {
            for level in 1..=LEVELS {
                let (mut a, mut b) = (Rng::new(42), Rng::new(42));
                for _ in 0..20 {
                    assert_eq!(make(skill, level, &mut a).user, make(skill, level, &mut b).user);
                }
            }
        }
    }

    /// Every made-up task can be answered: its hint passes its own check,
    /// and its numbers stay sensible.
    #[test]
    fn every_task_has_a_right_answer() {
        let mut rng = Rng::new(7);
        for _ in 0..300 {
            for skill in [Skill::Math, Skill::Careful, Skill::Time] {
                for level in 1..=LEVELS {
                    let t = make(skill, level, &mut rng);
                    let hint = t.hint().unwrap();
                    assert!(passes(&t, &reply(&format!("Some working.\nAnswer: {hint}"))), "level {level}: {} / {hint}", t.user);
                    if let Check::Answer(Want::Number(n)) = t.check {
                        assert!(n >= 0.0 || t.user.contains('-'), "{} gives {n}", t.user);
                    }
                }
            }
        }
    }

    #[test]
    fn wrong_answers_fail() {
        let t = answer_task(Skill::Math, "q".into(), Want::Number(42.0));
        assert!(passes(&t, &reply("6 x 7 = 42\n**Answer:** 42")));
        assert!(passes(&t, &reply("Answer: $42.")));
        assert!(!passes(&t, &reply("Answer: 43")));
        assert!(!passes(&t, &reply("It's 42.")), "the answer line was asked for");
        let big = answer_task(Skill::Math, "q".into(), Want::Number(14_582_400.0));
        assert!(passes(&big, &reply("Answer: 14,582,400 items")));
        assert!(!passes(&big, &reply("Answer: 14,582,000")));
    }

    #[test]
    fn reads_times_dates_words_and_lists() {
        let film = answer_task(Skill::Time, "q".into(), Want::Time(22 * 60 + 20));
        assert!(passes(&film, &reply("Answer: 10:20 PM")));
        assert!(passes(&film, &reply("Answer: 22:20")));
        assert!(passes(&film, &reply("Answer: 10:20 p.m.")));
        assert!(!passes(&film, &reply("Answer: 10:20 AM")));
        let date = answer_task(Skill::Time, "q".into(), Want::Date(2027, 4, 4, false));
        assert!(passes(&date, &reply("Answer: April 4, 2027")));
        assert!(passes(&date, &reply("Answer: Apr 4")));
        assert!(passes(&date, &reply("Answer: 2027-04-04")));
        assert!(!passes(&date, &reply("Answer: April 5, 2027")));
        assert!(!passes(&date, &reply("Answer: April 4, 2028")));
        let later = answer_task(Skill::Time, "q".into(), Want::Date(2028, 2, 29, true));
        assert!(passes(&later, &reply("Answer: February 29, 2028")));
        assert!(!passes(&later, &reply("Answer: February 29")), "the year was asked for");
        let day = answer_task(Skill::Time, "q".into(), Want::Word("thursday".into()));
        assert!(passes(&day, &reply("Answer: Thursday")));
        assert!(passes(&day, &reply("Answer: It will be Thursday.")));
        assert!(!passes(&day, &reply("Answer: Friday")));
        let sorted = answer_task(Skill::Careful, "q".into(), Want::Numbers(vec![-3.5, 15.0, 200.0]));
        assert!(passes(&sorted, &reply("Answer: −3.5, 15, 200")));
        assert!(!passes(&sorted, &reply("Answer: 15, -3.5, 200")));
        let words = answer_task(Skill::Careful, "q".into(), Want::Words(vec!["apple".into(), "biscuit".into()]));
        assert!(passes(&words, &reply("Answer: Apple, biscuit")));
        assert!(!passes(&words, &reply("Answer: biscuit, apple")));
    }

    #[test]
    fn checks_the_shape_of_a_reply() {
        assert!(Rule::Bullets(3).holds("- one\n- two\n- three"));
        assert!(!Rule::Bullets(3).holds("Here you go:\n- one\n- two\n- three"));
        assert!(Rule::Json(&["tip", "reason"]).holds("{\"tip\": \"walk\", \"reason\": \"health\"}"));
        assert!(!Rule::Json(&["tip", "reason"]).holds("```json\n{\"tip\": \"walk\", \"reason\": \"health\"}\n```"));
        assert!(Rule::JsonList(2).holds("{\"items\": [\"walk\", \"rest\"]}"));
        assert!(!Rule::JsonList(3).holds("{\"items\": [\"walk\", \"rest\"]}"));
        assert!(Rule::Paragraphs(2).holds("One.\n\nTwo."));
        assert!(!Rule::Paragraphs(2).holds("One.\nTwo."));
        assert!(Rule::Title.holds("<<Tea>>\nBoil water."));
        assert!(Rule::Include("simple", 2).holds("A simple idea. Simple!"));
        assert!(Rule::StartWith("Sure").holds("Sure, here it is."));
        assert!(!Rule::StartWith("Sure").holds("Surely not."));
        assert!(Rule::EndWith("That's all.").holds("Tea.\nThat's all.\n"));
        assert!(!Rule::Lowercase.holds("Tea"));
        assert!(Rule::Caps.holds("TEA, 2 CUPS."));
        assert!(Rule::ExactWords(5).holds("Boil water, then add tea."));
        assert!(!Rule::ExactWords(5).holds("Boil the water, then add tea."));
        assert!(Rule::Sentences(3).holds("Boil water. Add tea! Is it ready?"));
        assert!(!Rule::Sentences(3).holds("Boil water. Add tea."));
        assert!(Rule::Quoted.holds("\"Tea is calming.\""));
        assert!(!Rule::Quoted.holds("Tea is \"calming\"."));
    }

    #[test]
    fn instructions_asked_together_can_all_be_kept() {
        let mut rng = Rng::new(3);
        for level in 1..=LEVELS {
            for _ in 0..300 {
                if let Check::Format(rules) = format_task(level, &mut rng).check {
                    assert!(!rules.is_empty() && rules.len() <= 4);
                    for (i, a) in rules.iter().enumerate() {
                        for b in &rules[i + 1..] {
                            assert!(!a.clashes(*b), "{a:?} with {b:?}");
                        }
                    }
                }
            }
        }
    }

    #[test]
    fn reads_tool_calls_in_either_form() {
        let r = reply("<think>\nThe user wants weather.\n</think>\n\n<tool_call>\n{\"name\": \"get_weather\", \"arguments\": {\"city\": \"Paris\"}}\n</tool_call>");
        assert_eq!(r.calls, vec![("get_weather".to_string(), json!({ "city": "Paris" }))]);
        let x = reply("<tool_call>\n<function=set_timer>\n<parameter=minutes>\n25\n</parameter>\n</function>\n</tool_call>");
        assert_eq!(x.calls, vec![("set_timer".to_string(), json!({ "minutes": 25 }))]);
        let t = Task { skill: Skill::Tools, level: 1, user: String::new(), tools: None, check: Check::Call { name: "set_timer".into(), args: vec![("minutes".into(), Arg::Number(25.0))] } };
        assert!(passes(&t, &x));
        assert!(passes(&t, &reply("<tool_call>\n{\"name\": \"set_timer\", \"arguments\": {\"minutes\": \"25\"}}\n</tool_call>")));
        assert!(!passes(&t, &reply("<tool_call>\n{\"name\": \"set_timer\", \"arguments\": {\"minutes\": 30}}\n</tool_call>")));
        assert!(!passes(&t, &reply("I've set a 25 minute timer.")), "saying it isn't doing it");
        let plain = Task { skill: Skill::Tools, level: 1, user: String::new(), tools: None, check: Check::NoCall { says: vec!["tokyo".into()] } };
        assert!(passes(&plain, &reply("The capital of Japan is Tokyo.")));
        assert!(!passes(&plain, &reply("<tool_call>\n{\"name\": \"web_search\", \"arguments\": {\"query\": \"capital of Japan\"}}\n</tool_call>")));
    }

    #[test]
    fn tool_tasks_offer_the_right_tool_and_more_as_levels_rise() {
        let mut rng = Rng::new(11);
        for (level, count) in [(1, 3), (2, 3), (3, 4), (4, 4), (5, 4), (6, 4)] {
            for _ in 0..200 {
                let t = tools_task(level, &mut rng);
                let offered: Vec<String> = t.tools.as_ref().unwrap().as_array().unwrap().iter().map(|x| tool_name(x).to_string()).collect();
                assert_eq!(offered.len(), count, "level {level}");
                if let Check::Calls(calls) = &t.check {
                    assert!(calls.iter().all(|(n, _)| offered.contains(n)), "{offered:?}");
                }
                if let Check::Call { name, args } = &t.check {
                    assert!(offered.contains(name), "{name} not in {offered:?}");
                    if let Some((_, Arg::Number(n))) = args.iter().find(|(k, _)| k == "minutes") {
                        assert!(*n > 0.0, "{}", t.user);
                    }
                }
            }
        }
    }

    #[test]
    fn spoken_tool_details_are_worked_out_right() {
        // "Set a timer for 2 hours minus 15 minutes" is 105 minutes; a date
        // two days after the last day of February 2027 is in March.
        let mut rng = Rng::new(99);
        for _ in 0..300 {
            let t = tools_task(4, &mut rng);
            if let Check::Call { name, args } = &t.check {
                if name == "add_event" {
                    if let Some((_, Arg::Text(d))) = args.iter().find(|(k, _)| k == "date") {
                        assert!(NaiveDate::parse_from_str(d, "%Y-%m-%d").is_ok(), "{d}");
                    }
                }
            }
        }
        assert_eq!(duration_text(105), "1 hour 45 minutes");
        assert_eq!(thousands(14_582_400), "14,582,400");
    }

    #[test]
    fn unfinished_thinking_is_no_reply() {
        assert!(read("<think>\nStill going").is_none());
        assert_eq!(read("<think>\nok\n</think>\n\nHi").unwrap().text, "Hi");
    }
}
