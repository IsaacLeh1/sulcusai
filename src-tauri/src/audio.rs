// SPDX-License-Identifier: AGPL-3.0-only
//! Sound in and out through Windows' audio system (WASAPI).
//!
//! Capture delivers 16 kHz mono samples, the format speech recognition uses,
//! from a microphone or from what the computer itself is playing ("loopback",
//! which hears the other people in a call without joining it). Playback plays
//! mono clips such as synthesized speech and can be cut off at once.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use serde::Serialize;

/// Sample rate of everything capture delivers.
pub const RATE: u32 = 16_000;

#[derive(Debug, Clone, Serialize)]
pub struct Device {
    pub id: String,
    pub name: String,
    pub default: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Flow {
    Input,
    Output,
}

#[derive(Debug, Clone)]
pub enum Source {
    /// A microphone. `voice` asks Windows for its voice processing: echo
    /// cancellation and noise suppression, where the PC provides them.
    Mic { device: Option<String>, voice: bool },
    /// What the computer is playing, such as the other people in a call.
    System { device: Option<String> },
}

pub type Sink = tokio::sync::mpsc::UnboundedSender<Vec<f32>>;

/// A running capture. Samples go to the sink until this is dropped.
pub struct Capture {
    stop: Arc<AtomicBool>,
    thread: Option<std::thread::JoinHandle<()>>,
    level: Arc<AtomicU32>,
    error: Arc<Mutex<Option<String>>>,
}

impl Capture {
    pub fn start(source: Source, sink: Sink) -> Result<Capture, String> {
        let stop = Arc::new(AtomicBool::new(false));
        let level = Arc::new(AtomicU32::new(0));
        let error = Arc::new(Mutex::new(None));
        let (ready_tx, ready_rx) = std::sync::mpsc::sync_channel::<Result<(), String>>(1);
        let (s, l, e) = (stop.clone(), level.clone(), error.clone());
        let thread = std::thread::Builder::new()
            .name("audio-capture".into())
            .spawn(move || imp::capture_thread(source, sink, s, l, e, ready_tx))
            .map_err(|e| e.to_string())?;
        match ready_rx.recv() {
            Ok(Ok(())) => Ok(Capture { stop, thread: Some(thread), level, error }),
            Ok(Err(e)) => {
                let _ = thread.join();
                Err(e)
            }
            Err(_) => Err("The audio device couldn't be opened.".into()),
        }
    }

    /// Recent loudness, 0.0–1.0 (RMS).
    pub fn level(&self) -> f32 {
        f32::from_bits(self.level.load(Ordering::Relaxed))
    }

