// SPDX-License-Identifier: AGPL-3.0-only
//! Research practice: answer a question from numbered sources, citing the
//! ones used, and say so when the sources don't have the answer. The places
//! and buildings are made up, so the model can't answer from memory: it has
//! to read. Harder levels add look-alike sources (the same name in another
//! town), questions that take two sources, and sums across them.

use super::tasks::{Check, Rng, Skill, Task, Want};

const TOWNS: &[&str] = &["Brightwater", "Fenmoor", "Ashcombe", "Redhollow", "Lindale", "Stoneport", "Marrow Bay", "Port Ellis"];
const RIVERS: &[&str] = &["Wren", "Tamsin", "Ouse", "Calder", "Brede"];
const NAMES: &[&str] = &["Kestrel", "Alder", "Heron", "Juniper", "Osprey", "Thistle"];
const KINDS: &[(&str, &str)] = &[("Bridge", "long"), ("Tunnel", "long"), ("Tower", "tall"), ("Pier", "long")];

struct Town {
    name: &'static str,
    people: i64,
    founded: i64,
    river: &'static str,
}

struct Place {
    name: String,
    town: usize,
    opened: i64,
    size: i64,
    measure: &'static str,
    cost: i64,
}

fn towns(rng: &mut Rng, n: usize) -> Vec<Town> {
    let mut names = TOWNS.to_vec();
    rng.shuffle(&mut names);
    names.into_iter().take(n).map(|name| Town { name, people: rng.range(40, 900) * 100, founded: rng.range(1200, 1890), river: *rng.pick(RIVERS) }).collect()
}

fn place(rng: &mut Rng, name: &str, kind: (&'static str, &'static str), town: usize) -> Place {
    Place { name: format!("{name} {}", kind.0), town, opened: rng.range(1950, 2024), size: rng.range(12, 180) * 10, measure: kind.1, cost: rng.range(8, 400) }
}

fn town_text(t: &Town) -> String {
    format!("{} is a town of {} people on the River {}. It was founded in {}.", t.name, super::tasks::thousands(t.people), t.river, t.founded)
}

fn place_text(p: &Place, towns: &[Town]) -> String {
    format!(
        "The {} in {} opened in {}. It is {} metres {} and cost {} million dollars to build.",
        p.name,
        towns[p.town].name,
        p.opened,
        super::tasks::thousands(p.size),
        p.measure,
        p.cost
    )
}

