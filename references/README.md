# Reference sources

This directory contains commit-pinned source repositories used to audit Jairs'
compatibility claims. They are research inputs, not build dependencies.

## `The_Way_to_Jai`

[`The_Way_to_Jai`](The_Way_to_Jai/) is a git submodule pinned by the parent
repository. Initialise it after cloning with:

```sh
git submodule update --init --recursive
```

The guide is a secondary account of a closed-beta language. Its checked-in
`.jai` examples are useful compatibility probes, but they are not a substitute
for Jai's unpublished compiler and standard-library sources. Every Jairs ADR
that derives a compatibility claim from this reference must name the submodule
commit and distinguish:

- syntax demonstrated by a concrete example;
- standard-library declarations quoted by the guide;
- the guide author's interpretation;
- behaviour confirmed independently from a public vendored Jai source.

The submodule must never be added to Jairs' build, module search path, test
corpus, or install artefacts. A nested checkout is deliberately excluded from
workspace discovery (ADR-0203).

ADR-0219 keeps the executable compatibility baseline in
[`../tests/compatibility/probes.toml`](../tests/compatibility/probes.toml).
That manifest records paths and the pinned revision as provenance, but ordinary
tests execute only the small Jairs-owned files below `tests/compatibility/probes/`.
The suite therefore remains complete when the submodule is not initialized.
