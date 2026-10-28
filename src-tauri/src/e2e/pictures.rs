// SPDX-License-Identifier: AGPL-3.0-only
//! Pictures: making, editing, the Lens, the prompt writer, seeing pictures.

use super::*;

/// The person's own LoRA on a picture model of its family (a pixel-art
/// LoRA on RealVisXL): the same picture, same seed, with it and without it,
/// the engine loading it; then a new picture of the same character, from
/// the first as an example, on FLUX.2 [klein] 4B. The pictures go to
/// C:\dev\tools\lora-test for a look.
///     cargo test e2e_pictures_with_a_lora -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_pictures_with_a_lora_and_the_same_character() {
    use crate::media::{self, store, Request};
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    let c = state.work_cipher().unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\lora-test");
    media::install_for_test(&state, "realvisxl-v5-lightning").await.unwrap();
    let lora = media::loras::add(&state, &out.join("pixel-art-xl.safetensors")).unwrap();
    println!("added {} as {:?}", lora.name, lora.family);
    assert_eq!(lora.family, "sdxl");
    // Its trigger words, as its page gives them.
    {
        let conn = state.db.lock().unwrap();
        let mut all = media::loras::list(&conn);
        all.iter_mut().for_each(|l| l.trigger = "pixel art".into());
        media::loras::store(&conn, &all).unwrap();
    }
    let pic = |loras: Vec<(String, f32)>| Request { op: "generate".into(), prompt: "a red fox sitting on a mossy rock in a forest".into(), model_id: Some("realvisxl-v5-lightning".into()), shape: Some("square".into()), seed: Some(11), write_prompt: Some(false), check: Some(false), loras, ..Default::default() };
    let plain = media::run(&state, store::new_id(), pic(vec![])).await.unwrap().remove(0);
    let with = media::run(&state, store::new_id(), pic(vec![(lora.id.clone(), 1.0)])).await.unwrap().remove(0);
    let (a, b) = (store::read(&state.paths, &c, &plain.id).unwrap(), store::read(&state.paths, &c, &with.id).unwrap());
    std::fs::write(out.join("fox-plain.png"), &a).unwrap();
    std::fs::write(out.join("fox-pixel-lora.png"), &b).unwrap();
    let (ia, ib) = (crate::media::imaging::decode(&a).unwrap().to_rgb8(), crate::media::imaging::decode(&b).unwrap().to_rgb8());
    let diff = ia.pixels().zip(ib.pixels()).map(|(p, q)| p.0.iter().zip(q.0.iter()).map(|(x, y)| (*x as i32 - *y as i32).unsigned_abs()).sum::<u32>() as f64).sum::<f64>() / (ia.width() * ia.height() * 3) as f64;
    println!("mean change with the LoRA: {diff:.1} of 255");
    assert!(diff > 12.0, "the LoRA changed little ({diff:.1})");
    let log = std::fs::read_to_string(state.paths.logs.join("app.log")).unwrap_or_default();
    assert!(!log.contains("lora") || !log.to_lowercase().contains("failed to load lora"), "the engine didn't load it");
    // The same character, in a new scene.
    media::install_for_test(&state, "flux2-klein-4b").await.unwrap();
    let hero = media::run(&state, store::new_id(), Request { op: "generate".into(), prompt: "a young woman with short silver hair, round glasses and a yellow raincoat, standing in a park".into(), model_id: Some("flux2-klein-4b".into()), shape: Some("square".into()), seed: Some(5), write_prompt: Some(false), check: Some(false), ..Default::default() }).await.unwrap().remove(0);
    let again = media::run(&state, store::new_id(), Request { op: "generate".into(), prompt: "she is reading a book in a cozy library".into(), model_id: Some("flux2-klein-4b".into()), shape: Some("square".into()), seed: Some(6), examples: vec![hero.id.clone()], example_use: Some("subject".into()), write_prompt: Some(false), check: Some(false), ..Default::default() }).await.unwrap().remove(0);
    std::fs::write(out.join("hero.png"), store::read(&state.paths, &c, &hero.id).unwrap()).unwrap();
    std::fs::write(out.join("hero-again.png"), store::read(&state.paths, &c, &again.id).unwrap()).unwrap();
    println!("pictures in {}", out.display());
    // The test's LoRA out of the models folder it shares with the app.
    std::fs::remove_file(media::loras::dir(&state.paths).join(format!("{}.safetensors", lora.id))).ok();
}

/// A pose taken from a picture: someone in a clear pose first, then a
/// different person in a different place in that pose, on FLUX.2 [klein]
/// 4B. The pictures go to C:\dev\tools\lora-test for a look.
///     cargo test e2e_a_pose_from_a_picture -- --ignored --nocapture
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_a_pose_from_a_picture() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.work_cipher().unwrap();
    let out = std::path::PathBuf::from(r"C:\dev\tools\lora-test");
    media::install_for_test(&state, "flux2-klein-4b").await.unwrap();
    // The background remover, for the pose as a plain figure.
    media::install_for_test(&state, "birefnet-lite").await.unwrap();
    let pic = |prompt: &str, seed: i64, pose: Option<String>| Request { op: "generate".into(), prompt: prompt.into(), model_id: Some("flux2-klein-4b".into()), shape: Some("square".into()), seed: Some(seed), write_prompt: Some(false), check: Some(false), pose, ..Default::default() };
    let source = media::run(&state, store::new_id(), pic("a man in a grey t-shirt standing on one leg with both arms raised straight up, full body, plain studio background", 21, None)).await.unwrap().remove(0);
    let posed = media::run(&state, store::new_id(), pic("a knight in shining armor in a castle hall, full body", 22, Some(source.id.clone()))).await.unwrap().remove(0);
    std::fs::write(out.join("pose-source.png"), store::read(&state.paths, &c, &source.id).unwrap()).unwrap();
    std::fs::write(out.join("pose-knight.png"), store::read(&state.paths, &c, &posed.id).unwrap()).unwrap();
    println!("pictures in {}", out.display());
}

/// The Lens on Qwen3 4B (installed, imported here where it is): a reply
/// word by word with what else was weighed, carried on from another
/// choice; the layer map against everyday text; and an edited copy with
/// the three units that stood out most turned off, asked the same thing.
/// The copy is removed afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_the_lens_sees_and_edits() {
    let state = media_state();
    let real = Paths::new(app_data_dir()).unwrap();
    let file = real.models.join("qwen3-4b-2507").join("Qwen3-4B-Instruct-2507-Q4_K_M.gguf");
    crate::local::import(&state, &file).unwrap();
    let id = crate::local::list(&state.db.lock().unwrap()).into_iter().find(|m| m.local.as_ref().is_some_and(|l| std::path::Path::new(&l.path) == file)).unwrap().id;
    let prompt = "In one short sentence, what color is the sky on a clear day?";
    let w = crate::lens::words(&state, Some(&id), prompt, "", 5, 40).await.unwrap();
    let reply: String = w.pieces.iter().map(|p| p.text.as_str()).collect();
    println!("{}: {reply:?}", w.model);
    for p in w.pieces.iter().take(6) {
        println!("  {:?} {:.0}% (else: {:?})", p.text, p.prob * 100.0, p.alts.iter().skip(1).map(|a| format!("{:?} {:.0}%", a.text, a.prob * 100.0)).collect::<Vec<_>>());
    }
    assert!(!w.pieces.is_empty() && w.pieces[0].alts.len() >= 2, "odds for each piece");
    // Carried on from the first piece's second choice.
    let other = w.pieces[0].alts.iter().find(|a| a.text != w.pieces[0].text).unwrap().text.clone();
    let b = crate::lens::words(&state, Some(&id), prompt, &other, 5, 30).await.unwrap();
    println!("from {other:?}: {:?}", b.pieces.iter().map(|p| p.text.as_str()).collect::<String>());
    assert_eq!(b.given, other);

    let started = std::time::Instant::now();
    let map = crate::lens::layers(&state, Some(&id), &format!("{prompt}\n\n{reply}")).await.unwrap();
    println!("layers: {} ({} ms), editable: {:?}, top units: {:?}", map.layers, started.elapsed().as_millis(), map.editable, map.neurons.iter().take(5).map(|n| format!("L{}#{} {:.1}x", n.layer, n.index, n.ratio)).collect::<Vec<_>>());
    assert_eq!(map.energy.len(), map.layers as usize);
    assert!(map.layers >= 30 && !map.neurons.is_empty() && map.editable.is_none());

    let edits: Vec<crate::lens::Edit> = map.neurons.iter().take(3).map(|n| crate::lens::Edit { layer: n.layer, index: n.index, scale: 0.0 }).collect();
    let copy = crate::lens::edited_copy(&state, Some(&id), edits).await.unwrap();
    let e = crate::lens::words(&state, Some(&copy), prompt, "", 5, 40).await.unwrap();
    println!("edited ({}): {:?}", e.model, e.pieces.iter().map(|p| p.text.as_str()).collect::<String>());
    state.engine.lock().await.stop().await;
    let copy_path = crate::local::list(&state.db.lock().unwrap()).into_iter().find(|m| m.id == copy).and_then(|m| m.local).map(|l| std::path::PathBuf::from(l.path)).unwrap();
    assert!(copy_path.starts_with(&state.paths.models) || copy_path.exists());
    let _ = std::fs::remove_file(&copy_path);
    assert!(!e.pieces.is_empty());
}

