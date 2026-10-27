# SPDX-License-Identifier: AGPL-3.0-only
"""SulcusAI's runner for Heretic (catalog `heretic`): makes an uncensored
copy of a model without anyone at the keyboard.

Heretic finds the direction in a model's thinking that makes it refuse
(from prompts it refuses and prompts it answers), then tries many ways of
taking that direction out of the model's weights. Each try is scored on how
many test requests it still refuses and how far its answers to ordinary
requests drift from the original's (KL divergence). Heretic asks at the end
which try to keep and what to do with it; this runner answers instead:

* an interrupted run carries on from the tries it already made;
* the try kept refuses the fewest test requests among those that drift no
  more than `--max-kl` (Heretic's own note: above 0.5 the model is usually
  noticeably damaged), else the one that drifts least;
* it's saved whole (the change merged into the weights) to `--out`, with
  the original's settings and tokenizer files beside it.

Lines starting with "SULCUS " tell the app how far it is:
  SULCUS stage <prompts|batch|directions|saving>
  SULCUS base <refusals> <of>          the original, before any change
  SULCUS trial <n> <of>                a try starting
  SULCUS tried <refusals> <kl>         that try's result
  SULCUS chose <trial> <refusals> <kl> the try kept
  SULCUS saved

    uv run --frozen --no-sync python uncensor.py --model <folder> --out <folder>
        --work <folder> --prompts <folder> [--trials 200] [--quantize] [--max-kl 0.5]
"""
import argparse
import os
import re
import shutil
import sys


def say(*words):
    print("SULCUS", *words, flush=True)


def prompt_files(prompts, work):
    """Heretic's prompt sets (Hugging Face parquet files the app downloaded
    and checked) as text files, one prompt to a line."""
    import pyarrow.parquet as pq

    out = {}
    for name in ["harmless-train", "harmless-test", "harmful-train", "harmful-test"]:
        rows = pq.read_table(os.path.join(prompts, f"{name}.parquet"), columns=["text"]).column("text").to_pylist()
        path = os.path.join(work, f"{name}.txt")
        with open(path, "w", encoding="utf-8") as f:
            for r in rows:
                line = " ".join(str(r).split())
                if line:
                    f.write(line + "\n")
        out[name] = path.replace("\\", "/")
    return out


def write_config(a, files):
    import tomli_w

    config = {
        "model": a.model.replace("\\", "/"),
        "quantization": "bnb_4bit" if a.quantize else "none",
        "n_trials": a.trials,
        # Heretic starts with a third of its tries at random, then aims.
        "n_startup_trials": max(1, min(60, a.trials * 3 // 10)),
        "study_checkpoint_dir": "checkpoints",
        "export_strategy": "merge",
        "good_prompts": {"dataset": files["harmless-train"], "split": "[:400]"},
        "bad_prompts": {"dataset": files["harmful-train"], "split": "[:400]"},
        "good_evaluation_prompts": {"dataset": files["harmless-test"], "split": "[:100]"},
        "bad_evaluation_prompts": {"dataset": files["harmful-test"], "split": "[:100]"},
    }
    with open(os.path.join(a.work, "config.toml"), "wb") as f:
        tomli_w.dump(config, f)


def weights(name):
    return name.endswith(".safetensors") or name.endswith(".safetensors.index.json")


def keep_original_files(model, out):
    """The saved model with the original's settings and tokenizer files: the
    weights are all that changed, and llama.cpp's converter is made for the
    files as the model's makers published them (Transformers 5 writes its
    own layout of them)."""
    for n in os.listdir(out):
        if not weights(n) and os.path.isfile(os.path.join(out, n)):
            os.remove(os.path.join(out, n))
    for n in os.listdir(model):
        if not weights(n) and os.path.isfile(os.path.join(model, n)):
            shutil.copyfile(os.path.join(model, n), os.path.join(out, n))


def pick(trials, max_kl):
    """The try to keep, from Heretic's best ones (fewest refusals first,
    each drifting less than the one before)."""
    for t in trials:
        if t.user_attrs["kl_divergence"] <= max_kl:
            return t
    return trials[-1]


def main():
    p = argparse.ArgumentParser()
    p.add_argument("--model", required=True)
    p.add_argument("--out", required=True)
    p.add_argument("--work", required=True)
    p.add_argument("--prompts", required=True)
    p.add_argument("--trials", type=int, default=200)
    p.add_argument("--quantize", action="store_true")
    p.add_argument("--max-kl", type=float, default=0.5)
    a = p.parse_args()

    os.makedirs(a.work, exist_ok=True)
    write_config(a, prompt_files(a.prompts, a.work))
    # Heretic reads config.toml from where it runs, and its own command line
    # (none: everything is in the file).
    os.chdir(a.work)
    sys.argv = ["heretic"]

    import heretic.main as hm
    from questionary import Choice

    saved = {"done": False}
    shown = hm.print
    stages = [
        ("Loading good prompts", "prompts"),
        ("Determining optimal batch size", "batch"),
        ("Calculating per-layer refusal directions", "directions"),
        ("Saving merged model", "saving"),
    ]

    def print_(*args, **kwargs):
        shown(*args, **kwargs)
        text = str(args[0]) if args else ""
        for start, stage in stages:
            if text.startswith(start):
                say("stage", stage)
        if text.startswith("Running trial"):
            n = re.findall(r"\d+", text)
            if len(n) >= 2:
                say("trial", n[0], n[1])
        if text.startswith("Model saved to"):
            saved["done"] = True
            say("saved")

    hm.print = print_

    score = hm.Evaluator.get_score

    def get_score(self):
        result = score(self)
        _, kl, refusals = result
        say("tried", refusals, f"{kl:.4f}")
        return result

    hm.Evaluator.get_score = get_score

    start = hm.Evaluator.__init__

    def init(self, settings, model):
        start(self, settings, model)
        say("base", self.base_refusals, len(self.bad_prompts))

    hm.Evaluator.__init__ = init

    def select(message, choices):
        values = [c.value if isinstance(c, Choice) else c for c in choices]
        if message.startswith("How would you like to proceed"):
            # A run that was stopped (or finished) carries on from its tries.
            return "continue"
        if message.startswith("Which trial"):
            if saved["done"]:
                return ""
            t = pick([v for v in values if not isinstance(v, str)], a.max_kl)
            say("chose", t.user_attrs["index"], t.user_attrs["refusals"], f"{t.user_attrs['kl_divergence']:.4f}")
            return t
        if message.startswith("What do you want to do"):
            return "" if saved["done"] else "Save the model to a local folder"
        raise SystemExit(f"Heretic asked something SulcusAI can't answer: {message}")

    def path(message):
        return a.out

    def text(message, **kwargs):
        raise SystemExit(f"Heretic asked something SulcusAI can't answer: {message}")

    hm.prompt_select = select
    hm.prompt_path = path
    hm.prompt_text = text
    hm.prompt_password = text

    hm.run()
    if not saved["done"]:
        raise SystemExit("Heretic finished without saving the model.")
    keep_original_files(a.model, a.out)


if __name__ == "__main__":
    main()
