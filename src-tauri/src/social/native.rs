// SPDX-License-Identifier: AGPL-3.0-only
//! Native capture for calls: one game's sound, and the screen when a game
//! runs in exclusive full screen. Both read what Windows already composes or
//! mixes; nothing is loaded into or opened inside the game's process.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

/// Sound format asked of the process loopback device. That device has no mix
/// format of its own, so the stream states one and Windows converts to it.
pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: u16 = 2;

/// One captured screen frame, top row first, 4 bytes per pixel (B, G, R, A).
#[derive(Debug, Clone)]
pub struct Frame {
    pub width: u32,
    pub height: u32,
    pub bgra: Vec<u8>,
}

/// Copies rows out of a mapped texture whose rows may be padded past the
/// visible width.
pub fn unpad_rows(src: &[u8], pitch: usize, width: u32, height: u32) -> Vec<u8> {
    let row = width as usize * 4;
    let mut out = Vec::with_capacity(row * height as usize);
    for y in 0..height as usize {
        let start = y * pitch;
        out.extend_from_slice(&src[start..start + row]);
    }
    out
}

/// Stops a running capture when dropped, so a call ending can't leave one on.
pub struct Running {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

impl Running {
    pub fn stop(mut self) {
        self.halt();
    }
    fn halt(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

impl Drop for Running {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Starts capturing only `pid`'s sound (and its child processes', since games
/// often play audio from a helper). `on_samples` gets interleaved stereo f32
/// at [`SAMPLE_RATE`]. Off unless the game sound switch is on.
pub fn start_game_sound(enabled: bool, pid: u32, on_samples: impl FnMut(&[f32]) + Send + 'static) -> Result<Running, String> {
    if !enabled {
        return Err("Game-only sound is off in Settings.".into());
    }
    if pid == 0 {
        return Err("No game process to capture.".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let s = stop.clone();
    let thread = std::thread::Builder::new()
        .name("game-sound".into())
        .spawn(move || imp::loopback_thread(pid, s, ready_tx, on_samples))
        .map_err(|e| e.to_string())?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Running { stop, thread: Some(thread) }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err("The sound capture stopped before it started.".into()),
    }
}

/// Reads frames of one monitor through DXGI desktop duplication, which still
/// sees exclusive full screen games that window capture shows as black.
/// Off unless the native screen capture switch is on.
pub fn start_screen(enabled: bool, output: u32, fps: u32, on_frame: impl FnMut(Frame) + Send + 'static) -> Result<Running, String> {
    if !enabled {
        return Err("Full-screen game capture is off in Settings.".into());
    }
    let stop = Arc::new(AtomicBool::new(false));
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let s = stop.clone();
    let thread = std::thread::Builder::new()
        .name("screen-dup".into())
        .spawn(move || imp::duplication_thread(output, fps.clamp(1, 120), s, ready_tx, on_frame))
        .map_err(|e| e.to_string())?;
    match ready_rx.recv() {
        Ok(Ok(())) => Ok(Running { stop, thread: Some(thread) }),
        Ok(Err(e)) => {
            let _ = thread.join();
            Err(e)
        }
        Err(_) => Err("The screen capture stopped before it started.".into()),
    }
}

#[cfg(not(windows))]
mod imp {
    use super::*;
    use std::sync::mpsc::Sender;
    pub fn loopback_thread(_: u32, _: Arc<AtomicBool>, ready: Sender<Result<(), String>>, _: impl FnMut(&[f32])) {
        let _ = ready.send(Err("Game-only sound needs Windows.".into()));
    }
    pub fn duplication_thread(_: u32, _: u32, _: Arc<AtomicBool>, ready: Sender<Result<(), String>>, _: impl FnMut(Frame)) {
        let _ = ready.send(Err("Full-screen game capture needs Windows.".into()));
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::sync::mpsc::Sender;
    use std::sync::Mutex;
    use windows::core::{implement, Interface, IUnknown, PROPVARIANT};
    use windows::Win32::Foundation::{CloseHandle, HANDLE, WAIT_OBJECT_0};
    use windows::Win32::Graphics::Direct3D::D3D_DRIVER_TYPE_HARDWARE;
    use windows::Win32::Graphics::Direct3D11::*;
    use windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC;
    use windows::Win32::Graphics::Dxgi::*;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::{CoInitializeEx, CoUninitialize, COINIT_MULTITHREADED};
    use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const VT_BLOB: u16 = 65;

    /// PROPVARIANT holding a BLOB, laid out as the system reads it.
    #[repr(C)]
    struct BlobVariant {
        vt: u16,
        r1: u16,
        r2: u16,
        r3: u16,
        size: u32,
        data: *const u8,
    }
    const _: () = assert!(std::mem::size_of::<BlobVariant>() == std::mem::size_of::<PROPVARIANT>());

    // The macro also makes it agile, which the activation callback requires.
    #[implement(IActivateAudioInterfaceCompletionHandler)]
    struct Done(Mutex<Option<Sender<()>>>);

    impl IActivateAudioInterfaceCompletionHandler_Impl for Done_Impl {
        fn ActivateCompleted(&self, _: Option<&IActivateAudioInterfaceAsyncOperation>) -> windows::core::Result<()> {
            if let Some(tx) = self.0.lock().ok().and_then(|mut g| g.take()) {
                let _ = tx.send(());
            }
            Ok(())
        }
    }

    struct Com;
    impl Com {
        fn init() -> Self {
            unsafe {
                let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            }
            Com
        }
    }
    impl Drop for Com {
        fn drop(&mut self) {
            unsafe { CoUninitialize() }
        }
    }

    struct Event(HANDLE);
    impl Drop for Event {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }

    unsafe fn activate(pid: u32) -> Result<IAudioClient, String> {
        let params = AUDIOCLIENT_ACTIVATION_PARAMS {
            ActivationType: AUDIOCLIENT_ACTIVATION_TYPE_PROCESS_LOOPBACK,
            Anonymous: AUDIOCLIENT_ACTIVATION_PARAMS_0 {
                ProcessLoopbackParams: AUDIOCLIENT_PROCESS_LOOPBACK_PARAMS {
                    TargetProcessId: pid,
                    ProcessLoopbackMode: PROCESS_LOOPBACK_MODE_INCLUDE_TARGET_PROCESS_TREE,
                },
            },
        };
        let blob = BlobVariant {
            vt: VT_BLOB,
            r1: 0,
            r2: 0,
            r3: 0,
            size: std::mem::size_of::<AUDIOCLIENT_ACTIVATION_PARAMS>() as u32,
            data: &params as *const _ as *const u8,
        };
        let (tx, rx) = std::sync::mpsc::channel();
        let handler: IActivateAudioInterfaceCompletionHandler = Done(Mutex::new(Some(tx))).into();
        let op = ActivateAudioInterfaceAsync(
            VIRTUAL_AUDIO_DEVICE_PROCESS_LOOPBACK,
            &IAudioClient::IID,
            Some(&blob as *const BlobVariant as *const PROPVARIANT),
            &handler,
        )
        .map_err(|e| format!("Windows refused game-only sound (needs Windows 10 2004 or newer): {e}"))?;
        rx.recv_timeout(std::time::Duration::from_secs(5)).map_err(|_| "Windows didn't answer the sound capture request.".to_string())?;
        let mut hr = windows::core::HRESULT(0);
        let mut unk: Option<IUnknown> = None;
        op.GetActivateResult(&mut hr, &mut unk).map_err(|e| e.to_string())?;
        hr.ok().map_err(|e| format!("Couldn't capture that game's sound: {e}"))?;
        unk.ok_or("No sound device came back.")?.cast::<IAudioClient>().map_err(|e| e.to_string())
    }

    pub fn loopback_thread(pid: u32, stop: Arc<AtomicBool>, ready: Sender<Result<(), String>>, mut on_samples: impl FnMut(&[f32])) {
        let _com = Com::init();
        let started = unsafe {
            (|| -> Result<(IAudioClient, IAudioCaptureClient, Event), String> {
                let client = activate(pid)?;
                let block = CHANNELS * 4;
                let fmt = WAVEFORMATEX {
                    wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
                    nChannels: CHANNELS,
                    nSamplesPerSec: SAMPLE_RATE,
                    nAvgBytesPerSec: SAMPLE_RATE * block as u32,
                    nBlockAlign: block,
                    wBitsPerSample: 32,
                    cbSize: 0,
                };
                let flags = AUDCLNT_STREAMFLAGS_LOOPBACK | AUDCLNT_STREAMFLAGS_EVENTCALLBACK | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM;
                // 20 ms buffer: short enough for voice-chat latency.
                client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, 200_000, 0, &fmt, None).map_err(|e| e.to_string())?;
                let ev = Event(CreateEventW(None, false, false, None).map_err(|e| e.to_string())?);
                client.SetEventHandle(ev.0).map_err(|e| e.to_string())?;
                let cap: IAudioCaptureClient = client.GetService().map_err(|e| e.to_string())?;
                client.Start().map_err(|e| e.to_string())?;
                Ok((client, cap, ev))
            })()
        };
        let (client, cap, ev) = match started {
            Ok(v) => {
                let _ = ready.send(Ok(()));
                v
            }
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        let mut silence = Vec::new();
        while !stop.load(Ordering::SeqCst) {
            if unsafe { WaitForSingleObject(ev.0, 100) } != WAIT_OBJECT_0 {
                continue;
            }
            loop {
                let n = match unsafe { cap.GetNextPacketSize() } {
                    Ok(n) if n > 0 => n,
                    _ => break,
                };
                let mut data = std::ptr::null_mut();
                let (mut frames, mut flags) = (n, 0u32);
                if unsafe { cap.GetBuffer(&mut data, &mut frames, &mut flags, None, None) }.is_err() {
                    break;
                }
                let count = frames as usize * CHANNELS as usize;
                if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 || data.is_null() {
                    silence.clear();
                    silence.resize(count, 0.0);
                    on_samples(&silence);
                } else {
                    on_samples(unsafe { std::slice::from_raw_parts(data as *const f32, count) });
                }
                unsafe {
                    let _ = cap.ReleaseBuffer(frames);
                }
            }
        }
        unsafe {
            let _ = client.Stop();
        }
    }

    struct Dup {
        device: ID3D11Device,
        ctx: ID3D11DeviceContext,
        dup: IDXGIOutputDuplication,
        staging: Option<(ID3D11Texture2D, u32, u32)>,
    }

    unsafe fn open(output: u32) -> Result<Dup, String> {
        let mut device = None;
        let mut ctx = None;
        D3D11CreateDevice(None, D3D_DRIVER_TYPE_HARDWARE, None, D3D11_CREATE_DEVICE_BGRA_SUPPORT, None, D3D11_SDK_VERSION, Some(&mut device), None, Some(&mut ctx))
            .map_err(|e| format!("No graphics device for screen capture: {e}"))?;
        let device: ID3D11Device = device.ok_or("No graphics device.")?;
        let ctx = ctx.ok_or("No graphics context.")?;
        let adapter = device.cast::<IDXGIDevice>().and_then(|d| d.GetAdapter()).map_err(|e| e.to_string())?;
        let out = adapter.EnumOutputs(output).map_err(|_| "That screen isn't on the graphics card the capture uses.".to_string())?;
        let dup = out.cast::<IDXGIOutput1>().and_then(|o| o.DuplicateOutput(&device)).map_err(|e| format!("Windows refused screen duplication: {e}"))?;
        Ok(Dup { device, ctx, dup, staging: None })
    }

    /// One frame, or None when nothing changed before the timeout.
    unsafe fn grab(d: &mut Dup, timeout_ms: u32) -> windows::core::Result<Option<Frame>> {
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut res = None;
        match d.dup.AcquireNextFrame(timeout_ms, &mut info, &mut res) {
            Ok(()) => {}
            Err(e) if e.code() == DXGI_ERROR_WAIT_TIMEOUT => return Ok(None),
            Err(e) => return Err(e),
        }
        let result = (|| {
            if info.LastPresentTime == 0 {
                return Ok(None);
            }
            let tex: ID3D11Texture2D = res.ok_or_else(|| windows::core::Error::from(DXGI_ERROR_INVALID_CALL))?.cast()?;
            let mut desc = D3D11_TEXTURE2D_DESC::default();
            tex.GetDesc(&mut desc);
            if d.staging.as_ref().map(|s| (s.1, s.2)) != Some((desc.Width, desc.Height)) {
                let sd = D3D11_TEXTURE2D_DESC {
                    Width: desc.Width,
                    Height: desc.Height,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: desc.Format,
                    SampleDesc: DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
                    Usage: D3D11_USAGE_STAGING,
                    BindFlags: 0,
                    CPUAccessFlags: D3D11_CPU_ACCESS_READ.0 as u32,
                    MiscFlags: 0,
                };
                let mut t = None;
                d.device.CreateTexture2D(&sd, None, Some(&mut t))?;
                d.staging = t.map(|t| (t, desc.Width, desc.Height));
            }
            let (st, w, h) = d.staging.clone().ok_or_else(|| windows::core::Error::from(DXGI_ERROR_INVALID_CALL))?;
            d.ctx.CopyResource(&st, &tex);
            let mut map = D3D11_MAPPED_SUBRESOURCE::default();
            d.ctx.Map(&st, 0, D3D11_MAP_READ, 0, Some(&mut map))?;
            let pitch = map.RowPitch as usize;
            let src = std::slice::from_raw_parts(map.pData as *const u8, pitch * h as usize);
            let bgra = unpad_rows(src, pitch, w, h);
            d.ctx.Unmap(&st, 0);
            Ok(Some(Frame { width: w, height: h, bgra }))
        })();
        let _ = d.dup.ReleaseFrame();
        result
    }

    pub fn duplication_thread(output: u32, fps: u32, stop: Arc<AtomicBool>, ready: Sender<Result<(), String>>, mut on_frame: impl FnMut(Frame)) {
        let mut d = match unsafe { open(output) } {
            Ok(d) => {
                let _ = ready.send(Ok(()));
                d
            }
            Err(e) => {
                let _ = ready.send(Err(e));
                return;
            }
        };
        let gap = std::time::Duration::from_millis(1000 / fps as u64);
        while !stop.load(Ordering::SeqCst) {
            let t0 = std::time::Instant::now();
            match unsafe { grab(&mut d, 100) } {
                Ok(Some(f)) => on_frame(f),
                Ok(None) => {}
                // A mode change or a full screen game taking over the output
                // loses the duplication; it must be opened again.
                Err(e) if e.code() == DXGI_ERROR_ACCESS_LOST => {
                    std::thread::sleep(std::time::Duration::from_millis(200));
                    if let Ok(n) = unsafe { open(output) } {
                        d = n;
                    }
                }
                Err(_) => std::thread::sleep(std::time::Duration::from_millis(200)),
            }
            if let Some(rest) = gap.checked_sub(t0.elapsed()) {
                std::thread::sleep(rest);
            }
        }
    }
}
