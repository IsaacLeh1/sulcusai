// SPDX-License-Identifier: AGPL-3.0-only
//! Writing practice, on the parts of good writing a program can check:
//! summaries that keep the facts asked for and add none, proofreading with
//! known mistakes to fix, rewrites for a young reader that keep every name
//! and number, and emails that say everything they were asked to.

use super::tasks::{Check, Rng, Skill, Task};

const TOWNS: &[&str] = &["Brightwater", "Fenmoor", "Ashcombe", "Redhollow", "Lindale", "Stoneport", "Marrow Bay", "Port Ellis"];
const PEOPLE: &[&str] = &["Dana Okafor", "Luis Ferreira", "Hana Sato", "Tom Brennan", "Amara Singh", "Petra Novak", "Felix Moreau", "Grace Mensah"];
const MONTHS: &[&str] = &["January", "February", "March", "April", "May", "June", "July", "August", "September", "October", "November", "December"];
const DAYS: &[&str] = &["Monday", "Tuesday", "Wednesday", "Thursday", "Friday"];

/// One way of writing a number the reply may use: "2,400" or "2400".
fn number_forms(n: i64) -> Vec<String> {
    let plain = n.to_string();
    let grouped = super::tasks::thousands(n);
    if plain == grouped {
        vec![plain]
    } else {
        vec![grouped, plain]
    }
}

/// A made-up local news report: its text and the facts in it, each with the
/// ways it may be written.
struct Report {
    text: String,
    numbers: Vec<Vec<String>>,
    names: Vec<Vec<String>>,
    date: Vec<String>,
}

fn report(rng: &mut Rng) -> Report {
    let town = *rng.pick(TOWNS);
    let person = *rng.pick(PEOPLE);
    let month = *rng.pick(MONTHS);
    let day = rng.range(1, 28);
    let (people, money, helpers) = (rng.range(12, 95) * 100, rng.range(15, 240) * 100, rng.range(12, 90));
    let (what, raised) = *rng.pick(&[
        ("charity run", "raised"),
        ("food festival", "raised"),
        ("book fair", "raised"),
        ("river clean-up day", "collected in donations"),
    ]);
    let filler = *rng.pick(&[
        "The weather stayed dry for most of the day, and the mood was cheerful.",
        "Local shops stayed open late, and many families made a day of it.",
        "Music from the main stage could be heard across the town square.",
    ]);
    let text = format!(
        "{town} held its yearly {what} on {month} {day}. About {} people came, more than ever before. {filler} The event {raised} ${} for the town's youth centre. It was organised by {person}, who thanked the {helpers} volunteers who worked through the day. Plans for next year are already under way.",
        super::tasks::thousands(people),
        super::tasks::thousands(money),
    );
    Report {
        text,
        numbers: vec![number_forms(people), number_forms(money), number_forms(helpers)],
        names: vec![vec![town.to_string()], vec![person.to_string(), person.split(' ').next_back().unwrap_or(person).to_string()]],
        date: vec![format!("{month} {day}"), format!("{day} {month}")],
    }
}

fn summarize(level: u32, rng: &mut Rng) -> Task {
    let r = report(rng);
    let (asks, mut must, max_words, sentences) = match level {
        1 => ("Keep the date and how many people came.", vec![r.date.clone(), r.numbers[0].clone()], 70, None),
        2 => ("Keep every number in it.", r.numbers.clone(), 55, None),
        3 => ("Keep every number and every name in it, and don't add anything the report doesn't say.", [r.numbers.clone(), r.names.clone()].concat(), 50, None),
        _ => ("Write exactly two sentences. Keep the date, every number and every name, and don't add anything the report doesn't say.", [r.numbers.clone(), r.names.clone(), vec![r.date.clone()]].concat(), 45, Some(2)),
    };
    must.retain(|m| !m.is_empty());
    Task {
        skill: Skill::Writing,
        level,
        user: format!("Summarize this report in at most {max_words} words. {asks}\n\n\"{}\"\n\nReply with only the summary.", r.text),
        tools: None,
        check: Check::Facts { must, source: Some(r.text), max_words: Some(max_words), max_sentence_words: None, sentences, last_line: None },
    }
}

