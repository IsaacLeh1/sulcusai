// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

#[tokio::test]
#[ignore]
async fn caldav_round_trip() {
    let a = CalAccount { name: "Test".into(), url: "http://127.0.0.1:5232/".into(), username: "demo".into(), password: "demopass".into(), calendars: vec![], ..Default::default() };
    let dav = Dav::new(&a).unwrap();
    // A fresh calendar to work in.
    let cal = format!("http://127.0.0.1:5232/demo/t{}/", uuid::Uuid::new_v4().simple());
    let body = r#"<?xml version="1.0"?><c:mkcalendar xmlns:d="DAV:" xmlns:c="urn:ietf:params:xml:ns:caldav"><d:set><d:prop><d:displayname>Work</d:displayname></d:prop></d:set></c:mkcalendar>"#;
    let (status, _) = dav.call("MKCALENDAR", &cal, "0", body.into()).await.unwrap();
    assert_eq!(status, 201);

    let wrong = Dav::new(&CalAccount { password: "nope".into(), ..a.clone() }).unwrap();
    let e = wrong.discover(&a.url).await.unwrap_err();
    println!("wrong password: {e}");

    let cals = dav.discover(&a.url).await.unwrap();
    println!("calendars: {cals:?}");
    assert!(cals.iter().any(|c| c.href == cal && c.name == "Work"));

    let now = db::now_ms();
    let start = (now / 3_600_000 + 24) * 3_600_000;
    let one = Event {
        id: "x".into(),
        account_id: None,
        calendar: None,
        start,
        end: start + 3_600_000,
        all_day: false,
        data: EventData { title: "Dentist, then lunch".into(), location: "Main St".into(), uid: "one@sulcusai".into(), ..Default::default() },
    };
    let etag = dav.put(&format!("{cal}one.ics"), to_ics(&one, None), None).await.unwrap();
    println!("created, etag {etag:?}");
    // A weekly event, three times.
    let weekly = to_ics(&Event { data: EventData { title: "Standup".into(), uid: "weekly@sulcusai".into(), ..Default::default() }, start: start + 7_200_000, end: start + 9_000_000, ..one.clone() }, None)
        .replace("END:VEVENT", "RRULE:FREQ=WEEKLY;COUNT=3\r\nEND:VEVENT");
    dav.put(&format!("{cal}weekly.ics"), weekly, None).await.unwrap();

    let got = dav.events(&cal, now - DAY_MS, now + 60 * DAY_MS).await.unwrap();
    let parsed: Vec<_> = got.iter().flat_map(|(_, _, ics)| parse_ics(ics)).collect();
    for (s, _, _, d) in &parsed {
        println!("  {} at {} (recurring {})", d.title, Local.timestamp_millis_opt(*s).unwrap(), d.recurring);
    }
    assert_eq!(parsed.iter().filter(|p| p.3.title == "Standup").count(), 3, "expanded by the server");
    let dentist = parsed.iter().find(|p| p.3.title == "Dentist, then lunch").unwrap();
    assert_eq!((dentist.0, dentist.1), (one.start, one.end));
    assert_eq!(dentist.3.location, "Main St");

    dav.delete(&format!("{cal}one.ics"), etag.as_deref()).await.unwrap();
    let after = dav.events(&cal, now - DAY_MS, now + 60 * DAY_MS).await.unwrap();
    assert!(!after.iter().any(|(_, _, ics)| ics.contains("Dentist")));
    dav.call("DELETE", &cal, "0", String::new()).await.unwrap();
}
