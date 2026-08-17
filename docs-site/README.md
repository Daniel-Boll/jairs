# Jairs documentation site

A [Starlight](https://starlight.astro.build/) (Astro) documentation site for the Jairs
language, organised as three books:

- **Book I — The Jairs Language** (`src/content/docs/language/`): a narrative, Rust-Book-style
  tour of the whole language.
- **Book II — Jairs by Example** (`src/content/docs/by-example/`): every feature as a small,
  annotated program, mirrored from the compiler's `tests/corpus/valid/` files.
- **Book III — Jairs in Practice** (`src/content/docs/in-practice/`): complete programs, each
  compiled and run against the real `jr` driver before being written up.

## Develop

```sh
npm install
npm run dev        # local dev server with hot reload
npm run build      # production build into dist/
npm run preview    # serve the production build
```

## Jairs syntax highlighting

Code fences tagged ```` ```jr ```` (aliases: `jairs`, `jai`) are highlighted with a custom
TextMate grammar at `src/grammars/jairs.tmLanguage.json`, registered with Expressive Code in
`astro.config.mjs`. Extend that grammar when the language gains new keywords or directives.

## Editing conventions

- The three books are three top-level sidebar groups in `astro.config.mjs`; pages within each
  are ordered by their `sidebar.order` frontmatter.
- Book II pages are faithful to the corpus: their code comes from `tests/corpus/valid/*.jr`.
- Book III programs are verified — run `jr run -I ../modules <program>.jr` to re-check them.
- Frontmatter `title`/`description` values containing `:`, a leading `` ` ``, or `#` must be
  quoted, or the YAML parse fails the build.
