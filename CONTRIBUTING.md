# Contributing to Jairs

Jairs is a solo project in early pre-alpha. External contributions are welcome
but the bar is high: every change must pass the full definition of done below.

---

## Definition of done (per-wave checklist)

A feature is **not done** until every box is checked. Partial implementations
are not merged. This checklist is verbatim from PLAN.md §2.0:

- [ ] **Spec** chapter written in `docs/spec/`
- [ ] **Corpus** files added (they *are* the spec examples)
- [ ] **Parser** + error recovery + `fmt`
- [ ] **Sema** + diagnostics with good spans
- [ ] **MIR** lowering + **VM** + **Cranelift**, verified equal by differential test
- [ ] **LSP** understands it (hover, completion, goto where applicable)
- [ ] **tree-sitter** grammar + highlight queries updated; drift gate green
- [ ] **Stdlib** uses it where it should (dogfooding is the acceptance test)

---

## The corpus rule

`tests/corpus/*.jr` files are simultaneously:

1. **Spec examples** — the canonical illustration of each language feature.
2. **Compiler parser tests** — run by `cargo test --test corpus`.
3. **tree-sitter tests** — run by `tree-sitter test` in CI.

**Any grammar change requires a corpus file.** A change that parses in the
compiler but not in tree-sitter (or vice versa) is a bug. The `corpus-drift` CI
job enforces this automatically once both parsers exist.

---

## Dependency pinning

`cranelift-*` and `salsa` are pinned with `=` exact versions in
`Cargo.toml` because their APIs are explicitly **not semver-stable** — a minor
version bump breaks compilation without warning.

- All Cranelift API contact **must stay inside `jr-codegen-clif`** behind the
  `Backend` trait. No other crate may import a `cranelift-*` crate directly.
- `salsa` is similarly confined to `jr-db`.

When bumping either, update the pin in `[workspace.dependencies]`, verify the
full workspace builds, and record the bump in `docs/adr/`.

---

## Toolchain and style

- **Stable Rust only.** Nightly features are not permitted. The toolchain is
  pinned in `rust-toolchain.toml`.
- Run `cargo fmt --all` before every push. CI runs `cargo fmt --all --check`
  and will reject unformatted code.
- Run `cargo clippy --workspace --all-targets -- -D warnings` before every
  push. CI treats all clippy warnings as errors.
- `unsafe` blocks require a `// SAFETY:` comment explaining the invariant.
  The workspace lint `unsafe_op_in_unsafe_fn = "deny"` is enforced.
- Public items require doc comments (`missing_docs = "warn"` workspace-wide).

---

## Commit style

One logical change per commit. Commit messages: imperative mood, ≤72 chars on
the subject line, blank line before body if a body is needed.

---

## Licence

By contributing you agree that your contributions will be dual-licensed under
MIT OR Apache-2.0, the same terms as the project.
