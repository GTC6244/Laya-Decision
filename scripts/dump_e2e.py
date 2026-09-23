#!/usr/bin/env python3
"""Dump end-to-end reference answers from the upstream Python `laya` package.

Runs the real `Agent.system_one` on preset question sets over a handful of states, per
checkpoint, and writes reference JSON that the Rust candle backend is checked against.

Requires a Python env with `torch`, `transformers`, `safetensors` and the `laya` package,
and downloads the checkpoint(s) from the Hugging Face Hub on first use.

Usage:
    python3 scripts/dump_e2e.py [--checkpoint english|multilingual|typed-decisions] [out_dir]

Output: crates/laya/tests/golden_e2e/<checkpoint>.json — a list of
    { "state": <state>, "questions": <questions>, "result": <system_one output> }

The Rust gate (a feature-flagged test) loads the same checkpoint via candle, runs the same
(state, questions), and asserts equality of choice labels, score/noul values, confidence and
act_probability within 1e-4.
"""
import argparse
import json
import os

import laya
from laya.presets import triage_questions, guard_questions, moderation_questions

CHECKPOINTS = {
    "english": ("convaiinnovations/laya", None),
    "multilingual": ("convaiinnovations/laya", "multilingual"),
    "typed-decisions": ("convaiinnovations/laya", "typed-decisions"),
}

STATES = [
    {"message": "I was charged twice this month and I'm furious. Refund me now."},
    {"message": "Just wanted to say thanks, everything works great!"},
    "Can you help me reset my password?",
    ["Hi, I have a problem", "What is it?", "My invoice is wrong and I need it fixed today"],
]

QUESTION_SETS = {
    "triage": triage_questions,
    "guard": guard_questions,
    "moderation": moderation_questions,
}


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--checkpoint", default="english", choices=list(CHECKPOINTS))
    ap.add_argument("out_dir", nargs="?", default=os.path.join(
        os.path.dirname(__file__), "..", "crates", "laya", "tests", "golden_e2e"))
    args = ap.parse_args()

    repo, subfolder = CHECKPOINTS[args.checkpoint]
    agent = laya.load(repo, subfolder=subfolder, device="cpu")

    rows = []
    for qname, qfn in QUESTION_SETS.items():
        questions = qfn()
        for state in STATES:
            result = agent.system_one(state, questions)
            rows.append({
                "question_set": qname,
                "state": state,
                "questions": questions,
                "result": result,
            })

    os.makedirs(args.out_dir, exist_ok=True)
    path = os.path.join(args.out_dir, args.checkpoint + ".json")
    with open(path, "w", encoding="utf-8") as f:
        json.dump(rows, f, ensure_ascii=False, indent=2)
    print("wrote", path, "(%d cases)" % len(rows))


if __name__ == "__main__":
    main()
