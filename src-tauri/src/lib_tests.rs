// SPDX-License-Identifier: AGPL-3.0-only
//! Tests for helpers at the crate root.

#[cfg(all(test, windows))]
mod open_tests {
    /// Opens a sign-in-length address (about 700 characters) in the default
    /// browser; Explorer used to show a folder instead. Opens a real tab.
    #[test]
    #[ignore]
    fn a_long_address_opens_in_the_browser() {
        let url = format!("https://sulcusai.sseditor.com/?test={}", "x".repeat(700));
        crate::open_with_system(std::ffi::OsStr::new(&url)).unwrap();
    }
}

#[cfg(test)]
mod think_tests {
    /// About a minute and a quarter of thinking at the model's speed: a 27B
    /// partly in system memory thought for minutes.
    #[test]
    fn thinking_is_kept_to_about_a_minute() {
        assert_eq!(crate::think_tokens(10.0), Some(1024), "a slow model still gets 1K");
        assert_eq!(crate::think_tokens(30.0), Some(2250));
        assert_eq!(crate::think_tokens(400.0), Some(8192), "a fast one at most 8K");
        assert_eq!(crate::think_tokens(0.0), None, "no speed known: no limit");
    }
}

#[cfg(test)]
mod hub_link_tests {
    use serde_json::json;

    /// Stable Diffusion 1.5 and FLUX.1 models pasted in: the message used to
    /// read like a broken GGUF file.
    #[test]
    fn a_model_that_cant_be_used_says_why_in_plain_words() {
        let flux = crate::hub_why_not(&json!({ "id": "city96/FLUX.1-schnell-gguf", "pipeline_tag": "text-to-image", "cardData": { "base_model": "black-forest-labs/FLUX.1-schnell" } }));
        assert!(flux.starts_with("That FLUX.1 model") && flux.contains("Juggernaut XL"), "{flux}");
        let sd3 = crate::hub_why_not(&json!({ "id": "stabilityai/stable-diffusion-3.5-large", "pipeline_tag": "text-to-image", "tags": ["diffusers"] }));
        assert!(sd3.contains("picture model") && !sd3.contains("GGUF"), "{sd3}");
        let split = crate::hub_why_not(&json!({ "id": "Lykon/dreamshaper-8", "pipeline_tag": "text-to-image", "siblings": [{ "rfilename": "unet/diffusion_pytorch_model.safetensors" }, { "rfilename": "vae/diffusion_pytorch_model.safetensors" }] }));
        assert!(split.contains("split into folders"), "{split}");
        let chat = crate::hub_why_not(&json!({ "id": "Qwen/Qwen3-8B", "pipeline_tag": "text-generation" }));
        assert!(chat.contains("no GGUF file") && chat.contains("unsloth"), "{chat}");
    }
}

#[cfg(all(test, windows))]
mod incognito_tests {
    /// Leaving an incognito chat takes everything it left with it: pictures
    /// pasted or made there, file backups, and what it sent; other chats keep theirs.
    #[test]
    fn leaving_an_incognito_chat_leaves_nothing_behind() {
        let state = crate::testutil::temp_state(Some(std::env::temp_dir().join(format!("sulcusai-inc-models-{}", uuid::Uuid::new_v4().simple()))));
        let c = state.cipher().unwrap();
        let (secret, kept) = {
            let conn = state.db.lock().unwrap();
            let secret = crate::db::create_chat_in(&conn, &c, None, None, true).unwrap().id;
            let kept = crate::db::create_chat_in(&conn, &c, None, None, false).unwrap().id;
            for (chat, hidden) in [(&secret, true), (&secret, false), (&kept, true)] {
                let item = crate::media::store::MediaItem { id: crate::media::store::new_id(), kind: "image".into(), op: "paste".into(), mime: "image/png".into(), chat_id: Some(chat.clone()), hidden, ..Default::default() };
                crate::media::store::save(&conn, &state.paths, &c, item, b"png", None).unwrap();
            }
            let backup = state.paths.data.join("x.bak");
            std::fs::write(&backup, b"old").unwrap();
            crate::db::add_checkpoint(&conn, &c, &secret, "t", "write", "C:/x.txt", None, Some(&backup.display().to_string())).unwrap();
            (secret, kept)
        };
        let r = crate::outgoing::review_event("your Outlook calendar", &[("title", "Title", "dentist")], &crate::outgoing::Me::new("", vec![], ""));
        crate::outgoing::record(&state, &c, &secret, "calendar_create_event", &r, true);
        crate::outgoing::record(&state, &c, &kept, "calendar_create_event", &r, true);
        crate::clear_incognito(&state, None).unwrap();
        let conn = state.db.lock().unwrap();
        assert!(crate::media::store::in_chat(&conn, &c, &secret).is_empty());
        assert_eq!(crate::media::store::in_chat(&conn, &c, &kept).len(), 1);
        assert!(crate::db::chat_checkpoints(&conn, &secret).is_empty() && !state.paths.data.join("x.bak").exists());
        drop(conn);
        let sent = crate::outgoing::sent_list(&state.db.lock().unwrap(), &c);
        assert!(sent.iter().all(|s| s.chat_id != secret) && sent.iter().any(|s| s.chat_id == kept));
        let _ = std::fs::remove_dir_all(&state.paths.data);
    }
}

#[cfg(test)]
mod model_file_tests {
    /// A model file copied part way, overwritten or gone is said plainly;
    /// a whole GGUF file is fine.
    #[test]
    fn a_damaged_model_file_is_found() {
        let dir = std::env::temp_dir().join(format!("sulcusai-damaged-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        let good = dir.join("good.gguf");
        let mut bytes = b"GGUF".to_vec();
        bytes.extend_from_slice(&[0u8; 60]);
        std::fs::write(&good, &bytes).unwrap();
        assert_eq!(crate::model_file_problem(&good, 64), None);
        assert_eq!(crate::model_file_problem(&good, 0), None, "size not known");
        let zeros = dir.join("zeros.gguf");
        std::fs::write(&zeros, [0u8; 64]).unwrap();
        assert!(crate::model_file_problem(&zeros, 64).unwrap().contains("isn't a model file"));
        assert!(crate::model_file_problem(&good, 128).unwrap().contains("instead of"), "cut short");
        assert!(crate::model_file_problem(&dir.join("gone.gguf"), 64).unwrap().contains("missing"));
        // A model in three files: its size is all three (the first alone
        // called every big model damaged); a part gone is said.
        let part = |n: u32| dir.join(format!("big-Q4_K_M-{n:05}-of-00003.gguf"));
        for n in 1..=3 {
            std::fs::write(part(n), &bytes).unwrap();
        }
        assert_eq!(crate::model_file_problem(&part(1), 192), None);
        assert!(crate::model_file_problem(&part(1), 256).unwrap().contains("instead of"));
        std::fs::remove_file(part(2)).unwrap();
        assert_eq!(crate::model_file_problem(&part(1), 192).unwrap(), "part 2 of its 3 files is missing");
        std::fs::remove_dir_all(&dir).ok();
    }
}
