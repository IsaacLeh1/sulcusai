# SPDX-License-Identifier: AGPL-3.0-only
"""Exports Seed-VC's singing voice conversion (v1, 44.1 kHz, f0) to ONNX for
SulcusAI, and saves a reference run's in-between values for checking the
Rust side against.

Run from the seed-vc checkout (its modules and ./checkpoints):
    ..\\venv\\Scripts\\python.exe -E -s ..\\export\\export_svc.py <out dir> <source.wav> <reference.wav>

Pieces (all float32, batch 1):
  whisper_mel.onnx   16 kHz wave (1, 480000)          -> log-mel (1, 80, 3000)
  whisper.onnx       log-mel (1, 80, 3000)            -> content (1, 1500, 768)
  rmvpe.onnx         16 kHz wave (1, N)               -> salience (1, frames, 360)
  campplus.onnx      fbank (1, frames, 80), mean-normalized -> style (1, 192)
  mel44.onnx         44.1 kHz wave (1, N)             -> mel (1, 128, frames)
  regulator.onnx     content (1, L, 768) and f0 (1, L), both already resized
                     to L mel frames                  -> condition (1, L, 768)
  dit.onnx           x, prompt (1, 128, L), condition (1, L, 768),
                     style (1, 192), t (1), cfg (1)   -> velocity (1, 128, L)
  bigvgan.onnx       mel (1, 128, L)                  -> wave (1, 1, L * 512)
"""
import os
import sys
import types

import numpy as np
import soundfile as sf
import torch
import torch.nn as nn
import torch.nn.functional as F
import torchaudio

sys.path.insert(0, os.getcwd())
torch.set_grad_enabled(False)

OUT = sys.argv[1]
SOURCE = sys.argv[2]
REFERENCE = sys.argv[3]
os.makedirs(OUT, exist_ok=True)
OPSET = 17