/// Pictures look real by default, and a picture can be edited from an
/// instruction: a portrait, then a top hat added, then a party behind him.
/// SULCUSAI_MEDIA_OUT: where the three pictures are written to look at;
/// SULCUSAI_EDIT_MODEL: the model (FLUX.2 klein 4B unless set).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_realistic_picture_and_edits() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").expect("SULCUSAI_MEDIA_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let c = state.cipher().unwrap();
    let model = std::env::var("SULCUSAI_EDIT_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    media::install_for_test(&state, &model).await.unwrap();
    let mut source: Option<String> = None;
    let steps = [
        ("1-portrait", "generate", "a portrait of a smiling man in his twenties with short brown hair and a denim jacket, standing outdoors in a park"),
        ("2-top-hat", "edit", "add a tall black top hat on his head; keep his face, clothes and the background the same"),
        ("3-party", "edit", "put him at a lively house party at night with friends laughing and string lights behind him; keep his face and the top hat, and keep it a realistic photo"),
    ];
    for (name, op, prompt) in steps {
        let started = std::time::Instant::now();
        let req = Request { op: op.into(), prompt: prompt.into(), source: source.clone(), shape: Some("portrait".into()), model_id: Some(model.clone()), ..Default::default() };
        let made = media::run(&state, store::new_id(), req).await.unwrap_or_else(|e| panic!("{name}: {e}"));
        let item = &made[0];
        std::fs::write(out.join(format!("{name}.png")), store::read(&state.paths, &c, &item.id).unwrap()).unwrap();
        println!("{name}: {:.1}s, {}x{}, {:?}", started.elapsed().as_secs_f64(), item.width, item.height, item.stats);
        source = Some(item.id.clone());
    }
}

/// An edit of a real photo, planned and checked by the picture reader (the
/// barbell photo: asked to have the child lift the barbell overhead, the
/// edit model drew a second barbell and left the first on the floor).
/// SULCUSAI_EDIT_SOURCE: the photo; SULCUSAI_EDIT_PROMPT: the request;
/// SULCUSAI_MEDIA_OUT: where the result goes; SULCUSAI_EDIT_MODEL: the edit
/// model (FLUX.2 klein 4B unless set); SULCUSAI_READER: the installed chat
/// model that reads pictures (Qwen3.5 9B unless set); SULCUSAI_PLAN_ONLY:
/// print the plan and stop (the edit model isn't needed for that).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_edit_a_photo() {
    use crate::media::{self, prompts, store, Request};
    let state = media_state();
    // What the job says about its checks, printed at the end.
    crate::applog::init(&state.paths.logs);
    let var = |k: &str| std::env::var(k).unwrap_or_else(|_| panic!("set {k}"));
    let out = std::path::PathBuf::from(var("SULCUSAI_MEDIA_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let c = state.cipher().unwrap();
    let model = std::env::var("SULCUSAI_EDIT_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    let reader = std::env::var("SULCUSAI_READER").unwrap_or_else(|_| "qwen3.5-9b".into());
    let file = std::fs::read_dir(state.paths.models.join(&reader))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".gguf") && !n.starts_with("mmproj"))
        .expect("the reader's model file");
    let size = std::fs::metadata(state.paths.models.join(&reader).join(&file)).unwrap().len();
    crate::db::save_installed(&state.db.lock().unwrap(), &crate::db::InstalledModel { model_id: reader.clone(), quant: "Q4_K_M".into(), path: file, size, installed_at: 0, tps: None, prompt_tps: None }).unwrap();
    assert!(crate::vision::can_read(&state, None), "{reader} reads pictures");
    let photo = var("SULCUSAI_EDIT_SOURCE");
    let request = var("SULCUSAI_EDIT_PROMPT");
    let img = media::imaging::decode(&std::fs::read(&photo).unwrap()).unwrap();
    let m = media::models(&state).into_iter().find(|x| x.id == model).expect("a known edit model");

    // The plan, as the job makes it.
    let started = std::time::Instant::now();
    let (w, h) = media::imaging::work_size(img.width(), img.height(), 1024 * 1024);
    let before = img.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    let r = prompts::reader(&state, Some(&m)).await.unwrap();
    let plan = prompts::plan_edit(&r, &before, &request).await;
    r.done(&state).await;
    let plan = plan.unwrap();
    println!("planned in {:.1}s", started.elapsed().as_secs_f64());
    for (i, s) in plan.steps.iter().enumerate() {
        println!("step {}: {s}", i + 1);
    }
    for x in &plan.checklist {
        println!("check: {x}");
    }
    if std::env::var("SULCUSAI_PLAN_ONLY").is_ok() {
        return;
    }

    media::install_for_test(&state, &model).await.unwrap();
    let bytes = std::fs::read(&photo).unwrap();
    let mime = if photo.ends_with(".webp") { "image/webp" } else if photo.ends_with(".png") { "image/png" } else { "image/jpeg" };
    let source = media::add(&state, &c, bytes, "image", mime, "import", "photo", None, None, false).unwrap();
    let started = std::time::Instant::now();
    let req = Request { op: "edit".into(), prompt: request, source: Some(source.id.clone()), model_id: Some(model.clone()), ..Default::default() };
    let made = media::run(&state, store::new_id(), req).await.unwrap();
    let item = &made[0];
    std::fs::write(out.join("edited.png"), store::read(&state.paths, &c, &item.id).unwrap()).unwrap();
    println!("edited in {:.1}s: {}x{} (the photo is {}x{})", started.elapsed().as_secs_f64(), item.width, item.height, img.width(), img.height());
    let log = std::fs::read_to_string(state.paths.logs.join(crate::applog::APP_LOG)).unwrap_or_default();
    for l in log.lines().filter(|l| l.contains("[media]")) {
        println!("{l}");
    }
    assert_eq!((item.width, item.height), (img.width(), img.height()), "the photo's own size");
}

/// The prompt coach, run as the Studio runs it (`op: "coach"`): a picture
/// model's own language model writes prompts and edit plans, the picture
/// model makes them, the picture reader scores them, and the writer is
/// trained on its best tries (kept only when it then scores higher). About
/// 30 minutes for FLUX.2 klein 4B on an RTX 5070. Prints the stages as they
/// go and the job's log lines; stops the job if the card reaches 83 °C.
/// SULCUSAI_COACH_MODEL: the picture model (FLUX.2 klein 4B unless set);
/// SULCUSAI_READER: the installed chat model that reads pictures (Qwen3.5 9B
/// unless set).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_coach_the_prompt_writer() {
    use crate::media::{self, coach, store, Request};
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    let model = std::env::var("SULCUSAI_COACH_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    let reader = add_reader(&state);
    media::install_for_test(&state, &model).await.unwrap();
    let m = media::models(&state).into_iter().find(|x| x.id == model).expect("a known picture model");
    let writer = media::installed_file(&state, &m, "llm").expect("the picture model's own language model");
    println!("coaching {} ({}), scored by {reader}", m.name, writer.display());

    let id = store::new_id();
    let started = std::time::Instant::now();
    let watch = {
        let id = id.clone();
        tokio::spawn(async move {
            let mut last = String::new();
            let mut hottest = 0f32;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(5)).await;
                let Some(j) = media::list_jobs().into_iter().find(|j| j.id == id) else { continue };
                let temp = j.gpu_temp.unwrap_or(0.0);
                let now = format!("{} ({:.0}%)", j.stage, j.fraction * 100.0);
                if now != last || temp > hottest + 2.0 {
                    hottest = hottest.max(temp);
                    println!("[{:5.1} min] {now} · card {temp:.0} °C, {:.0}% busy", started.elapsed().as_secs_f64() / 60.0, j.gpu_usage.unwrap_or(0.0));
                    last = now;
                }
                if temp >= 83.0 {
                    println!("the card reached {temp:.0} °C: stopping");
                    media::cancel(&id);
                }
            }
        })
    };
    let req = Request { op: "coach".into(), prompt: String::new(), model_id: Some(model.clone()), ..Default::default() };
    let result = media::run(&state, id, req).await;
    watch.abort();
    println!("finished in {:.1} min", started.elapsed().as_secs_f64() / 60.0);
    let log = std::fs::read_to_string(state.paths.logs.join(crate::applog::APP_LOG)).unwrap_or_default();
    for l in log.lines().filter(|l| l.contains("[media]") || l.contains("[model]")) {
        println!("{l}");
    }
    match coach::trained(&state, &m) {
        Some(c) => println!("kept: {c:?}"),
        None => println!("no coach kept"),
    }
    result.unwrap();
}

/// Registers the installed chat model that reads pictures in a test's own
/// list: SULCUSAI_READER, or Qwen3.5 9B.
fn add_reader(state: &crate::AppState) -> String {
    let reader = std::env::var("SULCUSAI_READER").unwrap_or_else(|_| "qwen3.5-9b".into());
    let file = std::fs::read_dir(state.paths.models.join(&reader))
        .unwrap()
        .flatten()
        .map(|e| e.file_name().to_string_lossy().into_owned())
        .find(|n| n.ends_with(".gguf") && !n.starts_with("mmproj"))
        .expect("the reader's model file");
    let size = std::fs::metadata(state.paths.models.join(&reader).join(&file)).unwrap().len();
    crate::db::save_installed(&state.db.lock().unwrap(), &crate::db::InstalledModel { model_id: reader.clone(), quant: "Q4_K_M".into(), path: file, size, installed_at: 0, tps: None, prompt_tps: None }).unwrap();
    assert!(crate::vision::can_read(state, None), "{reader} reads pictures");
    reader
}