    /// Why capture stopped on its own (for example, a device was unplugged).
    pub fn error(&self) -> Option<String> {
        self.error.lock().unwrap().clone()
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

pub fn devices(flow: Flow) -> Result<Vec<Device>, String> {
    // COM setup is per thread; a short-lived thread keeps it off the caller's.
    std::thread::spawn(move || imp::devices(flow)).join().map_err(|_| "Listing audio devices failed.".to_string())?
}

pub fn rms(samples: &[f32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    (samples.iter().map(|s| s * s).sum::<f32>() / samples.len() as f32).sqrt()
}

// ---------- format helpers ----------

/// 16-bit PCM WAV bytes for mono samples at `rate`.
pub fn wav_encode(samples: &[f32], rate: u32) -> Vec<u8> {
    let data_len = (samples.len() * 2) as u32;
    let mut out = Vec::with_capacity(44 + data_len as usize);
    out.extend_from_slice(b"RIFF");
    out.extend_from_slice(&(36 + data_len).to_le_bytes());
    out.extend_from_slice(b"WAVEfmt ");
    out.extend_from_slice(&16u32.to_le_bytes());
    out.extend_from_slice(&1u16.to_le_bytes()); // PCM
    out.extend_from_slice(&1u16.to_le_bytes()); // mono
    out.extend_from_slice(&rate.to_le_bytes());
    out.extend_from_slice(&(rate * 2).to_le_bytes());
    out.extend_from_slice(&2u16.to_le_bytes());
    out.extend_from_slice(&16u16.to_le_bytes());
    out.extend_from_slice(b"data");
    out.extend_from_slice(&data_len.to_le_bytes());
    for s in samples {
        out.extend_from_slice(&to_i16(*s).to_le_bytes());
    }
    out
}

pub fn to_i16(s: f32) -> i16 {
    (s.clamp(-1.0, 1.0) * 32767.0).round() as i16
}

/// Decoded audio: mono samples and their rate.
#[derive(Debug, Clone)]
pub struct Pcm {
    pub rate: u32,
    pub samples: Vec<f32>,
}

/// Reads a PCM (8/16/24/32-bit) or float WAV, mixing channels down to mono.
pub fn wav_decode(bytes: &[u8]) -> Result<Pcm, String> {
    let bad = || "That isn't a WAV file this app can read.".to_string();
    if bytes.len() < 12 || &bytes[0..4] != b"RIFF" || &bytes[8..12] != b"WAVE" {
        return Err(bad());
    }
    let mut pos = 12;
    let mut fmt: Option<Format> = None;
    while pos + 8 <= bytes.len() {
        let id = &bytes[pos..pos + 4];
        let len = u32::from_le_bytes(bytes[pos + 4..pos + 8].try_into().unwrap()) as usize;
        let body = &bytes[pos + 8..(pos + 8 + len).min(bytes.len())];
        if id == b"fmt " && body.len() >= 16 {
            let tag = u16::from_le_bytes([body[0], body[1]]);
            let float = tag == 3 || (tag == 0xFFFE && body.len() >= 26 && body[24] == 3);
            fmt = Some(Format {
                channels: u16::from_le_bytes([body[2], body[3]]).max(1),
                rate: u32::from_le_bytes(body[4..8].try_into().unwrap()),
                bits: u16::from_le_bytes([body[14], body[15]]),
                float,
            });
        } else if id == b"data" {
            let f = fmt.ok_or_else(bad)?;
            return Ok(Pcm { rate: f.rate, samples: f.to_mono(body) });
        }
        pos += 8 + len + (len & 1);
    }
    Err(bad())
}

/// An interleaved sample format, as a device or file describes it.
#[derive(Debug, Clone, Copy)]
pub struct Format {
    pub channels: u16,
    pub rate: u32,
    pub bits: u16,
    pub float: bool,
}

impl Format {
    /// Interleaved bytes to mono floats.
    pub fn to_mono(&self, data: &[u8]) -> Vec<f32> {
        let width = (self.bits as usize / 8).max(1);
        let ch = self.channels as usize;
        let frames = data.len() / (width * ch);
        let mut out = Vec::with_capacity(frames);
        for f in 0..frames {
            let mut sum = 0.0f32;
            for c in 0..ch {
                let at = (f * ch + c) * width;
                let b = &data[at..at + width];
                sum += match (self.float, width) {
                    (true, 4) => f32::from_le_bytes(b.try_into().unwrap()),
                    (true, 8) => f64::from_le_bytes(b.try_into().unwrap()) as f32,
                    (_, 1) => (b[0] as f32 - 128.0) / 128.0,
                    (_, 2) => i16::from_le_bytes([b[0], b[1]]) as f32 / 32768.0,
                    (_, 3) => (i32::from_le_bytes([0, b[0], b[1], b[2]]) >> 8) as f32 / 8_388_608.0,
                    (_, 4) => i32::from_le_bytes(b.try_into().unwrap()) as f32 / 2_147_483_648.0,
                    _ => 0.0,
                };
            }
            out.push(sum / ch as f32);
        }
        out
    }
}

/// Streaming sample-rate conversion: an averaging low-pass sized to the
/// ratio, then linear interpolation. Plenty for speech recognition.
pub struct Resampler {
    step: f64,
    half: f64,
    buf: Vec<f32>,
    t: f64,
}

impl Resampler {
    pub fn new(from: u32, to: u32) -> Resampler {
        let step = from as f64 / to as f64;
        Resampler { step, half: (step / 2.0).max(0.5), buf: Vec::new(), t: 0.0 }
    }

    pub fn process(&mut self, input: &[f32]) -> Vec<f32> {
        if (self.step - 1.0).abs() < 1e-9 {
            return input.to_vec();
        }
        self.buf.extend_from_slice(input);
        let mut out = Vec::with_capacity((input.len() as f64 / self.step) as usize + 1);
        while self.t + self.half + 1.0 < self.buf.len() as f64 {
            let lo = (self.t - self.half).max(0.0);
            let hi = self.t + self.half;
            let (a, b) = (lo.floor() as usize, (hi.ceil() as usize).min(self.buf.len() - 1));
            let avg = self.buf[a..=b].iter().sum::<f32>() / (b - a + 1) as f32;
            out.push(avg);
            self.t += self.step;
        }
        let consumed = ((self.t - self.half).floor() as isize - 1).max(0) as usize;
        let consumed = consumed.min(self.buf.len());
        self.buf.drain(..consumed);
        self.t -= consumed as f64;
        out
    }
}

/// Resamples a whole clip in one go.
pub fn resample(samples: &[f32], from: u32, to: u32) -> Vec<f32> {
    let mut r = Resampler::new(from, to);
    let mut out = r.process(samples);
    // Flush the tail with silence so the end isn't cut off.
    out.extend(r.process(&vec![0.0; (from / to + 2) as usize * 4]));
    out.truncate((samples.len() as u64 * to as u64 / from as u64) as usize);
    out
}

// ---------- playback ----------

struct Clip {
    rate: u32,
    samples: Vec<i16>,
    generation: u64,
}

/// Plays clips one after another on the default speakers. `stop` cuts off
/// the current clip and drops everything queued.
#[derive(Clone)]
pub struct Player {
    tx: std::sync::mpsc::Sender<Clip>,
    generation: Arc<AtomicU64>,
    pending: Arc<AtomicUsize>,
    busy: Arc<tokio::sync::watch::Sender<bool>>,
}

impl Default for Player {
    fn default() -> Self {
        Self::new()
    }
}

impl Player {
    pub fn new() -> Player {
        let (tx, rx) = std::sync::mpsc::channel::<Clip>();
        let generation = Arc::new(AtomicU64::new(0));
        let pending = Arc::new(AtomicUsize::new(0));
        let (busy, _) = tokio::sync::watch::channel(false);
        let busy = Arc::new(busy);
        let (g, p, b) = (generation.clone(), pending.clone(), busy.clone());
        std::thread::Builder::new()
            .name("audio-playback".into())
            .spawn(move || imp::playback_thread(rx, g, p, b))
            .expect("playback thread");
        Player { tx, generation, pending, busy }
    }

    pub fn play(&self, rate: u32, samples: Vec<i16>) {
        if samples.is_empty() {
            return;
        }
        self.pending.fetch_add(1, Ordering::SeqCst);
        self.busy.send_replace(true);
        let generation = self.generation.load(Ordering::SeqCst);
        if self.tx.send(Clip { rate, samples, generation }).is_err() {
            self.pending.fetch_sub(1, Ordering::SeqCst);
        }
    }

    pub fn stop(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    pub fn busy(&self) -> bool {
        self.pending.load(Ordering::SeqCst) > 0
    }

}

fn finish_clip(pending: &AtomicUsize, busy: &tokio::sync::watch::Sender<bool>) {
    if pending.fetch_sub(1, Ordering::SeqCst) == 1 {
        busy.send_replace(false);
    }
}

#[cfg(windows)]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
    use std::sync::{Arc, Mutex};
    use std::time::{Duration, Instant};

    use windows::core::{Interface, HSTRING};
    use windows::Win32::Devices::FunctionDiscovery::PKEY_Device_FriendlyName;
    use windows::Win32::Media::Audio::*;
    use windows::Win32::System::Com::StructuredStorage::PropVariantToStringAlloc;
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL, COINIT_MULTITHREADED, STGM_READ,
    };

    use super::{finish_clip, Clip, Device, Flow, Format, Resampler, Sink, Source, RATE};

    const WAVE_FORMAT_IEEE_FLOAT: u16 = 3;
    const WAVE_FORMAT_EXTENSIBLE: u16 = 0xFFFE;
    /// 200 ms of buffering, in 100 ns units.
    const BUFFER_HNS: i64 = 2_000_000;
    const E_ACCESSDENIED: i32 = 0x8007_0005u32 as i32;
    const AUDCLNT_E_DEVICE_INVALIDATED: i32 = 0x8889_0004u32 as i32;

    struct Com(bool);
    impl Com {
        fn init() -> Com {
            // SAFETY: per-thread COM setup, undone in Drop when it succeeded.
            Com(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
        }
    }
    impl Drop for Com {
        fn drop(&mut self) {
            if self.0 {
                // SAFETY: balances the successful CoInitializeEx above.
                unsafe { CoUninitialize() };
            }
        }
    }

    fn enumerator() -> windows::core::Result<IMMDeviceEnumerator> {
        // SAFETY: COM is initialized on this thread by the caller.
        unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL) }
    }

    fn data_flow(flow: Flow) -> EDataFlow {
        match flow {
            Flow::Input => eCapture,
            Flow::Output => eRender,
        }
    }

    fn device_id(d: &IMMDevice) -> String {
        // SAFETY: GetId returns a CoTaskMemAlloc'd string that we free.
        unsafe {
            match d.GetId() {
                Ok(p) => {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                }
                Err(_) => String::new(),
            }
        }
    }

    fn device_name(d: &IMMDevice) -> String {
        // SAFETY: the property value is converted to an owned string and freed.
        unsafe {
            let Ok(store) = d.OpenPropertyStore(STGM_READ) else { return String::new() };
            let Ok(value) = store.GetValue(&PKEY_Device_FriendlyName) else { return String::new() };
            match PropVariantToStringAlloc(&value) {
                Ok(p) => {
                    let s = p.to_string().unwrap_or_default();
                    CoTaskMemFree(Some(p.0 as *const _));
                    s
                }
                Err(_) => String::new(),
            }
        }
    }

    pub fn devices(flow: Flow) -> Result<Vec<Device>, String> {
        let _com = Com::init();
        let en = enumerator().map_err(|e| e.to_string())?;
        // SAFETY: plain COM calls on live interfaces.
        unsafe {
            let default = en.GetDefaultAudioEndpoint(data_flow(flow), eConsole).ok().map(|d| device_id(&d));
            let list = en.EnumAudioEndpoints(data_flow(flow), DEVICE_STATE_ACTIVE).map_err(|e| e.to_string())?;
            let mut out = Vec::new();
            for i in 0..list.GetCount().map_err(|e| e.to_string())? {
                let Ok(d) = list.Item(i) else { continue };
                let id = device_id(&d);
                out.push(Device { default: default.as_deref() == Some(id.as_str()), name: device_name(&d), id });
            }
            Ok(out)
        }
    }

    /// The chosen device, or the default one if it is gone or none was chosen.
    fn open_device(en: &IMMDeviceEnumerator, flow: EDataFlow, id: Option<&str>, role: ERole) -> windows::core::Result<IMMDevice> {
        // SAFETY: plain COM calls on a live enumerator.
        unsafe {
            if let Some(id) = id.filter(|s| !s.is_empty()) {
                if let Ok(d) = en.GetDevice(&HSTRING::from(id)) {
                    return Ok(d);
                }
            }
            en.GetDefaultAudioEndpoint(flow, role)
        }
    }

    fn friendly(e: &windows::core::Error, mic: bool) -> String {
        match e.code().0 {
            E_ACCESSDENIED if mic => "Windows is blocking microphone access. Open Settings › Privacy & security › Microphone and turn on \
                                      “Let desktop apps access your microphone”."
                .into(),
            AUDCLNT_E_DEVICE_INVALIDATED => "The audio device was disconnected.".into(),
            _ if e.code().0 == 0x8007_0490u32 as i32 => {
                if mic { "No microphone was found.".into() } else { "No speakers or headphones were found.".into() }
            }
            _ => format!("The audio device couldn't be used: {}", e.message()),
        }
    }

    unsafe fn format_of(wf: *const WAVEFORMATEX) -> Format {
        let w = &*wf;
        let mut float = w.wFormatTag == WAVE_FORMAT_IEEE_FLOAT;
        if w.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
            let ext = &*(wf as *const WAVEFORMATEXTENSIBLE);
            // KSDATAFORMAT_SUBTYPE_IEEE_FLOAT is 00000003-0000-0010-8000-00aa00389b71.
            float = ext.SubFormat.data1 == 3;
        }
        Format { channels: w.nChannels, rate: w.nSamplesPerSec, bits: w.wBitsPerSample, float }
    }

