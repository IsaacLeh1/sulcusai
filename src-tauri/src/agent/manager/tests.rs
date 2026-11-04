use super::*;

fn tmp() -> PathBuf {
    let d = std::env::temp_dir().join(format!("sulcus-mgr-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&d).unwrap();
    d
}

#[test]
fn off_by_default_and_round_trip() {
    let d = tmp();
    assert!(!load(&d, "c1").on);
    let s = Settings { on: true, repeat_limit: 1, manager: ManagerKind::Model("m".into()), ..Default::default() };
    save(&d, "c1", &s).unwrap();
    let back = load(&d, "c1");
    assert!(back.on);
    assert_eq!(back.repeat_limit, 2, "clamped");
    assert_eq!(back.manager, ManagerKind::Model("m".into()));
    assert!(folder(&d, "../x").ends_with("manager/x") || folder(&d, "../x").ends_with("manager\\x"));
}

#[test]
fn budget_warns_once_then_stops() {
    let s = Settings { token_budget: 1000, minutes_budget: 10, ..Default::default() };
    let mut b = Budget::new(&s);
    b.add_tokens(500);
    assert_eq!(b.check_at(Duration::ZERO), BudgetState::Fine);
    b.add_tokens(300);
    assert_eq!(b.check_at(Duration::ZERO), BudgetState::Warn);
    assert_eq!(b.check_at(Duration::ZERO), BudgetState::Fine);
    b.add_tokens(200);
    assert_eq!(b.check_at(Duration::ZERO), BudgetState::Spent);
    let mut t = Budget::new(&s);
    assert_eq!(t.check_at(Duration::from_secs(600)), BudgetState::Spent);
    assert!(budget_note(BudgetState::Spent, &s).unwrap().contains("1000 tokens"));
}

#[test]
fn repeat_detector_escalates_on_third() {
    let mut r = Repeats::default();
    let k = action_key("run_command", "cargo   test");
    assert_eq!(k, action_key("run_command", "cargo test"));
    assert!(!r.failed(&k, 3));
    assert!(!r.failed("other", 3));
    assert!(!r.failed(&k, 3));
    assert!(r.failed(&k, 3));
    r.progressed();
    assert!(!r.failed(&k, 3));
}

#[test]
fn parses_plan_with_checks() {
    let reply = "Here's the plan.\n```plan\n1. [edit] change parser\n   check: run cargo test parser\n   check: file src/p.rs\n2. [hard] fix race\n   check: contains src/p.rs :: fn parse\n3. tidy up\n```\nStarting.";
    let p = parse_plan(reply).unwrap();
    assert_eq!(p.steps.len(), 3);
    assert_eq!(p.steps[0].size, Some(Size::Edit));
    assert_eq!(p.steps[0].checks, vec![Check::Run { command: "cargo test parser".into() }, Check::File { path: "src/p.rs".into() }]);
    assert_eq!(p.steps[1].checks, vec![Check::Contains { path: "src/p.rs".into(), text: "fn parse".into() }]);
    assert_eq!(p.steps[2].size, None);
    assert!(parse_plan("no plan here").is_none());
}

#[test]
fn file_checks_run_and_stay_inside() {
    let d = tmp();
    std::fs::write(d.join("a.rs"), "fn parse() {}").unwrap();
    assert!(matches!(run_check(&Check::File { path: "a.rs".into() }, &d), Outcome::Pass));
    assert!(matches!(run_check(&Check::File { path: "b.rs".into() }, &d), Outcome::Fail(_)));
    assert!(matches!(run_check(&Check::Contains { path: "a.rs".into(), text: "fn parse".into() }, &d), Outcome::Pass));
    assert!(matches!(run_check(&Check::Contains { path: "a.rs".into(), text: "nope".into() }, &d), Outcome::Fail(_)));
    assert!(matches!(run_check(&Check::File { path: "../a.rs".into() }, &d), Outcome::Fail(_)));
    assert!(matches!(run_check(&Check::Run { command: "x".into() }, &d), Outcome::NeedsRun(_)));
}

#[test]
fn effort_steps_up_after_failure() {
    assert_eq!(pick_effort(Size::Lookup, false), "low");
    assert_eq!(pick_effort(Size::Lookup, true), "medium");
    assert_eq!(pick_effort(Size::Edit, false), "medium");
    assert_eq!(pick_effort(Size::Feature, true), "high");
    assert_eq!(pick_effort(Size::Hard, false), "high");
    assert_eq!(pick_effort(Size::Hard, true), "xhigh");
}

#[test]
fn model_pick_stays_on_list() {
    let list: Vec<String> = ["small", "mid", "big"].iter().map(|s| s.to_string()).collect();
    assert_eq!(pick_model(Size::Edit, false, &list), Some("small"));
    assert_eq!(pick_model(Size::Feature, false, &list), Some("mid"));
    assert_eq!(pick_model(Size::Hard, true, &list), Some("big"));
    assert_eq!(pick_model(Size::Hard, false, &list[..1]), Some("small"));
    assert_eq!(pick_model(Size::Hard, false, &[]), None);
}

#[test]
fn notes_taken_from_reply_and_written() {
    let reply = "Did it.\n```notes state\nparser: done src/p.rs:12\n```\n```notes decision\ns1 regex>nom: smaller\n```\nok";
    let (notes, clean) = take_notes(reply);
    assert_eq!(notes.len(), 2);
    assert_eq!(clean, "Did it.\n\n\nok");
    let d = tmp();
    write_notes(&d, "c", &notes).unwrap();
    write_notes(&d, "c", &[(Note::Decision, "s2 a>b".into()), (Note::State, "new".into())]).unwrap();
    let dir = folder(&d, "c");
    assert_eq!(std::fs::read_to_string(dir.join("state.md")).unwrap(), "new");
    assert_eq!(std::fs::read_to_string(dir.join("decisions.md")).unwrap(), "s1 regex>nom: smaller\ns2 a>b\n");
}

#[test]
fn handoff_from_notes_and_clean_breaks() {
    let d = tmp();
    let mut plan = parse_plan("```plan\n1. [edit] parser change\n2. [edit] update docs readme\n```").unwrap();
    plan.steps[0].state = StepState::Done;
    save_plan(&d, "c", &plan).unwrap();
    write_notes(&d, "c", &[(Note::State, "parser done".into()), (Note::Index, "readme -> README.md:1\nparser -> src/p.rs:3".into())]).unwrap();
    let h = Handoff::from_notes(&d, "c", "none").unwrap();
    assert!(h.is_complete());
    assert_eq!(h.next, "update docs readme");
    let t = h.text(&d, "c");
    assert!(t.contains("README.md:1") && !t.contains("src/p.rs:3"));
    assert!(!Handoff { questions: " ".into(), ..Handoff::from_notes(&d, "c", "x").unwrap() }.is_complete());

    let on = Settings { on: true, ..Default::default() };
    assert!(should_clear(&on, Some(&plan), 0.8));
    assert!(!should_clear(&on, Some(&plan), 0.1));
    plan.steps[1].state = StepState::Done;
    assert!(should_clear(&on, Some(&plan), 0.1));
    assert!(!should_clear(&Settings::default(), Some(&plan), 0.9));
}

#[test]
fn cheapest_first_unpriced_last() {
    let m = vec![("big".to_string(), Some(15.0)), ("x".to_string(), None), ("small".to_string(), Some(0.25)), ("mid".to_string(), Some(3.0))];
    assert_eq!(by_cost(&m), vec!["small", "mid", "big", "x"]);
}

#[test]
fn verify_marks_steps_and_turns_back() {
    let d = tmp();
    std::fs::write(d.join("a.rs"), "fn parse() {}").unwrap();
    let mut plan = parse_plan("```plan\n1. [edit] parser\n   check: contains a.rs :: fn parse\n2. [edit] tests\n   check: run cargo test\n```").unwrap();
    match verify(&mut plan, &d, &[], &[]) {
        Verdict::Back(1, why) => assert!(why.contains("hasn't been run")),
        v => panic!("{v:?}"),
    }
    assert_eq!(plan.steps[0].state, StepState::Done);
    assert_eq!(plan.steps[1].state, StepState::Stuck);
    let failed = vec![r#"run_command{"command":"cargo test"}"#.to_string()];
    assert!(matches!(verify(&mut plan, &d, &[], &failed), Verdict::Back(1, w) if w.contains("failed")));
    match verify(&mut plan, &d, &failed, &[]) {
        Verdict::AllDone(e) => assert_eq!(e, vec!["ran cargo test"]),
        v => panic!("{v:?}"),
    }
    assert!(should_clear(&Settings { on: true, ..Default::default() }, Some(&plan), 0.0));
}

#[test]
fn reports_are_short() {
    assert!(Report::Finished { evidence: vec!["ran cargo test".into()] }.text("Parser").contains("job finished. Checks passed: ran cargo test"));
    assert!(Report::Problem { what: "stuck".into() }.text("P").ends_with("problem. stuck"));
}

#[test]
fn plan_answers_are_two_lines() {
    assert_eq!(two_lines("approve\n\n  use high for step 2 \nand also this\nmore"), "approve\nuse high for step 2");
    let d = tmp();
    assert!(save_answer(&d, "c", "  \n ").is_err());
    save_answer(&d, "c", "adjust: skip 3\nlow effort\nchatter").unwrap();
    let a = take_answer(&d, "c").unwrap();
    assert!(a.contains("skip 3") && a.contains("low effort") && !a.contains("chatter"));
    assert!(take_answer(&d, "c").is_none(), "read once");
    let plan = parse_plan("```plan\n1. [edit] change parser\ncheck: file a.rs\n```").unwrap();
    assert!(plan_for_manager(&plan, "w").contains("two lines"));
}

#[test]
fn redo_reopens_the_step_and_reaches_the_next_turn() {
    let d = tmp();
    let mut plan = parse_plan("```plan\n1. [edit] one\ncheck: file a\n2. [edit] two\ncheck: file b\n3. [edit] three\ncheck: file c\n```").unwrap();
    for s in plan.steps.iter_mut() {
        s.state = StepState::Done;
    }
    save_plan(&d, "c", &plan).unwrap();
    assert!(save_redo(&d, "c", "step 2, used regex", "").is_err());
    save_redo(&d, "c", "step 2, used regex, alt parser, faster", "use the parser").unwrap();
    let back = load_plan(&d, "c").unwrap();
    assert_eq!(back.steps.iter().map(|s| s.state.clone()).collect::<Vec<_>>(), vec![StepState::Done, StepState::Waiting, StepState::Waiting]);
    let hint = take_redo(&d, "c").unwrap();
    assert!(hint.contains("=> use the parser"));
    assert!(take_redo(&d, "c").is_none());
}

#[test]
fn usage_caps_roll_over_and_stop() {
    let d = tmp();
    let day = chrono::NaiveDate::from_ymd_opt(2026, 11, 4).unwrap();
    let s = Settings { day_tokens: 1000, week_tokens: 0, ..Default::default() };
    assert_eq!(cap_state(&usage(&d, day), &s), BudgetState::Fine);
    record_usage(&d, 850, day);
    assert_eq!(cap_state(&usage(&d, day), &s), BudgetState::Warn);
    record_usage(&d, 200, day);
    assert_eq!(cap_state(&usage(&d, day), &s), BudgetState::Spent);
    assert!(cap_note(BudgetState::Spent, &s).unwrap().contains("1000 a day"));
    let next = day.succ_opt().unwrap();
    assert_eq!(usage(&d, next).day_tokens, 0, "a new day starts fresh");
    assert_eq!(usage(&d, next).week_tokens, 1050, "same week");
    assert_eq!(cap_state(&usage(&d, day), &Settings::default()), BudgetState::Fine, "no caps by default");
}

#[test]
fn nudge_only_with_work_left_and_the_switch_on() {
    let mut plan = parse_plan("```plan\n1. [edit] one\ncheck: file a\n2. [edit] two\ncheck: file b\n```").unwrap();
    let on = Settings { on: true, through: true, nudge_minutes: 10, ..Default::default() };
    let long = Duration::from_secs(3600);
    assert!(nudge_due(&on, Some(&plan), long, false));
    assert!(!nudge_due(&on, Some(&plan), Duration::from_secs(60), false), "not quiet long enough");
    assert!(!nudge_due(&on, Some(&plan), long, true), "one nudge per quiet spell");
    assert!(!nudge_due(&Settings { through: false, ..on.clone() }, Some(&plan), long, false), "off by default");
    for s in plan.steps.iter_mut() {
        s.state = StepState::Done;
    }
    assert!(!nudge_due(&on, Some(&plan), long, false), "nothing left");
    assert!(!Settings::default().through);
}