pub fn task(level: u32, rng: &mut Rng) -> Task {
    if level >= 5 {
        return hard(level, rng);
    }
    let ts = towns(rng, 3);
    let base = *rng.pick(NAMES);
    let kind = *rng.pick(KINDS);
    let other_kind = loop {
        let k = *rng.pick(KINDS);
        if k.0 != kind.0 {
            break k;
        }
    };
    let second = loop {
        let n = *rng.pick(NAMES);
        if n != base {
            break n;
        }
    };
    // The one asked about, a look-alike (same name, another kind, another
    // town) and another place.
    let main = place(rng, base, kind, 0);
    let twin = place(rng, base, other_kind, 1);
    let mut extra = place(rng, second, kind, 2);
    while extra.opened == main.opened {
        extra.opened = rng.range(1950, 2024);
    }
    // Sources: (text, what it's about).
    #[derive(PartialEq, Clone, Copy)]
    enum About {
        Main,
        Twin,
        Extra,
        Town(usize),
    }
    let mut docs: Vec<(String, About)> = vec![(place_text(&main, &ts), About::Main)];
    match level {
        1 => docs.push((town_text(&ts[1]), About::Town(1))),
        2 => {
            docs.push((place_text(&twin, &ts), About::Twin));
            docs.push((town_text(&ts[2]), About::Town(2)));
        }
        _ => {
            docs.push((place_text(&twin, &ts), About::Twin));
            docs.push((place_text(&extra, &ts), About::Extra));
            docs.push((town_text(&ts[0]), About::Town(0)));
            docs.push((town_text(&ts[1]), About::Town(1)));
        }
    }
    rng.shuffle(&mut docs);
    let at = |a: About| docs.iter().position(|d| d.1 == a).map(|i| i + 1).unwrap_or(0);

    let unanswerable = match level {
        1 => false,
        2 => rng.chance(0.15),
        _ => rng.chance(0.25),
    };
    // A name that mustn't be in the answer (the other one of two).
    let mut not = Vec::new();
    let (question, want, cite): (String, Option<Want>, Vec<usize>) = if unanswerable {
        let q = rng
            .pick(&[format!("Who designed the {}?", main.name), format!("How many people use the {} each day?", main.name), format!("What is the {} made of?", main.name)])
            .clone();
        (q, None, vec![])
    } else {
        match (level, rng.range(0, 2)) {
            (1, 0) | (2, 0) => (format!("How many metres {} is the {}?", main.measure, main.name), Some(Want::Number(main.size as f64)), vec![at(About::Main)]),
            (1, _) | (2, _) => (format!("In what year did the {} open?", main.name), Some(Want::Number(main.opened as f64)), vec![at(About::Main)]),
            (3, 0) => (format!("How many people live in the town where the {} is?", main.name), Some(Want::Number(ts[0].people as f64)), vec![at(About::Main), at(About::Town(0))]),
            (3, 1) => {
                let (first, later) = if main.opened < extra.opened { (&main.name, &extra.name) } else { (&extra.name, &main.name) };
                not.push(later.split(' ').next().unwrap_or("").to_lowercase());
                (format!("Which opened first, the {} or the {}?", main.name, extra.name), Some(Want::Word(first.split(' ').next().unwrap_or("").to_lowercase())), vec![at(About::Main), at(About::Extra)])
            }
            (3, _) => (format!("On which river is the town where the {} is?", twin.name), Some(Want::Word(ts[1].river.to_lowercase())), vec![at(About::Twin), at(About::Town(1))]),
            (_, 0) => (
                format!("How many years after its town was founded did the {} open?", main.name),
                Some(Want::Number((main.opened - ts[0].founded) as f64)),
                vec![at(About::Main), at(About::Town(0))],
            ),
            (_, 1) => (
                format!("Together, how many million dollars did the {} and the {} cost to build?", main.name, twin.name),
                Some(Want::Number((main.cost + twin.cost) as f64)),
                vec![at(About::Main), at(About::Twin)],
            ),
            _ => (
                format!("How many more people live in {} than in {}?", if ts[0].people >= ts[1].people { ts[0].name } else { ts[1].name }, if ts[0].people >= ts[1].people { ts[1].name } else { ts[0].name }),
                Some(Want::Number((ts[0].people - ts[1].people).abs() as f64)),
                vec![at(About::Town(0)), at(About::Town(1))],
            ),
        }
    };
    let sources = docs.iter().enumerate().map(|(i, d)| format!("[{}] {}", i + 1, d.0)).collect::<Vec<_>>().join("\n");
    let mut cite = cite;
    cite.sort_unstable();
    Task {
        skill: Skill::Research,
        level,
        user: format!(
            "Answer the question using only these sources.\n\n{sources}\n\nQuestion: {question}\n\nEnd with a line \"Answer: <answer> [numbers of the sources you used]\", like \"Answer: 42 [2]\". If the sources don't say, end with \"Answer: not in the sources\"."
        ),
        tools: None,
        check: Check::Cited { want, sources: cite, may: Vec::new(), not },
    }
}

fn ask(sources: &str, question: &str) -> String {
    format!(
        "Answer the question using only these sources.\n\n{sources}\n\nQuestion: {question}\n\nEnd with a line \"Answer: <answer> [numbers of the sources you used]\", like \"Answer: 42 [2]\". If the sources don't say, end with \"Answer: not in the sources\"."
    )
}

