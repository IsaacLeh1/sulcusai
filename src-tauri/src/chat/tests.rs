// SPDX-License-Identifier: AGPL-3.0-only

use super::*;

/// gpt-oss's Harmony tool calls that reach the reply as text are run,
/// not shown to the user; its built-in tools become ours.
#[test]
fn harmony_tool_calls_written_as_text_are_run() {
    let offered: Vec<String> = ["find_files", "run_command", "fetch_page"].map(String::from).to_vec();
    let raw = "<|channel|>commentary to=functions.find_files <|constrain|>json<|message|>{\"pattern\": \"*hello*\"}<|call|>";
    let (calls, rest) = harmony_calls(raw, &offered);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "find_files");
    assert_eq!(serde_json::from_str::<Value>(&calls[0].arguments).unwrap()["pattern"], "*hello*");
    assert!(rest.is_empty(), "{rest:?}");
    // The special tokens left out, and words around the call kept.
    let (calls, rest) = harmony_calls("Let me look. commentary to=functions.run_command json{\"command\": \"dir\"}", &offered);
    assert_eq!(calls[0].name, "run_command");
    assert_eq!(rest, "Let me look.");
    // Built-in tools as ours.
    let (calls, _) = harmony_calls("to=browser.open <|message|>{\"url\":\"https://example.com\"}", &offered);
    assert_eq!(calls[0].name, "fetch_page");
    assert_eq!(harmony_alias("python", &json!({ "code": "print(1)" })).unwrap().0, "run_code");
    // Not calls: an unknown tool, or other words between the name and the JSON.
    assert!(harmony_calls("to=functions.launch_rockets json{\"x\":1}", &offered).0.is_empty());
    assert!(harmony_calls("I went to=functions.find_files and then {\"a\":1}", &offered).0.is_empty());
    // Braces inside strings don't end the arguments.
    let (calls, _) = harmony_calls("to=functions.run_command<|message|>{\"command\": \"echo }{\"}", &offered);
    assert_eq!(serde_json::from_str::<Value>(&calls[0].arguments).unwrap()["command"], "echo }{");
}

#[test]
fn explaining_instead_of_doing_is_noticed() {
    assert!(asks_to_act("Fetch this page and apply its instructions to my project folder"));
    assert!(asks_to_act("find hello.bat and run it"));
    assert!(!asks_to_act("What is the capital of France?"));
    assert!(explains_instead("Here are the steps:\n1. Open a terminal\n2. Run npm install"));
    assert!(explains_instead("Would you like me to go ahead and run it?"));
    assert!(explains_instead("You'll need to run the installer first."));
    assert!(!explains_instead("Done. I created notes.txt and ran the script; it printed 42."));
}

fn m(role: &str, id: &str) -> Message {
    Message { id: id.into(), role: role.into(), content: id.into(), ..Default::default() }
}

#[test]
fn too_long_is_read_from_each_provider() {
    assert!(too_long("This conversation no longer fits in the model's memory. Start a new chat to continue."));
    assert!(too_long("Anthropic returned 400 Bad Request: prompt is too long: 203456 tokens > 200000 maximum"));
    assert!(too_long("OpenAI returned 400 Bad Request: This model's maximum context length is 128000 tokens. However, your messages resulted in 130512 tokens."));
    assert!(too_long(r#"{"error":{"code":"context_length_exceeded"}}"#));
    assert!(!too_long("OpenAI returned 429: rate limit"));
}

/// Past 60% the old long results go in batches of six (the newest six
/// stay), so the start of the request changes only now and then.
#[test]
fn old_results_are_trimmed_in_batches() {
    let long = "x".repeat(3_000);
    let chat = |n: usize| -> Vec<Message> {
        let mut all = vec![m("user", "u")];
        for i in 0..n {
            all.push(Message { id: format!("a{i}"), role: "assistant".into(), ..Default::default() });
            all.push(Message { id: format!("t{i}"), role: "tool".into(), content: long.clone(), ..Default::default() });
        }
        all
    };
    let trimmed = |h: &[Message]| h.iter().filter(|m| is_trimmed(m)).count();
    // 13 results of 1,000 tokens in a 20K memory: past 60%, 7 are older than the last 6, and one batch of 6 goes.
    assert_eq!(trimmed(&trim_in_steps(&chat(13), 20_000)), 6);
    // One more result changes nothing: the batch isn't full yet.
    assert_eq!(trimmed(&trim_in_steps(&chat(14), 22_000)), 6);
    assert_eq!(trimmed(&trim_in_steps(&chat(13), 64_000)), 0);
    // Past 75%, only the last 3 stay whole (in batches still).
    assert_eq!(trimmed(&trim_in_steps(&chat(15), 16_384)), 12);
    let mut turn = chat(4);
    trim_turn(&mut turn, "u", 2);
    assert_eq!(trimmed(&turn), 2);
    assert!(!is_trimmed(&turn[8]) && !is_trimmed(&turn[6]));
}

#[test]
fn a_fix_pasted_into_the_reply_is_saved_next() {
    let code: String = (0..20).map(|i| format!("line {i}\n")).collect();
    let reply = format!("Great! Now I'll update the `app.js` file.\n\n```javascript\n{code}```");
    assert!(pasted_code(&reply) && !pasted_code("Here's a short one:\n```js\nlet a = 1;\n```"));
    let written = vec![std::path::PathBuf::from("C:/s/index.html"), std::path::PathBuf::from("C:/s/app.js"), std::path::PathBuf::from("C:/s/style.css")];
    assert_eq!(named_file(&reply, &written), Some(std::path::PathBuf::from("C:/s/app.js")));
    assert_eq!(named_file("fixed it", &written), Some(std::path::PathBuf::from("C:/s/app.js")), "the newest script");
    let tools = json!([{ "type": "function", "function": { "name": "write_file", "description": "", "parameters": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } } } } }]);
    let f = forced_save(&tools, std::path::Path::new("live-test2/app.js")).unwrap();
    assert!(f.nudge.contains("never saved"));
    assert_eq!(f.response_format["json_schema"]["schema"]["properties"]["arguments"]["properties"]["path"]["const"], "live-test2/app.js");
}