/// The prompt writer learning as it's used (`media::learn`): checked new
/// pictures and edits, each written again from the reviewer's notes until
/// it shows what was asked (up to 5 makes), then the same kinds of request
/// again with what it learnt. Prints every job's tries, the rules and
/// examples it kept, and how first tries did in each half. Stops a job if
/// the card reaches 83 °C. SULCUSAI_LEARN_MODEL: the picture model (FLUX.2
/// klein 4B unless set); SULCUSAI_READER as in `add_reader`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_prompt_writer_learns() {
    use crate::media::{self, learn, store, Request};
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    // Off unless turned on (since 0.9.20).
    learn::set_on(&state, true).unwrap();
    let model = std::env::var("SULCUSAI_LEARN_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    let reader = add_reader(&state);
    media::install_for_test(&state, &model).await.unwrap();
    let m = media::models(&state).into_iter().find(|x| x.id == model).expect("a known picture model");
    println!("{} learning, reviewed by {reader}", m.name);
    // SULCUSAI_MEDIA_OUT: where every kept picture goes, to check the reviewer by eye.
    let out = std::env::var("SULCUSAI_MEDIA_OUT").ok().map(std::path::PathBuf::from);
    let c = state.cipher().unwrap();
    let keep = |name: &str, id: &str| {
        if let Some(dir) = &out {
            let _ = std::fs::create_dir_all(dir);
            if let Ok(png) = store::read(&state.paths, &c, id) {
                let _ = std::fs::write(dir.join(format!("{name}.png")), png);
            }
        }
    };
    let started = std::time::Instant::now();
    let hot = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = {
        let hot = hot.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                for j in media::list_jobs() {
                    if j.gpu_temp.is_some_and(|t| t >= 83.0) {
                        println!("the card reached {:.0} °C: stopping", j.gpu_temp.unwrap_or(0.0));
                        hot.store(true, std::sync::atomic::Ordering::Relaxed);
                        media::cancel(&j.id);
                    }
                }
            }
        })
    };
    let log_path = state.paths.logs.join(crate::applog::APP_LOG);
    let mut seen = 0usize;
    // The job's own lines in the log since the last one.
    let mut news = || {
        let log = std::fs::read_to_string(&log_path).unwrap_or_default();
        let lines: Vec<String> = log.lines().skip(seen).map(str::to_string).collect();
        seen += lines.len();
        lines.into_iter().filter(|l| l.contains("[media]") && (l.contains("Picture ") || l.contains("Edit ") || l.contains("Kept picture") || l.contains("learnt") || l.contains("Made again") || l.contains("reviewer"))).collect::<Vec<_>>()
    };
    let first_score = |lines: &[String], word: &str| -> Option<f32> {
        let l = lines.iter().find(|l| l.contains(word))?;
        let pct = l.split(": it ").nth(1)?.split_whitespace().find(|w| w.ends_with('%'))?;
        pct.trim_end_matches('%').parse::<f32>().ok()
    };
    let pictures = [
        ("a bakery storefront with a striped awning and a sign that says \"SUNRISE BAKERY\"", "a coffee shop storefront with a green awning and a sign that says \"MORNING BREW\""),
        ("three red apples and one green pear in a wooden bowl on a white kitchen table", "two yellow lemons and three green limes on a white plate on a wooden table"),
        ("an astronaut riding a horse across a desert at sunset, two moons in the sky", "a knight riding a camel across sand dunes at noon, three suns in the sky"),
        ("a girl in a yellow raincoat holding a blue umbrella, a red bicycle on the left", "a boy in a green jacket holding an orange kite, a black dog on the right"),
        ("a birthday cake with five lit candles and the words \"HAPPY 5TH\" in blue icing", "a wedding cake with two lit candles and the words \"JUST MARRIED\" in gold icing"),
        ("a chessboard with a white queen and a black knight in front, shallow depth of field", "a checkers board with two red pieces and one black piece in front, shallow depth of field"),
    ];
    let edits = [
        (("a photo of a park bench where an old man reads a newspaper, a red bicycle leaning on a tree behind the bench", "remove the bicycle"), ("a photo of a quiet street with a blue car parked at the curb and a green trash can on the sidewalk", "remove the trash can")),
        (("a photo of a boy in a green t-shirt standing on grass next to a black and white soccer ball", "the boy holds the soccer ball above his head with both hands"), ("a photo of a girl in a red dress standing in a garden next to a yellow beach ball", "the girl holds the beach ball above her head with both hands")),
    ];
    // Three passes: learning; alike requests with what it learnt; the same
    // alike requests (same seeds, same pictures to edit) with nothing learnt
    // and learning off, to compare first tries pair by pair.
    let mut firsts: [Vec<Option<f32>>; 3] = Default::default();
    let mut edit_firsts: [Vec<Option<f32>>; 3] = Default::default();
    let mut sources: Vec<String> = Vec::new();
    let mut learnt = None;
    for half in 0..3 {
        if half == 2 {
            let l = learn::load(&state, &m);
            learn::forget(&state, &m);
            learn::set_on(&state, false).unwrap();
            learnt = Some(l);
        }
        println!("\n=== {} ===", ["first pass: learning", "second pass: alike requests, with what it learnt", "third pass: the same alike requests, nothing learnt (learning off)"][half]);
        for (i, pair) in pictures.iter().enumerate() {
            if hot.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let prompt = if half == 0 { pair.0 } else { pair.1 };
            let t0 = std::time::Instant::now();
            let req = Request { op: "generate".into(), prompt: prompt.into(), model_id: Some(model.clone()), shape: Some("square".into()), seed: Some(40 + i as i64), write_prompt: Some(true), check: Some(true), ..Default::default() };
            let made = media::run(&state, store::new_id(), req).await;
            if let Some(item) = made.as_ref().ok().and_then(|v| v.first()) {
                keep(&format!("pass{}-picture{}", half + 1, i + 1), &item.id);
            }
            let lines = news();
            println!("\n[{:5.1} min] {prompt} ({:.0} s){}", started.elapsed().as_secs_f64() / 60.0, t0.elapsed().as_secs_f64(), made.as_ref().err().map(|e| format!(" FAILED: {e}")).unwrap_or_default());
            for l in &lines {
                println!("  {}", l.split("[media] ").nth(1).unwrap_or(l));
            }
            firsts[half].push(first_score(&lines, "Picture 1 of"));
        }
        for (i, pair) in edits.iter().enumerate() {
            if hot.load(std::sync::atomic::Ordering::Relaxed) {
                break;
            }
            let (scene, change) = if half == 0 { pair.0 } else { pair.1 };
            // The third pass edits the second pass's pictures.
            let picture = match sources.get(i).filter(|_| half == 2) {
                Some(id) => id.clone(),
                None => {
                    let source = Request { op: "generate".into(), prompt: scene.into(), model_id: Some(model.clone()), shape: Some("landscape".into()), seed: Some(70 + i as i64), ..Default::default() };
                    let Ok(made) = media::run(&state, store::new_id(), source).await else { continue };
                    news();
                    if half == 1 {
                        sources.push(made[0].id.clone());
                    }
                    made[0].id.clone()
                }
            };
            let t0 = std::time::Instant::now();
            let req = Request { op: "edit".into(), prompt: change.into(), source: Some(picture), model_id: Some(model.clone()), seed: Some(90 + i as i64), ..Default::default() };
            let done = media::run(&state, store::new_id(), req).await;
            if let Some(item) = done.as_ref().ok().and_then(|v| v.first()) {
                keep(&format!("pass{}-edit{}", half + 1, i + 1), &item.id);
                if let Some(parent) = &item.parent {
                    keep(&format!("pass{}-edit{}-before", half + 1, i + 1), parent);
                }
            }
            let lines = news();
            println!("\n[{:5.1} min] edit: {change} ({:.0} s){}", started.elapsed().as_secs_f64() / 60.0, t0.elapsed().as_secs_f64(), done.as_ref().err().map(|e| format!(" FAILED: {e}")).unwrap_or_default());
            for l in &lines {
                println!("  {}", l.split("[media] ").nth(1).unwrap_or(l));
            }
            edit_firsts[half].push(first_score(&lines, "Edit 1 of"));
        }
    }
    watch.abort();
    let l = learnt.unwrap_or_else(|| learn::load(&state, &m));
    println!("\n=== what it learnt ({:.1} min) ===", started.elapsed().as_secs_f64() / 60.0);
    println!("examples: {} pictures, {} edits", l.pictures.len(), l.edits.len());
    for r in l.rules.iter().chain(&l.edit_rules) {
        println!("rule (helped {}): {}", r.helped, r.text);
    }
    let mean = |v: &[Option<f32>]| {
        let got: Vec<f32> = v.iter().flatten().copied().collect();
        if got.is_empty() { f32::NAN } else { got.iter().sum::<f32>() / got.len() as f32 }
    };
    // With what it learnt against without, on the same request, seed and picture.
    let paired = |with: &[Option<f32>], without: &[Option<f32>]| -> (Vec<f32>, f32) {
        let d: Vec<f32> = with.iter().zip(without).filter_map(|(a, b)| Some(a.as_ref()? - b.as_ref()?)).collect();
        let m = if d.is_empty() { f32::NAN } else { d.iter().sum::<f32>() / d.len() as f32 };
        (d, m)
    };
    for (what, v) in [("new pictures", &firsts), ("edits", &edit_firsts)] {
        println!("first tries, {what}: learning {:?} (mean {:.0}%); alike, with it {:?} (mean {:.0}%); the same, without {:?} (mean {:.0}%)", v[0], mean(&v[0]), v[1], mean(&v[1]), v[2], mean(&v[2]));
        let (d, m) = paired(&v[1], &v[2]);
        println!("  with minus without, request by request: {d:?} points (mean {m:+.1})");
    }
    assert!(!hot.load(std::sync::atomic::Ordering::Relaxed), "stopped: the card got too hot");
    assert!(l.firsts.len() + l.edit_firsts.len() > 0, "it learnt from the checked jobs");
}

