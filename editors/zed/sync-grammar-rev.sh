#!/usr/bin/env bash
# Stamps the current `HEAD` into `extension.toml`'s grammar revision.
#
# # Why a revision has to be stamped at all
#
# Zed builds a grammar by cloning the repository at a revision and compiling `src/parser.c`
# (verified in Zed's own `extension_builder.rs` — it runs `git fetch --depth 1 origin <rev>` then
# `git checkout <rev>`, and never runs `tree-sitter generate`). So the manifest must name a revision
# that exists, and a branch name will not do: after a shallow fetch there is no local branch of that
# name to check out.
#
# The grammar lives in this repository rather than one of its own, which is deliberate — the grammar
# and the compiler that must agree with it cannot then drift apart. The cost is this script: a commit
# that changes `grammar.js` needs the revision moved forward, and nothing else notices.
#
# Run it, then re-install the dev extension in Zed so the grammar is rebuilt.
set -euo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
manifest="$here/extension.toml"
repo="$(cd "$here/../.." && pwd)"

rev="$(git -C "$repo" rev-parse HEAD)"

if ! git -C "$repo" diff --quiet -- tree-sitter-jairs; then
  echo "sync-grammar-rev.sh: tree-sitter-jairs has uncommitted changes." >&2
  echo "  Commit them first — Zed clones a revision, so it cannot see a working-tree edit." >&2
  exit 1
fi

# `-i ''` is the BSD spelling; this repository is developed on macOS.
sed -i '' -E "s/^commit = \"[0-9a-f]{40}\"$/commit = \"$rev\"/" "$manifest"

echo "sync-grammar-rev.sh: grammar revision is now $rev"
echo "  Re-install the dev extension in Zed to rebuild the parser."