    struct Stream {
        client: IAudioClient,
        capture: IAudioCaptureClient,
        /// Set when Windows couldn't convert for us and we convert ourselves.
        convert: Option<(Format, Resampler)>,
    }

    unsafe fn open_capture(source: &Source) -> windows::core::Result<Stream> {
        let en = enumerator()?;
        let (flow, id, loopback, voice) = match source {
            Source::Mic { device, voice } => (eCapture, device.as_deref(), false, *voice),
            Source::System { device } => (eRender, device.as_deref(), true, false),
        };
        let role = if voice { eCommunications } else { eConsole };
        let device = open_device(&en, flow, id, role)?;
        let base_flags = if loopback { AUDCLNT_STREAMFLAGS_LOOPBACK } else { 0 };

        let make_client = || -> windows::core::Result<IAudioClient> {
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            if voice {
                if let Ok(c2) = client.cast::<IAudioClient2>() {
                    let props = AudioClientProperties {
                        cbSize: std::mem::size_of::<AudioClientProperties>() as u32,
                        bIsOffload: false.into(),
                        eCategory: AudioCategory_Communications,
                        Options: AUDCLNT_STREAMOPTIONS_NONE,
                    };
                    let _ = c2.SetClientProperties(&props);
                }
            }
            Ok(client)
        };

        // Ask Windows to deliver 16 kHz mono floats directly.
        let want = WAVEFORMATEX {
            wFormatTag: WAVE_FORMAT_IEEE_FLOAT,
            nChannels: 1,
            nSamplesPerSec: RATE,
            nAvgBytesPerSec: RATE * 4,
            nBlockAlign: 4,
            wBitsPerSample: 32,
            cbSize: 0,
        };
        let client = make_client()?;
        let flags = base_flags | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY;
        let (client, convert) = match client.Initialize(AUDCLNT_SHAREMODE_SHARED, flags, BUFFER_HNS, 0, &want, None) {
            Ok(()) => (client, None),
            Err(e) if e.code().0 == E_ACCESSDENIED => return Err(e),
            Err(_) => {
                // Use the device's own format and convert here instead.
                let client = make_client()?;
                let mix = client.GetMixFormat()?;
                let fmt = format_of(mix);
                let r = client.Initialize(AUDCLNT_SHAREMODE_SHARED, base_flags, BUFFER_HNS, 0, mix, None);
                CoTaskMemFree(Some(mix as *const _));
                r?;
                (client, Some((fmt, Resampler::new(fmt.rate, RATE))))
            }
        };
        let capture: IAudioCaptureClient = client.GetService()?;
        // Don't turn other apps down while we listen.
        if let Ok(ctl) = client.GetService::<IAudioSessionControl>() {
            if let Ok(ctl2) = ctl.cast::<IAudioSessionControl2>() {
                let _ = ctl2.SetDuckingPreference(true);
            }
        }
        client.Start()?;
        Ok(Stream { client, capture, convert })
    }

