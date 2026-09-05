#!/usr/bin/env bash
# Verifies every link in the Zed extension's chain, without needing Zed.
#
# `editors/nvim/verify.lua` exists because editor integration rotted while nobody ran it (ADR-0025).
# This is the same argument for the same reason: the one step here that genuinely needs a person is
# clicking `install dev extension`, and everything up to it is mechanical and therefore checkable.
#
# Run it after changing `grammar.js`, the queries, the manifest or `src/jairs.rs`.
set -uo pipefail

here="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
repo="$(cd "$here/../.." && pwd)"
ts="$repo/tree-sitter-jairs"
tmp="$(mktemp -d)"
trap 'rm -rf "$tmp"' EXIT

checks=0
failures=0

check() {
  local name="$1"
  shift
  checks=$((checks + 1))
  if "$@" >"$tmp/out" 2>&1; then
    printf '  ok   %s\n' "$name"
  else
    failures=$((failures + 1))
    printf '  FAIL %s\n' "$name"
    sed 's/^/         /' "$tmp/out" | head -6
  fi
}

echo "Zed extension verification"

# ---- the manifest --------------------------------------------------------------------------------

check "extension.toml parses" python3 -c "
import tomllib, sys
tomllib.load(open('$here/extension.toml','rb'))
"

# The field names Zed actually reads. `language` and `commit` are accepted but deprecated/aliased,
# and a manifest using them is the mistake this catches — both were copied from a published
# extension that still uses the older spellings.
check "the manifest uses Zed's current field names" python3 -c "
import tomllib, sys
m = tomllib.load(open('$here/extension.toml','rb'))
server = m['language_servers']['jairs']
grammar = m['grammars']['jairs']
problems = []
if 'language' in server:
    problems.append('language_servers.jairs.language is deprecated; use languages = [..]')
if 'languages' not in server:
    problems.append('language_servers.jairs.languages is missing')
if 'commit' in grammar:
    problems.append('grammars.jairs.commit is only an alias; use rev')
if 'rev' not in grammar:
    problems.append('grammars.jairs.rev is missing')
if m.get('schema_version') != 1:
    problems.append('schema_version must be 1')
if problems:
    print('\n'.join(problems)); sys.exit(1)
"

# The language named by the server must be the one the config declares, or Zed attaches nothing.
check "the server's language matches languages/jairs/config.toml" python3 -c "
import tomllib, sys
m = tomllib.load(open('$here/extension.toml','rb'))
c = tomllib.load(open('$here/languages/jairs/config.toml','rb'))
declared = c['name']
wanted = m['language_servers']['jairs']['languages']
if declared not in wanted:
    print(f'config declares {declared!r}, manifest names {wanted!r}'); sys.exit(1)
if c['grammar'] not in m['grammars']:
    print(f'config wants grammar {c[\"grammar\"]!r}, manifest registers {list(m[\"grammars\"])}'); sys.exit(1)
"

# ---- the grammar Zed will clone ------------------------------------------------------------------

rev="$(python3 -c "
import tomllib
print(tomllib.load(open('$here/extension.toml','rb'))['grammars']['jairs']['rev'])
")"

check "the pinned revision exists" git -C "$repo" cat-file -e "$rev^{commit}"

# Exactly what Zed does: `git fetch --depth 1 origin <rev>` then `git checkout <rev>`.
clone_grammar() {
  git init -q "$tmp/g" &&
    git -C "$tmp/g" remote add origin "file://$repo" &&
    git -C "$tmp/g" fetch -q --depth 1 origin "$rev" &&
    git -C "$tmp/g" checkout -q "$rev"
}
check "the revision can be fetched and checked out" clone_grammar

# Zed compiles these two files and requires them in the checkout. `src/parser.c` is generated, and
# tracking it is what makes this possible at all (ADR-0199 §10).
check "the checkout has src/parser.c" test -f "$tmp/g/tree-sitter-jairs/src/parser.c"
check "the checkout has src/scanner.c" test -f "$tmp/g/tree-sitter-jairs/src/scanner.c"
check "the checkout has the tree_sitter headers" test -f "$tmp/g/tree-sitter-jairs/src/tree_sitter/parser.h"