/// The app log's picture-job lines since `seen` (without their time and tag).
fn media_lines(state: &crate::AppState, seen: &mut usize) -> Vec<String> {
    let log = std::fs::read_to_string(state.paths.logs.join(crate::applog::APP_LOG)).unwrap_or_default();
    let lines: Vec<String> = log.lines().skip(*seen).map(str::to_string).collect();
    *seen += lines.len();
    lines.iter().filter_map(|l| l.split("[media] ").nth(1)).map(str::to_string).collect()
}

/// Each make's score in a checked job, in order, from lines like "Picture 2
/// of at most 5: it showed 75% of what was asked for".
fn make_scores(lines: &[String]) -> Vec<f32> {
    lines
        .iter()
        .filter(|l| (l.starts_with("Picture ") || l.starts_with("Edit ")) && l.contains(" of at most "))
        .filter_map(|l| l.split(": it ").nth(1)?.split_whitespace().find(|w| w.ends_with('%'))?.trim_end_matches('%').parse().ok())
        .collect()
}

/// One held-out job in the learning evaluation.
struct Tried {
    name: String,
    edit: bool,
    case: usize,
    seed: usize,
    with: bool,
    item: Option<String>,
    before: Option<String>,
    /// The system's own score for each make.
    scores: Vec<f32>,
    /// What was handed over, against a checklist written by hand.
    fixed: Option<f32>,
    missed: Vec<String>,
}

/// Paired differences: how many, their mean, its standard error, and how
/// many came out ahead and behind.
fn paired(d: &[f32]) -> (usize, f32, f32, usize, usize) {
    let n = d.len();
    if n == 0 {
        return (0, f32::NAN, f32::NAN, 0, 0);
    }
    let mean = d.iter().sum::<f32>() / n as f32;
    let var = if n > 1 { d.iter().map(|x| (x - mean).powi(2)).sum::<f32>() / (n - 1) as f32 } else { 0.0 };
    (n, mean, (var / n as f32).sqrt(), d.iter().filter(|x| **x > 0.0).count(), d.iter().filter(|x| **x < 0.0).count())
}