fn simpler(level: u32, rng: &mut Rng) -> Task {
    let r = report(rng);
    let longest = match level {
        1..=3 => 12,
        4 => 10,
        _ => 8,
    };
    Task {
        skill: Skill::Writing,
        level,
        user: format!(
            "Rewrite this report for a 10-year-old. Use short sentences of at most {longest} words each, keep every name and number, and don't add anything the report doesn't say.\n\n\"{}\"\n\nReply with only the rewritten report.",
            r.text
        ),
        tools: None,
        check: Check::Facts { must: [r.numbers, r.names].concat(), source: Some(r.text), max_words: Some(160), max_sentence_words: Some(longest), sentences: None, last_line: None },
    }
}

fn email(level: u32, rng: &mut Rng) -> Task {
    let to = rng.pick(PEOPLE).split(' ').next().unwrap_or("Sam").to_string();
    let me = *rng.pick(&["Jordan", "Riley", "Casey", "Morgan"]);
    let event = *rng.pick(&["project review", "team lunch", "budget meeting", "site visit"]);
    let month = *rng.pick(MONTHS);
    let day = rng.range(3, 20);
    let later = day + rng.range(1, 6);
    let (day1, day2) = (*rng.pick(DAYS), *rng.pick(DAYS));
    let hour = rng.range(9, 16);
    let at = format!("{}:{:02} {}", if hour > 12 { hour - 12 } else { hour }, *rng.pick(&[0, 30]), if hour >= 12 { "PM" } else { "AM" });
    let words = if level == 1 { 100 } else { 70 };
    let hhmm = at.split(' ').next().unwrap_or("").to_string();
    Task {
        skill: Skill::Writing,
        level,
        user: format!(
            "Write a short, friendly email to {to} saying you can't make the {event} on {day1}, {month} {day}, and asking to move it to {day2}, {month} {later} at {at}. Sign it \"{me}\". Keep it under {words} words, and reply with only the email."
        ),
        tools: None,
        check: Check::Facts {
            must: vec![vec![to.clone()], vec![event.to_string()], vec![format!("{month} {day}"), format!("{day} {month}")], vec![format!("{month} {later}"), format!("{later} {month}")], vec![hhmm]],
            source: None,
            max_words: Some(words),
            max_sentence_words: None,
            sentences: None,
            last_line: Some(me.to_string()),
        },
    }
}

/// Clean sentences, each with mistakes that can be made in it: (right, wrong).
const PROOF: &[(&str, &[(&str, &str)])] = &[
    ("Their new office is much bigger than the old one, and it's closer to the station.", &[("Their", "There"), ("than", "then"), ("it's", "its"), ("bigger", "biger")]),
    ("We definitely need to receive the signed forms before the meeting tomorrow.", &[("definitely", "definately"), ("receive", "recieve"), ("tomorrow", "tommorow")]),
    ("The council decided to separate the two projects because their budgets were too different.", &[("separate", "seperate"), ("their", "there"), ("too", "to"), ("different", "diffrent")]),
    ("You're welcome to bring your own chair if you want to sit near the front.", &[("You're", "Your"), ("your", "you're"), ("welcome", "welcom")]),
    ("The storm didn't affect the bridge, but the road beside it was closed until Friday.", &[("affect", "effect"), ("until", "untill"), ("didn't", "didnt")]),
    ("Whose bag is this, and why is it on the kitchen floor?", &[("Whose", "Who's"), ("kitchen", "kitchin"), ("floor", "flor")]),
    ("It was the beginning of a long week, so everyone was tired by Wednesday.", &[("beginning", "begining"), ("Wednesday", "Wensday"), ("tired", "tierd")]),
    ("The museum's address is printed on the back of every ticket.", &[("museum's", "museums"), ("address", "adress"), ("ticket", "tickit")]),
    ("I believe the package arrived on Monday, although nobody has opened it yet.", &[("believe", "beleive"), ("arrived", "arived"), ("although", "altough")]),
    ("Please don't lose the key, because we only have one copy of it.", &[("lose", "loose"), ("because", "becuase"), ("only", "onlly")]),
    ("The weather was perfect, so we ate our lunch outside by the lake.", &[("weather", "whether"), ("our", "are"), ("outside", "outsied")]),
    ("The team has already finished more tasks than it planned for this month.", &[("already", "allready"), ("than", "then"), ("planned", "planed")]),
    ("She accepted the offer, and she will start her new job next week.", &[("accepted", "excepted"), ("offer", "ofer"), ("week", "weak")]),
    ("It's important to check the address twice before you send anything.", &[("It's", "Its"), ("important", "importent"), ("twice", "twise")]),
];

