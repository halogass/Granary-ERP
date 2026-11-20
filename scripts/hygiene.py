#!/usr/bin/env python3
"""Repository hygiene gate (run in CI and before a release).

1. Content: no word of the project's private denylist may appear in a tracked text file. The denylist holds
   salted SHA-256 digests only (scripts/hygiene-denylist.txt), so the published gate does not publish the words
   it guards against. Maintainers add a word with `scripts/hygiene.py --hash WORD`.
2. Commits (with --commits RANGE): no AI-assistant attribution trailers in commit messages.

Usage: scripts/hygiene.py [--commits <git range>]   |   scripts/hygiene.py --hash WORD [WORD ...]
"""
import hashlib, os, re, subprocess, sys

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
SALT = b"granary-hygiene-v1:"
DENYLIST = os.path.join(ROOT, "scripts", "hygiene-denylist.txt")
ALLOWLIST = os.path.join(ROOT, "scripts", "hygiene-allow.txt")
WORD = re.compile(r"[A-Za-z]+")
SKIP_EXT = {".png", ".jpg", ".jpeg", ".gif", ".ico", ".webp", ".woff", ".woff2", ".ttf", ".otf", ".pdf",
            ".der", ".p12", ".onnx", ".bin", ".wasm", ".zip", ".gz", ".mp4"}
B64 = re.compile(r"[A-Za-z0-9+/=_-]{40,}")
ATTRIBUTION = re.compile(r"^(co-authored-by|claude-session|generated-by)\s*:.*(claude|anthropic)", re.I | re.M)


def digest(word: str) -> str:
    return hashlib.sha256(SALT + word.lower().encode()).hexdigest()


def load_lines(path):
    if not os.path.exists(path):
        return []
    return [l.strip() for l in open(path, encoding="utf-8") if l.strip() and not l.startswith("#")]


def check_content() -> int:
    deny = set(load_lines(DENYLIST))
    allow = [re.compile(p) for p in load_lines(ALLOWLIST)]
    files = subprocess.run(["git", "-C", ROOT, "ls-files", "-z"], capture_output=True, text=True,
                           check=True).stdout.split("\0")
    bad = 0
    for rel in filter(None, files):
        if os.path.splitext(rel)[1].lower() in SKIP_EXT or any(a.search(rel) for a in allow):
            continue
        try:
            text = open(os.path.join(ROOT, rel), encoding="utf-8").read()
        except (UnicodeDecodeError, FileNotFoundError, IsADirectoryError):
            continue
        for n, line in enumerate(text.split("\n"), 1):
            line = B64.sub(" ", line)
            for w in WORD.findall(line):
                # whole words and every snake/camel part: "Fooz" in "getFoozName" or "fooz_id"
                parts = [w] + re.findall(r"[A-Z]?[a-z]+|[A-Z]+(?![a-z])", w)
                if any(digest(p) in deny for p in parts):
                    print(f"{rel}:{n}: denylisted word: {w}")
                    bad += 1
    return bad


def check_commits(rng: str) -> int:
    log = subprocess.run(["git", "-C", ROOT, "log", "--format=%H%n%an <%ae>%n%B%n\x1e", rng],
                         capture_output=True, text=True, check=True).stdout
    bad = 0
    for entry in filter(str.strip, log.split("\x1e")):
        sha, author, *body = entry.strip().split("\n")
        if ATTRIBUTION.search("\n".join(body)) or re.search(r"claude|anthropic", author, re.I):
            print(f"commit {sha[:12]}: AI-assistant attribution in the author or message")
            bad += 1
    return bad


def main(argv):
    if argv[:1] == ["--hash"]:
        for w in argv[1:]:
            print(digest(w))
        return 0
    bad = check_content()
    if argv[:1] == ["--commits"] and len(argv) > 1:
        bad += check_commits(argv[1])
    print(f"hygiene: {bad} problem(s)")
    return 1 if bad else 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