#[test]
fn asking_where_they_are_for_a_near_me_question_is_a_stall() {
    let offered = vec!["find_places".to_string()];
    assert!(stalls("Find coffee shops near me.", "To find coffee shops near you, I'll need your location. Could you please provide the city or address where you are?", &offered));
    let tools = json!([
        { "type": "function", "function": { "name": "web_search", "description": "", "parameters": { "type": "object" } } },
        { "type": "function", "function": { "name": "find_places", "description": "", "parameters": { "type": "object" } } },
    ]);
    assert_eq!(forced_lookup(&tools, "Find coffee shops near me.", |_| true).unwrap().names, vec!["find_places".to_string()]);
}

#[test]
fn announcing_a_lookup_without_doing_it_is_a_stall() {
    let offered = vec!["research".to_string(), "web_search".to_string()];
    let said = "To research which is better, I'll perform a few web searches. Here are the searches I'll run:\n\n1. \"iPhone 17 battery life\"\n2. \"Pixel 10 battery life\"\n\nLet's execute these searches and gather the results.";
    assert!(stalls("Research which is better for battery life", said, &offered));
    // An answer that mentions looking things up along the way isn't one.
    assert!(!stalls("Who is Link?", "Link is the hero of The Legend of Zelda. Let me know if you'd like more details!", &offered));
    // A tool's name in code it wrote isn't telling the user to use it.
    assert!(!stalls("Write a search helper", "Here it is:\n```js\nfunction web_search(q) { return fetch(q); }\n```\nCall `web_search` with your query.", &offered));
    assert!(stalls("Find the news", "You can use web_search to find it.", &offered));
    let tools = json!([
        { "type": "function", "function": { "name": "web_search", "description": "", "parameters": { "type": "object" } } },
        { "type": "function", "function": { "name": "research", "description": "", "parameters": { "type": "object" } } },
    ]);
    let f = forced_lookup(&tools, "Research which is better, the iPhone 17 or the Pixel 10", |_| true).unwrap();
    assert_eq!(f.names, vec!["research".to_string()]);
}

#[test]
fn sums_are_worked_out_not_guessed() {
    for ask in [
        "what's 37*48?",
        "What is 15% of 2,345",
        "how much is 1250 divided by 7",
        "1234 + 5678",
        "what is 2^10",
        "How many days until Christmas?",
        "what's the monthly payment on a 300k mortgage at 6.5% for 30 years",
        "split $184.50 4 ways",
        "square root of 1764",
        "Solve x^2 - 5x + 6 = 0 and show the steps.",
        "What's the integral of x^2 from 0 to 3?",
        "find the derivative of sin(x) at 1",
    ] {
        assert!(asks_math(ask), "{ask}");
    }
    for not in ["what's on 10/17?", "make a 3x3 grid game", "my birthday is 2026-10-17", "Build me a calculator app", "tell me about Apollo 11", "I have 2 cats"] {
        assert!(!asks_math(not), "{not}");
    }
    let tools = json!([{ "type": "function", "function": { "name": "calculate", "description": "", "parameters": { "type": "object" } } }]);
    let f = forced_calc(&tools).unwrap();
    assert_eq!(f.response_format["json_schema"]["schema"]["properties"]["name"]["const"], "calculate");
    assert!(forced_calc(&json!([])).is_none());
}

/// gpt-oss thinks even when asked not to: with a small allowance it ran
/// out before answering, so chats kept the raw request as their name.
#[tokio::test]
async fn a_model_that_spends_its_allowance_thinking_gets_room_to_answer() {
    use axum::{routing::post, Json, Router};
    use std::sync::atomic::{AtomicU64, Ordering};
    static LAST_MAX: AtomicU64 = AtomicU64::new(0);
    async fn reply(Json(body): Json<Value>) -> Json<Value> {
        let max = body["max_tokens"].as_u64().unwrap_or(0);
        LAST_MAX.store(max, Ordering::SeqCst);
        assert_eq!(body["chat_template_kwargs"]["reasoning_effort"], "low");
        Json(if max < 100 {
            json!({ "choices": [{ "finish_reason": "length", "message": { "content": "", "reasoning_content": "The user wants a title. Let me think about" } }] })
        } else {
            json!({ "choices": [{ "finish_reason": "stop", "message": { "content": "Summarize Latest Google Email", "reasoning_content": "Short title." } }] })
        })
    }
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, Router::new().route("/v1/chat/completions", post(reply))).await.unwrap() });
    let ep = Endpoint { port, key: "k".into(), ctx: 8192, extra: json!({}), cloud: None };
    assert_eq!(summary_title(&ep, "Summarize my most recent email from Google").await.as_deref(), Some("Summarize Latest Google Email"));
    assert_eq!(LAST_MAX.load(Ordering::SeqCst), 48 + 768, "asked again with room");
}