/// Swaps the first whole-word `right` in `s` for `wrong`.
fn swap_word(s: &str, right: &str, wrong: &str) -> Option<String> {
    let mut from = 0;
    while let Some(off) = s[from..].find(right) {
        let at = from + off;
        let before = s[..at].chars().next_back();
        let after = s[at + right.len()..].chars().next();
        let edge = |c: Option<char>| c.is_none_or(|c| !c.is_alphanumeric() && c != '\'');
        if edge(before) && edge(after) {
            return Some(format!("{}{wrong}{}", &s[..at], &s[at + right.len()..]));
        }
        from = at + right.len();
    }
    None
}

fn proofread(level: u32, rng: &mut Rng) -> Task {
    let (count, mistakes) = match level {
        1 => (1, 1),
        2 => (2, 2),
        3 => (3, 3),
        4 => (4, 5),
        5 => (5, 6),
        _ => (6, 8),
    };
    let mut idx: Vec<usize> = (0..PROOF.len()).collect();
    rng.shuffle(&mut idx);
    let chosen: Vec<usize> = idx.into_iter().take(count).collect();
    let right: Vec<String> = chosen.iter().map(|i| PROOF[*i].0.to_string()).collect();
    let mut wrong = right.clone();
    // Spread the mistakes over the sentences, at most two each.
    let mut made = 0;
    let mut round = 0;
    while made < mistakes && round < 3 {
        for (k, i) in chosen.iter().enumerate() {
            if made >= mistakes {
                break;
            }
            let options = PROOF[*i].1;
            let (r, w) = options[(round + rng.range(0, options.len() as i64 - 1) as usize) % options.len()];
            if let Some(next) = swap_word(&wrong[k], r, w) {
                if next != wrong[k] {
                    wrong[k] = next;
                    made += 1;
                }
            }
        }
        round += 1;
    }
    // At the harder levels, one sentence also loses its capital or its full stop.
    if level >= 3 && made < mistakes + 1 {
        let k = rng.range(0, wrong.len() as i64 - 1) as usize;
        if rng.chance(0.5) {
            if let Some(f) = wrong[k].get_mut(0..1) {
                f.make_ascii_lowercase();
            }
        } else if wrong[k].ends_with('.') {
            wrong[k].pop();
        }
    }
    Task {
        skill: Skill::Writing,
        level,
        user: format!("Fix the spelling, grammar and punctuation mistakes in this text. Don't change anything else.\n\n{}\n\nReply with only the corrected text.", wrong.join(" ")),
        tools: None,
        check: Check::Exact(right.join(" ")),
    }
}

