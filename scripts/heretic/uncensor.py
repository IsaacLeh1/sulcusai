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
* the try kept drifts least among those that refuse at most two more test
  requests than the fewest, of those drifting no more than `--max-kl`
  (Heretic's own note: above 0.5 the model is usually noticeably damaged);
  with none that close, the one that drifts least;
* it's saved whole (the change merged into the weights) to `--out`, with
  the original's settings and tokenizer files beside it, and the runner ends
  there (Heretic would load the whole model again just to say it's saved);
* a run started again with other settings (more tries, 4 bits) starts over:
  Heretic carries on with the settings stored with its tries.

Lines starting with "SULCUS " tell the app how far it is:
  SULCUS stage <prompts|batch|directions|saving>
  SULCUS base <refusals> <of>          the original, before any change
  SULCUS trial <n> <of>                a try starting
  SULCUS tried <refusals> <kl>         that try's result
  SULCUS chose <trial> <refusals> <kl> the try kept
  SULCUS saved
  SULCUS error <text>                  why it stopped

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
        # Relative to the work folder: Heretic names its checkpoint after
        # this, and a whole path ran past Windows' 260 characters.
        "model": os.path.relpath(a.model, a.work).replace("\\", "/"),
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


def pick(trials, max_kl, close=2):
    """The try to keep, from Heretic's best ones: of those drifting no more
    than `max_kl`, the one drifting least among those within `close`
    refusals of the fewest (one or two refusals in a hundred is noise, and
    drift is damage); with none under `max_kl`, the one drifting least."""
    kl = lambda t: t.user_attrs["kl_divergence"]
    ok = [t for t in trials if kl(t) <= max_kl]
    if not ok:
        return min(trials, key=kl)
    fewest = min(t.user_attrs["refusals"] for t in ok)
    return min((t for t in ok if t.user_attrs["refusals"] <= fewest + close), key=kl)


def fresh_if_changed(a):
    """Tries made with other settings aren't carried on: Heretic would use
    those settings (a Quick run stopped and started as Thorough stayed
    Quick; a full-precision run continued at full precision on a card that
    now only fits 4 bits)."""
    import json

    now = {"trials": a.trials, "quantize": bool(a.quantize)}
    path = os.path.join(a.work, "run.json")
    try:
        with open(path, encoding="utf-8") as f:
            before = json.load(f)
    except (OSError, ValueError):
        before = None
    if before is not None and before != now:
        shutil.rmtree(os.path.join(a.work, "checkpoints"), ignore_errors=True)
    with open(path, "w", encoding="utf-8") as f:
        json.dump(now, f)


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
    fresh_if_changed(a)
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
            # Done: Heretic would now load the whole model again for nothing.
            keep_original_files(a.model, a.out)
            say("saved")
            sys.stdout.flush()
            os._exit(0)

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
        # Nothing to take out: it already answers nearly everything.
        if self.base_refusals <= max(3, len(self.bad_prompts) // 20):
            raise SystemExit(f"This model already answers {len(self.bad_prompts) - self.base_refusals} of the {len(self.bad_prompts)} test requests a model usually refuses, so there's nothing to take out.")

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
    try:
        main()
    except SystemExit as e:
        # The runner's own reasons go to the app as they are.
        if isinstance(e.code, str):
            say("error", e.code)
            sys.exit(1)
        raise