#[test]
fn replies_that_dont_know_the_thing_are_caught() {
    for r in [
        "I'm not familiar with \"Skibidi Fortnite Rizz\". Could you tell me more?",
        "I'm not sure what Sulcus Studio is, but here's a guess.",
        "As of my knowledge cutoff in 2023, there is no game called that.",
        "I don't know what the Lumen Loop refers to.",
        "Could you clarify what a bloopsnorf is?",
        "I couldn't find information about that band.",
    ] {
        assert!(doesnt_know(r), "{r}");
    }
    for r in [
        "Here's a picture of a castle at sunset.",
        "I'm not sure what style you'd like, so I made it bright and colorful.",
        "I don't know your schedule yet, so let me check it.",
        "The Legend of Zelda is an action adventure series by Nintendo.",
    ] {
        assert!(!doesnt_know(r), "{r}");
    }
    let tools = json!([
        { "type": "function", "function": { "name": "request_web", "description": "", "parameters": { "type": "object" } } },
        { "type": "function", "function": { "name": "search_notes", "description": "", "parameters": { "type": "object" } } },
    ]);
    let f = forced_web(&tools).unwrap();
    assert_eq!(f.names, vec!["request_web".to_string()]);
    assert_eq!(f.response_format["json_schema"]["schema"]["properties"]["name"]["const"], "request_web");
    assert!(forced_web(&json!([{ "type": "function", "function": { "name": "search_notes", "parameters": {} } }])).is_none());
}

#[test]
fn files_it_already_wrote_are_left_out_of_what_the_model_is_sent() {
    let page = format!("<html>{}</html>", "<p>row</p>\n".repeat(80));
    let pasted = format!("Here it is:\n```html\n{page}\n```\nNow I'll save it.");
    let call = |name: &str, args: Value| ToolCall { id: "c".into(), name: name.into(), arguments: args.to_string() };
    let history = vec![
        Message { role: "user".into(), content: "Build a site".into(), ..Default::default() },
        Message { role: "assistant".into(), content: pasted.clone(), ..Default::default() },
        Message { role: "assistant".into(), tool_calls: Some(vec![call("write_file", json!({ "path": "s/index.html", "content": page }))]), ..Default::default() },
        Message { role: "tool".into(), content: "Created s/index.html".into(), tool_call_id: Some("c".into()), ..Default::default() },
        Message { role: "assistant".into(), tool_calls: Some(vec![call("write_file", json!({ "path": "s/app.js", "content": "x".repeat(900) }))]), ..Default::default() },
    ];
    let slim = without_written(&history);
    assert!(!slim[1].content.contains("<p>row</p>") && !slim[1].content.contains('[') && slim[1].content.starts_with("Here it is:"), "{}", slim[1].content);
    let args: Value = serde_json::from_str(&slim[2].tool_calls.as_ref().unwrap()[0].arguments).unwrap();
    assert!(args.get("content").is_none(), "no stand-in for the model to copy: {args}");
    assert_eq!(args["path"], "s/index.html");
    // The newest message is sent whole.
    assert_eq!(slim[4].tool_calls, history[4].tool_calls);
    assert_eq!(slim[0].content, "Build a site");
}

#[test]
fn a_built_site_is_previewed_and_its_problems_fixed() {
    let def = |n: &str| json!({ "type": "function", "function": { "name": n, "description": "", "parameters": { "type": "object", "properties": { "path": { "type": "string" }, "port": { "type": "integer" } } } } });
    let tools = json!([def("preview_site"), def("edit_file"), def("write_file"), def("run_command")]);
    let p = forced_preview(&tools, "stock-site/").unwrap();
    let args = &p.response_format["json_schema"]["schema"]["properties"]["arguments"];
    assert_eq!(args["properties"]["path"]["const"], "stock-site/");
    assert!(args["properties"].get("port").is_none());
    let f = forced_fix(&tools, "- Script error: x is not defined").unwrap();
    assert_eq!(f.names, vec!["edit_file".to_string(), "write_file".to_string()]);
    assert!(f.nudge.contains("x is not defined"));
    assert!(forced_preview(&json!([def("write_file")]), "a/").is_none());
    let written = vec![std::path::PathBuf::from("C:/s/site/app.js"), std::path::PathBuf::from("C:/s/site/index.html"), std::path::PathBuf::from("C:/s/site/style.css")];
    assert_eq!(site_folder(&written), Some(std::path::PathBuf::from("C:/s/site")));
    assert_eq!(site_folder(&written[..1]), None);
    // A stylesheet and script but no page (what Qwen2.5-Coder 14B left in the live test).
    let base = std::env::temp_dir().join(format!("sulcus-page-{}", uuid::Uuid::new_v4().simple()));
    let parts = vec![base.join("style.css"), base.join("js").join("app.js")];
    let (dir, links) = page_missing(&parts).unwrap();
    assert_eq!(dir, base);
    assert_eq!(links, vec!["style.css".to_string(), "js/app.js".to_string()]);
    assert!(page_missing(&written).is_none(), "a page was written");
    let tools = json!([def("write_file")]);
    let f = forced_page(&tools, &dir.join("index.html"), &links).unwrap();
    assert!(f.nudge.contains("style.css and js/app.js"));
    assert_eq!(f.response_format["json_schema"]["schema"]["properties"]["arguments"]["properties"]["path"]["const"], dir.join("index.html").display().to_string());
}