/// Two reports summarized together: every number from both, and nothing
/// made up.
fn combined(level: u32, rng: &mut Rng) -> Task {
    let a = report(rng);
    let b = loop {
        let b = report(rng);
        if b.names[0] != a.names[0] {
            break b;
        }
    };
    let source = format!("{} {}", a.text, b.text);
    let (asks, must, sentences) = if level <= 5 {
        ("Keep every number and both towns' names, and don't add anything the reports don't say.", [a.numbers.clone(), b.numbers.clone(), vec![a.names[0].clone(), b.names[0].clone()]].concat(), None)
    } else {
        ("Write exactly two sentences. Keep every number, both dates and every name, and don't add anything the reports don't say.", [a.numbers, b.numbers, a.names, b.names, vec![a.date.clone(), b.date.clone()]].concat(), Some(2))
    };
    Task {
        skill: Skill::Writing,
        level,
        user: format!("Summarize these two reports together in at most 60 words. {asks}\n\n\"{}\"\n\n\"{}\"\n\nReply with only the summary.", a.text, b.text),
        tools: None,
        check: Check::Facts { must, source: Some(source), max_words: Some(60), max_sentence_words: None, sentences, last_line: None },
    }
}

pub fn task(level: u32, rng: &mut Rng) -> Task {
    match (level, rng.range(0, 2)) {
        (1, 0) => email(level, rng),
        (1, _) => summarize(level, rng),
        (2, 0) => proofread(level, rng),
        (2, 1) => summarize(level, rng),
        (2, _) => email(level, rng),
        (3 | 4, 0) => proofread(level, rng),
        (3 | 4, 1) => summarize(level, rng),
        (3 | 4, _) => simpler(level, rng),
        (_, 0) => proofread(level, rng),
        (5, 1) => simpler(level, rng),
        _ => combined(level, rng),
    }
}

#[cfg(test)]
mod tests {
    use super::super::tasks::{passes, read};
    use super::*;

    fn reply(t: &str) -> super::super::tasks::Reply {
        read(t).unwrap()
    }

    #[test]
    fn proofreading_has_mistakes_to_fix_and_one_right_answer() {
        let mut rng = Rng::new(5);
        for level in 1..=6 {
            for _ in 0..100 {
                let t = proofread(level, &mut rng);
                let Check::Exact(right) = &t.check else { panic!() };
                let shown = t.user.split("\n\n").nth(1).unwrap();
                assert_ne!(shown, right, "no mistakes made");
                assert!(passes(&t, &reply(right)));
                assert!(passes(&t, &reply(&format!("\"{right}\""))), "quotes around it are fine");
                assert!(!passes(&t, &reply(shown)));
            }
        }
        for (s, swaps) in PROOF {
            for (r, w) in *swaps {
                assert!(swap_word(s, r, w).is_some(), "{r} not in {s}");
            }
        }
    }

    #[test]
    fn a_summary_keeps_the_facts_and_adds_none() {
        let mut rng = Rng::new(2);
        let t = summarize(3, &mut rng);
        let Check::Facts { must, source, .. } = &t.check else { panic!() };
        let source = source.clone().unwrap();
        let facts: Vec<String> = must.iter().map(|m| m[0].clone()).collect();
        let good = format!("Event facts: {}.", facts.join(", "));
        assert!(passes(&t, &reply(&good)), "{good}");
        assert!(!passes(&t, &reply(&format!("{good} It cost $999."))), "an invented number");
        assert!(!passes(&t, &reply(&format!("Event facts: {}.", facts[1..].join(", ")))), "a missing fact");
        assert!(source.contains(&facts[0]));
    }

    #[test]
    fn emails_need_every_detail_and_the_signature() {
        let mut rng = Rng::new(9);
        let t = email(2, &mut rng);
        let Check::Facts { must, last_line, .. } = &t.check else { panic!() };
        let body = must.iter().map(|m| m[0].clone()).collect::<Vec<_>>().join(", ");
        let me = last_line.clone().unwrap();
        assert!(passes(&t, &reply(&format!("Hi,\n\n{body}.\n\nThanks,\n{me}"))));
        assert!(!passes(&t, &reply(&format!("Hi,\n\n{body}.\n\nThanks"))), "no signature");
    }

    #[test]
    fn every_writing_task_can_be_made() {
        let mut rng = Rng::new(4);
        for level in 1..=6 {
            for _ in 0..50 {
                let t = task(level, &mut rng);
                assert!(!t.user.is_empty());
            }
        }
    }
}
