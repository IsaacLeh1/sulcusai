# SPDX-License-Identifier: AGPL-3.0-only
"""Trains a LoRA for an SDXL picture model on a few pictures, for SulcusAI.

Reads pictures and their captions (NN.png with NN.txt) from --data, teaches
the U-Net's attention layers, and writes the LoRA in the kohya layout that
stable-diffusion.cpp loads. Prints "SULCUS step <done> <of> <loss>" as it
goes. Runs in the music trainer's environment (PyTorch, diffusers, PEFT)."""
import argparse
import glob
import os
import random

import numpy as np
import torch
import torch.nn.functional as F
from diffusers import DDPMScheduler, StableDiffusionXLPipeline
from diffusers.utils.state_dict_utils import convert_all_state_dict_to_peft, convert_state_dict_to_diffusers, convert_state_dict_to_kohya
from peft import LoraConfig
from peft.utils import get_peft_model_state_dict
from PIL import Image
from safetensors.torch import save_file


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--base", required=True)
    p.add_argument("--config", required=True)
    p.add_argument("--data", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--steps", type=int, default=1000)
    p.add_argument("--rank", type=int, default=16)
    p.add_argument("--lr", type=float, default=1e-4)
    p.add_argument("--seed", type=int, default=7)
    a = p.parse_args()
    random.seed(a.seed)
    torch.manual_seed(a.seed)
    dev = "cuda"

    pipe = StableDiffusionXLPipeline.from_single_file(a.base, config=a.config, local_files_only=True, torch_dtype=torch.float16)
    files = sorted(glob.glob(os.path.join(a.data, "*.png")))
    if not files:
        raise SystemExit("SULCUS error no pictures")

    # Captions to text embeddings, then the text encoders go.
    pipe.text_encoder.to(dev)
    pipe.text_encoder_2.to(dev)
    texts = []
    with torch.no_grad():
        for f in files:
            caption = open(os.path.splitext(f)[0] + ".txt", encoding="utf-8").read().strip()
            emb, _, pooled, _ = pipe.encode_prompt(prompt=caption, device=dev, num_images_per_prompt=1, do_classifier_free_guidance=False)
            texts.append((emb.cpu(), pooled.cpu()))
    pipe.text_encoder.to("cpu")
    pipe.text_encoder_2.to("cpu")

    # Pictures to latents, by the VAE in full precision (SDXL's VAE overflows in half).
    vae = pipe.vae.to(dev, torch.float32)
    latents = []
    with torch.no_grad():
        for f in files:
            img = Image.open(f).convert("RGB")
            x = torch.from_numpy(np.asarray(img).astype(np.float32) / 127.5 - 1.0).permute(2, 0, 1)[None].to(dev)
            z = vae.encode(x).latent_dist.sample() * vae.config.scaling_factor
            latents.append((z.cpu(), img.height, img.width))
    vae.to("cpu")
    torch.cuda.empty_cache()

    unet = pipe.unet.to(dev)
    unet.requires_grad_(False)
    unet.enable_gradient_checkpointing()
    unet.add_adapter(LoraConfig(r=a.rank, lora_alpha=a.rank, init_lora_weights="gaussian", target_modules=["to_k", "to_q", "to_v", "to_out.0"]))
    params = [q for q in unet.parameters() if q.requires_grad]
    for q in params:
        q.data = q.data.float()
    opt = torch.optim.AdamW(params, lr=a.lr, weight_decay=1e-2)
    scaler = torch.amp.GradScaler("cuda")
    sched = DDPMScheduler.from_pretrained(a.config, subfolder="scheduler", local_files_only=True)
    order = []
    for step in range(1, a.steps + 1):
        if not order:
            order = list(range(len(files)))
            random.shuffle(order)
        i = order.pop()
        z, h, w = latents[i]
        z = z.to(dev, torch.float16)
        emb, pooled = (t.to(dev, torch.float16) for t in texts[i])
        # A little offset noise keeps pictures from all coming out mid-grey.
        noise = torch.randn_like(z) + 0.0357 * torch.randn(z.shape[0], z.shape[1], 1, 1, device=dev, dtype=z.dtype)
        t = torch.randint(0, sched.config.num_train_timesteps, (1,), device=dev).long()
        noisy = sched.add_noise(z, noise, t)
        time_ids = torch.tensor([[h, w, 0, 0, h, w]], device=dev, dtype=torch.float16)
        with torch.autocast("cuda", dtype=torch.float16):
            pred = unet(noisy, t, encoder_hidden_states=emb, added_cond_kwargs={"text_embeds": pooled, "time_ids": time_ids}).sample
        loss = F.mse_loss(pred.float(), noise.float())
        opt.zero_grad(set_to_none=True)
        scaler.scale(loss).backward()
        scaler.unscale_(opt)
        torch.nn.utils.clip_grad_norm_(params, 1.0)
        scaler.step(opt)
        scaler.update()
        if step % 10 == 0 or step == a.steps:
            print(f"SULCUS step {step} {a.steps} {loss.item():.4f}", flush=True)

    lora = convert_state_dict_to_diffusers(get_peft_model_state_dict(unet))
    lora = convert_all_state_dict_to_peft({f"unet.{k}": v for k, v in lora.items()})
    kohya = {k: v.to(torch.float16).contiguous() for k, v in convert_state_dict_to_kohya(lora).items()}
    save_file(kohya, a.out)
    print("SULCUS done", flush=True)


if __name__ == "__main__":
    main()
