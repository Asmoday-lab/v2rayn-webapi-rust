#!/usr/bin/env python3
"""Advance oracle.lock's oracle.commit to the current upstream master HEAD.

Called by the contract-watch CI job. Kept as a separate script so the lock
mutation is reviewable in a PR rather than silently applied on a schedule.
"""
import json
import urllib.request
from pathlib import Path

lock_path = Path(__file__).resolve().parent.parent / "oracle.lock"
lock = json.loads(lock_path.read_text())

url = (
    f"https://api.github.com/repos/{lock['oracle']['repository']}"
    f"/commits/{lock['oracle']['branch']}"
)
req = urllib.request.Request(url, headers={"User-Agent": "contract-watch"})
with urllib.request.urlopen(req, timeout=30) as response:
    head = json.load(response)

old = lock["oracle"]["commit"]
lock["oracle"]["commit"] = head["sha"]
lock["oracle"]["commit_date"] = head["commit"]["committer"]["date"]
lock["oracle"]["commit_message"] = head["commit"]["message"].splitlines()[0]

lock_path.write_text(json.dumps(lock, indent=2, ensure_ascii=False) + "\n")
print(f"oracle.lock: {old} -> {head['sha']}")