/// Levels 5 and 6: seven or eight sources, three places of the same kind
/// and a look-alike, questions that take two or three of them, and (at 6)
/// a correction that overrides an earlier source.
fn hard(level: u32, rng: &mut Rng) -> Task {
    let ts = towns(rng, 3);
    let mut names = NAMES.to_vec();
    rng.shuffle(&mut names);
    let mut kinds = KINDS.to_vec();
    rng.shuffle(&mut kinds);
    let (kind, other_kind, missing_kind) = (kinds[0], kinds[1], kinds[2]);
    let main = place(rng, names[0], kind, 0);
    let twin = place(rng, names[0], other_kind, 1);
    let mut extra = place(rng, names[1], kind, 2);
    let mut extra2 = place(rng, names[2], kind, 1);
    while extra.opened == main.opened {
        extra.opened = rng.range(1950, 2024);
    }
    while extra2.opened == main.opened || extra2.opened == extra.opened {
        extra2.opened = rng.range(1950, 2024);
    }
    let fixed = loop {
        let f = rng.range(12, 180) * 10;
        if f != main.size {
            break f;
        }
    };
    #[derive(PartialEq, Clone, Copy)]
    enum About {
        Main,
        Twin,
        Extra,
        Extra2,
        Town(usize),
        Fix,
    }
    let mut docs: Vec<(String, About)> = vec![
        (place_text(&main, &ts), About::Main),
        (place_text(&twin, &ts), About::Twin),
        (place_text(&extra, &ts), About::Extra),
        (place_text(&extra2, &ts), About::Extra2),
        (town_text(&ts[0]), About::Town(0)),
        (town_text(&ts[1]), About::Town(1)),
        (town_text(&ts[2]), About::Town(2)),
    ];
    if level >= 6 {
        docs.push((format!("Correction: an earlier report gave the wrong figure for the {}. It is actually {} metres {}.", main.name, super::tasks::thousands(fixed), main.measure), About::Fix));
    }
    rng.shuffle(&mut docs);
    let at = |a: About| docs.iter().position(|d| d.1 == a).map(|i| i + 1).unwrap_or(0);
    let first_word = |p: &Place| p.name.split(' ').next().unwrap_or("").to_lowercase();
    let mut not = Vec::new();
    let mut may = Vec::new();
    let (question, want, mut cite): (String, Option<Want>, Vec<usize>) = match (level, rng.range(0, 3)) {
        (_, 0) if rng.chance(0.5) => (format!("When did the {} {} open?", names[0], missing_kind.0), None, vec![]),
        (5, 0) => (
            format!("How many people live in the town where the {} that opened in {} is?", kind.0.to_lowercase(), main.opened),
            Some(Want::Number(ts[0].people as f64)),
            vec![at(About::Main), at(About::Town(0))],
        ),
        (5, 1) => (
            format!("Altogether, how many million dollars did the {}, the {} and the {} cost to build?", main.name, extra.name, extra2.name),
            Some(Want::Number((main.cost + extra.cost + extra2.cost) as f64)),
            vec![at(About::Main), at(About::Extra), at(About::Extra2)],
        ),
        (5, _) => {
            let (older, newer) = if ts[2].founded <= ts[1].founded { (2, 1) } else { (1, 2) };
            (
                format!("How many years before {} was {} founded?", ts[newer].name, ts[older].name),
                Some(Want::Number((ts[newer].founded - ts[older].founded) as f64)),
                vec![at(About::Town(1)), at(About::Town(2))],
            )
        }
        (_, 0) => {
            may.push(at(About::Main));
            (format!("How many metres {} is the {}?", main.measure, main.name), Some(Want::Number(fixed as f64)), vec![at(About::Fix)])
        }
        (_, 1) => {
            let all = [&main, &extra, &extra2];
            let first = all.iter().min_by_key(|p| p.opened).map(|p| first_word(*p)).unwrap_or_default();
            not = all.iter().map(|p| first_word(*p)).filter(|w| *w != first).collect();
            (
                format!("Which of these opened first: the {}, the {} or the {}?", main.name, extra.name, extra2.name),
                Some(Want::Word(first)),
                vec![at(About::Main), at(About::Extra), at(About::Extra2)],
            )
        }
        _ => (
            format!("How many people live in the town where the {} is?", twin.name),
            Some(Want::Number(ts[1].people as f64)),
            vec![at(About::Twin), at(About::Town(1))],
        ),
    };
    cite.sort_unstable();
    let sources = docs.iter().enumerate().map(|(i, d)| format!("[{}] {}", i + 1, d.0)).collect::<Vec<_>>().join("\n");
    Task { skill: Skill::Research, level, user: ask(&sources, &question), tools: None, check: Check::Cited { want, sources: cite, may, not } }
}

#[cfg(test)]
mod tests {
    use super::super::tasks::{passes, read};
    use super::*;

    #[test]
    fn answers_need_the_right_sources() {
        let mut rng = Rng::new(8);
        for level in 1..=6 {
            for _ in 0..200 {
                let t = task(level, &mut rng);
                let Check::Cited { want, sources, may, .. } = &t.check else { panic!() };
                let cites = format!("[{}]", sources.iter().map(usize::to_string).collect::<Vec<_>>().join(", "));
                match want {
                    None => {
                        assert!(passes(&t, &read("The sources don't mention it.\nAnswer: not in the sources").unwrap()));
                        assert!(!passes(&t, &read("Answer: 42 [1]").unwrap()));
                    }
                    Some(w) => {
                        assert!(sources.iter().all(|s| *s > 0), "{}", t.user);
                        let answer = super::super::tasks::want_text(w);
                        assert!(passes(&t, &read(&format!("From the sources.\nAnswer: {answer} {cites}")).unwrap()), "{}\n{answer} {cites}", t.user);
                        assert!(!passes(&t, &read(&format!("Answer: {answer}")).unwrap()), "no sources cited");
                        let wrong = (1..=9).find(|n| !sources.contains(n) && !may.contains(n)).unwrap();
                        assert!(!passes(&t, &read(&format!("Answer: {answer} [{wrong}]")).unwrap()), "the wrong source");
                        assert!(!passes(&t, &read("Answer: not in the sources").unwrap()));
                    }
                }
            }
        }
    }
}
