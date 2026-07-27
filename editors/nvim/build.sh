#!/bin/sh
# Builds the tree-sitter parser Neovim loads for highlighting.
#
# Neovim looks for `parser/<language>.so` on the runtimepath and loads it with no
# registration call, so this script's only job is to put a shared library there. That is
# why this directory needs no plugin manager and no `nvim-treesitter`.
#
# `src/parser.c` is generated and git-ignored (see `.gitignore`, which explains that
# `src/scanner.c` is hand-written because nested block comments cannot be a token regex),
# so `tree-sitter generate` runs first.
set -eu

here=$(cd "$(dirname "$0")" && pwd)
grammar="$here/../../tree-sitter-jairs"
version=0.26.11

# Match the version the corpus-drift CI gate uses, so a parser built here and a parse
# checked in CI cannot disagree about the grammar.
( cd "$grammar" && npx --yes "tree-sitter-cli@$version" generate )

# `-I src` for `tree_sitter/parser.h`, which `generate` writes next to the parser.
cc -O2 -fPIC -shared -std=c11 \
   -I "$grammar/src" \
   -o "$here/parser/jairs.so" \
   "$grammar/src/parser.c" "$grammar/src/scanner.c"

echo "built $here/parser/jairs.so"