    pub fn capture_thread(
        source: Source,
        sink: Sink,
        stop: Arc<AtomicBool>,
        level: Arc<AtomicU32>,
        error: Arc<Mutex<Option<String>>>,
        ready: std::sync::mpsc::SyncSender<Result<(), String>>,
    ) {
        let _com = Com::init();
        let mic = matches!(source, Source::Mic { .. });
        let loopback = !mic;
        // SAFETY: COM is initialized on this thread; the stream lives here.
        let mut stream = match unsafe { open_capture(&source) } {
            Ok(s) => s,
            Err(e) => {
                let _ = ready.send(Err(friendly(&e, mic)));
                return;
            }
        };
        let _ = ready.send(Ok(()));

        let started = Instant::now();
        let mut delivered: u64 = 0;
        let mut smoothed = 0.0f32;
        let result: windows::core::Result<()> = (|| {
            while !stop.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(10));
                let mut got = false;
                // SAFETY: buffers are released right after copying.
                unsafe {
                    while stream.capture.GetNextPacketSize()? > 0 {
                        let mut data = std::ptr::null_mut();
                        let (mut frames, mut flags) = (0u32, 0u32);
                        stream.capture.GetBuffer(&mut data, &mut frames, &mut flags, None, None)?;
                        let silent = flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0;
                        let samples = match &mut stream.convert {
                            None if silent => vec![0.0; frames as usize],
                            None => std::slice::from_raw_parts(data as *const f32, frames as usize).to_vec(),
                            Some((fmt, rs)) => {
                                let bytes = frames as usize * fmt.channels as usize * (fmt.bits as usize / 8);
                                let mono = if silent {
                                    vec![0.0; frames as usize]
                                } else {
                                    fmt.to_mono(std::slice::from_raw_parts(data, bytes))
                                };
                                rs.process(&mono)
                            }
                        };
                        stream.capture.ReleaseBuffer(frames)?;
                        delivered += samples.len() as u64;
                        let r = super::rms(&samples);
                        smoothed = if r > smoothed { r } else { smoothed * 0.85 + r * 0.15 };
                        level.store(smoothed.to_bits(), Ordering::Relaxed);
                        got = true;
                        if sink.send(samples).is_err() {
                            return Ok(());
                        }
                    }
                }
                // Loopback sends nothing while the PC is silent; fill the gap
                // so timestamps stay true to the clock.
                if loopback && !got {
                    let expected = (started.elapsed().as_secs_f64() * RATE as f64) as u64;
                    if expected > delivered + RATE as u64 / 5 {
                        let fill = (expected - delivered - RATE as u64 / 10) as usize;
                        delivered += fill as u64;
                        smoothed *= 0.85;
                        level.store(smoothed.to_bits(), Ordering::Relaxed);
                        if sink.send(vec![0.0; fill]).is_err() {
                            return Ok(());
                        }
                    }
                }
            }
            Ok(())
        })();
        if let Err(e) = result {
            *error.lock().unwrap() = Some(friendly(&e, mic));
        }
        // SAFETY: stopping a started client.
        unsafe {
            let _ = stream.client.Stop();
        }
    }

    pub fn playback_thread(
        rx: std::sync::mpsc::Receiver<Clip>,
        generation: Arc<AtomicU64>,
        pending: Arc<AtomicUsize>,
        busy: Arc<tokio::sync::watch::Sender<bool>>,
    ) {
        let _com = Com::init();
        let mut next: Option<Clip> = None;
        loop {
            let clip = match next.take() {
                Some(c) => c,
                None => match rx.recv() {
                    Ok(c) => c,
                    Err(_) => return,
                },
            };
            if clip.generation != generation.load(Ordering::SeqCst) {
                finish_clip(&pending, &busy);
                continue;
            }
            // SAFETY: COM is initialized on this thread.
            let r = unsafe { play_run(clip, &rx, &generation, &pending, &busy, &mut next) };
            if let Err(e) = r {
                eprintln!("playback failed: {e}");
            }
        }
    }

    /// Plays a clip, then keeps the stream open for queued clips at the same
    /// rate so sentences follow each other without gaps.
    unsafe fn play_run(
        first: Clip,
        rx: &std::sync::mpsc::Receiver<Clip>,
        generation: &AtomicU64,
        pending: &AtomicUsize,
        busy: &tokio::sync::watch::Sender<bool>,
        next: &mut Option<Clip>,
    ) -> windows::core::Result<()> {
        let rate = first.rate;
        let opened = (|| -> windows::core::Result<(IAudioClient, IAudioRenderClient, u32)> {
            let en = enumerator()?;
            let device = open_device(&en, eRender, None, eConsole)?;
            let client: IAudioClient = device.Activate(CLSCTX_ALL, None)?;
            let fmt = WAVEFORMATEX {
                wFormatTag: 1,
                nChannels: 1,
                nSamplesPerSec: rate,
                nAvgBytesPerSec: rate * 2,
                nBlockAlign: 2,
                wBitsPerSample: 16,
                cbSize: 0,
            };
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_HNS,
                0,
                &fmt,
                None,
            )?;
            let render: IAudioRenderClient = client.GetService()?;
            let size = client.GetBufferSize()?;
            Ok((client, render, size))
        })();
        let (client, render, size) = match opened {
            Ok(v) => v,
            Err(e) => {
                finish_clip(pending, busy);
                return Err(e);
            }
        };
        let mut started = false;
        let mut clip = first;
        let result = (|| -> windows::core::Result<()> {
            loop {
                let mut at = 0usize;
                while at < clip.samples.len() {
                    if generation.load(Ordering::SeqCst) != clip.generation {
                        client.Stop()?;
                        client.Reset()?;
                        return Ok(());
                    }
                    let padding = client.GetCurrentPadding()?;
                    let n = ((size - padding) as usize).min(clip.samples.len() - at);
                    if n > 0 {
                        let buf = render.GetBuffer(n as u32)? as *mut i16;
                        std::ptr::copy_nonoverlapping(clip.samples[at..].as_ptr(), buf, n);
                        render.ReleaseBuffer(n as u32, 0)?;
                        at += n;
                        if !started {
                            client.Start()?;
                            started = true;
                        }
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                // Another clip at the same rate may already be waiting.
                let deadline = Instant::now() + Duration::from_millis(40);
                let mut follow = None;
                while Instant::now() < deadline {
                    if let Ok(c) = rx.try_recv() {
                        follow = Some(c);
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(5));
                }
                match follow {
                    Some(c) if c.rate == rate && c.generation == generation.load(Ordering::SeqCst) => {
                        finish_clip(pending, busy);
                        clip = c;
                    }
                    Some(c) => {
                        *next = Some(c);
                        break;
                    }
                    None => break,
                }
            }
            // Let what's buffered finish playing.
            while client.GetCurrentPadding()? > 0 && generation.load(Ordering::SeqCst) == clip.generation {
                std::thread::sleep(Duration::from_millis(10));
            }
            client.Stop()?;
            Ok(())
        })();
        // The clip in hand is done however the run ended.
        finish_clip(pending, busy);
        result
    }
}

