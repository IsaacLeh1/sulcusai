// SPDX-License-Identifier: AGPL-3.0-only
use super::*;

#[test]
fn the_model_goes_on_the_dedicated_card() {
    use crate::hardware::Gpu;
    let listed = parse_devices("Available devices:
  Vulkan0: NVIDIA GeForce RTX 5070 Ti Laptop GPU (11944 MiB, 11176 MiB free)
  Vulkan1: Intel(R) Graphics (18347 MiB, 17579 MiB free)
");
    assert_eq!(listed.len(), 2);
    assert_eq!(listed[1], ("Vulkan1".to_string(), "Intel(R) Graphics".to_string()));
    let card = |name: &str, integrated: bool| Gpu { name: name.into(), integrated, ..Default::default() };
    let laptop = [card("NVIDIA GeForce RTX 5070 Ti Laptop GPU", false), card("Intel(R) Graphics", true)];
    assert_eq!(dedicated_of(&listed, &laptop), vec![("Vulkan0".to_string(), "NVIDIA GeForce RTX 5070 Ti Laptop GPU".to_string())]);
    // Only a dedicated card, or only integrated graphics: nothing to choose.
    assert!(dedicated_of(&listed[..1], &laptop[..1]).is_empty());
    assert!(dedicated_of(&listed[1..], &laptop[1..]).is_empty());
    // Names that don't match what Windows reports: still never the
    // built-in Intel graphics (its memory is the PC's RAM) while there's
    // a separate card.
    assert_eq!(dedicated_of(&listed, &[card("Some other card", false)]), vec![listed[0].clone()]);
    let renamed = parse_devices("  Vulkan0: Intel(R) Arc(TM) Graphics (18347 MiB, 17579 MiB free)\n  Vulkan1: NVIDIA RTX 5070 Ti (11944 MiB, 11176 MiB free)\n");
    assert_eq!(dedicated_of(&renamed, &laptop), vec![renamed[1].clone()], "the engine's names differ from Windows' for both");
    // An Intel Arc card on its own, beside Intel built-in graphics, is found by name as before.
    let arc = [card("Intel(R) Arc(TM) A770 Graphics", false), card("Intel(R) UHD Graphics", true)];
    let both_intel = parse_devices("  Vulkan0: Intel(R) UHD Graphics (8000 MiB)\n  Vulkan1: Intel(R) Arc(TM) A770 Graphics (16000 MiB)\n");
    assert_eq!(dedicated_of(&both_intel, &arc), vec![both_intel[1].clone()]);
    // Whether the engine sees a card it may use: not when it lists only
    // the integrated graphics, or nothing at all (an old driver).
    assert!(sees_a_card(&listed, &laptop));
    assert!(!sees_a_card(&listed[1..], &laptop));
    assert!(!sees_a_card(&[], &laptop));
    assert!(devices_line(&[]).contains("none"));
    assert!(devices_line(&listed).contains("Vulkan1 Intel(R) Graphics"));
}

/// The card picked as the main one is the device the model runs on,
/// found by its name whatever number the engine gives it.
#[test]
fn the_main_card_is_found_by_name() {
    use crate::hardware::Gpu;
    let card = |name: &str, integrated: bool| Gpu { name: name.into(), integrated, ..Default::default() };
    let listed = parse_devices("  Vulkan0: Intel(R) UHD Graphics 770 (16239 MiB, 15471 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 3060 (12288 MiB, 11000 MiB free)\n  Vulkan2: NVIDIA GeForce RTX 4060 (8188 MiB, 7900 MiB free)\n");
    let cards = [card("Intel(R) UHD Graphics 770", true), card("NVIDIA GeForce RTX 3060", false), card("NVIDIA GeForce RTX 4060", false)];
    let pick = |main: usize| pick_devices(&listed, &cards, Some(&cards[main]));
    assert_eq!(pick(2), vec![("Vulkan2".to_string(), "NVIDIA GeForce RTX 4060".to_string())]);
    assert_eq!(pick(1)[0].0, "Vulkan1");
    // Renumbered by a driver update: still the same card.
    let renumbered = parse_devices("  Vulkan0: NVIDIA GeForce RTX 4060 (8188 MiB, 7900 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 3060 (12288 MiB, 11000 MiB free)\n  Vulkan2: Intel(R) UHD Graphics 770 (16239 MiB, 15471 MiB free)\n");
    assert_eq!(pick_devices(&renumbered, &cards, Some(&cards[2]))[0].0, "Vulkan0");
    // Two cards whose names start alike: the exact one.
    let ti = [card("NVIDIA GeForce RTX 4060 Ti", false), card("NVIDIA GeForce RTX 4060", false)];
    let both = parse_devices("  Vulkan0: NVIDIA GeForce RTX 4060 Ti (16380 MiB, 16000 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 4060 (8188 MiB, 7900 MiB free)\n");
    assert_eq!(pick_devices(&both, &ti, Some(&ti[1]))[0].0, "Vulkan1");
    // The engine names the card differently: every dedicated card, as before.
    assert_eq!(pick_devices(&listed, &cards, Some(&card("Radeon RX 7600", false))).len(), 2);
    // Never the integrated graphics, even when asked.
    assert_ne!(pick_devices(&listed, &cards, Some(&cards[0])).first().map(|d| d.1.as_str()), Some("Intel(R) UHD Graphics 770"));
}

/// Several cards of one maker share a model (none picked): two identical
/// RTX 5090s are both used, the built-in graphics never; a smaller card
/// from another maker stays out; a picked card is used alone.
#[test]
fn several_cards_share_a_model() {
    use crate::hardware::{model_cards, Gpu};
    let gib = 1u64 << 30;
    let card = |name: &str, vendor: &str, gb: u64, integrated: bool| Gpu { name: name.into(), vendor: vendor.into(), vram_bytes: gb * gib, integrated, ..Default::default() };
    let cards = [
        card("AMD Radeon(TM) Graphics", "AMD", 2, true),
        card("NVIDIA GeForce RTX 5090", "NVIDIA", 32, false),
        card("NVIDIA GeForce RTX 5090", "NVIDIA", 32, false),
        card("AMD Radeon RX 6400", "AMD", 4, false),
    ];
    let several = model_cards(&cards, None);
    assert_eq!(several.len(), 2, "both 5090s, not the other maker's card");
    let listed = parse_devices("  Vulkan0: AMD Radeon(TM) Graphics (2048 MiB, 1900 MiB free)\n  Vulkan1: NVIDIA GeForce RTX 5090 (32607 MiB, 32000 MiB free)\n  Vulkan2: NVIDIA GeForce RTX 5090 (32607 MiB, 32000 MiB free)\n  Vulkan3: AMD Radeon RX 6400 (4096 MiB, 4000 MiB free)\n");
    let ids: Vec<String> = pick_several(&listed, &cards, &several).into_iter().map(|d| d.0).collect();
    assert_eq!(ids, ["Vulkan1", "Vulkan2"]);
    assert_eq!(model_cards(&cards, Some("NVIDIA GeForce RTX 5090")).len(), 1, "picked: that card alone");
    // The budget adds them up, and a model's window can grow on them.
    let hw = crate::hardware::Hardware { gpus: cards.to_vec(), ram_total: 128 * gib, disk_free: 4000 * gib, cpu_cores: Some(24), cpu_threads: 48, ..Default::default() };
    let b = crate::catalog::Budget::from_hardware(&hw);
    assert_eq!((b.cards, b.vram_card), (2, 32 * gib - 32 * gib / 10));
    assert_eq!(b.vram, 2 * b.vram_card);
    assert!(b.gpu_bw_gbs >= 1500.0, "{}", b.gpu_bw_gbs);
    assert_eq!(b.gpu_bw_card, b.gpu_bw_gbs);
}

/// llama.cpp b11450 at verbosity 4, as it logs a load: the layers it
/// put on the card and where the weights went.
#[test]
fn the_engine_log_says_where_the_model_runs() {
    let on_card = "\
0.00.252.757 I srv  llama_server: initializing ...
0.00.270.019 I srv    load_model: loading model 'C:\\models\\qwen3-4b.gguf'
0.00.301.100 I llama_model_load_from_file_impl: using device Vulkan0 (NVIDIA GeForce RTX 5070) (0000:01:00.0) - 11175 MiB free
0.00.600.000 I load_tensors: offloading 36 repeating layers to GPU
0.00.600.001 I load_tensors: offloading output layer to GPU
0.00.600.002 I load_tensors: offloaded 37/37 layers to GPU
0.00.600.003 I load_tensors:      Vulkan0 model buffer size =  2254.10 MiB
0.00.600.004 I load_tensors:  Vulkan_Host model buffer size =   304.28 MiB
0.01.000.000 I llama_kv_cache:    Vulkan0 KV buffer size =  1152.00 MiB
0.01.945.286 I srv  llama_server: model loaded
";
    let p = parse_placed(on_card);
    assert_eq!(p.layers, Some((37, 37)));
    assert_eq!(p.devices, vec![("Vulkan0".to_string(), "NVIDIA GeForce RTX 5070".to_string())]);
    assert_eq!((p.on_card(), p.total()), (Some(37), Some(37)));
    assert_eq!(p.device().as_deref(), Some("NVIDIA GeForce RTX 5070"));
    assert!((p.card_mib - 2254.10).abs() < 0.01 && (p.memory_mib - 304.28).abs() < 0.01, "{p:?}");
    assert_eq!(p.describe(), "running 37/37 layers on NVIDIA GeForce RTX 5070");

    // Part of it on the card (the older programs' prefix, a name with brackets).
    let split = "\
llama_model_load_from_file_impl: using device Vulkan1 (Intel(R) Arc(TM) A370M Graphics) (0000:03:00.0) - 3900 MiB free
print_backend_buffers_info: offloaded 12/37 layers to GPU
print_backend_buffers_info:      Vulkan1 model buffer size =   800.00 MiB
print_backend_buffers_info:   CPU_Mapped model buffer size =  1700.00 MiB
";
    let p = parse_placed(split);
    assert_eq!(p.devices[0].1, "Intel(R) Arc(TM) A370M Graphics");
    assert_eq!((p.on_card(), p.total()), (Some(12), Some(37)));
    assert!(p.describe().ends_with("the rest on the processor"), "{}", p.describe());

    // Asked for 33 layers, but the engine had no card to use: it still
    // counts them, while every weight sits in system memory.
    let no_card = "\
0.00.600.002 I load_tensors: offloaded 33/37 layers to GPU
0.00.600.003 I load_tensors:   CPU_Mapped model buffer size =  2500.00 MiB
";
    let p = parse_placed(no_card);
    assert_eq!(p.on_card(), Some(0));
    assert_eq!(p.device(), None);
    assert_eq!(p.describe(), "running 0/37 layers on the graphics card: all on the processor");

    // The card listed but no layers given to it (GPU limit Off).
    let off = "\
I llama_model_load_from_file_impl: using device Vulkan0 (NVIDIA GeForce GTX 1650) (0000:01:00.0) - 3500 MiB free
I load_tensors: offloaded 0/37 layers to GPU
I load_tensors:   CPU_Mapped model buffer size =  2500.00 MiB
";
    assert_eq!(parse_placed(off).on_card(), Some(0));

    // A trial load before the real one: the last counts.
    let trial = format!("{split}{on_card}");
    assert_eq!(parse_placed(&trial).on_card(), Some(37));

    // Nothing about loading in the log (an older engine at verbosity 3).
    let quiet = "0.00.252.757 I srv  llama_server: initializing ...\n0.01.945.286 I srv  llama_server: model loaded\n";
    let p = parse_placed(quiet);
    assert_eq!((p.on_card(), p.total()), (None, None));
    assert_eq!(p.describe(), "the engine's log didn't say where it runs");
    // Lines that look alike but aren't about layers.
    assert_eq!(parse_placed("I llama_context: sampler 'dist' for seq_id = 0, cannot be offloaded to the backend\n").layers, None);
}

/// On any system: downloads this system's engine build into
/// SULCUSAI_DATA (a scratch folder), loads SULCUSAI_MODEL (a Qwen3 GGUF)
/// and gets a streamed answer.
#[tokio::test]
#[ignore]
async fn e2e_engine_runs_on_this_system() {
    let data = std::path::PathBuf::from(std::env::var("SULCUSAI_DATA").expect("SULCUSAI_DATA"));
    let model = std::path::PathBuf::from(std::env::var("SULCUSAI_MODEL").expect("SULCUSAI_MODEL"));
    let paths = Paths::new(data).unwrap();
    let hw = crate::hardware::detect(&paths.models);
    println!("{} · {} · gpus {:?}", hw.os, hw.cpu_name, hw.gpus);
    let budget = Budget::from_hardware(&hw);
    let backend = backend_for(&budget).unwrap();
    let cat = crate::catalog::Catalog::bundled();
    let cancel = AtomicBool::new(false);
    let exe = ensure_installed(&paths, &cat.engine, backend, &cancel, |_, _| {}).await.unwrap();
    println!("engine {backend}: {}", exe.display());
    let mut eng = Engine::default();
    let log = paths.engine_log();
    let ep = eng
        .ensure(
            LaunchSpec {
                exe: &exe,
                model_path: &model,
                model_id: "qwen3-1.7b",
                quant: "Q4_K_M",
                ctx: 4096,
                gpu_layers: if budget.vram > 0 { 99 } else { 0 },
                threads: 0,
                low_priority: false,
                log: &log,
                mmproj: None,
                batch: 0,
                lora: None,
                steer: None,
            },
            None,
        )
        .await
        .unwrap_or_else(|e| panic!("engine failed to start: {e}"));
    let messages = vec![serde_json::json!({ "role": "user", "content": "What is 2 + 2? Answer with just the number. /no_think" })];
    let mut deltas = 0;
    let done = crate::chat::stream(&ep, messages, None, &cancel, |_| deltas += 1).await.unwrap();
    println!("reply ({deltas} deltas): {:?} · tps {:?}", done.content, done.tps);
    assert!(done.content.contains('4'));
    eng.stop().await;
}

/// A reply keeps the model it was given: a request for another model
/// waits for it, the reply's own later requests don't, and a reply that
/// was stopped or ended doesn't hold anything.
#[tokio::test]
async fn a_reply_keeps_the_model_it_was_given() {
    let task = || async { tokio::spawn(async { tokio::task::id() }).await.unwrap() };
    let (ta, tb, tc) = (Some(task().await), Some(task().await), Some(task().await));
    let flag = || Arc::new(AtomicBool::new(false));
    let (a, b) = (flag(), flag());
    let mut users = Users::default();

    // Chat A asks for model X and gets it.
    let before_a = users.unplaced(&[a.clone()]);
    users.place(&[a.clone()], &before_a, "x", ta);
    // Chat B wants another model: A is still on X.
    let running = [a.clone(), b.clone()];
    let before_b = users.unplaced(&running);
    assert_eq!(before_b.len(), 1);
    assert_eq!(users.others(&running, &before_b, "x", tb), 1);
    // A's own later request (a picture, say) doesn't wait on itself.
    assert_eq!(users.others(&running, &[], "x", ta), 0);
    // So does a request that isn't a chat (the Studio, a speed test).
    assert_eq!(users.others(&[a.clone()], &[], "x", tc), 1);
    // Another model's replies, and the cloud's, don't hold X.
    assert_eq!(users.others(&running, &before_b, "y", tb), 0);
    users.place(&running, &before_b, "", tb);
    assert_eq!(users.others(&running, &[], "x", tc), 1, "B, on the cloud, doesn't count");
    // A stopped, or finished: X is free.
    a.store(true, Ordering::Relaxed);
    assert_eq!(users.others(&running, &[], "x", tc), 0);
    a.store(false, Ordering::Relaxed);
    assert_eq!(users.others(&[b.clone()], &[], "x", tc), 0);
}

/// Two chats starting at once: either may be the one asking, so the
/// second waits only for the other one, never for itself.
#[tokio::test]
async fn two_chats_at_once_dont_wait_on_themselves() {
    let task = || async { tokio::spawn(async { tokio::task::id() }).await.unwrap() };
    let (ta, tb) = (Some(task().await), Some(task().await));
    let (a, b) = (Arc::new(AtomicBool::new(false)), Arc::new(AtomicBool::new(false)));
    let mut users = Users::default();
    let running = [a.clone(), b.clone()];
    // A's request came in after both chats started: both look like A's.
    let before_a = users.unplaced(&running);
    users.place(&running, &before_a, "x", ta);
    // B wants another model: it waits for A...
    let before_b = users.unplaced(&running);
    assert_eq!(users.others(&running, &before_b, "x", tb), 1);
    // ...and only for A.
    assert_eq!(users.others(&[b.clone()], &before_b, "x", tb), 0);
    // A task that ran before the first chat got a model (a scheduled
    // task, a reply to another chat) still holds it for a chat's request.
    let s = Arc::new(AtomicBool::new(false));
    let fresh = Users::default();
    let running = [s.clone(), b.clone()];
    let before = fresh.unplaced(&running);
    assert_eq!(fresh.others(&running, &before, "x", tb), 1);
    assert_eq!(fresh.others(&[b.clone()], &[b.clone()], "x", tb), 0);
}

#[test]
fn engine_builds_per_system() {
    assert_eq!(backend_named("windows", "x86_64", true).unwrap(), "vulkan-x64");
    assert_eq!(backend_named("linux", "x86_64", false).unwrap(), "linux-cpu-x64");
    assert_eq!(backend_named("macos", "aarch64", true).unwrap(), "macos-arm64");
    assert!(backend_named("freebsd", "x86_64", true).is_err());
}

#[test]
fn requests_for_another_model_wait_in_the_order_they_asked() {
    let first = Ticket::new();
    let second = Ticket::new();
    assert_eq!(second.place(), first.place() + 1);
    let was = second.place();
    drop(first);
    assert_eq!(second.place(), was - 1, "the one ahead left, so it moves up");
}

#[test]
fn engine_zips_unpack_inside_their_folder() {
    use std::io::Write;
    let dir = std::env::temp_dir().join(format!("sulcusai-unzip-{}", uuid::Uuid::new_v4()));
    std::fs::create_dir_all(&dir).unwrap();
    let zip_path = dir.join("engine.zip");
    let mut z = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
    let opts = zip::write::SimpleFileOptions::default();
    for (name, data) in [("bin/llama-server.exe", "server"), ("lib/ggml.dll", "library"), ("../escaped.txt", "outside")] {
        z.start_file(name, opts).unwrap();
        z.write_all(data.as_bytes()).unwrap();
    }
    z.finish().unwrap();

    let into = dir.join("engine");
    unzip(&zip_path, &into).unwrap();
    assert_eq!(std::fs::read_to_string(into.join("bin/llama-server.exe")).unwrap(), "server");
    assert!(!dir.join("escaped.txt").exists(), "an entry naming a place outside the folder is skipped");

    let libs = dir.join("libs");
    std::fs::create_dir_all(&libs).unwrap();
    unzip_libraries(&zip_path, &libs).unwrap();
    assert_eq!(std::fs::read_to_string(libs.join("ggml.dll")).unwrap(), "library");
    assert_eq!(std::fs::read_dir(&libs).unwrap().count(), 1, "only the libraries");
    std::fs::remove_dir_all(&dir).unwrap();
}
