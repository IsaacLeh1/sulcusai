# SPDX-License-Identifier: AGPL-3.0-only
# Ditto (Ant Group, Apache-2.0): a picture's face moves to a voice, about
# 25 frames a second. SulcusAI runs this from Ditto's folder with uv; the
# video comes out silent and the app puts the sound under it with FFmpeg.
#
# Ditto is used as published, with three changes made here as it loads:
#   - its settings are read with an unpickler that only makes numpy arrays
#     (the published file is a pickle, which could otherwise run code);
#   - the face is blended back with numpy (its copy is Cython, which needs
#     a C compiler on the PC);
#   - a picture with no face says so ("SULCUS noface") instead of failing
#     somewhere later.
# With --processor it runs on the processor (PCs without an NVIDIA card),
# from the packages in cpu/.
# Lines starting with "SULCUS " are read by the app for progress.
import argparse
import math
import os
import pickle
import shutil
import sys
import time
import types

import numpy as np

p = argparse.ArgumentParser()
p.add_argument("--models", required=True, help="the ditto_pytorch folder")
p.add_argument("--cfg", required=True, help="v0.4_hubert_cfg_pytorch.pkl")
p.add_argument("--image", required=True)
p.add_argument("--audio", required=True, help="the voice, any rate (read at 16 kHz)")
p.add_argument("--out", required=True, help="the silent .mp4")
p.add_argument("--steps", type=int, default=50, help="sampling steps of the motion model")
p.add_argument("--max-size", type=int, default=1920, help="the picture's longest side")
p.add_argument("--processor", action="store_true", help="run on the processor")
a = p.parse_args()

# Ditto's settings put every part on "cuda". DirectML on AMD and Intel
# cards was tried: it has no 3D convolutions (the warp, every frame), and on
# an Intel UHD 770 the mix was slower than the processor alone.
device = None
if a.processor:
    import contextlib

    import torch

    device = "cpu"
    # Its half-precision autocast is for CUDA; the processor runs full precision.
    torch.autocast = lambda *args, **kwargs: contextlib.nullcontext()

here = os.path.dirname(os.path.abspath(__file__))
sys.path.insert(0, here)
os.chdir(here)

blend = types.ModuleType("core.utils.blend")


def blend_images_cy(mask_warped, frame_warped, frame_rgb, result):
    m = mask_warped[..., None].astype(np.float32)
    out = m * frame_warped + (1.0 - m) * frame_rgb.astype(np.float32)
    np.clip(out, 0, 255, out=out)
    result[...] = out.astype(np.uint8)


blend.blend_images_cy = blend_images_cy
sys.modules["core.utils.blend"] = blend


class NumpyOnly(pickle.Unpickler):
    ALLOWED = {
        ("numpy._core.multiarray", "_reconstruct"): lambda: np._core.multiarray._reconstruct,
        ("numpy.core.multiarray", "_reconstruct"): lambda: np._core.multiarray._reconstruct,
        ("numpy", "ndarray"): lambda: np.ndarray,
        ("numpy", "dtype"): lambda: np.dtype,
    }

    def find_class(self, module, name):
        make = self.ALLOWED.get((module, name))
        if make is None:
            raise pickle.UnpicklingError(f"not allowed in the settings: {module}.{name}")
        return make()


def on_device(v):
    if isinstance(v, dict):
        return {k: device if k == "device" and x == "cuda" else on_device(x) for k, x in v.items()}
    if isinstance(v, list):
        return [on_device(x) for x in v]
    return v


def load_pkl(path):
    with open(path, "rb") as f:
        loaded = NumpyOnly(f).load()
    return on_device(loaded) if device else loaded


import core.atomic_components.cfg as cfg  # noqa: E402

cfg.load_pkl = load_pkl

from core.atomic_components.source2info import Source2Info  # noqa: E402

_crop = Source2Info._crop


def crop(self, img, last_lmk=None, **kwargs):
    found = _crop(self, img, last_lmk, **kwargs)
    if found is None:
        print("SULCUS noface", flush=True)
        sys.exit(3)
    return found


Source2Info._crop = crop

if device:
    from core.models.modules.LMDM import LMDM

    _lmdm_setup = LMDM.setup

    # The motion model keeps "cuda" for its noise whatever card its weights are on.
    def lmdm_setup(self, *args, **kwargs):
        self.device = next(self.parameters()).device
        return _lmdm_setup(self, *args, **kwargs)

    LMDM.setup = lmdm_setup

import librosa  # noqa: E402
from stream_pipeline_offline import StreamSDK  # noqa: E402

t0 = time.time()
sdk = StreamSDK(a.cfg, a.models)
print(f"SULCUS loaded {time.time() - t0:.1f}", flush=True)
sdk.setup(a.image, a.out, sampling_timesteps=a.steps, max_size=a.max_size)
audio, _ = librosa.core.load(a.audio, sr=16000)
frames = math.ceil(len(audio) / 16000 * 25)
print(f"SULCUS frames {frames}", flush=True)


class Counted:
    def __init__(self, writer):
        self.writer = writer
        self.n = 0

    def __call__(self, img, fmt="bgr"):
        self.writer(img, fmt)
        self.n += 1
        if self.n % 10 == 0:
            print(f"SULCUS frame {self.n}", flush=True)

    def close(self):
        self.writer.close()


sdk.writer = Counted(sdk.writer)
sdk.setup_Nd(N_d=frames)
sdk.audio2motion_queue.put(sdk.wav2feat.wav2feat(audio))
sdk.close()
shutil.move(sdk.tmp_output_path, a.out)
print(f"SULCUS done {time.time() - t0:.1f}", flush=True)
# MediaPipe's landmarker fails in its own clean-up at exit; nothing is left to do.
os._exit(0)