def export(module, args, name, inputs, outputs, dynamic):
    path = os.path.join(OUT, name)
    torch.onnx.export(module, args, path, input_names=inputs, output_names=outputs, dynamic_axes=dynamic,
                      opset_version=OPSET, do_constant_folding=True, dynamo=False)
    print("exported", name, os.path.getsize(path) // (1 << 20), "MB")
    return path


def check(path, feeds, expected, label):
    import onnxruntime as ort
    s = ort.InferenceSession(path, providers=["CPUExecutionProvider"])
    got = s.run(None, {k: v.numpy() if torch.is_tensor(v) else v for k, v in feeds.items()})[0]
    exp = expected.numpy() if torch.is_tensor(expected) else expected
    err = np.abs(got - exp).max()
    rel = err / (np.abs(exp).max() + 1e-9)
    print(f"check {label}: max abs {err:.3g}, relative {rel:.3g}, shape {got.shape}")
    return got


def save(name, x):
    np.save(os.path.join(OUT, "ref", name + ".npy"), x.numpy() if torch.is_tensor(x) else np.asarray(x))


os.makedirs(os.path.join(OUT, "ref"), exist_ok=True)

import inference  # noqa: E402  (seed-vc's own loader)

args = types.SimpleNamespace(f0_condition=True, checkpoint=None, config=None, fp16=False)
model, semantic_fn, f0_fn, vocoder_fn, campplus_model, mel_fn, mel_fn_args = inference.load_models(args)
for k in model:
    model[k].float().cpu().eval()
rmvpe = f0_fn.__self__
campplus_model = campplus_model.float().cpu().eval()
bigvgan = vocoder_fn.float().cpu().eval()

from transformers import AutoFeatureExtractor, WhisperModel  # noqa: E402
whisper = WhisperModel.from_pretrained("openai/whisper-small", torch_dtype=torch.float32).eval()
whisper_fe = AutoFeatureExtractor.from_pretrained("openai/whisper-small")

SR = 44100
HOP = 512

# ---------- the reference run (one window, so prompt + source <= 30 s) ----------
import librosa  # noqa: E402
source = torch.tensor(librosa.load(SOURCE, sr=SR)[0]).unsqueeze(0)
ref = torch.tensor(librosa.load(REFERENCE, sr=SR)[0][: SR * 12]).unsqueeze(0)
source = source[:, -SR * 15:]
src16 = torchaudio.functional.resample(source, SR, 16000)
ref16 = torchaudio.functional.resample(ref, SR, 16000)
save("src44", source[0])
save("ref44", ref[0])
save("src16", src16[0])
save("ref16", ref16[0])


# ---------- whisper: features and encoder ----------
class WhisperMel(nn.Module):
    """transformers' WhisperFeatureExtractor (numpy) in torch: a 400-point
    periodic Hann STFT every 160 samples, power, Slaney mel, log10, the top
    8 dB kept, scaled. The wave comes padded or cut to 30 s."""

    def __init__(self, filters):
        super().__init__()
        self.register_buffer("filters", torch.tensor(filters, dtype=torch.float32))  # (201, 80)
        self.register_buffer("window", torch.hann_window(400, periodic=True))

    def forward(self, wave):
        spec = torch.stft(wave, 400, 160, window=self.window, center=True, pad_mode="reflect", return_complex=False)
        power = spec[..., 0] ** 2 + spec[..., 1] ** 2  # (1, 201, 3001)
        power = power[..., :-1]
        mel = torch.matmul(self.filters.transpose(0, 1), power)
        log = torch.log10(torch.clamp(mel, min=1e-10))
        log = torch.maximum(log, log.amax(dim=(1, 2), keepdim=True) - 8.0)
        return (log + 4.0) / 4.0


def pad30(w):
    out = torch.zeros(1, 480000)
    n = min(w.shape[-1], 480000)
    out[0, :n] = w[0, :n]
    return out


wmel = WhisperMel(whisper_fe.mel_filters).eval()
p = export(wmel, (pad30(src16),), "whisper_mel.onnx", ["wave"], ["mel"], None)
hf = whisper_fe([src16[0].numpy()], sampling_rate=16000, return_tensors="pt").input_features
check(p, {"wave": pad30(src16)}, hf, "whisper_mel vs transformers")


class Encoder(nn.Module):
    def __init__(self, enc):
        super().__init__()
        self.enc = enc

    def forward(self, mel):
        return self.enc(mel).last_hidden_state


enc = Encoder(whisper.encoder).eval()
p = export(enc, (hf,), "whisper.onnx", ["mel"], ["content"], None)
S_alt = semantic_fn(src16)
S_ori = semantic_fn(ref16)
full = enc(hf)
check(p, {"mel": hf}, full, "whisper encoder")
print("semantic_fn vs encoder (fp16 there):", float((S_alt - full[:, : S_alt.shape[1]]).abs().max()))
save("S_alt", full[:, : src16.shape[-1] // 320 + 1])
S_alt = full[:, : src16.shape[-1] // 320 + 1]
S_ori = enc(whisper_fe([ref16[0].numpy()], sampling_rate=16000, return_tensors="pt").input_features)[:, : ref16.shape[-1] // 320 + 1]
save("S_ori", S_ori)


# ---------- rmvpe ----------
class Rmvpe(nn.Module):
    """RMVPE's mel (128 bands, 1024-point Hann, hop 160, 30-8000 Hz, log) and
    its network; frames padded to a multiple of 32 and cut back."""

    def __init__(self, r):
        super().__init__()
        m = r.mel_extractor
        self.register_buffer("basis", m.mel_basis.float())
        self.register_buffer("window", torch.hann_window(m.win_length))
        self.n_fft, self.hop, self.clamp = m.n_fft, m.hop_length, m.clamp
        self.net = r.model

    def forward(self, wave):
        spec = torch.stft(wave, self.n_fft, self.hop, self.n_fft, window=self.window, center=True, return_complex=False)
        mag = torch.sqrt(spec[..., 0] ** 2 + spec[..., 1] ** 2)
        mel = torch.log(torch.clamp(torch.matmul(self.basis, mag), min=self.clamp))
        n = mel.shape[-1]
        pad = (32 - n % 32) % 32
        mel = F.pad(mel, (0, pad))
        return self.net(mel)[:, :n]


rm = Rmvpe(rmvpe).float().eval()
rmvpe.model.float()
p = export(rm, (src16,), "rmvpe.onnx", ["wave"], ["salience"], {"wave": {1: "n"}, "salience": {1: "frames"}})
sal = rm(src16)
orig = rmvpe.mel2hidden(rmvpe.mel_extractor(src16, center=True))
print("rmvpe rewrite vs its own:", float((sal - orig).abs().max()))
got = check(p, {"wave": src16}, sal, "rmvpe")
F0_alt = rmvpe.decode(sal.squeeze(0).numpy(), thred=0.03)
F0_ori = rmvpe.decode(rm(ref16).squeeze(0).numpy(), thred=0.03)
save("F0_alt", F0_alt)
save("F0_ori", F0_ori)
save("cents_mapping", rmvpe.cents_mapping)
print("F0 frames", len(F0_alt), "voiced median", float(np.median(F0_alt[F0_alt > 1])) if (F0_alt > 1).any() else 0)

# ---------- campplus ----------
feat2 = torchaudio.compliance.kaldi.fbank(ref16, num_mel_bins=80, dither=0, sample_frequency=16000)
feat2 = feat2 - feat2.mean(dim=0, keepdim=True)
save("fbank_ref", feat2)
style2 = campplus_model(feat2.unsqueeze(0))
save("style2", style2)
p = export(campplus_model, (feat2.unsqueeze(0),), "campplus.onnx", ["fbank"], ["style"], {"fbank": {1: "frames"}})
check(p, {"fbank": feat2.unsqueeze(0)}, style2, "campplus")


# ---------- 44.1 kHz mel ----------
class Mel44(nn.Module):
    """seed-vc's modules.audio.mel_spectrogram with a real-valued STFT: reflect
    padding of (n_fft - hop) / 2, no centering, magnitude, Slaney mel, log."""

    def __init__(self):
        super().__init__()
        from librosa.filters import mel as librosa_mel_fn
        a = mel_fn_args
        self.register_buffer("basis", torch.from_numpy(librosa_mel_fn(sr=a["sampling_rate"], n_fft=a["n_fft"], n_mels=a["num_mels"], fmin=a["fmin"], fmax=a["fmax"])).float())
        self.register_buffer("window", torch.hann_window(a["win_size"]))
        self.n_fft, self.hop = a["n_fft"], a["hop_size"]

    def forward(self, wave):
        pad = (self.n_fft - self.hop) // 2
        y = F.pad(wave.unsqueeze(1), (pad, pad), mode="reflect").squeeze(1)
        spec = torch.stft(y, self.n_fft, self.hop, self.n_fft, window=self.window, center=False, return_complex=False)
        mag = torch.sqrt(spec[..., 0] ** 2 + spec[..., 1] ** 2 + 1e-9)
        return torch.log(torch.clamp(torch.matmul(self.basis, mag), min=1e-5))


m44 = Mel44().eval()
mel = mel_fn(source)
mel2 = mel_fn(ref)
print("mel44 rewrite vs its own:", float((m44(ref) - mel2).abs().max()))
save("mel2", mel2)
p = export(m44, (source,), "mel44.onnx", ["wave"], ["mel"], {"wave": {1: "n"}, "mel": {2: "frames"}})
check(p, {"wave": ref}, mel2, "mel44 (other length)")


# ---------- length regulator ----------
def nearest(x, size):
    """F.interpolate(mode='nearest') along the last axis, as the Rust side does it."""
    n = x.shape[-1]
    scale = np.float32(n) / np.float32(size)
    idx = np.minimum(np.floor(np.arange(size, dtype=np.float32) * scale).astype(np.int64), n - 1)
    return x[..., idx]


class Regulator(nn.Module):
    def __init__(self, lr):
        super().__init__()
        self.lr = lr

    def forward(self, content, f0):
        from modules.length_regulator import f0_to_coarse
        x = self.lr.content_in_proj(content).transpose(1, 2)  # (1, 768, L)
        q = f0_to_coarse(f0, self.lr.n_f0_bins).clamp(0, self.lr.n_f0_bins - 1).long()
        x = x + self.lr.f0_embedding(q).transpose(1, 2)
        return self.lr.model(x).transpose(1, 2)


reg = Regulator(model.length_regulator).eval()
L_src = mel.shape[2]
L_ref = mel2.shape[2]

voiced_ori = F0_ori[F0_ori > 1]
voiced_alt = F0_alt[F0_alt > 1]
shifted = torch.from_numpy(F0_alt).float()[None]

cond_ref, _, _, _, _ = model.length_regulator(S_alt, ylens=torch.LongTensor([L_src]), n_quantizers=3, f0=shifted)
prompt_ref, _, _, _, _ = model.length_regulator(S_ori, ylens=torch.LongTensor([L_ref]), n_quantizers=3, f0=torch.from_numpy(F0_ori).float()[None])

c_in = nearest(S_alt.transpose(1, 2), L_src).transpose(1, 2)
f_in = nearest(shifted, L_src)
p = export(reg, (c_in, f_in), "regulator.onnx", ["content", "f0"], ["condition"], {"content": {1: "frames"}, "f0": {1: "frames"}, "condition": {1: "frames"}})
check(p, {"content": c_in, "f0": f_in}, cond_ref, "regulator vs seed-vc's (source)")
pc_in = nearest(S_ori.transpose(1, 2), L_ref).transpose(1, 2)
pf_in = nearest(torch.from_numpy(F0_ori).float()[None], L_ref)
check(p, {"content": pc_in, "f0": pf_in}, prompt_ref, "regulator vs seed-vc's (prompt)")
save("cond", cond_ref)
save("prompt_cond", prompt_ref)


# ---------- the diffusion transformer, with guidance ----------
class DitStep(nn.Module):
    def __init__(self, est):
        super().__init__()
        self.est = est

    def forward(self, x, prompt, cond, style, t, cfg):
        T = x.shape[2]
        lens = (torch.ones(1, dtype=torch.long) * T)
        d = self.est(torch.cat([x, x], 0), torch.cat([prompt, torch.zeros_like(prompt)], 0), lens, torch.cat([t, t], 0),
                     torch.cat([style, torch.zeros_like(style)], 0), torch.cat([cond, torch.zeros_like(cond)], 0))
        a, b = d.chunk(2, 0)
        return (1.0 + cfg) * a - cfg * b


est = model.cfm.estimator
est.setup_caches(max_batch_size=2, max_seq_length=8192)
# Numbers, not bools: the exporter can't add a bool to the lengths.
est.style_as_token = int(est.style_as_token)
est.time_as_token = int(est.time_as_token)
step = DitStep(est).eval()
cat = torch.cat([prompt_ref, cond_ref], 1)
L = cat.shape[1]
torch.manual_seed(7)
z = torch.randn(1, 128, L)
prompt_x = torch.zeros_like(z)
prompt_x[..., :L_ref] = mel2[..., :L_ref]
x0 = z.clone()
x0[..., :L_ref] = 0
t = torch.tensor([0.1])
cfg = torch.tensor([0.7])
p = export(step, (x0, prompt_x, cat, style2, t, cfg), "dit.onnx", ["x", "prompt", "condition", "style", "t", "cfg"], ["velocity"],
           {"x": {2: "frames"}, "prompt": {2: "frames"}, "condition": {1: "frames"}, "velocity": {2: "frames"}})
check(p, {"x": x0, "prompt": prompt_x, "condition": cat, "style": style2, "t": t, "cfg": cfg}, step(x0, prompt_x, cat, style2, t, cfg), "dit step")

# The whole sampling, seed-vc's way, from the same noise.
STEPS = 10
save("z", z)
torch.manual_seed(7)
vc = model.cfm.inference(cat, torch.LongTensor([L]), mel2, style2, None, STEPS, inference_cfg_rate=0.7)
save("vc_mel", vc)
vc_target = vc[:, :, L_ref:]

# ---------- bigvgan ----------
# Its anti-aliasing filters are spread over the channels at run time
# (filter.expand(C, ...)), a kernel the exporter can't size: give each its
# channels' filters as a fixed buffer, learned from one run.
from modules.bigvgan.alias_free_activation.torch.resample import UpSample1d  # noqa: E402
from modules.bigvgan.alias_free_activation.torch.filter import LowPassFilter1d  # noqa: E402

wave_orig = bigvgan(vc_target)
channels = {}
hooks = [m.register_forward_pre_hook(lambda mod, inp: channels.__setitem__(mod, inp[0].shape[1]))
         for m in bigvgan.modules() if isinstance(m, (UpSample1d, LowPassFilter1d))]
bigvgan(vc_target[..., :32])
for h in hooks:
    h.remove()


def up_forward(self, x):
    x = F.pad(x, (self.pad, self.pad), mode="replicate")
    x = self.ratio * F.conv_transpose1d(x, self.full, stride=self.stride, groups=self.full.shape[0])
    return x[..., self.pad_left: -self.pad_right]


def low_forward(self, x):
    if self.padding:
        x = F.pad(x, (self.pad_left, self.pad_right), mode=self.padding_mode)
    return F.conv1d(x, self.full, stride=self.stride, groups=self.full.shape[0])


for m, c in channels.items():
    m.register_buffer("full", m.filter.expand(c, -1, -1).contiguous())
    m.forward = types.MethodType(up_forward if isinstance(m, UpSample1d) else low_forward, m)

p = export(bigvgan, (vc_target,), "bigvgan.onnx", ["mel"], ["wave"], {"mel": {2: "frames"}, "wave": {2: "n"}})
wave = bigvgan(vc_target)
print("bigvgan fixed filters vs its own:", float((wave - wave_orig).abs().max()))
check(p, {"mel": vc_target}, wave, "bigvgan")
save("wave", wave[0, 0])
sf.write(os.path.join(OUT, "ref", "converted.wav"), wave[0, 0].numpy(), SR)
print("done: steps", STEPS, "L_ref", L_ref, "L_src", L_src)