#[test]
fn picture_requests_get_a_picture_even_when_the_model_says_it_cant() {
    for a in ["Make me a picture of Link from The Legend of Zelda in pixel art.", "can you draw a cat wearing a hat", "I want a logo for my bakery", "image of a sunset over Provo"] {
        assert!(asks_for_picture(a), "{a}");
    }
    for a in ["What's in this picture?", "Build me a stock exchange website", "how do I resize an image in Photoshop"] {
        assert!(!asks_for_picture(a), "{a}");
    }
    // Lyrics pasted in are theirs to edit, not a picture or a site to build,
    // however much imagery they have.
    let lyrics = "Can you refine these lyrics? Make them flow better but keep the meaning.\n\n[Verse]\nI want to paint the sky with you tonight\n\
                  Every picture of us shining bright\nWe write our story on the same page\nCarrying the dreams we never told\n\n[Chorus]\nHold on, don't let go";
    assert!(!asks_for_picture(lyrics));
    assert!(!is_build_ask(&[lyrics]));
    assert!(edits_text(lyrics));
    let after = "[Verse]\nThe kitchen floor is cold beneath our feet\nGrandma hums a song so soft and sweet\nWe spin around beneath the moon\n\nThese are my lyrics. Make them rhyme better and fix the rhythm of the second line.";
    assert!(edits_text(after));
    assert!(edits_text("Rewrite this chorus so it's catchier:\nDraw me a picture of the place we met\nThe sun was setting, I can't forget"));
    assert!(edits_text("make the second verse sadder"), "a follow-up about lyrics earlier in the chat");
    assert!(edits_text("Can you proofread this paragraph for me? \"Their going to the store tomorow.\""));
    // Asked to hear, see or save it too: not only words.
    assert!(!edits_text("Refine these lyrics and then make a song with them:\nLine one\nLine two\nLine three"));
    assert!(!edits_text("Rewrite the lyrics in lyrics.txt on my desktop"));
    assert!(!edits_text("Improve this and draw a picture of it:\nA red fox\nin the snow"));
    assert!(!edits_text("Make me a song about the sea"));
    // A song written is words; a song made is audio.
    assert_eq!(writes_song("Write me a song about my dog"), Some(SongAsk::Unclear));
    assert_eq!(writes_song("can you write lyrics for a sad love song?"), Some(SongAsk::Words));
    assert_eq!(writes_song("write a rap about Monday mornings"), Some(SongAsk::Unclear));
    assert_eq!(writes_song("Make me a song about the sea"), None);
    assert_eq!(writes_song("Write me a song and sing it"), None);
    assert_eq!(writes_song("write a song about rain and make the music for it"), None);
    assert_eq!(writes_song("Write the lyrics into lyrics.txt"), None);
    // Carried on past the length limit: the seams Qwen3.5 9B made.
    let verse = "# Max\n**[Verse 1]**\nThe morning sun comes through the window pane\nYou're already awake, running through the rain\nA happy bark, a wagging tail so high";
    let joined = join_continued(verse, "\n[Verse 1]\nThe morning sun comes through the window pane\nYou're already awake, running through the rain\nA happy bark, a wagging tail so high\nYou're the sunshine in my sky");
    assert_eq!(joined.matches("morning sun").count(), 1, "{joined}");
    assert!(joined.starts_with("# Max\n") && joined.ends_with("You're the sunshine in my sky"), "{joined}");
    let cut = "[Chorus]\nOh Max, my best friend\nWith your fluffy fur and eyes";
    assert_eq!(join_continued(cut, "With your fluffy fur and eyes so deep\nYou're the best gift"), "[Chorus]\nOh Max, my best friend\nWith your fluffy fur and eyes so deep\nYou're the best gift");
    let line_end = "[Chorus]\nOh Max, my best friend, my loyal dog\nWe play all day\nOh Max, my best friend, my loyal dog";
    assert!(join_continued(line_end, "You make everything feel like a happy song").ends_with("loyal dog\nYou make everything feel like a happy song"));
    // A new section after a whole one isn't taken for going back.
    let whole = "[Chorus]\nOh Max\n\n[Verse 2]\nWe chase the leaves\nIn the park\n";
    assert_eq!(join_continued(whole, "[Chorus]\nOh Max"), format!("{whole}[Chorus]\nOh Max"));
    assert!(join_continued("[Bridge]\nNo matter where we go\n\n[", "[Outro]\nMy Max").ends_with("\n\n[Outro]\nMy Max"));
    // Prose cut mid-word goes straight on.
    assert_eq!(join_continued("The weather today is sun", "ny and warm."), "The weather today is sunny and warm.");
    // The answer to "just the lyrics, or the whole song?"
    let asked = ["Write me a song about my dog"];
    assert_eq!(writes_song_in("Just the lyrics please, upbeat pop", &asked), Some(SongAsk::Words));
    assert_eq!(writes_song_in("The complete song with lyrics, please", &asked), None);
    assert_eq!(writes_song_in("the lyrics should be happy", &["Make me a song"]), None);
    assert!(!edits_text("Draw me a cat in a hat"));
    assert!(!edits_text("Build me a stock exchange website"));
    assert_eq!(own_words("Refine these:\n[Verse]\na\nb\nc\nMake them rhyme"), "Refine these:\nMake them rhyme");
    assert!(denies_pictures("I'm sorry, but I can't generate images directly. However, I can help you find templates."));
    assert!(denies_pictures("As a text model I am unable to create pictures."));
    assert!(!denies_pictures("Here's Link in pixel art, standing on a grassy hill."));
    let tools = json!([{ "type": "function", "function": { "name": "create_image", "description": "", "parameters": { "type": "object", "properties": { "prompt": { "type": "string" } } } } }]);
    assert_eq!(forced_picture(&tools, false).unwrap().names, vec!["create_image".to_string()]);
    assert!(forced_picture(&json!([]), false).is_none());
    let both = json!([
        { "type": "function", "function": { "name": "create_image", "description": "", "parameters": { "type": "object", "properties": { "prompt": { "type": "string" } } } } },
        { "type": "function", "function": { "name": "edit_image", "description": "", "parameters": { "type": "object", "properties": { "instruction": { "type": "string" } } } } }
    ]);
    assert_eq!(forced_picture(&both, true).unwrap().names, vec!["edit_image".to_string()]);
    assert_eq!(forced_picture(&tools, true).unwrap().names, vec!["create_image".to_string()], "no edit tool: make one instead");
    assert!(asks_picture_change("can you add a pirate hat to me"));
    assert!(asks_picture_change("Make me look like a cartoon"));
    assert!(asks_picture_change("remove the car in the back"));
    assert!(!asks_picture_change("what is in this picture?"));
    assert!(!wants_several("Make me a picture of Link from The Legend of Zelda in pixel art."));
    assert!(wants_several("make 3 pictures of a cat") && wants_several("show me a few versions of the logo"));
}

