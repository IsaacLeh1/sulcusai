// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

#[test]
fn long_tool_results_are_cut_to_fit() {
    let long = Outcome::ok("x".repeat(60_000), "Read an email", "text", None);
    let cut = fit_result(long, "email_read", 8192);
    assert!(cut.text.chars().count() < 8_400, "{}", cut.text.len());
    assert!(cut.text.contains("[Cut here: 51810 more characters"));
    let short = fit_result(Outcome::ok("hello", "t", "text", None), "email_read", 8192);
    assert_eq!(short.text, "hello");
}

/// A big memory still gets no more than the tool's own limit, and a
/// command's output keeps its end, where errors are.
#[test]
fn each_tool_has_its_own_limit() {
    let listing = fit_result(Outcome::ok("x".repeat(20_000), "t", "text", None), "list_dir", 200_000);
    assert!(listing.text.contains("[Cut here: 12000 more characters"));
    let mut out = "start ".repeat(1_000);
    out.push_str(&"y".repeat(30_000));
    out.push_str("error: it broke");
    let cut = fit_result(Outcome::ok(out, "t", "text", None), "run_command", 200_000);
    assert!(cut.text.starts_with("start start"));
    assert!(cut.text.ends_with("error: it broke"));
    assert!(cut.text.chars().count() < 16_300);
    // A cloud model with no memory size set still has the limits.
    let page = fit_result(Outcome::ok("z".repeat(30_000), "t", "text", None), "fetch_page", 0);
    assert!(page.text.contains("[Cut here: 6000 more characters"));
}


#[test]
fn runs_of_lookups_end_at_the_first_that_cannot_join() {
    let (t, f) = (true, false);
    assert_eq!(run_end(&[t, t, f, t], 0), 2);
    assert_eq!(run_end(&[t, t, f, t], 3), 4);
    assert_eq!(run_end(&[f], 0), 0);
    assert_eq!(run_end(&[t, t, t], 0), 3);
}

/// `join_all` keeps the order the calls came in, whatever finishes first
/// (a stream left unordered wouldn't, and the results would be mixed up).
#[tokio::test]
async fn lookups_run_together_come_back_in_order() {
    let delays = [30u64, 10, 20];
    let running: Vec<_> = delays
        .iter()
        .map(|&ms| async move {
            tokio::time::sleep(std::time::Duration::from_millis(ms)).await;
            ms
        })
        .collect();
    let started = std::time::Instant::now();
    let got = futures_util::future::join_all(running).await;
    assert_eq!(got, delays);
    assert!(started.elapsed() < std::time::Duration::from_millis(55), "they ran at the same time");
}
