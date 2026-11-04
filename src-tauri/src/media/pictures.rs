// SPDX-License-Identifier: AGPL-3.0-only
//! Pictures made, made bigger, and cut out of their backgrounds.

use super::*;

/// Picture jobs: make, edit, fill in, extend, restyle.
pub(super) async fn picture(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let req = ctx.req;
    let d = &m.defaults;
    let hw = ctx.state.hardware.read().unwrap().clone();
    let on_gpu = fit(m, &hw, &ctx.state.budget(), 0, None).on_gpu;
    // On the processor, smaller pictures keep the wait reasonable.
    let side = if on_gpu { d.width.max(d.height).max(512) } else { 640 };
    let seed = req.seed.unwrap_or_else(random_seed);
    let steps = req.steps.unwrap_or(d.steps).clamp(1, 60);
    let out = work.join("out_%d.png");
    // Some models want sides in steps of 32.
    let al = |(w, h): (u32, u32)| if d.align > 16 { ((w / d.align).max(1) * d.align, (h / d.align).max(1) * d.align) } else { (w, h) };
    let mut source: Option<(MediaItem, image::DynamicImage)> = None;
    let mut fill_mask: Option<image::GrayImage> = None;
    // An edit: the picture as the model got it, to find what it changed.
    let mut edit_before: Option<image::DynamicImage> = None;
    let mut count = 1;
    let (prompt, w, h, init, mask, refs) = match req.op.as_str() {
        "generate" => {
            count = if req.wallpaper.is_some() { 1 } else { req.count.unwrap_or(1).clamp(1, 4) };
            let (w, h) = al(match (&req.wallpaper, req.width, req.height) {
                (Some(spec), _, _) => wallpaper::make_size(spec, side, d.align),
                (None, Some(w), Some(h)) => (imaging::round16(w.clamp(256, 2048) as f64), imaging::round16(h.clamp(256, 2048) as f64)),
                _ => imaging::shape_size(req.shape.as_deref().unwrap_or("square"), side),
            });
            // Example pictures: references for models that read them (image 1,
            // image 2…); for the layout (or with nothing to read them), the
            // picture starts from the first one and is redrawn partway.
            let examples = ctx.examples()?;
            let (mut init, mut refs) = (None, vec![]);
            // For a style or a subject, a model that can't take them gets the
            // picture reader's words only: starting from the example copied its
            // room, layout and even its sign into a picture asked to share only
            // its look. Without a reader, the layout is all that can be used.
            let layout = req.example_use.as_deref() == Some("layout");
            let words_only = !layout && (crate::vision::can_read(ctx.state, None) || installed_file(ctx.state, m, "llm_vision").is_some());
            if !examples.is_empty() && !(words_only && !m.can("edit")) {
                if m.can("edit") && !layout {
                    for (i, img) in examples.iter().enumerate() {
                        let (ew, eh) = imaging::work_size(img.width(), img.height(), w * h);
                        let p = work.join(format!("example{i}.png"));
                        std::fs::write(&p, imaging::png(&img.resize_exact(ew, eh, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
                        refs.push(p);
                    }
                } else {
                    let p = work.join("start.png");
                    std::fs::write(&p, imaging::png(&imaging::fill_to(&examples[0], w, h))?).map_err(|e| e.to_string())?;
                    init = Some(p);
                }
            }
            // A pose from a picture: given to a model that reads pictures as
            // the last image, with only its pose to be taken.
            let mut pose_at = None;
            if let Some(pid) = req.pose.as_deref().filter(|p| !p.is_empty()) {
                if !m.can("edit") {
                    return Err("Taking a pose from a picture needs a picture model that reads pictures: FLUX.2 [klein] or Qwen-Image (Studio › Models).".into());
                }
                let c = ctx.state.work_cipher()?;
                let img = store::read(&ctx.state.paths, &c, pid).and_then(|b| imaging::decode(&b)).map_err(|_| "The pose picture is no longer in the gallery.".to_string())?;
                // Its pose only: given as it was, the model copied the face too.
                let img = pose_only(ctx, img).await;
                let (pw, ph) = imaging::work_size(img.width(), img.height(), w * h);
                let p = work.join("pose.png");
                std::fs::write(&p, imaging::png(&img.resize_exact(pw, ph, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
                init = None;
                refs.push(p);
                pose_at = Some(refs.len());
            }
            // With their own LoRAs and the look left to Auto, the LoRA sets the
            // look: Auto's photo words turned a pixel-art LoRA back into a photo.
            let look = if !req.loras.is_empty() && matches!(req.style.as_deref(), None | Some("auto")) { Some("as_written") } else { req.style.as_deref() };
            let mut prompt = styled(&req.prompt, look);
            // The same character as the example: said plainly to a model that
            // gets it as a reference (the prompt writer says it its own way).
            let examples_given = pose_at.map_or(refs.len(), |k| k - 1);
            if req.example_use.as_deref() == Some("subject") && examples_given > 0 && req.write_prompt != Some(true) {
                let which = if examples_given == 1 { "image 1".to_string() } else { format!("images 1 to {examples_given}") };
                prompt = format!("{}. The same character as in {which}: keep the face, hair, body, clothes and colors exactly the same.", prompt.trim_end_matches('.'));
            }
            if let Some(k) = pose_at {
                prompt = format!("{}. The body pose exactly as the figure in image {k}: the same position of the head, arms and legs. Image {k} only shows the pose.", prompt.trim_end_matches('.'));
            }
            if let Some(spec) = req.wallpaper.as_ref().filter(|_| req.style.as_deref() != Some("as_written")) {
                prompt.push_str(wallpaper::plain_words(spec));
            }
            (prompt, w, h, init, None, refs)
        }
        "edit" | "restyle" => {
            let s = ctx.source()?;
            let (w, h) = al(imaging::work_size(s.1.width(), s.1.height(), side * side));
            let src = work.join("source.png");
            let small = s.1.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
            std::fs::write(&src, imaging::png(&small)?).map_err(|e| e.to_string())?;
            if req.op == "edit" {
                edit_before = Some(small);
            }
            let mut prompt = if req.op == "restyle" {
                format!("Redraw this picture in this style: {}. Keep the same subject, layout and composition.", req.prompt.trim())
            } else {
                req.prompt.trim().to_string()
            };
            let mut refs = vec![src];
            // An edit of an edit: the first picture goes along too, so faces and
            // details don't drift a little further with every edit in a chain
            // (a few edits in, pictures had lost what was asked for).
            // Not when the change is to how people look (older, a new hairdo,
            // a cartoon of them): the original's faces would undo it.
            let changes_looks = req.op == "restyle" || changes_how_people_look(&req.prompt);
            if let Some(original) = if changes_looks { None } else { ctx.original_of(&s.0)? } {
                let first = work.join("original.png");
                std::fs::write(&first, imaging::png(&original.resize(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
                refs.push(first);
                prompt = format!(
                    "{}. Work on the first picture and keep everything in it that isn't mentioned. The second picture is the original: keep every person's face and features exactly as there.",
                    prompt.trim_end_matches('.')
                );
            }
            source = Some(s);
            (prompt, w, h, None, None, refs)
        }
        "fill" => {
            let s = ctx.source()?;
            let (w, h) = al(imaging::work_size(s.1.width(), s.1.height(), side * side));
            let bytes = base64::engine::general_purpose::STANDARD.decode(req.mask.as_deref().unwrap_or("")).map_err(|_| "The painted area couldn't be read.".to_string())?;
            let mk = imaging::grow(&imaging::mask_from_paint(&bytes, w, h)?, 4);
            let (src, mpath) = (work.join("source.png"), work.join("mask.png"));
            std::fs::write(&src, imaging::png(&s.1.resize_exact(w, h, image::imageops::FilterType::Lanczos3))?).map_err(|e| e.to_string())?;
            std::fs::write(&mpath, imaging::png(&image::DynamicImage::ImageLuma8(mk.clone()))?).map_err(|e| e.to_string())?;
            let prompt = if req.prompt.trim().is_empty() { "Fill in this area naturally so it matches its surroundings.".to_string() } else { req.prompt.trim().to_string() };
            fill_mask = Some(mk);
            source = Some(s);
            (prompt, w, h, Some(src), Some(mpath), vec![])
        }
        "extend" => {
            let s = ctx.source()?;
            let [l, t, r, b] = req.extend.unwrap_or_else(|| {
                // Default: wider, a third on each side.
                let add = s.1.width() / 3;
                [add, 0, add, 0]
            });
            if l + t + r + b == 0 {
                return Err("Choose which sides to extend.".into());
            }
            let (full, full_mask) = imaging::extend(&s.1, l, t, r, b);
            let (w, h) = al(imaging::work_size(full.width(), full.height(), side * side));
            let full = image::DynamicImage::ImageRgba8(full);
            let canvas = full.resize_exact(w, h, image::imageops::FilterType::Lanczos3);
            let mk = image::imageops::resize(&full_mask, w, h, image::imageops::FilterType::Nearest);
            let (src, mpath) = (work.join("source.png"), work.join("mask.png"));
            std::fs::write(&src, imaging::png(&canvas)?).map_err(|e| e.to_string())?;
            std::fs::write(&mpath, imaging::png(&image::DynamicImage::ImageLuma8(mk))?).map_err(|e| e.to_string())?;
            let prompt = if req.prompt.trim().is_empty() {
                "Continue the picture naturally beyond its edges, with the same style, lighting and detail.".to_string()
            } else {
                req.prompt.trim().to_string()
            };
            // The new edges go around the original at full size (it came
            // back smaller than the picture it started from).
            fill_mask = Some(full_mask);
            source = Some((s.0, full));
            (prompt, w, h, Some(src), Some(mpath), vec![])
        }
        _ => return Err("Unknown kind of picture job.".into()),
    };
    let negative = req
        .negative
        .as_deref()
        .filter(|n| !n.trim().is_empty())
        .or((!d.negative.is_empty()).then_some(d.negative.as_str()))
        .map(|n| fit_negative(n, req.style.as_deref(), &prompt))
        // A wallpaper is the scene alone, never a picture of a desktop.
        .map(|n| if req.wallpaper.is_some() { format!("{n}, user interface, taskbar, desktop icons, window frame, text") } else { n })
        // With their own LoRAs, the model's usual "no drawings" words go: they
        // fought a pixel-art LoRA into a photo.
        .map(|n| if req.loras.is_empty() { n } else { without_style_words(&n) })
        .filter(|n| !n.is_empty());
    // Their own LoRAs: named in the prompt, loaded from their folder.
    let loras = loras::for_job(&ctx.state.db.lock().unwrap(), &ctx.state.paths, &m.id, &req.loras);
    let prompt = match &loras {
        Some((words, _)) => format!("{prompt}, {words}"),
        None => prompt,
    };
    let mut args = sd::picture_args(
        &ctx.files(m),
        d,
        &sd::Picture {
            prompt: &prompt,
            negative: negative.as_deref(),
            width: w,
            height: h,
            steps,
            seed,
            count,
            init: init.as_deref(),
            mask: mask.as_deref(),
            refs: refs.iter().map(|p| p.as_path()).collect(),
            // Filling in redraws the masked area whole; starting from an example keeps its layout and colors.
            strength: match (&init, &mask) {
                (Some(_), Some(_)) => Some(1.0),
                (Some(_), None) => Some(0.62),
                _ => None,
            },
            out: &out,
        },
        &ctx.limits(),
    );
    if let Some((_, folder)) = &loras {
        args.extend(["--lora-model-dir".into(), folder.clone().into_os_string()]);
    }
    if let Some(extra) = &req.picture_extra {
        super::create::run::picture_args(ctx, extra, w, h, &mut args, work)?;
    }
    ctx.run_sd(args, work).await?;
    let mut made: Vec<PathBuf> = std::fs::read_dir(work)
        .map_err(|e| e.to_string())?
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.file_name().is_some_and(|n| n.to_string_lossy().starts_with("out_")))
        .collect();
    made.sort();
    if made.is_empty() {
        return Err("The picture engine finished without making a picture.".into());
    }
    let mut items = Vec::new();
    for (i, p) in made.iter().enumerate() {
        let mut img = imaging::decode(&std::fs::read(p).map_err(|e| e.to_string())?)?;
        // Filling in: put the new area into the full-size original.
        if let (Some(mk), Some((_, orig))) = (&fill_mask, &source) {
            img = imaging::composite(orig, &img, mk);
        } else if let (Some(b), Some((_, orig))) = (&edit_before, &source) {
            // An edit: what it didn't change stays the original's, at its size.
            if let Some(kept) = imaging::keep_unchanged(orig, b, &img) {
                img = kept;
            }
        }
        if let Some(spec) = req.wallpaper.as_ref().filter(|_| req.op == "generate") {
            let big = ctx.enlarge_for(&img, spec.width, spec.height, work).await;
            let done = wallpaper::finish(&big, spec);
            if spec.oled {
                crate::applog::line("media", &format!("OLED wallpaper: {:.0}% of its pixels are off", wallpaper::black_share(&done.to_rgba8()) * 100.0));
            }
            items.push(ctx.keep_as("wallpaper", &done, Some(m), None, Some(seed))?);
            continue;
        }
        items.push(ctx.keep_picture(&img, Some(m), source.as_ref().map(|s| s.0.id.as_str()), Some(seed + i as i64))?);
    }
    Ok(items)
}

pub(super) async fn upscale(ctx: &RunCtx<'_>, m: &MediaModel, work: &std::path::Path) -> Result<Vec<MediaItem>, String> {
    let (item, img) = ctx.source()?;
    // Four times a picture up to 2048 pixels, kept within 4096 (shrinking a
    // big photo to 1024 first gave back no more detail than it had).
    let long = img.width().max(img.height());
    if long > 2048 {
        return Err(format!("This picture is already large ({} × {}). Upscaling is for pictures up to 2048 pixels across.", img.width(), img.height()));
    }
    let (src, out) = (work.join("source.png"), work.join("out.png"));
    std::fs::write(&src, imaging::png(&img)?).map_err(|e| e.to_string())?;
    let args = sd::upscale_args(&ctx.files(m), &src, &out, &ctx.limits());
    ctx.run_sd(args, work).await?;
    let big = imaging::cap(imaging::decode(&std::fs::read(&out).map_err(|_| "The upscaler didn't produce a picture.".to_string())?)?, 4096);
    // A cut-out stays one: its transparency, enlarged to match.
    let big = imaging::keep_alpha(&img, big);
    Ok(vec![ctx.keep_picture(&big, Some(m), Some(&item.id), None)?])
}

pub(super) async fn remove_background(ctx: &RunCtx<'_>, m: &MediaModel) -> Result<Vec<MediaItem>, String> {
    let (item, img) = ctx.source()?;
    let dll = crate::natural::runtime_dll(&ctx.state.paths, &ctx.state.catalog.voices).ok_or("ONNX Runtime isn't installed. Reinstall BiRefNet from the Studio's Models list.")?;
    let f = m.file("onnx").ok_or("The background model is missing a file.")?;
    let model_path = file_path(&ctx.state.paths, m, f);
    let threads = ctx.state.limits().threads.max(2);
    (ctx.progress)("Finding the subject", 0.2);
    let (img2, out) = tokio::task::spawn_blocking(move || -> Result<(image::DynamicImage, image::RgbaImage), String> {
        let matte = cutout::matte(&dll, &model_path, &img, threads)?;
        let out = cutout::apply(&img, &matte);
        Ok((img, out))
    })
    .await
    .map_err(|e| e.to_string())??;
    drop(img2);
    if ctx.cancel.load(Ordering::Relaxed) {
        return Err(download::CANCELLED.into());
    }
    Ok(vec![ctx.keep_picture(&image::DynamicImage::ImageRgba8(out), Some(m), Some(&item.id), None)?])
}