#[cfg(not(windows))]
mod imp {
    use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize};
    use std::sync::{Arc, Mutex};

    use super::{finish_clip, Clip, Device, Flow, Sink, Source};

    pub fn devices(_flow: Flow) -> Result<Vec<Device>, String> {
        Err("Audio is only supported on Windows so far.".into())
    }

    pub fn capture_thread(
        _source: Source,
        _sink: Sink,
        _stop: Arc<AtomicBool>,
        _level: Arc<AtomicU32>,
        _error: Arc<Mutex<Option<String>>>,
        ready: std::sync::mpsc::SyncSender<Result<(), String>>,
    ) {
        let _ = ready.send(Err("Audio is only supported on Windows so far.".into()));
    }

    pub fn playback_thread(
        rx: std::sync::mpsc::Receiver<Clip>,
        _generation: Arc<AtomicU64>,
        pending: Arc<AtomicUsize>,
        busy: Arc<tokio::sync::watch::Sender<bool>>,
    ) {
        while rx.recv().is_ok() {
            finish_clip(&pending, &busy);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sine(freq: f32, rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize).map(|i| (i as f32 * freq * std::f32::consts::TAU / rate as f32).sin() * 0.5).collect()
    }

    #[test]
    fn wav_round_trip_keeps_samples_and_rate() {
        let s = sine(440.0, 16_000, 0.1);
        let pcm = wav_decode(&wav_encode(&s, 16_000)).unwrap();
        assert_eq!(pcm.rate, 16_000);
        assert_eq!(pcm.samples.len(), s.len());
        assert!(pcm.samples.iter().zip(&s).all(|(a, b)| (a - b).abs() < 1e-3));
    }

    #[test]
    fn stereo_float_wav_is_mixed_to_mono() {
        let mut data = Vec::new();
        for _ in 0..10 {
            data.extend_from_slice(&0.5f32.to_le_bytes());
            data.extend_from_slice(&(-0.1f32).to_le_bytes());
        }
        let f = Format { channels: 2, rate: 48_000, bits: 32, float: true };
        let mono = f.to_mono(&data);
        assert_eq!(mono.len(), 10);
        assert!((mono[0] - 0.2).abs() < 1e-6);
    }

    #[test]
    fn resampling_48k_to_16k_keeps_length_and_loudness() {
        let s = sine(300.0, 48_000, 1.0);
        let mut r = Resampler::new(48_000, 16_000);
        // In uneven chunks, as a device delivers them.
        let mut out = Vec::new();
        for chunk in s.chunks(441) {
            out.extend(r.process(chunk));
        }
        assert!((out.len() as i64 - 16_000).abs() < 10, "{}", out.len());
        let ratio = rms(&out[100..15_000]) / rms(&s);
        assert!((ratio - 1.0).abs() < 0.05, "{ratio}");
    }

    #[test]
    fn resample_whole_clip_has_the_expected_length() {
        let s = sine(200.0, 22_050, 0.5);
        let out = resample(&s, 22_050, 16_000);
        assert_eq!(out.len(), 8_000);
    }

    #[test]
    fn non_wav_bytes_are_refused() {
        assert!(wav_decode(b"hello world, not audio").is_err());
    }
}
