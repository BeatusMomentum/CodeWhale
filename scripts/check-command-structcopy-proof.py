#!/usr/bin/env python3
"""Keep the independent proof attached to the real command's complete source.

Compilation supplies the dependency proof. This small inventory check prevents
silently replacing the production include with a stub or omitting a new helper.
The current closure is one file; adding a file requires explicitly extending
this inventory and the portable compilation package in the same change.
"""
from pathlib import Path
import re

ROOT = Path(__file__).resolve().parent.parent
LEAF = "crates/tui/src/commands/groups/session/structcopy.rs"
PROOF = "tests/portable-session-structcopy/src/commands/groups/session/mod.rs"


def violations(root):
    errors = []
    wrapper = root / PROOF
    text = wrapper.read_text()
    includes = re.findall(r'#\[path\s*=\s*"([^"]+)"\]\s*pub mod structcopy;', text)
    if len(includes) != 1 or (wrapper.parent / includes[0]).resolve() != (root / LEAF).resolve():
        errors.append("proof must include the actual structcopy production source")
    source = (root / LEAF).read_text()
    # External modules would grow the closure: fail closed until explicitly
    # added to this proof inventory. Inline pure tests are compiled normally.
    if re.search(r'^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*;', source, re.M):
        errors.append("external helper module requires a complete proof inventory update")
    return errors


def main():
    errors = violations(ROOT)
    for error in errors:
        print(f"[structcopy-proof] FAIL: {error}")
    if not errors:
        print("[structcopy-proof] PASS: actual production leaf and complete helper inventory")
    return bool(errors)


if __name__ == "__main__":
    raise SystemExit(main())