/// Whether learning from use helps, on requests it hasn't seen. Training:
/// 10 hard pictures and 3 edits, learning on. Then learning is frozen and
/// each held-out request runs twice back to back, with what it learnt and
/// with nothing learnt (alternating which goes first), with the same seed
/// (two seeds a picture request) and the same picture to edit. Every job is
/// the product as it is: written, made, reviewed and written again, up to 5
/// makes. At the end one reader judges everything handed over against
/// checklists written by hand, so the yardstick doesn't depend on what the
/// system wrote. Prints each job and the paired differences, and writes the
/// pictures and `verdicts.txt` to SULCUSAI_MEDIA_OUT (or a temp folder). About
/// 80 minutes on an RTX 5070; a job is stopped if the card reaches 83 °C.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_prompt_learning_eval() {
    use crate::media::{self, learn, prompts, store, Request};
    use std::sync::atomic::Ordering::Relaxed;
    let state = media_state();
    crate::applog::init(&state.paths.logs);
    // Off unless turned on (since 0.9.20).
    learn::set_on(&state, true).unwrap();
    let model = std::env::var("SULCUSAI_LEARN_MODEL").unwrap_or_else(|_| "flux2-klein-4b".into());
    let reader = add_reader(&state);
    media::install_for_test(&state, &model).await.unwrap();
    let m = media::models(&state).into_iter().find(|x| x.id == model).expect("a known picture model");
    let out = std::env::var("SULCUSAI_MEDIA_OUT").map(std::path::PathBuf::from).unwrap_or_else(|_| std::env::temp_dir().join("sulcusai-learning-eval"));
    std::fs::create_dir_all(&out).unwrap();
    println!("{} learning evaluation, reviewed by {reader}; pictures in {}", m.name, out.display());
    let c = state.cipher().unwrap();
    let save = |name: &str, id: &str| {
        if let Ok(png) = store::read(&state.paths, &c, id) {
            let _ = std::fs::write(out.join(format!("{name}.png")), png);
        }
    };
    let hot = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let watch = {
        let hot = hot.clone();
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(3)).await;
                for j in media::list_jobs() {
                    if j.gpu_temp.is_some_and(|t| t >= 83.0) {
                        println!("the card reached {:.0} °C: stopping", j.gpu_temp.unwrap_or(0.0));
                        hot.store(true, Relaxed);
                        media::cancel(&j.id);
                    }
                }
            }
        })
    };
    let started = std::time::Instant::now();
    let min = || started.elapsed().as_secs_f64() / 60.0;
    let mut seen = 0usize;
    let picture = |prompt: &str, seed: i64| Request { op: "generate".into(), prompt: prompt.into(), model_id: Some(model.clone()), shape: Some("square".into()), seed: Some(seed), write_prompt: Some(true), check: Some(true), ..Default::default() };
    let scene = |prompt: &str, seed: i64| Request { op: "generate".into(), prompt: prompt.into(), model_id: Some(model.clone()), shape: Some("landscape".into()), seed: Some(seed), ..Default::default() };
    let edit = |change: &str, source: &str, seed: i64| Request { op: "edit".into(), prompt: change.into(), source: Some(source.into()), model_id: Some(model.clone()), seed: Some(seed), ..Default::default() };

    // Hard on purpose: counts, words, left and right, odd colors, a clock.
    const TRAIN: &[&str] = &[
        "a bookshelf with exactly four red books on the top shelf and two green books on the bottom shelf",
        "a street sign that says \"MAPLE AVE\" above a stop sign",
        "a woman in a red coat on the left and a man in a yellow raincoat on the right, both holding coffee cups",
        "a blue banana and a purple apple on a wooden cutting board",
        "a wall clock showing a quarter past three",
        "five balloons tied to a park bench: red, orange, yellow, green and blue",
        "a cat sitting inside a cardboard box that says \"FRAGILE\" in black letters",
        "a chef holding a frying pan in his left hand and a wooden spoon in his right hand",
        "a snowman wearing sunglasses and a striped scarf on a sunny beach",
        "a neon sign that reads \"OPEN 24 HOURS\" in a dark diner window",
    ];
    const TEST: &[(&str, &[&str])] = &[
        ("a kitchen shelf with exactly three blue mugs on the top shelf and two white bowls on the bottom shelf", &["a kitchen shelf", "three blue mugs on the top shelf", "two white bowls on the bottom shelf"]),
        ("a shop sign that says \"HARBOR BOOKS\" above a red door", &["a sign that says \"HARBOR BOOKS\"", "a red door", "the sign is above the door"]),
        ("a boy in a green hoodie on the left and a girl in a purple dress on the right, both holding ice cream cones", &["a boy in a green hoodie on the left", "a girl in a purple dress on the right", "the boy holds an ice cream cone", "the girl holds an ice cream cone"]),
        ("a pink lemon and an orange cucumber on a white plate", &["a pink lemon", "an orange cucumber", "a white plate"]),
        ("a wall clock showing half past seven", &["a wall clock", "the clock shows half past seven"]),
        ("four kites in the sky above a hill: red, yellow, green and blue", &["exactly four kites in the sky", "a red kite", "a yellow kite", "a green kite", "a blue kite", "a hill"]),
        ("a puppy sitting inside a wooden crate that says \"HANDLE WITH CARE\" in red letters", &["a puppy sitting inside a wooden crate", "the crate says \"HANDLE WITH CARE\"", "the letters are red"]),
        ("a painter holding a brush in her right hand and a palette in her left hand", &["a painter holding a brush in her right hand", "the painter holds a palette in her left hand"]),
        ("a penguin wearing a straw hat and a flower necklace in a desert", &["a penguin", "the penguin wears a straw hat", "the penguin wears a flower necklace", "a desert"]),
        ("a chalkboard sign that reads \"FRESH COFFEE\" outside a cafe", &["a chalkboard sign that reads \"FRESH COFFEE\"", "the sign stands outside a cafe"]),
    ];
    const TRAIN_EDITS: &[(&str, &str)] = &[
        ("a photo of a living room with a grey sofa, a red cushion on the sofa and a black cat on the floor", "remove the cat"),
        ("a photo of a girl standing in a park holding a red balloon on a string", "make the balloon blue and add a second, yellow balloon"),
        ("a photo of a man sitting at a desk with a laptop and a white coffee mug", "the man lifts the coffee mug to his mouth"),
    ];
    // Scene, change, checklist written by hand, and the thing that moves (one of it after).
    const TEST_EDITS: &[(&str, &str, &[&str], &str)] = &[
        ("a photo of a kitchen with a white table, a bowl of oranges on the table and a brown dog on the floor", "remove the dog", &["no dog in the picture", "the white table is still there", "the bowl of oranges is still on the table"], ""),
        ("a photo of a boy standing on a beach holding a green kite", "make the kite red and add a second, blue kite in the sky", &["the boy holds a red kite", "a blue kite in the sky", "no green kite"], ""),
        ("a photo of a woman sitting on a bench with a book and a water bottle beside her", "the woman lifts the water bottle to her mouth", &["the woman holds the water bottle at her mouth", "the book is still on the bench"], "bottle"),
    ];

    // 1. Training, learning on.
    println!("\n=== training: {} pictures and {} edits, learning on ===", TRAIN.len(), TRAIN_EDITS.len());
    for (i, prompt) in TRAIN.iter().enumerate() {
        if hot.load(Relaxed) {
            break;
        }
        let made = media::run(&state, store::new_id(), picture(prompt, 100 + i as i64)).await;
        let lines = media_lines(&state, &mut seen);
        println!("[{:5.1} min] train {}: {:?} {}", min(), i + 1, make_scores(&lines), made.as_ref().err().map(|e| format!("FAILED: {e}")).unwrap_or_default());
        for l in lines.iter().filter(|l| l.starts_with("Stopped") || l.contains("learnt")) {
            println!("    {l}");
        }
        if let Some(it) = made.ok().and_then(|v| v.into_iter().next()) {
            save(&format!("train-picture{:02}", i + 1), &it.id);
        }
    }
    // A picture model that doesn't edit is judged on new pictures only.
    let edits_too = m.can("edit");
    for (i, (place, change)) in TRAIN_EDITS.iter().enumerate() {
        if hot.load(Relaxed) || !edits_too {
            break;
        }
        let Ok(src) = media::run(&state, store::new_id(), scene(place, 200 + i as i64)).await else { continue };
        media_lines(&state, &mut seen);
        let made = media::run(&state, store::new_id(), edit(change, &src[0].id, 300 + i as i64)).await;
        let lines = media_lines(&state, &mut seen);
        println!("[{:5.1} min] train edit {}: {:?} {change}", min(), i + 1, make_scores(&lines));
        for l in lines.iter().filter(|l| l.starts_with("Stopped") || l.contains("learnt")) {
            println!("    {l}");
        }
        if let Some(it) = made.ok().and_then(|v| v.into_iter().next()) {
            save(&format!("train-edit{}", i + 1), &it.id);
        }
    }
    let snapshot = learn::load(&state, &m);
    learn::set_on(&state, false).unwrap();
    println!("\nlearnt: {} picture examples, {} edit examples", snapshot.pictures.len(), snapshot.edits.len());
    for r in snapshot.rules.iter().chain(&snapshot.edit_rules) {
        println!("rule (helped {}): {}", r.helped, r.text);
    }

    // 2. Held out, learning frozen: with and without, back to back.
    println!("\n=== held out: {} pictures x 2 seeds and {} edits, each with and without what it learnt ===", TEST.len(), TEST_EDITS.len());
    let use_learning = |on: bool| {
        if on {
            learn::save(&state, &m, &snapshot).unwrap();
        } else {
            learn::forget(&state, &m);
        }
    };
    let mut tried: Vec<Tried> = Vec::new();
    for (i, (prompt, _)) in TEST.iter().enumerate() {
        for k in 0..2 {
            let order = if (i + k) % 2 == 0 { [true, false] } else { [false, true] };
            for with in order {
                if hot.load(Relaxed) {
                    break;
                }
                use_learning(with);
                let made = media::run(&state, store::new_id(), picture(prompt, 500 + (2 * i + k) as i64)).await;
                let lines = media_lines(&state, &mut seen);
                let name = format!("test{:02}-seed{}-{}", i + 1, k + 1, if with { "with" } else { "without" });
                let item = made.ok().and_then(|v| v.into_iter().next()).map(|x| x.id);
                if let Some(id) = &item {
                    save(&name, id);
                }
                let scores = make_scores(&lines);
                println!("[{:5.1} min] {name}: {scores:?}", min());
                tried.push(Tried { name, edit: false, case: i, seed: k, with, item, before: None, scores, fixed: None, missed: Vec::new() });
            }
        }
    }
    for (i, (place, change, _, _)) in TEST_EDITS.iter().enumerate() {
        if hot.load(Relaxed) || !edits_too {
            break;
        }
        let Ok(src) = media::run(&state, store::new_id(), scene(place, 600 + i as i64)).await else { continue };
        media_lines(&state, &mut seen);
        let source = src[0].id.clone();
        save(&format!("test-edit{}-before", i + 1), &source);
        let order = if i % 2 == 0 { [true, false] } else { [false, true] };
        for with in order {
            if hot.load(Relaxed) {
                break;
            }
            use_learning(with);
            let made = media::run(&state, store::new_id(), edit(change, &source, 700 + i as i64)).await;
            let lines = media_lines(&state, &mut seen);
            let name = format!("test-edit{}-{}", i + 1, if with { "with" } else { "without" });
            let item = made.ok().and_then(|v| v.into_iter().next()).map(|x| x.id);
            if let Some(id) = &item {
                save(&name, id);
            }
            let scores = make_scores(&lines);
            println!("[{:5.1} min] {name}: {scores:?}", min());
            tried.push(Tried { name, edit: true, case: i, seed: 0, with, item, before: Some(source.clone()), scores, fixed: None, missed: Vec::new() });
        }
    }
    watch.abort();

    // 3. Everything handed over, judged against the checklists written by hand.
    println!("\n=== judging what was handed over against fixed checklists ({:.1} min) ===", min());
    let r = prompts::reader(&state, Some(&m)).await.unwrap();
    let decode = |id: &str| store::read(&state.paths, &c, id).and_then(|b| media::imaging::decode(&b));
    for t in &mut tried {
        let Some(after) = t.item.as_deref().and_then(|id| decode(id).ok()) else { continue };
        let v = if t.edit {
            let (_, _, list, moved) = TEST_EDITS[t.case];
            let Some(before) = t.before.as_deref().and_then(|id| decode(id).ok()) else { continue };
            let plan = prompts::EditPlan { checklist: list.iter().map(|s| s.to_string()).collect(), moved: if moved.is_empty() { Vec::new() } else { vec![moved.to_string()] }, ..Default::default() };
            prompts::judge_edit(&r, &before, &after, &plan).await
        } else {
            prompts::judge(&r, &after, &TEST[t.case].1.iter().map(|s| s.to_string()).collect::<Vec<_>>()).await
        };
        if let Ok(v) = v {
            t.fixed = Some(v.score * 100.0);
            t.missed = v.missing;
        }
    }
    r.done(&state).await;

    // 4. Results.
    let mut report = String::new();
    for t in &tried {
        let line = format!("{}: system's scores {:?}, by hand {}{}", t.name, t.scores, t.fixed.map_or("-".to_string(), |f| format!("{f:.0}%")), if t.missed.is_empty() { String::new() } else { format!(" (missing: {})", t.missed.join("; ")) });
        println!("{line}");
        report.push_str(&line);
        report.push('\n');
    }
    for edits in [false, true] {
        let pairs: Vec<(&Tried, &Tried)> = tried.iter().filter(|t| t.edit == edits && t.with).filter_map(|a| tried.iter().find(|b| b.edit == edits && !b.with && b.case == a.case && b.seed == a.seed).map(|b| (a, b))).collect();
        let diff = |f: &dyn Fn(&Tried) -> Option<f32>| -> Vec<f32> { pairs.iter().filter_map(|(a, b)| Some(f(a)? - f(b)?)).collect() };
        let what = if edits { "edits" } else { "new pictures" };
        for (label, d) in [
            ("first try (system's checklist)", diff(&|t: &Tried| t.scores.first().copied())),
            ("handed over (system's checklist)", diff(&|t: &Tried| t.scores.iter().copied().reduce(f32::max))),
            ("handed over (checklist by hand)", diff(&|t: &Tried| t.fixed)),
            ("makes needed (fewer is better)", diff(&|t: &Tried| (!t.scores.is_empty()).then_some(t.scores.len() as f32))),
        ] {
            let (n, mean, se, ahead, behind) = paired(&d);
            let line = format!("{what}, {label}: with minus without over {n} pairs: mean {mean:+.1} (standard error {se:.1}); with ahead {ahead}, behind {behind}, level {}", n - ahead - behind);
            println!("{line}");
            report.push_str(&line);
            report.push('\n');
        }
    }
    let _ = std::fs::write(out.join("verdicts.txt"), report);
    println!("\n{:.1} min in all", min());
    assert!(!hot.load(Relaxed), "stopped: the card got too hot");
}

/// A model from Hugging Face, installed the way the Studio installs one
/// (sharing the catalog model's text reader and decoder), makes a picture.
/// SULCUSAI_HUB_REPO / SULCUSAI_HUB_FILE pick it (a FLUX.2 klein 4B
/// quantization unless set). Its own file is deleted afterwards.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_hub_media_model() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let c = state.cipher().unwrap();
    let repo = std::env::var("SULCUSAI_HUB_REPO").unwrap_or_else(|_| "unsloth/FLUX.2-klein-4B-GGUF".into());
    let file = std::env::var("SULCUSAI_HUB_FILE").unwrap_or_else(|_| "flux-2-klein-4b-Q5_K_M.gguf".into());
    let spec = media::hub::prepare(&state, &repo, &file).await.unwrap();
    println!("prepared {} ({}): {:?}", spec.name, spec.id, spec.files.iter().map(|f| (&f.role, &f.file)).collect::<Vec<_>>());
    media::install_for_test(&state, &spec.id).await.unwrap();
    assert!(media::installed(&state.db.lock().unwrap()).iter().any(|i| i.model_id == spec.id));
    let req = Request { op: "generate".into(), prompt: "a lighthouse on a rocky cliff at dusk, photograph".into(), model_id: Some(spec.id.clone()), shape: Some("landscape".into()), seed: Some(7), ..Default::default() };
    let made = media::run(&state, store::new_id(), req).await.unwrap();
    assert_eq!(made[0].model_id.as_deref(), Some(spec.id.as_str()), "made with the Hugging Face model");
    let png = store::read(&state.paths, &c, &made[0].id).unwrap();
    assert!(image::load_from_memory(&png).is_ok());
    if let Ok(out) = std::env::var("SULCUSAI_MEDIA_OUT") {
        std::fs::write(std::path::Path::new(&out).join("hub-model.png"), &png).unwrap();
    }
    println!("made: {:?}", made[0].stats);
    // Only its own file goes; the shared ones stay for the catalog model.
    let own: Vec<std::path::PathBuf> = spec.files.iter().filter(|f| f.file.starts_with("hfm-")).map(|f| media::model_dir(&state.paths, &spec).join(&f.file)).collect();
    assert_eq!(own.len(), 1);
    for p in own {
        std::fs::remove_file(&p).unwrap();
    }
}

