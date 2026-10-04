#!/usr/bin/env python3
"""Check Git blobs without printing their contents. No external dependencies."""
import argparse
import hashlib
import json
from pathlib import PurePosixPath
import re
import subprocess
import sys

PRIVATE_DIRS = {"outputs", "test-reports", "research", "local-data", "staging"}
PRIVATE_SUFFIXES = {
    ".mp3", ".wav", ".flac", ".aiff", ".aif", ".m4a", ".ogg", ".opus",
    ".pdb", ".db", ".sqlite", ".sqlite3", ".img", ".p12", ".pfx", ".key",
}
PATTERNS = {
    "private key material": re.compile(rb"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY-----"),
    "GitHub credential": re.compile(rb"(?:gh[pousr]_[A-Za-z0-9]{20,}|github_pat_[A-Za-z0-9_]{30,})"),
    "Google credential": re.compile(rb"(?:AIza[A-Za-z0-9_-]{30,}|ya29\.[A-Za-z0-9_-]{20,})"),
    "AWS access key": re.compile(rb"(?:AKIA|ASIA)[A-Z0-9]{16}"),
    "OAuth credential value": re.compile(
        rb'["\x27](?:access_token|refresh_token|client_secret)["\x27]\s*:\s*["\x27][^"\x27\r\n]{16,}["\x27]'
    ),
}
HOME_PATH = re.compile(rb"(?:/Users/|/home/)([A-Za-z0-9_.@-]+)(?:/|\\/)")
WINDOWS_HOME = re.compile(rb"[A-Za-z]:\\{1,2}Users\\{1,2}([A-Za-z0-9_.@-]+)\\{1,2}")
EXAMPLE_USERS = {b"dj", b"example", b"test", b"user", b"username"}
ALLOWLIST = "scripts/privacy-fixtures.json"


def git(*args):
    return subprocess.check_output(["git", *args], stderr=subprocess.DEVNULL)


def inspect_blob(path, data, approved):
    """Return rule names, never matching secrets or file contents."""
    p = PurePosixPath(path)
    reasons = []
    if any(part.lower() in PRIVATE_DIRS for part in p.parts):
        reasons.append("local report or library directory")
    if path.startswith(("app/screenshots/", "app/test-results/")):
        reasons.append("generated UI artifact")
    if p.name.startswith(".env") and p.name not in {".env.example", ".env.sample"}:
        reasons.append("local environment file")
    if p.suffix.lower() in PRIVATE_SUFFIXES:
        reasons.append("music, library database, disk image or signing material")
    if p.name.lower() in {"credentials.json", "token.json", "tokens.json"}:
        reasons.append("credential storage file")
    if p.name.startswith("boothready-diagnostics-"):
        reasons.append("raw diagnostics filename")
    for label, pattern in PATTERNS.items():
        if pattern.search(data):
            reasons.append(label)
    if any(m.group(1) not in EXAMPLE_USERS for pattern in (HOME_PATH, WINDOWS_HOME) for m in pattern.finditer(data)):
        reasons.append("personal home directory path")
    if p.suffix.lower() == ".json":
        try:
            value = json.loads(data)
        except (ValueError, UnicodeDecodeError):
            value = None
        if isinstance(value, dict) and (
            value.get("kind") == "real_read_only"
            or {"source", "files", "revision"}.issubset(value)
        ):
            reasons.append("private runtime manifest or real-device report")
        if isinstance(value, dict) and "captures" in value and "devices" in value:
            if approved.get(path) != hashlib.sha256(data).hexdigest():
                reasons.append("diagnostics require reviewed redaction and an approved content hash")
    return reasons


def scan(all_files=False):
    # Inspect the index. Reading the working tree would miss secrets that were
    # staged and subsequently removed from the file on disk.
    listed = git("ls-files", "-z") if all_files else git(
        "diff", "--cached", "--name-only", "--diff-filter=ACMR", "-z"
    )
    paths = [p.decode("utf-8", "surrogateescape") for p in listed.split(b"\0") if p]
    try:
        approved = json.loads(git("show", ":" + ALLOWLIST))
    except subprocess.CalledProcessError:
        approved = {}
    if not isinstance(approved, dict):
        raise ValueError("fixture allowlist must be an object")
    failures = []
    for path in paths:
        data = git("show", ":" + path)
        for reason in inspect_blob(path, data, approved):
            failures.append((path, reason))
    return paths, failures


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--all", action="store_true", help="inspect every tracked file, for CI")
    args = parser.parse_args()
    try:
        paths, failures = scan(args.all)
    except (subprocess.CalledProcessError, ValueError):
        print("Privacy check could not read the Git index or fixture allowlist.", file=sys.stderr)
        return 2
    if failures:
        print("Privacy check blocked these files (contents withheld):", file=sys.stderr)
        for path, reason in failures:
            # JSON escaping prevents a crafted filename from adding terminal lines.
            print(f"  {json.dumps(path)}: {reason}", file=sys.stderr)
        return 1
    print(f"Privacy check passed for {len(paths)} Git files.")
    return 0


if __name__ == "__main__":
    sys.exit(main())
