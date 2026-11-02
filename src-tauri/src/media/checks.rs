// SPDX-License-Identifier: AGPL-3.0-only
//! Pictures and edits checked against what was asked, made again when they
//! miss, and what the checks teach the prompt writer.

use super::*;

/// A new picture or clip with the prompt writer and the picture reader at
/// work: the description written for this model (with example pictures
/// read), and, when asked, the picture checked against what was asked for
/// and made once more with what was missing, keeping the better one.
pub(super) async fn made_well(state: &Arc<AppState>, id: &str, req: &Request, m: &MediaModel, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<MediaItem>, String> {
    let prepared = match prompts::prepare(state, req, m, progress).await {
        Ok(p) => p,
        Err(e) if e == download::CANCELLED => return Err(e),
        Err(e) => {
            crate::applog::line("media", &format!("The prompt writer couldn't help ({e}); using the description as written"));
            None
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    let Some(p) = prepared else { return run_kind(state, id, req, Some(m), cancel, progress).await };
    let mut first_req = req.clone();
    if let Some(w) = &p.written {
        first_req.asked = Some(req.prompt.clone());
        first_req.prompt = w.prompt.clone();
        first_req.negative = (!w.negative.is_empty()).then(|| w.negative.clone());
        // The writer already put the look in.
        first_req.style = Some("as_written".into());
    }
    // Checked: the first make fills part of the bar, and each one after it a share of the rest.
    let checking = p.check && req.op == "generate";
    let slice = |round: usize| -> (f64, f64) {
        let rest = 0.35 / (learn::MAX_MAKES - 1) as f64;
        match round {
            0 if checking => (0.0, 0.6),
            0 => (0.0, 1.0),
            r => (0.6 + rest * (r - 1) as f64, rest),
        }
    };
    let (from, len) = slice(0);
    let bar = |stage: &str, f: f64| progress(stage, from + len * f.clamp(0.0, 1.0));
    let mut items = run_kind(state, id, &first_req, Some(m), cancel, &bar).await?;
    // Checking: one picture, a checklist worth checking, and something to check with.
    if !checking || items.len() != 1 || items[0].kind != "image" || cancel.load(Ordering::Relaxed) {
        return Ok(items);
    }
    let c = state.work_cipher()?;
    // The same seed for every make, so each differs by its words only.
    let seed = items[0].seed;
    let mut item = items.remove(0);
    let mut written = p.written.clone().unwrap_or(prompts::Written { prompt: req.prompt.clone(), negative: req.negative.clone().unwrap_or_default() });
    let mut lesson: Option<String> = None;
    let (mut made, mut rounds, mut tries): (Vec<MediaItem>, Vec<prompts::Round>, Vec<learn::Try>) = (Vec::new(), Vec::new(), Vec::new());
    // What must show, as the reviewer reads the request (once, for every make).
    let mut checklist: Vec<String> = Vec::new();
    loop {
        // The reviewer: what shows, and for what doesn't, why.
        progress("Checking the picture against what you asked for", slice(made.len()).0 + slice(made.len()).1);
        let checked: Result<Option<(prompts::Verdict, Vec<String>)>, String> = async {
            let img = store::read(&state.paths, &c, &item.id).and_then(|b| imaging::decode(&b))?;
            let r = prompts::reader(state, Some(m)).await?;
            if checklist.is_empty() {
                checklist = prompts::checklist(&r, &req.prompt).await;
                crate::applog::line("media", &format!("The reviewer listed {} things to check", checklist.len()));
            }
            // Too little to check against.
            if checklist.len() < 3 {
                r.done(state).await;
                return Ok(None);
            }
            let v = prompts::judge(&r, &img, &checklist).await;
            let notes = match &v {
                Ok(v) if v.score < learn::GOOD => prompts::review(&r, &img, &req.prompt, &written.prompt, &v.missing).await,
                _ => Vec::new(),
            };
            r.done(state).await;
            v.map(|v| Some((v, notes)))
        }
        .await;
        if let Err(e) = &checked {
            crate::applog::line("media", &format!("Couldn't check picture {}: {e}", made.len() + 1));
        }
        let Ok(Some((verdict, notes))) = checked else {
            // Couldn't check it: the first stays as it is; a later one goes.
            if made.is_empty() {
                return Ok(vec![item]);
            }
            let _ = store::delete(&state.db.lock().unwrap(), &state.paths, &item.id);
            break;
        };
        crate::applog::line("media", &format!("Picture {} of at most {}: it showed {:.0}% of what was asked for", made.len() + 1, learn::MAX_MAKES, verdict.score * 100.0));
        tries.push(learn::Try { score: verdict.score, missing: verdict.missing.clone(), lesson: lesson.take(), from: None });
        rounds.push(prompts::Round { written: written.clone(), score: verdict.score, missing: verdict.missing.clone(), notes });
        made.push(item);
        let stop = |why: &str| crate::applog::line("media", &format!("Stopped writing again: {why}"));
        if learn::finished(&tries) || verdict.missing.is_empty() || cancel.load(Ordering::Relaxed) {
            if made.len() > 1 || verdict.score < learn::GOOD {
                stop(if verdict.score >= learn::GOOD || verdict.missing.is_empty() { "it shows what was asked" } else if made.len() >= learn::MAX_MAKES { "the most pictures for one request" } else if cancel.load(Ordering::Relaxed) { "stopped" } else { "two rewrites did no better" });
            }
            break;
        }
        // Model 1 writes again, seeing every try so far and the reviewer's notes.
        let (from, len) = slice(made.len());
        progress("Writing the description again from what was missing", from);
        let Ok(w) = prompts::writer(state, m).await else {
            stop("the writer didn't start");
            break;
        };
        let mut again = prompts::revise(&w, &p.program, &p.ask, &rounds, 0.4).await;
        // The same words again would make the same picture: asked once more, with more variety.
        if again.as_ref().is_ok_and(|(n, _)| rounds.iter().any(|r| r.written.prompt == n.prompt)) {
            again = prompts::revise(&w, &p.program, &p.ask, &rounds, 0.9).await;
        }
        w.done(state).await;
        let (next, said) = match again {
            Ok(x) => x,
            Err(e) => {
                stop(&format!("the writer failed ({e})"));
                break;
            }
        };
        if rounds.iter().any(|r| r.written.prompt == next.prompt) {
            stop("the writer gave back a prompt it had tried, twice");
            break;
        }
        let mut again_req = first_req.clone();
        again_req.asked = Some(req.prompt.clone());
        again_req.prompt = next.prompt.clone();
        again_req.negative = (!next.negative.is_empty()).then(|| next.negative.clone());
        again_req.style = Some("as_written".into());
        again_req.seed = seed;
        again_req.count = Some(1);
        let bar = |stage: &str, f: f64| progress(stage, from + len * f.clamp(0.0, 1.0));
        match run_kind(state, &format!("{id}-{}", made.len()), &again_req, Some(m), cancel, &bar).await {
            Ok(mut s) if s.len() == 1 => item = s.remove(0),
            Ok(s) => {
                for extra in s {
                    let _ = store::delete(&state.db.lock().unwrap(), &state.paths, &extra.id);
                }
                stop("the picture model made more than one");
                break;
            }
            Err(e) => {
                stop(&format!("the picture model failed ({e})"));
                break;
            }
        }
        written = next;
        lesson = said;
    }
    // The best is kept (the first of equals); the others go.
    let best = (0..tries.len()).fold(0, |b, i| if tries[i].score > tries[b].score { i } else { b });
    if made.len() > 1 {
        crate::applog::line("media", &format!("Kept picture {} of {}: {:.0}% of what was asked for (the first {:.0}%)", best + 1, made.len(), tries[best].score * 100.0, tries[0].score * 100.0));
    }
    let mut keep = Vec::new();
    for (i, it) in made.into_iter().enumerate() {
        if i == best {
            keep.push(it);
        } else {
            let _ = store::delete(&state.db.lock().unwrap(), &state.paths, &it.id);
        }
    }
    if learn::may_learn(state, req.chat_id.as_deref()) {
        learn_from_picture(state, m, req, &p, &rounds, &tries, best);
    }
    Ok(if tries[best].score < learn::GOOD { said_missing(state, keep, &tries[best].missing) } else { keep })
}

/// What the writer keeps from a checked picture: how its first try did, the
/// rules that made missing things show, and the prompt that passed.
pub(super) fn learn_from_picture(state: &AppState, m: &MediaModel, req: &Request, p: &prompts::Prepared, rounds: &[prompts::Round], tries: &[learn::Try], best: usize) {
    let mut l = learn::load(state, m);
    let now = db::now_ms();
    learn::add_first(&mut l.firsts, tries[0].score);
    let rules = learn::confirmed(tries);
    for r in &rules {
        learn::add_rule(&mut l.rules, r, now);
    }
    // A prompt the writer wrote (not the person's words as given) that passed.
    let wrote = p.written.is_some() || best > 0;
    let passed = tries[best].score >= learn::GOOD && wrote;
    if passed {
        let w = &rounds[best].written;
        let answer = json!({ "prompt": w.prompt, "negative": w.negative }).to_string();
        learn::add_example(&mut l.pictures, learn::Example { request: req.prompt.clone(), ask: p.ask.render(), answer, score: tries[best].score, makes: best as u32 + 1, at: now });
    }
    match learn::save(state, m, &l) {
        Ok(()) => crate::applog::line("media", &format!("The prompt writer learnt from this picture: {}{} ({} examples and {} rules for {} so far)", if passed { "an example" } else { "its first try's score" }, if rules.is_empty() { String::new() } else { format!(", {} rule(s)", rules.len()) }, l.pictures.len(), l.rules.len(), m.name)),
        Err(e) => crate::applog::line("media", &format!("Couldn't keep what the prompt writer learnt: {e}")),
    }
}

/// An edit with the picture reader at work. Asked to have a child lift the
/// barbell in front of her overhead, the edit model drew a second barbell
/// above her and left the first on the floor, with plates missing or of
/// another kind, and the result came back smaller than the photo. So:
/// - the reader looks at the picture and plans the request as steps the
///   edit model makes well (a move as "take it away", then "put it there,
///   looking like this"), with a checklist that includes what must be gone;
/// - the steps run one after another, each on the last one's result;
/// - the reader checks the result against the picture before, and when it
///   falls short the steps are planned again from what was wrong and made
///   once more, keeping the better of the two;
/// - the result goes back into the full-size original, so whatever the edit
///   didn't change keeps its own pixels.
pub(super) async fn edited_well(state: &Arc<AppState>, id: &str, req: &Request, m: &MediaModel, cancel: &AtomicBool, progress: &(dyn Fn(&str, f64) + Sync)) -> Result<Vec<MediaItem>, String> {
    let reserved = chat_model_vram(state).await;
    let ctx = RunCtx { state, req, cancel, progress, reserved };
    let work = work_dir(&state.paths, id)?;
    let (item, full) = ctx.source()?;
    let d = &m.defaults;
    let hw = state.hardware.read().unwrap().clone();
    let on_gpu = fit(m, &hw, &state.budget(), 0, None).on_gpu;
    let side = if on_gpu { d.width.max(d.height).max(512) } else { 640 };
    let (w, h) = imaging::work_size(full.width(), full.height(), side * side);
    let (w, h) = if d.align > 16 { ((w / d.align).max(1) * d.align, (h / d.align).max(1) * d.align) } else { (w, h) };
    let before = full.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
    // The first picture of an edit chain keeps faces from drifting (see picture()).
    let original = if changes_how_people_look(&req.prompt) { None } else { ctx.original_of(&item)? };
    let stopped = || cancel.load(Ordering::Relaxed);

    progress("Looking at the picture and planning the change", 0.0);
    // What the planner has learnt about this edit model: rules, and edits like this one.
    let help = learn::planner_help(&learn::load(state, m), &req.prompt);
    let coached = coach::trained(state, m).is_some();
    // One picture reader for the whole edit when the card holds it beside
    // the picture model: it was started and stopped three to five times.
    let mut kept: Option<crate::vision::Helper> = None;
    let (plan, rubric) = match prompts::reader(state, Some(m)).await {
        Ok(r) => {
            let p = prompts::plan_edit_with(&r, &before, &req.prompt, &help).await;
            // A writer coached for this model plans from a description of
            // the picture, the way it practised.
            let caption = if coached { prompts::caption(&r, &before).await.ok() } else { None };
            // (Not kept when the coached writer plans next: one helper at a time.)
            if !coached && reader_fits(state, &r, m).await {
                kept = Some(r);
            } else {
                r.done(state).await;
            }
            let p = p.map_err(|e| crate::applog::line("media", &format!("Couldn't plan the edit ({e}); using the instruction as written"))).ok();
            let mine = match caption {
                Some(c) => match prompts::writer(state, m).await {
                    Ok(w) => {
                        let x = prompts::plan_from_words(&w, &c, &req.prompt, 0.3).await.ok();
                        w.done(state).await;
                        x
                    }
                    Err(_) => None,
                },
                None => None,
            };
            if mine.is_some() {
                crate::applog::line("media", "The coached prompt writer planned the edit");
            }
            // The reader's plan says what to check, either way.
            (mine.or_else(|| p.clone()), p)
        }
        Err(e) => {
            crate::applog::line("media", &format!("Nothing could look at the picture ({e}); using the instruction as written"));
            (None, None)
        }
    };
    let plan = plan.unwrap_or_else(|| prompts::EditPlan { steps: vec![req.prompt.trim().to_string()], ..Default::default() });
    let check = rubric.unwrap_or_else(|| plan.clone());
    crate::applog::line("media", &format!("Edit planned as {} step(s) with {} things to check", plan.steps.len(), plan.checklist.len()));
    if stopped() {
        return Err(download::CANCELLED.into());
    }
    let seed = req.seed.unwrap_or_else(random_seed);
    // Something taken away or moved: a change in one place, so the rest stays put.
    let local = plan.changes["leaving"].as_array().is_some_and(|l| !l.is_empty());
    // The picture model leaves room for a reader kept on the card.
    let room = |k: &Option<crate::vision::Helper>| RunCtx { state, req, cancel, progress, reserved: reserved + k.as_ref().map_or(0, |r| r.card_bytes) };
    // Made, checked by the reader and planned again from what was missing,
    // up to `learn::MAX_MAKES` times; the best is kept.
    let checks = check.checklist.len() >= 2;
    let (mut plan_now, mut seed_now, mut lesson) = (plan.clone(), seed, None::<String>);
    let mut tries: Vec<learn::Try> = Vec::new();
    // The best so far: its points, the picture, whether it redrew the whole
    // scene, its verdict, its plan and which make it was.
    let mut best: Option<(f32, image::DynamicImage, bool, Option<prompts::Verdict>, prompts::EditPlan, usize)> = None;
    // The best one as the edit model made it, and which try it was.
    let mut best_made: Option<(image::DynamicImage, Option<usize>)> = None;
    // The try the next plan was made from.
    let mut from: Option<usize> = None;
    let rest = 0.45 / (learn::MAX_MAKES - 1) as f64;
    for round in 0..learn::MAX_MAKES {
        let slice = if round == 0 { (0.1, 0.45) } else { (0.55 + rest * (round - 1) as f64, rest * 0.7) };
        if round > 0 {
            progress("Making it again so it shows everything you asked for", slice.0);
        }
        let made = match edit_steps(&room(&kept), m, &before, original.as_ref(), &plan_now.steps, seed_now, &work.0.join(round.to_string()), slice).await {
            Ok(x) => x,
            Err(e) if round == 0 => return Err(e),
            Err(_) => break,
        };
        // Back into the full-size original, when it fits (None: the edit model redrew the whole scene).
        let made_kept = imaging::keep_unchanged(&full, &before, &made);
        let whole = local && made_kept.is_none();
        if whole {
            crate::applog::line("media", "The edit redrew the whole scene, so it doesn't fit back into the original");
        }
        let (mut verdict, mut next, mut reader) = (None, None, None);
        if checks && !stopped() {
            progress("Checking the change against what you asked for", slice.0 + slice.1);
            let r = match kept.take() {
                Some(r) => Ok(r),
                None => prompts::reader(state, Some(m)).await,
            };
            if let Ok(r) = r {
                if let Ok(v) = prompts::judge_edit(&r, &before, &made, &check).await {
                    crate::applog::line("media", &format!("Edit {} of at most {}: it did {:.0}% of what was asked for", round + 1, learn::MAX_MAKES, v.score * 100.0));
                    tries.push(learn::Try { score: v.score, missing: v.missing.clone(), lesson: lesson.take(), from: from.take() });
                    verdict = Some(v);
                }
                reader = Some(r);
            }
        }
        // The better one, with a little extra for one that fits back into the original.
        let points = verdict.as_ref().map_or(0.0, |v| v.score) + if local && made_kept.is_some() { 0.1 } else { 0.0 };
        if round > 0 {
            crate::applog::line("media", &format!("Made again: {:.0} points against {:.0} for the best before", points * 100.0, best.as_ref().map_or(0.0, |b| b.0) * 100.0));
        }
        if best.as_ref().is_none_or(|b| points > b.0) {
            best = Some((points, made_kept.clone().unwrap_or_else(|| made.clone()), made_kept.is_none(), verdict.clone(), plan_now.clone(), round));
            best_made = Some((made.clone(), tries.len().checked_sub(1)));
        }
        // Planned again from the best so far (its picture and what it
        // missed), not the last: after a worse try, that wandered off.
        if let Some(r) = reader {
            if let (Some((_, _, _, Some(bv), bplan, _)), Some((bmade, bi))) = (best.as_ref(), best_made.as_ref()) {
                if bv.score < learn::GOOD && !bv.missing.is_empty() && !learn::finished(&tries) && !stopped() {
                    next = prompts::replan_edit(&r, &before, bmade, &req.prompt, bplan, &bv.missing, &help).await.ok();
                    from = *bi;
                }
            }
            if reader_fits(state, &r, m).await {
                kept = Some(r);
            } else {
                r.done(state).await;
            }
        }
        if stopped() {
            break;
        }
        // Next: a new plan for what was missing, with the same seed so it
        // differs by its plan; or, when the first redrew the whole scene for
        // a change in one place, the same steps with a new seed.
        match next {
            Some((p, said)) => {
                plan_now = p;
                lesson = said;
                if whole {
                    seed_now = random_seed();
                }
            }
            None if round == 0 && whole => seed_now = random_seed(),
            None => break,
        }
    }
    let (_, mut result, redrawn, v1, best_plan, best_round) = best.ok_or("The edit didn't come out.")?;
    if learn::may_learn(state, req.chat_id.as_deref()) && !tries.is_empty() {
        learn_from_edit(state, m, req, &tries, v1.as_ref(), &best_plan, best_round, kept.as_ref(), &before).await;
    }
    if let Some(r) = kept.take() {
        r.done(state).await;
    }
    if stopped() {
        return Err(download::CANCELLED.into());
    }
    // The whole scene redrawn: back to the photo's size (with the upscaler
    // when there is one), not left at the edit model's working size.
    let mut note = None;
    if redrawn && result.dimensions() != full.dimensions() {
        let big = ctx.enlarge_for(&result, full.width(), full.height(), &work.0).await;
        result = big.resize_exact(full.width(), full.height(), image::imageops::FilterType::Lanczos3);
        note = Some("The edit redrew the whole scene, so everything in it was drawn again (at the edit model's detail) rather than kept from the photo.".to_string());
    }
    let kept = ctx.keep_picture(&result, Some(m), Some(&item.id), Some(seed))?;
    let missing = v1.filter(|v| v.score < 0.85).map(|v| v.missing).unwrap_or_default();
    let mut items = said_missing(state, vec![kept], &missing);
    if let Some(n) = note {
        if let Some(i) = items.first_mut() {
            i.note = Some(n);
            if let Ok(c) = state.work_cipher() {
                let _ = store::update(&state.db.lock().unwrap(), &c, i);
            }
        }
    }
    Ok(items)
}

/// What the planner keeps from a checked edit: how its first plan did, the
/// rules that made missing things show, and the plan that passed (with the
/// picture in words: the reader's, when it's still on the card).
#[allow(clippy::too_many_arguments)]
pub(super) async fn learn_from_edit(state: &AppState, m: &MediaModel, req: &Request, tries: &[learn::Try], best: Option<&prompts::Verdict>, plan: &prompts::EditPlan, round: usize, reader: Option<&crate::vision::Helper>, before: &image::DynamicImage) {
    let mut l = learn::load(state, m);
    let now = db::now_ms();
    learn::add_first(&mut l.edit_firsts, tries[0].score);
    let rules = learn::confirmed(tries);
    for r in &rules {
        learn::add_rule(&mut l.edit_rules, r, now);
    }
    let passed = best.is_some_and(|v| v.score >= learn::GOOD) && !plan.changes.is_null();
    if passed {
        let words = match reader {
            Some(r) => prompts::caption(r, before).await.ok(),
            None => None,
        };
        let scene = words.unwrap_or_else(|| learn::scene_of(&plan.changes));
        let ask = format!("The request: {} (The picture: {})", req.prompt.trim().trim_end_matches('.'), scene);
        learn::add_example(&mut l.edits, learn::Example { request: req.prompt.clone(), ask, answer: plan.changes.to_string(), score: best.map_or(0.0, |v| v.score), makes: round as u32 + 1, at: now });
    }
    match learn::save(state, m, &l) {
        Ok(()) => crate::applog::line("media", &format!("The edit planner learnt from this edit: {}{} ({} examples and {} rules for {} so far)", if passed { "an example" } else { "its first plan's score" }, if rules.is_empty() { String::new() } else { format!(", {} rule(s)", rules.len()) }, l.edits.len(), l.edit_rules.len(), m.name)),
        Err(e) => crate::applog::line("media", &format!("Couldn't keep what the edit planner learnt: {e}")),
    }
}

/// A picture reader on its own engine can stay on the card through an
/// edit when the card holds it, the chat model's share and the picture
/// model (with a gigabyte to spare).
pub(super) async fn reader_fits(state: &Arc<AppState>, r: &crate::vision::Helper, m: &MediaModel) -> bool {
    let vram = state.budget().vram_card;
    let chat = chat_model_vram(state).await;
    let picture = (m.min_vram_gb * GIB as f64) as u64;
    r.card_bytes > 0 && vram >= chat + r.card_bytes + picture + GIB
}

/// The kept picture says what the check didn't find in it, so the person
/// hears it (the Studio card, the chat) instead of it only reaching the log.
pub(super) fn said_missing(state: &AppState, mut items: Vec<MediaItem>, missing: &[String]) -> Vec<MediaItem> {
    if missing.is_empty() || items.len() != 1 {
        return items;
    }
    items[0].missing = missing.to_vec();
    if let Ok(c) = state.work_cipher() {
        let _ = store::update(&state.db.lock().unwrap(), &c, &items[0]);
    }
    items
}

/// Runs an edit's steps in order, each on the last one's result, at the
/// size of `before`. Nothing is kept in the gallery.
/// `slice`: the part of the job's bar (start, length) these steps fill.
pub(super) async fn edit_steps(ctx: &RunCtx<'_>, m: &MediaModel, before: &image::DynamicImage, original: Option<&image::DynamicImage>, steps: &[String], seed: i64, dir: &std::path::Path, slice: (f64, f64)) -> Result<image::DynamicImage, String> {
    let d = &m.defaults;
    let (w, h) = (before.width(), before.height());
    let mut img = before.clone();
    let n = steps.len().max(1) as f64;
    for (i, step) in steps.iter().enumerate() {
        if ctx.cancel.load(Ordering::Relaxed) {
            return Err(download::CANCELLED.into());
        }
        let part = |label: &str, f: f64| (ctx.progress)(label, slice.0 + slice.1 * (i as f64 + f.clamp(0.0, 1.0)) / n);
        let ctx = &RunCtx { state: ctx.state, req: ctx.req, cancel: ctx.cancel, progress: &part, reserved: ctx.reserved };
        let dir = dir.join(i.to_string());
        std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
        let src = dir.join("source.png");
        std::fs::write(&src, imaging::png(&img)?).map_err(|e| e.to_string())?;
        let mut refs = vec![src];
        let mut prompt = step.trim().to_string();
        if let Some(o) = original {
            let first = dir.join("original.png");
            std::fs::write(&first, imaging::png(&o.resize(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
            refs.push(first);
            prompt = format!(
                "{}. Work on the first picture and keep everything in it that isn't mentioned. The second picture is the original: keep every person's face and features exactly as there.",
                prompt.trim_end_matches('.')
            );
        }
        let negative = (!d.negative.is_empty()).then(|| fit_negative(&d.negative, None, &prompt)).filter(|n| !n.is_empty());
        let out = dir.join("out_%d.png");
        let args = sd::picture_args(
            &ctx.files(m),
            d,
            &sd::Picture {
                prompt: &prompt,
                negative: negative.as_deref(),
                width: w,
                height: h,
                steps: ctx.req.steps.unwrap_or(d.steps).clamp(1, 60),
                seed: seed + i as i64,
                count: 1,
                init: None,
                mask: None,
                refs: refs.iter().map(|p| p.as_path()).collect(),
                strength: None,
                out: &out,
            },
            &ctx.limits(),
        );
        ctx.run_sd(args, &dir).await?;
        let made = std::fs::read_dir(&dir).map_err(|e| e.to_string())?.flatten().map(|e| e.path()).find(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("out_")));
        let next = imaging::decode(&std::fs::read(made.ok_or("The picture engine finished without making a picture.")?).map_err(|e| e.to_string())?)?;
        // A run that fell back to a smaller size: back to this size for the next step.
        img = if next.dimensions() == (w, h) { next } else { next.resize_exact(w, h, image::imageops::FilterType::Lanczos3) };
        // Each step's picture, to look at while testing (SULCUSAI_EDIT_STEPS: a folder).
        if let Some(out) = std::env::var_os("SULCUSAI_EDIT_STEPS") {
            let attempt = dir.parent().and_then(|p| p.file_name()).map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
            let _ = std::fs::write(std::path::Path::new(&out).join(format!("{attempt}-{}.png", i + 1)), imaging::png(&img).unwrap_or_default());
        }
    }
    Ok(img)
}