/// Renders a set of pictures to compare looks and models. SULCUSAI_PICS:
/// "name|model|style|steps|prompt" entries separated by ";;" (empty model,
/// style or steps: the defaults); SULCUSAI_MEDIA_OUT: where they go.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_picture_set() {
    use crate::media::{self, store, Request};
    let state = media_state();
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").expect("SULCUSAI_MEDIA_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let c = state.cipher().unwrap();
    let list = std::env::var("SULCUSAI_PICS").expect("SULCUSAI_PICS");
    for entry in list.split(";;").map(str::trim).filter(|e| !e.is_empty()) {
        let f: Vec<&str> = entry.splitn(5, '|').collect();
        let [name, model, style, steps, prompt] = f[..] else { panic!("bad entry: {entry}") };
        if !model.is_empty() {
            media::install_for_test(&state, model).await.unwrap_or_else(|e| panic!("installing {model}: {e}"));
        }
        let req = Request {
            op: "generate".into(),
            prompt: prompt.into(),
            model_id: (!model.is_empty()).then(|| model.to_string()),
            style: (!style.is_empty()).then(|| style.to_string()),
            steps: steps.parse().ok(),
            shape: Some("landscape".into()),
            seed: Some(42),
            ..Default::default()
        };
        let started = std::time::Instant::now();
        match media::run(&state, store::new_id(), req).await {
            Ok(made) => {
                std::fs::write(out.join(format!("{name}.png")), store::read(&state.paths, &c, &made[0].id).unwrap()).unwrap();
                println!("{name}: {:.1}s {:?}", started.elapsed().as_secs_f64(), made[0].stats);
            }
            Err(e) => println!("{name}: FAILED {e}"),
        }
    }
}

/// Pictures, edits, upscaling, cut-outs, music, narration and video with the
/// real engines, installed the way the app installs them. SULCUSAI_MEDIA
/// picks the parts (default: everything); results are written to
/// SULCUSAI_MEDIA_OUT for a look.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_media() {
    use crate::media::{self, store, Request};
    use base64::Engine as _;
    let state = media_state();
    let parts = std::env::var("SULCUSAI_MEDIA").unwrap_or_else(|_| "picture,upscale,cutout,music,narrate,video".into());
    let want = |p: &str| parts.split(',').any(|x| x == p);
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").unwrap_or_else(|_| std::env::temp_dir().join("sulcusai-media-out").display().to_string()));
    std::fs::create_dir_all(&out).unwrap();
    let c = state.cipher().unwrap();
    let keep = |item: &media::MediaItem, name: &str| {
        let bytes = store::read(&state.paths, &c, &item.id).unwrap();
        let ext = item.mime.split('/').nth(1).unwrap_or("bin").replace("mpeg", "mp3");
        std::fs::write(out.join(format!("{name}.{ext}")), &bytes).unwrap();
        bytes
    };
    let go = |req: Request| {
        let state = state.clone();
        async move {
            let started = std::time::Instant::now();
            let op = req.op.clone();
            let r = media::run(&state, store::new_id(), req).await;
            println!("{op}: {:.1}s -> {}", started.elapsed().as_secs_f64(), match &r {
                Ok(i) => format!("{} item(s)", i.len()),
                Err(e) => e.clone(),
            });
            r
        }
    };
    let install = |id: &'static str| {
        let state = state.clone();
        async move {
            let started = std::time::Instant::now();
            media::install_for_test(&state, id).await.unwrap_or_else(|e| panic!("installing {id}: {e}"));
            println!("installed {id} in {:.1}s", started.elapsed().as_secs_f64());
        }
    };

    if want("picture") || want("upscale") || want("cutout") || want("video") {
        install("flux2-klein-4b").await;
        let made = go(Request { op: "generate".into(), prompt: "a red fox sitting in a snowy forest at sunrise, photograph".into(), shape: Some("landscape".into()), ..Default::default() }).await.unwrap();
        assert_eq!(made.len(), 1);
        let fox = &made[0];
        assert!(fox.width > fox.height, "landscape: {}x{}", fox.width, fox.height);
        let png = keep(fox, "1-generate");
        assert!(image::load_from_memory(&png).is_ok());
        assert!(!png.windows(9).any(|w| w == b"snowy for"), "the prompt isn't written into the file");

        if want("picture") {
            let night = go(Request { op: "edit".into(), source: Some(fox.id.clone()), prompt: "make it night with a full moon".into(), ..Default::default() }).await.unwrap();
            keep(&night[0], "2-edit");
            assert_eq!(night[0].parent.as_deref(), Some(fox.id.as_str()));

            // Paint a circle over the middle: put a snowman there.
            let mut mask = image::RgbaImage::new(fox.width, fox.height);
            let (cx, cy, r) = (fox.width as i64 / 2, fox.height as i64 / 2, fox.height as i64 / 4);
            for (x, y, p) in mask.enumerate_pixels_mut() {
                if (x as i64 - cx).pow(2) + (y as i64 - cy).pow(2) < r * r {
                    *p = image::Rgba([255, 64, 64, 255]);
                }
            }
            let mut mask_png = Vec::new();
            image::DynamicImage::ImageRgba8(mask).write_to(&mut std::io::Cursor::new(&mut mask_png), image::ImageFormat::Png).unwrap();
            let filled = go(Request {
                op: "fill".into(),
                source: Some(fox.id.clone()),
                prompt: "a small snowman with a carrot nose".into(),
                mask: Some(base64::engine::general_purpose::STANDARD.encode(&mask_png)),
                ..Default::default()
            })
            .await
            .unwrap();
            assert_eq!((filled[0].width, filled[0].height), (fox.width, fox.height), "filled in at the original size");
            let filled_png = keep(&filled[0], "3-fill");
            let (a, b) = (image::load_from_memory(&png).unwrap().to_rgba8(), image::load_from_memory(&filled_png).unwrap().to_rgba8());
            assert_eq!(a.get_pixel(2, 2), b.get_pixel(2, 2), "outside the painted area nothing changes");

            let wide = go(Request { op: "extend".into(), source: Some(fox.id.clone()), extend: Some([256, 0, 256, 0]), ..Default::default() }).await.unwrap();
            keep(&wide[0], "4-extend");
            assert!(wide[0].width as f64 / wide[0].height as f64 > fox.width as f64 / fox.height as f64);

            let styled = go(Request { op: "restyle".into(), source: Some(fox.id.clone()), prompt: "watercolor".into(), ..Default::default() }).await.unwrap();
            keep(&styled[0], "5-restyle");

            let two = go(Request { op: "generate".into(), prompt: "a cozy cabin, oil painting".into(), count: Some(2), ..Default::default() }).await.unwrap();
            assert_eq!(two.len(), 2, "two versions");
        }
        if want("upscale") {
            install("realesrgan-x4plus").await;
            let big = go(Request { op: "upscale".into(), source: Some(fox.id.clone()), ..Default::default() }).await.unwrap();
            keep(&big[0], "6-upscale");
            assert!(big[0].width >= fox.width * 3, "{} vs {}", big[0].width, fox.width);
        }
        if want("cutout") {
            install("birefnet-lite").await;
            let cut = go(Request { op: "remove_background".into(), source: Some(fox.id.clone()), ..Default::default() }).await.unwrap();
            let bytes = keep(&cut[0], "7-cutout");
            let img = image::load_from_memory(&bytes).unwrap().to_rgba8();
            let clear = img.pixels().filter(|p| p[3] < 16).count() as f64 / (img.width() * img.height()) as f64;
            println!("cut-out: {:.0}% transparent", clear * 100.0);
            assert!(clear > 0.2 && clear < 0.95, "the background went, the fox stayed");
        }
        if want("video") {
            install("wan2.2-ti2v-5b").await;
            let clip = go(Request { op: "video".into(), prompt: "the fox turns its head and snow falls gently".into(), source: Some(fox.id.clone()), seconds: Some(2.0), ..Default::default() }).await.unwrap();
            let bytes = keep(&clip[0], "8-video");
            assert_eq!(&bytes[..4], &[0x1A, 0x45, 0xDF, 0xA3], "a WebM file");
            assert_eq!(clip[0].kind, "video");
            println!("video: {:?}", clip[0].stats);
            let s = clip[0].stats.as_ref().expect("what making it took");
            assert!(s.frames.is_some_and(|f| f > 0) && s.steps.is_some());
        }
    }
    if want("music") {
        // SULCUSAI_MUSIC_MODEL: another music model than ACE-Step 1.5.
        let music: &'static str = Box::leak(std::env::var("SULCUSAI_MUSIC_MODEL").unwrap_or_else(|_| "ace-step-1.5".into()).into_boxed_str());
        install(music).await;
        let song = go(Request { op: "music".into(), model_id: Some(music.into()), prompt: "upbeat acoustic folk song about a road trip through the mountains, warm male vocals".into(), seconds: Some(20.0), ..Default::default() }).await.unwrap();
        let bytes = keep(&song[0], "9-song");
        assert!(bytes.len() > 100_000);
        println!("song: {:.1}s, lyrics: {:?}, {:?}", song[0].seconds, song[0].lyrics.as_deref().map(|l| l.chars().take(160).collect::<String>()), song[0].stats);
        assert!(song[0].stats.as_ref().and_then(|s| s.audio_tokens).is_some_and(|t| t > 0), "audio tokens are counted");
        let sound = go(Request { op: "sound".into(), model_id: Some(music.into()), prompt: "rain on a tin roof with distant thunder".into(), seconds: Some(10.0), ..Default::default() }).await.unwrap();
        keep(&sound[0], "10-sound");
    }
    if want("narrate") {
        let n = go(Request { op: "narrate".into(), prompt: "Welcome to the studio. Everything here is made on this computer.".into(), ..Default::default() }).await.unwrap();
        let bytes = keep(&n[0], "11-narrate");
        assert_eq!(&bytes[..4], b"RIFF");
        assert!(n[0].seconds > 1.0);
    }
    println!("results in {}", out.display());
    let measured: Vec<(String, Option<f64>)> = media::installed(&state.db.lock().unwrap()).into_iter().map(|i| (i.model_id, i.secs)).collect();
    println!("measured: {measured:?}");
}