#[test]
fn named_things_get_their_look_spelled_out_for_the_picture_model() {
    let names = names_of(r#"{"names":[
        {"name":"Link","known":true,"looks":"elf-eared young hero in a green tunic and pointed green cap, blond hair, sword and blue-red shield"},
        {"name":"Hyrule","known":false,"looks":""},
        {"name":"link","known":true,"looks":"duplicate"},
        {"name":"","known":true,"looks":"no name"}]}"#);
    assert_eq!(names.len(), 2, "duplicates and blanks dropped: {names:?}");
    assert!(names[0].described() && !names[1].described());
    let p = with_looks("Pixel art of Link from the Legend of Zelda on a hill.", &names);
    assert_eq!(p, "Pixel art of Link from the Legend of Zelda on a hill. Link looks like this: elf-eared young hero in a green tunic and pointed green cap, blond hair, sword and blue-red shield.");
    assert_eq!(with_looks("A dog on a beach", &[]), "A dog on a beach");
    assert!(names_of("not json").is_empty());
    assert_eq!(looks_of(r#"{"looks":" green tunic, pointed cap "}"#), "green tunic, pointed cap");
    // "him" isn't a name; "Link" is, as written; "zelda" in lowercase only isn't.
    let listed = names_of(r#"{"names":[{"name":"him","known":false,"looks":""},{"name":"HIM","known":true,"looks":"lobster claws, red skin, black lips, pink tulle"},{"name":"Link","known":true,"looks":"green tunic and cap, pointy ears, blond"},{"name":"Zelda","known":false,"looks":""}]}"#);
    let kept = real_names(listed, "Now put him at a party with Link", "a party with balloons, the man smiling, Link beside him");
    assert_eq!(kept.iter().map(|n| n.name.as_str()).collect::<Vec<_>>(), vec!["Link"]);
    let (ask, format) = names_check("draw link", "Link on a hill");
    assert!(ask.contains("draw link") && ask.contains("Link on a hill"));
    assert_eq!(format["json_schema"]["name"], "names");
}

#[test]
fn title_uses_first_nonempty_line_and_truncates() {
    assert_eq!(title_from("\n  hello there \nmore"), "Hello there");
    assert_eq!(title_from("Hey, can you summarize my most recent email from Google? Thanks"), "Summarize my most recent email from Google");
    assert_eq!(title_from("please help me plan a three day trip to Zion National Park with my family in November"), "Plan a three day trip to Zion National Park with…");
    assert_eq!(title_from("what's the weather in Orem"), "What's the weather in Orem");
    let offered = vec!["web_search".to_string()];
    let calls = salvage_tool_calls("\n<tool_call>\n{\"name\": \"web_search\", \"arguments\": {\"query\": \"Mount Timpanogos height\"}}\n</tool_call>", &offered);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "web_search");
    assert!(calls[0].arguments.contains("Timpanogos"));
    assert!(salvage_tool_calls("<tool_call>{\"name\": \"delete_everything\", \"arguments\": {}}</tool_call>", &offered).is_empty(), "only offered tools");
    // How qwen2.5-coder wrote one: in the <tools> tag from its template.
    let cal = vec!["calendar_events".to_string()];
    let calls = salvage_tool_calls("Sure, here's the function call:

<tools> { \"name\": \"calendar_events\", \"arguments\": { \"query\": \"Sam's party\" } } </tools>", &cal);
    assert_eq!(calls.len(), 1);
    assert!(calls[0].arguments.contains("Sam's party"));
    assert_eq!(salvage_tool_calls("```json
{\"name\": \"calendar_events\", \"parameters\": {\"from\": \"today\"}}
```", &cal)[0].arguments, "{\"from\":\"today\"}");
    assert_eq!(salvage_tool_calls(" {\"name\": \"calendar_events\", \"arguments\": {}} ", &cal).len(), 1, "a reply that is only the call");
    // How qwen2.5-coder 14B wrote one: bare JSON after some words.
    let mail = vec!["email_read".to_string()];
    let (calls, rest) = salvage("Now, I'll read the email in full.
Let's read the email with the provided ID.
{\"name\": \"email_read\", \"arguments\": {\"id\": \"3b37\"}}", &mail);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].arguments, "{\"id\":\"3b37\"}");
    assert!(!rest.contains('{') && rest.starts_with("Now"), "{rest}");
    // A model that goes on to imagine more calls: only the first runs.
    let two = vec!["email_search".to_string(), "email_send".to_string()];
    let (calls, _) = salvage("<tool_call>{\"name\": \"email_search\", \"arguments\": {}}</tool_call>
Found it.
<tool_call>{\"name\": \"email_send\", \"arguments\": {\"to\": \"john@example.com\"}}</tool_call>", &two);
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].name, "email_search");
    assert!(salvage_tool_calls("```json
{\"name\": \"Sam\", \"age\": 3}
```", &cal).is_empty(), "ordinary JSON is left alone");
    let mut out = Finished { content: "Here:
```json
{\"a\": 1}
```
<tools>{\"name\": \"calendar_events\", \"arguments\": {}}</tools>".into(), thinking: String::new(), ..Default::default() };
    let tools = json!([{ "type": "function", "function": { "name": "calendar_events" } }]);
    rescue_tool_calls(&mut out, Some(&tools));
    assert_eq!(out.tool_calls.len(), 1);
    assert!(out.content.contains("\"a\": 1"), "other code blocks stay in the reply: {}", out.content);
}

#[test]
fn a_reply_stuck_on_one_line_is_noticed() {
    let line = "*(I'm here to help - just say the date and time!)*\n";
    assert!(repeats_itself(&format!("Hello.\n{}", line.repeat(4))));
    assert!(!repeats_itself(&format!("Hello.\n{}", line.repeat(3))));
    assert!(!repeats_itself("1. a\n2. b\n3. c\n4. d\n5. e\n"));
    assert!(!repeats_itself("---\n---\n---\n---\n"), "short lines like rules are fine");
}

#[test]
fn a_reply_that_says_what_it_does_next_is_not_the_end() {
    // The 20B model's last words in the real-work test, a test still failing.
    assert!(stops_midway("Need to adjust variable names. Let's update uses."));
    assert!(stops_midway("The import is wrong. Now I'll fix the path and run the tests again."));
    assert!(stops_midway("Next, we need to export pnl from src/pnl.js."));
    // Its notes on what's left, at length, ending with what it does next.
    let notes = "We added a pct function but the code still references pct(total).toFixed(2). But we didn't add pct function earlier. \
                 We added a function pct(n) that returns n. But we need to export it. At bottom we already export {report, pnl, money}. \
                 We need to export pct as well. Let's edit bottom.\n\nAlso we need to implement function pnl similar to report but with percent. \
                 We didn't create function pnl in report.js. We only exported it. We need to define function pnl. Let's add.\n\n\
                 Also tests need to check that pnl outputs correct lines.\n\nLet's edit report.js to add function pnl.\n\nAlso we need to export pct. Let's modify.";
    assert!(notes.chars().count() > 600 && stops_midway(notes));
    // Finished, or asking the user: the end.
    assert!(!stops_midway("All four tests pass now. The pnl command prints each position's profit in percent, best first."));
    assert!(!stops_midway("Done. Let me know if you'd like the report sorted differently."));
    assert!(!stops_midway("Should I also add a total at the bottom?"));
    assert!(!stops_midway("To try it, you'll need to run npm test."));
    assert!(!stops_midway(&"It works. ".repeat(80)), "a long answer is a summary");
}

#[test]
fn a_loop_on_one_line_is_noticed_and_said_once_for_the_answer() {
    // From a real chat: the thinking ran "IEX Cloud, IEX," to the end of its allowance.
    let start = "The user wants open-source real-time stock charts. Libraries: yfinance, alpha_vantage, ";
    let stuck = format!("{start}{}IEX Cl", "IEX Cloud, IEX, ".repeat(40));
    let (at, unit) = loop_at_end(&stuck).expect("the loop is noticed");
    assert_eq!(unit, "IEX Cloud, IEX, ".len());
    assert!(at <= start.len() + unit, "found where it began: {at}");
    let notes = thought_so_far(&stuck, 8192);
    assert_eq!(notes.matches("IEX Cloud").count(), 1, "said once: {notes}");
    assert!(notes.starts_with("The user wants"));
    // Not loops: a short repeat, numbers, dashes, ordinary writing.
    assert!(loop_at_end(&format!("{start}{}", "IEX Cloud, IEX, ".repeat(5))).is_none());
    assert!(loop_at_end(&format!("data = [{}]", "0, ".repeat(200))).is_none());
    assert!(loop_at_end(&"-".repeat(400)).is_none());
    let prose = "Stock charts can come from many places. TradingView's Lightweight Charts is open source; \
                 so are Plotly and Chart.js, which draw what yfinance or Alpha Vantage return. Each has its own page.";
    assert!(loop_at_end(prose).is_none());
    // A whole sentence again and again, with no line breaks.
    assert!(loop_at_end(&format!("Okay. {}", "Let me check the search results again. ".repeat(10))).is_some());
}

#[test]
fn parts_of_a_reply_join_with_a_gap() {
    assert_eq!(joined("", "Answer."), "Answer.");
    assert_eq!(joined("First.", "  "), "First.");
    assert_eq!(joined("First.\n", "Then."), "First.\n\nThen.");
}

#[test]
fn notices_a_model_pasting_what_it_was_asked_to_make() {
    let files = vec!["read_file".to_string(), "write_file".to_string()];
    let code = "Sure! Here's a simple site.\n\n```html\n<!doctype html>\n<html>\n<body>\n<h1>Hi</h1>\n</body>\n</html>\n```\nSave it as index.html and open it.";
    assert!(builds_in_chat(&["Make me a website for my bakery"], code, &files));
    assert!(builds_in_chat(&["can you create a python script that renames my photos"], "Step 1: open your terminal. Step 2: ...", &files));
    assert!(builds_in_chat(&["put this in notes.txt"], "Create a new file called notes.txt and paste this in.", &files));
    // No folder yet: asking for one still counts as making it.
    assert!(builds_in_chat(&["Build a todo app"], code, &["request_folder".to_string()]));
    // Nothing to make it with, a question, or a short snippet in an explanation: left alone.
    assert!(!builds_in_chat(&["Make me a website"], code, &["read_file".to_string()]));
    assert!(!builds_in_chat(&["What's a closure in Rust?"], "```rust\nlet f = |x| x + 1;\nprintln!(\"{}\", f(1));\nlet g = f;\n```", &files));
    assert!(!builds_in_chat(&["How do I write a for loop in Python?"], "Like this: `for x in xs:`", &files));
    // What qwen2.5-coder said after "try again to do it for me" (the build ask came earlier).
    let refusal = "I'm sorry for the confusion. As an AI, I can't directly execute commands on your computer to build a website. However, I can guide you through the process step-by-step.";
    assert!(builds_in_chat(&["try again to do it for me", "Do all this for me", "Build the website and run it"], refusal, &files));
    assert!(!builds_in_chat(&["try again to do it for me", "What's the weather like?"], refusal, &files), "nothing was asked to be made");
    // A page that uses files it didn't write: they're named for writing next.
    let dir = std::env::temp_dir().join(format!("sulcus-pieces-{}", uuid::Uuid::new_v4().simple()));
    std::fs::create_dir_all(&dir).unwrap();
    let page = dir.join("index.html");
    std::fs::write(&page, r#"<link rel="stylesheet" href="styles.css"><script src="script.js"></script><script src="https://cdn.example.com/x.js"></script>"#).unwrap();
    std::fs::write(dir.join("script.js"), "1").unwrap();
    let missing = missing_pieces(&[page.clone()]);
    assert_eq!(missing, vec![("index.html".to_string(), dir.join("styles.css"))], "styles.css is missing; script.js is there; the CDN one isn't ours");
    let tools = json!([{ "type": "function", "function": { "name": "write_file", "description": "", "parameters": { "type": "object", "properties": { "path": { "type": "string" }, "content": { "type": "string" } } } } }]);
    let f = forced_write(&tools, "index.html", &dir.join("styles.css")).unwrap();
    assert!(f.nudge.contains("styles.css") && f.response_format.to_string().contains("const"));
    std::fs::remove_dir_all(&dir).ok();
    // What qwen2.5-coder 1.5B said to "Build me a live stock exchange website".
    assert!(builds_in_chat(&["Build me a live stock exchange website"], "Sure, I can help you build a live stock exchange website. What kind of features do you need?", &files));
    assert!(!builds_in_chat(&["What's a good name for a bakery?"], "How about Sweet Crumbs? Would you like more ideas?", &files));
    // Documents go to create_document; sites and code don't.
    assert!(wants_a_document("Write a report as a Word doc") && wants_a_document("make slides about Q3") && !wants_a_document("Build me a live stock exchange website"));
    let forced = forced_build(&json!([{ "type": "function", "function": { "name": "write_file", "description": "", "parameters": { "type": "object" } } }, { "type": "function", "function": { "name": "read_file", "description": "", "parameters": { "type": "object" } } }])).unwrap();
    assert_eq!(forced.names, vec!["write_file"]);
}

#[test]
fn notices_a_model_asking_instead_of_looking() {
    // What qwen2.5-coder said.
    assert!(stalls("Summarize my most recent email from Google", "To summarize your most recent email from Google, I'll need to search your emails for the latest one from Google. Could you please confirm if you have any specific criteria or details you'd like me to look for in that email?", &[]));
    assert!(stalls("When is sam's party?", "To find out when Sam's party is, I'll need to search your calendar for any events related to Sam's party. Can you please provide me with the name of the event or any details that might help me locate it?", &[]));
    assert!(stalls("Summarize my most recent email from Google", "I don't have access to your email to read the most recent one. However, I can help you with other tasks or answer questions based on the information you provide. Let me know how I can assist you!", &[]));
    // Real answers, and questions that aren't about the user's things, are left alone.
    assert!(!stalls("When is sam's party?", "Sam's party is today at 6:00 PM at Sam's house.", &[]));
    assert!(!stalls("What's a closure in Rust?", "Could you mean closures as in Fn traits? A closure is...", &[]));
    assert!(!stalls("Write a note about my day", "", &[]));
    assert!(stalls("What did my mom ask me?", "I don\u{2019}t have access to your email or any messages you\u{2019}ve sent or received unless you\u{2019}ve shared them.", &[]));
    assert!(stalls("When is sam's party?", &format!("I don't have any information about Sam's party in my tools or memory. {}", "More options. ".repeat(100)), &[]), "a long ramble");
    assert!(stalls("What did my mom ask me?", "I'm sorry, but I don't have any information about what your mom asked you. I don't have access to any personal communications or messages outside of our current conversation.", &[]));
    assert!(!stalls("Write a note about my day", "Here's a note:\n\nA calm day: coffee, a long walk and some reading.\n\nWant me to save it to your notes?", &[]));
    assert!(stalls("Summarize my most recent email from Google", "I don't have direct access to your email inbox. You can check your email by using the `email_search` tool.", &["email_search".to_string()]));
    assert!(stalls("anything", "Use email_search with a query like from:google.", &["email_search".to_string()]), "naming a tool instead of calling it");
    assert_eq!(clean_title("\"Trip to Tokyo.\"").as_deref(), Some("Trip to Tokyo"));
    assert_eq!(clean_title("Title: Budget plan\nextra").as_deref(), Some("Budget plan"));
    assert_eq!(clean_title("   "), None);
    let long = "a".repeat(60);
    let t = title_from(&long);
    assert!(t.ends_with('…'));
    assert_eq!(t.chars().count(), 49);
}

#[test]
fn profile_section_is_empty_when_profile_is_blank() {
    let (base, about) = system_prompt(&Profile::default(), "2026-10-06");
    assert!(base.contains("2026-10-06"));
    assert!(about.is_empty());
}

#[test]
fn profile_section_includes_what_the_user_wrote() {
    let p = Profile { name: "Sam".into(), about: "I teach biology.".into(), preferences: "Be brief.".into(), ..Default::default() };
    let (_, about) = system_prompt(&p, "2026-10-06");
    assert!(about.contains("Sam") && about.contains("biology") && about.contains("Be brief."));
}

#[test]
fn turns_start_at_user_messages_and_keep_tool_pairs_together() {
    let h = vec![m("user", "u1"), m("assistant", "a1"), m("tool", "t1"), m("assistant", "a2"), m("user", "u2"), m("assistant", "a3")];
    let t = turns(&h);
    assert_eq!(t.len(), 2);
    assert_eq!(t[0].len(), 4);
    assert_eq!(t[1][0].id, "u2");
}

#[test]
fn tool_call_pieces_are_merged_by_index() {
    let mut calls = Vec::new();
    merge_tool_delta(&mut calls, &json!({ "index": 0, "id": "c1", "function": { "name": "read_file", "arguments": "" } }));
    merge_tool_delta(&mut calls, &json!({ "index": 0, "function": { "arguments": "{\"path\":" } }));
    merge_tool_delta(&mut calls, &json!({ "index": 1, "id": "c2", "function": { "name": "list_dir", "arguments": "{}" } }));
    merge_tool_delta(&mut calls, &json!({ "index": 0, "function": { "arguments": "\"a.txt\"}" } }));
    assert_eq!(calls.len(), 2);
    assert_eq!(calls[0].arguments, r#"{"path":"a.txt"}"#);
    assert_eq!(calls[1].name, "list_dir");
}

#[test]
fn api_messages_use_openai_tool_shapes() {
    let mut a = m("assistant", "a1");
    a.tool_calls = Some(vec![ToolCall { id: "c1".into(), name: "list_dir".into(), arguments: "{}".into() }]);
    let mut t = m("tool", "t1");
    t.tool_call_id = Some("c1".into());
    let msgs = api_messages("sys", &[m("user", "u1"), a, t]);
    assert_eq!(msgs[0]["role"], "system");
    assert_eq!(msgs[2]["tool_calls"][0]["function"]["name"], "list_dir");
    assert_eq!(msgs[3]["tool_call_id"], "c1");
}