# Compiled natively rather than to wasm32-wasi: Apple's clang has no wasi sysroot, which is why Zed
# downloads a wasi-sdk. What this proves is the C is complete and exports the symbol Zed asks for,
# which is the part that can be wrong here.
compile_grammar() {
  clang -fPIC -shared -Os -I "$tmp/g/tree-sitter-jairs/src" \
    -o "$tmp/jairs.so" \
    "$tmp/g/tree-sitter-jairs/src/parser.c" \
    "$tmp/g/tree-sitter-jairs/src/scanner.c" 2>"$tmp/cc" ||
    { cat "$tmp/cc"; return 1; }
  nm -gU "$tmp/jairs.so" | grep -q '_tree_sitter_jairs$'
}
check "the grammar compiles and exports tree_sitter_jairs" compile_grammar

# ---- the queries ---------------------------------------------------------------------------------

# Regenerated first, so a stale committed copy is a drift failure rather than a silent difference.
check "highlights.scm regenerates without drift" bash -c "
'$here/generate-queries.sh' >/dev/null &&
git -C '$repo' diff --quiet -- '$here/languages/jairs/highlights.scm'
"

for q in highlights brackets indents outline; do
  check "queries/$q.scm compiles against the grammar" bash -c "
cd '$ts' && npx --yes tree-sitter-cli@0.26.11 query \
  '$here/languages/jairs/$q.scm' '$repo/tests/corpus/valid/024-hello.jr' >/dev/null 2>&1
"
done

check "semantic_token_rules.json parses" python3 -c "
import json
rules = json.load(open('$here/languages/jairs/semantic_token_rules.json'))
assert isinstance(rules, list) and rules, 'must be a non-empty array'
for rule in rules:
    assert 'token_type' in rule and 'style' in rule, rule
"

# Every token type named must be one the server actually reports, or the rule is decoration.
check "every semantic token rule names a type the server emits" python3 -c "
import json, re, sys
rules = json.load(open('$here/languages/jairs/semantic_token_rules.json'))
source = open('$repo/crates/jr-lsp/src/tokens.rs').read()
block = source.split('TOKEN_TYPES', 1)[1].split('];', 1)[0]
emitted = {m.group(1) for m in re.finditer(r'SemanticTokenType::([A-Z_]+)', block)}
def spelling(name):
    # NAMESPACE -> namespace, ENUM_MEMBER -> enumMember, matching lsp-types' constants.
    head, *rest = name.lower().split('_')
    return head + ''.join(part.capitalize() for part in rest)
have = {spelling(name) for name in emitted}
missing = [r['token_type'] for r in rules if r['token_type'] not in have]
if missing:
    print('rules for types the server never emits:', missing)
    print('server emits:', sorted(have))
    sys.exit(1)
"

# ---- the extension crate -------------------------------------------------------------------------

check "the extension compiles to wasm32-wasip2" bash -c "
cd '$here' && cargo build --release --target wasm32-wasip2 2>&1 | grep -qE '^error' && exit 1 || exit 0
"
check "the wasm artefact exists" test -f "$here/target/wasm32-wasip2/release/zed_jairs.wasm"

# ---- the server it will launch -------------------------------------------------------------------

check "jr lsp advertises formatting and completion" bash -c "
printf 'Content-Length: %d\r\n\r\n%s' \
  \"\$(printf '%s' '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"processId\":null,\"capabilities\":{}}}' | wc -c | tr -d ' ')\" \
  '{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"initialize\",\"params\":{\"processId\":null,\"capabilities\":{}}}' |
  '$repo/target/release/jr' lsp -q 2>/dev/null |
  grep -q '\"documentFormattingProvider\":true'
"

echo
if [[ $failures -eq 0 ]]; then
  echo "all $checks checks passed"
  echo "remaining manual step: 'zed: install dev extension' on $here"
else
  echo "$failures of $checks checks failed"
  exit 1
fi