/// A model that can see: Qwen3 VL 4B with its image encoder describes a
/// picture sent the way chats send attachments. SULCUSAI_VISION_IMAGE is
/// the picture (a PNG or JPEG); SULCUSAI_VISION_EXPECT a word the answer
/// should contain.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_vision() {
    use base64::Engine as _;
    let paths = Paths::new(app_data_dir()).unwrap();
    let hw = hardware::detect(&paths.models);
    let budget = Budget::from_hardware(&hw);
    let cat = Catalog::bundled();
    let spec = cat.model("qwen3-vl-4b").unwrap();
    let variant = spec.variant("Q4_K_M").unwrap();
    let vision = spec.vision.as_ref().expect("Qwen3 VL has an image encoder");
    let cancel = AtomicBool::new(false);
    let exe = engine::ensure_installed(&paths, &cat.engine, engine::backend_for(&budget).unwrap(), &cancel, |_, _| {}).await.unwrap();
    let client = net::external_client(net::Connectivity::Offline, net::Purpose::ModelDownload, false).unwrap();
    let dest = paths.models.join(&spec.id).join(&variant.file);
    download::fetch_verified(&client, &variant.url, &dest, variant.size, &variant.sha256, &cancel, |_, _| {}).await.unwrap();
    let mmproj = paths.models.join(&spec.id).join(&vision.file);
    download::fetch_verified(&client, &vision.url, &mmproj, vision.size, &vision.sha256, &cancel, |_, _| {}).await.unwrap();
    println!("model and image encoder verified");

    let mut eng = engine::Engine::default();
    let ep = eng
        .ensure(
            engine::LaunchSpec { exe: &exe, model_path: &dest, model_id: &spec.id, quant: "Q4_K_M", ctx: 8192, gpu_layers: 99, threads: 0, low_priority: false, log: &paths.engine_log(), mmproj: Some(&mmproj), batch: 0, lora: None, steer: None },
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("engine failed to start: {e}"));
    assert!(eng.sees());

    let image = std::env::var("SULCUSAI_VISION_IMAGE").expect("set SULCUSAI_VISION_IMAGE to a picture");
    let bytes = std::fs::read(&image).unwrap();
    let mime = if image.to_lowercase().ends_with(".png") { "image/png" } else { "image/jpeg" };
    let url = format!("data:{mime};base64,{}", base64::engine::general_purpose::STANDARD.encode(&bytes));
    let history = vec![Message {
        id: "u1".into(),
        chat_id: "c".into(),
        role: "user".into(),
        content: "What animal is in this picture, and where is it? Answer in one sentence.".into(),
        created_at: crate::db::now_ms(),
        meta: Some(serde_json::json!({ "images": ["pic"] })),
        ..Default::default()
    }];
    let mut messages = chat::api_messages("You are a helpful assistant.", &history);
    let see = |_: &str| Some(url.clone());
    chat::attach_images(&mut messages, &history, Some(&see), false, &[]);
    assert!(messages[1]["content"].is_array(), "the picture went in as a content part");
    let started = std::time::Instant::now();
    let answer = chat::complete(&ep, messages, serde_json::json!({}), 120).await.unwrap();
    println!("answer in {:.1}s: {answer}", started.elapsed().as_secs_f64());
    let expect = std::env::var("SULCUSAI_VISION_EXPECT").unwrap_or_else(|_| "fox".into());
    assert!(answer.to_lowercase().contains(&expect), "expected “{expect}” in the answer");

    // Without its encoder the model is told it can't see, so it can say so.
    let mut blind = chat::api_messages("You are a helpful assistant.", &history);
    chat::attach_images(&mut blind, &history, None, false, &[]);
    assert!(blind[1]["content"].as_str().unwrap().contains("can't see pictures"));
    // With picture editing at hand, it's told it can still change them.
    let mut editing = chat::api_messages("You are a helpful assistant.", &history);
    chat::attach_images(&mut editing, &history, None, true, &[]);
    assert!(editing[1]["content"].as_str().unwrap().contains("edit_image"));
    // What the picture reader saw stands in for the picture.
    let mut read = chat::api_messages("You are a helpful assistant.", &history);
    let id = history[0].meta.as_ref().unwrap()["images"][0].as_str().unwrap().to_string();
    chat::attach_images(&mut read, &history, None, true, &[(id, "A red fox sitting in snow.".into())]);
    let said = read[1]["content"].as_str().unwrap();
    assert!(said.contains("A red fox sitting in snow.") && said.contains("picture reader") && said.contains("edit_image"), "{said}");
    eng.stop().await;
}

/// The assistant makes and edits a picture when asked in a chat, using the
/// create_image and edit_image tools (Qwen3 VL 4B as the chat model).
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_chat_makes_pictures() {
    let state = media_state();
    crate::media::install_for_test(&state, "flux2-klein-4b").await.unwrap();
    let cipher = state.cipher().unwrap();
    let budget = state.budget();
    let spec = state.catalog.model("qwen3-vl-4b").unwrap().clone();
    let exe = engine::installed_server(&state.paths, &state.catalog.engine, engine::backend_for(&budget).unwrap()).unwrap();
    let model = state.paths.models.join(&spec.id).join(&spec.variant("Q4_K_M").unwrap().file);
    let ep = state
        .engine
        .lock()
        .await
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: &spec.id, quant: "Q4_K_M", ctx: 16384, gpu_layers: 99, threads: 0, low_priority: false, log: &state.paths.engine_log(), mmproj: None, batch: 0, lora: None, steer: None }, None)
        .await
        .unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, Some(spec.id.clone())).unwrap();
    let made = |calls: &[&str]| {
        let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
        let used: Vec<String> = msgs.iter().filter_map(|m| m.tool_calls.as_ref()).flat_map(|c| c.iter().map(|c| c.name.clone())).collect();
        let media: usize = msgs.iter().filter_map(|m| m.meta.as_ref()).filter_map(|v| v["media"].as_array()).map(|a| a.len()).sum();
        for m in msgs.iter().filter(|m| m.role == "tool") {
            println!("  tool result: {}", m.content.chars().take(300).collect::<String>());
        }
        for m in msgs.iter().filter_map(|m| m.tool_calls.as_ref()).flatten() {
            println!("  call {}: {}", m.name, m.arguments);
        }
        println!("tools used: {used:?}, media made: {media}");
        (calls.iter().all(|c| used.iter().any(|u| u == c)), media)
    };
    let reply = say(&state, &ep, &cipher, &chat.id, "Make me a picture of a lighthouse on a rocky cliff at dusk.").await;
    println!("A: {reply}");
    let (ok, n) = made(&["create_image"]);
    assert!(ok && n >= 1, "the assistant made a picture");
    let reply = say(&state, &ep, &cipher, &chat.id, "Nice. Now change it so there's a storm with lightning.").await;
    println!("A: {reply}");
    let (ok, n) = made(&["create_image", "edit_image"]);
    assert!(ok && n >= 2, "the assistant edited the picture");
    let latest = crate::media::store::latest_in_chat(&state.db.lock().unwrap(), &cipher, &chat.id, "image").unwrap();
    assert_eq!(latest.op, "edit");
    assert!(latest.parent.is_some());
    state.engine.lock().await.stop().await;
}

/// Pictures from a chat: a realistic photo asked for in words, then a photo
/// the person attached, edited by instruction (the uploaded one, not a new
/// picture). SULCUSAI_TEST_GGUF: the chat model (one that sees pictures);
/// SULCUSAI_TEST_PHOTO: the photo to attach; SULCUSAI_MEDIA_OUT: results.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
#[ignore]
async fn e2e_chat_real_photo_and_uploaded_edit() {
    let state = media_state();
    for id in ["flux2-klein-4b", "z-image-turbo"] {
        crate::media::install_for_test(&state, id).await.unwrap();
    }
    let out = std::path::PathBuf::from(std::env::var("SULCUSAI_MEDIA_OUT").expect("SULCUSAI_MEDIA_OUT"));
    std::fs::create_dir_all(&out).unwrap();
    let cipher = state.cipher().unwrap();
    let model = std::path::PathBuf::from(std::env::var("SULCUSAI_TEST_GGUF").expect("SULCUSAI_TEST_GGUF"));
    let exe = engine::installed_server(&state.paths, &state.catalog.engine, engine::backend_for(&state.budget()).unwrap()).unwrap();
    let ep = state
        .engine
        .lock()
        .await
        .ensure(engine::LaunchSpec { exe: &exe, model_path: &model, model_id: "test-chat", quant: "Q4_K_M", ctx: 16384, gpu_layers: 99, threads: 0, low_priority: false, log: &state.paths.engine_log(), mmproj: None, batch: 0, lora: None, steer: None }, None)
        .await
        .unwrap();
    let chat = crate::db::create_chat(&state.db.lock().unwrap(), &cipher, None).unwrap();
    let report = |label: &str| {
        let msgs = crate::db::messages(&state.db.lock().unwrap(), &cipher, &chat.id);
        for c in msgs.iter().filter_map(|m| m.tool_calls.as_ref()).flatten() {
            println!("  {label} call {}: {}", c.name, c.arguments);
        }
        msgs
    };
    let keep = |item: &crate::media::MediaItem, name: &str| {
        std::fs::write(out.join(format!("{name}.png")), crate::media::store::read(&state.paths, &cipher, &item.id).unwrap()).unwrap();
    };

    let reply = say(&state, &ep, &cipher, &chat.id, "Make me a realistic photo of a man playing frisbee with his dog in a park.").await;
    println!("A: {reply}");
    report("1");
    let made = crate::media::store::latest_in_chat(&state.db.lock().unwrap(), &cipher, &chat.id, "image").expect("a picture was made");
    keep(&made, "chat-1-frisbee");

    // The person attaches a photo, as the paperclip button does.
    let bytes = std::fs::read(std::env::var("SULCUSAI_TEST_PHOTO").expect("SULCUSAI_TEST_PHOTO")).unwrap();
    let photo = crate::media::add(&state, &cipher, bytes, "image", "image/png", "attach", "photo", None, Some(chat.id.clone()), true).unwrap();
    crate::db::add_message(&state.db.lock().unwrap(), &cipher, &Message {
        id: uuid::Uuid::new_v4().to_string(),
        chat_id: chat.id.clone(),
        role: "user".into(),
        content: "Here's a photo of me. Add a black top hat on my head and keep everything else the same.".into(),
        created_at: crate::db::now_ms(),
        meta: Some(serde_json::json!({ "images": [photo.id] })),
        ..Default::default()
    })
    .unwrap();
    let turn = crate::agent::Turn {
        emit: std::sync::Arc::new(|_: &str, _| {}),
        state: state.clone(),
        chat_id: chat.id.clone(),
        turn_id: uuid::Uuid::new_v4().to_string(),
        cipher: cipher.clone(),
        ep: ep.clone(),
        mode: crate::tools::Mode::Auto,
        use_tools: true,
        vision: false,
        unattended: false,
        pre_approved: None,
        base: chat::system_prompt(&Profile::default(), &chrono::Local::now().format("%A, %B %-d, %Y").to_string()).0,
        about: String::new(),
        cancel: std::sync::Arc::new(AtomicBool::new(false)),
    };
    let reply = turn.run().await.unwrap().last.map(|m| m.content).unwrap_or_default();
    println!("A: {reply}");
    report("2");
    let edited = crate::media::store::latest_in_chat(&state.db.lock().unwrap(), &cipher, &chat.id, "image").unwrap();
    keep(&edited, "chat-2-uploaded-with-hat");
    state.engine.lock().await.stop().await;
    assert_eq!(edited.op, "edit", "the attached photo was edited");
    assert_eq!(edited.parent.as_deref(), Some(photo.id.as_str()), "from the uploaded photo");
}

/// Cloud pictures, edits, video and voices against a stand-in OpenAI server:
/// what's sent, what lands in the gallery, and the spend.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
#[ignore]
async fn e2e_cloud_media() {
    use axum::routing::{get, post};
    use base64::Engine;
    let png = {
        let img = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(8, 8, image::Rgba([200, 30, 30, 255])));
        crate::media::imaging::png(&img).unwrap()
    };
    let b64 = base64::engine::general_purpose::STANDARD.encode(&png);
    let seen: std::sync::Arc<std::sync::Mutex<Vec<String>>> = Default::default();
    let polls = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (s1, s2, s3, s4) = (seen.clone(), seen.clone(), seen.clone(), seen.clone());
    let (g, e) = (b64.clone(), b64.clone());
    let p = polls.clone();
    let wav = crate::audio::wav_encode(&vec![0.0; 24000], 24000);
    let app = axum::Router::new()
        .route("/v1/images/generations", post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            s1.lock().unwrap().push(format!("generate {body}"));
            let n = body["n"].as_u64().unwrap_or(1);
            let data: Vec<_> = (0..n).map(|_| serde_json::json!({ "b64_json": g })).collect();
            async move { axum::Json(serde_json::json!({ "data": data })) }
        }))
        .route("/v1/images/edits", post(move |body: axum::body::Bytes| {
            let text = String::from_utf8_lossy(&body).to_string();
            s2.lock().unwrap().push(format!("edit mask={} prompt={}", text.contains("name=\"mask\""), text.contains("Make it blue")));
            async move { axum::Json(serde_json::json!({ "data": [{ "b64_json": e }] })) }
        }))
        .route("/v1/videos", post(move |body: axum::body::Bytes| {
            let text = String::from_utf8_lossy(&body).to_string();
            s3.lock().unwrap().push(format!("video seconds8={}", text.contains("\r\n\r\n8\r\n")));
            async move { axum::Json(serde_json::json!({ "id": "vid_1", "status": "queued" })) }
        }))
        .route("/v1/videos/vid_1", get(move || {
            let n = p.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
            async move { axum::Json(if n == 0 { serde_json::json!({ "status": "in_progress", "progress": 50 }) } else { serde_json::json!({ "status": "completed" }) }) }
        }))
        .route("/v1/videos/vid_1/content", get(|| async { b"not really an mp4".to_vec() }))
        .route("/v1/audio/speech", post(move |axum::Json(body): axum::Json<serde_json::Value>| {
            s4.lock().unwrap().push(format!("speech {}", body["voice"]));
            let wav = wav.clone();
            async move { wav }
        }));
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move { axum::serve(listener, app).await.ok() });

    use crate::cloud::{CloudMedia, MediaKind};
    let media = vec![
        CloudMedia { id: "gpt-image-1".into(), name: "GPT Image".into(), kind: MediaKind::Image, price: Some(0.04) },
        CloudMedia { id: "sora-2".into(), name: "Sora 2".into(), kind: MediaKind::Video, price: Some(0.10) },
        CloudMedia { id: "gpt-4o-mini-tts".into(), name: "Voices".into(), kind: MediaKind::Speech, price: Some(12.0) },
    ];
    let state = cloud_state(crate::cloud::Provider { id: "openai".into(), name: "OpenAI".into(), kind: crate::cloud::Kind::Openai, base_url: format!("http://127.0.0.1:{port}/v1"), enabled: true, key: "sk-test".into(), models: Vec::new(), media, cookies: String::new(), share_pcs: false, share_people: vec![], proxy: String::new() });
    assert_eq!(crate::cloud::media_choices(&state.db.lock().unwrap()).len(), 3);
    assert!(crate::media::can_make(&state, crate::media::Kind::Image), "chats offer pictures through the cloud");

    let req = |op: &str, prompt: &str| crate::media::Request { op: op.into(), prompt: prompt.into(), ..Default::default() };
    // No picture model on this PC: the cloud one is used without being named.
    let mut r = req("generate", "A red square for jo@example.com");
    r.count = Some(2);
    let made = crate::media::run(&state, "j1".into(), r).await.unwrap();
    assert_eq!(made.len(), 2);
    assert_eq!(made[0].model_id.as_deref(), Some("cloud:openai:gpt-image-1"));
    assert_eq!((made[0].width, made[0].height), (8, 8));

    let mut r = req("fill", "Make it blue");
    r.model_id = Some("cloud:openai:gpt-image-1".into());
    r.source = Some(made[0].id.clone());
    r.mask = Some(b64.clone());
    let edited = crate::media::run(&state, "j2".into(), r).await.unwrap();
    assert_eq!(edited[0].parent.as_deref(), Some(made[0].id.as_str()));

    let mut r = req("video", "Waves");
    r.seconds = Some(8.0);
    let clip = crate::media::run(&state, "j3".into(), r).await.unwrap();
    assert_eq!(clip[0].mime, "video/mp4");
    assert_eq!(clip[0].seconds, 8.0);

    let mut r = req("narrate", "Hello there.");
    r.model_id = Some("cloud:openai:gpt-4o-mini-tts".into());
    r.voice = Some("nova".into());
    let spoken = crate::media::run(&state, "j4".into(), r).await.unwrap();
    assert!((spoken[0].seconds - 1.0).abs() < 0.01);

    let seen = seen.lock().unwrap().clone();
    eprintln!("{seen:#?}");
    assert!(seen[0].contains("[email 1]") && !seen[0].contains("jo@example.com"), "the email was swapped out");
    assert!(seen[0].contains("\"size\":\"1024x1024\""));
    assert!(seen.iter().any(|s| s == "edit mask=true prompt=true"));
    assert!(seen.iter().any(|s| s == "video seconds8=true"));
    assert!(seen.iter().any(|s| s == "speech \"nova\""));
    let spent = crate::cloud::spend(&state.db.lock().unwrap()).usd;
    // 2 pictures + 1 edit at $0.04, 8 s of video at $0.10, 12 characters at $12/M.
    assert!((spent - (3.0 * 0.04 + 0.8 + 12.0 * 12.0 / 1e6)).abs() < 1e-6, "{spent}");

    // Off the Cloud level, nothing goes out.
    crate::db::update_settings(&state.db.lock().unwrap(), |s| s.connectivity = crate::net::Connectivity::Offline).unwrap();
    let mut r = req("generate", "Another");
    r.model_id = Some("cloud:openai:gpt-image-1".into());
    assert!(crate::media::run(&state, "j5".into(), r).await.is_err());
}
