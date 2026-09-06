# Jairs documentation site

A [Starlight](https://starlight.astro.build/) (Astro) documentation site for the Jairs
language, organised as four books:

- **Book I — The Jairs Language** (`src/content/docs/language/`): a narrative, Rust-Book-style
  tour of the whole language.
- **Book II — Jairs by Example** (`src/content/docs/by-example/`): every feature as a small,
  annotated program, mirrored from the compiler's `tests/corpus/valid/` files.
- **Book III — Jairs in Practice** (`src/content/docs/in-practice/`): complete programs, each
  compiled and run against the real `jr` driver before being written up.
- **Book IV — Games with Jairs** (`src/content/docs/games/`): the graphics stack (`Window`,
  `Input`, `Simp`, `GL`, `Image`, `UI`), three complete games from `examples/games/`, and a
  maintained inventory of what a game cannot do yet.

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

- The four books are four top-level sidebar groups in `astro.config.mjs`; pages within each
  are ordered by their `sidebar.order` frontmatter, and each group is `autogenerate`d from its
  directory, so a new file appears automatically in its `order` position.
- Book II pages are faithful to the corpus: their code comes from `tests/corpus/valid/*.jr`.
- Book III programs are verified — run `jr run -I ../modules <program>.jr` to re-check them.
- Book IV pages come from `modules/{Window,Input,Simp,GL,Image,UI}/module.jr` and from
  `examples/games/`. Its three games are rebuilt with
  `jr build examples/games/build.jr -I modules`, and the drawing ones need SDL2 and a `-L`.
- **Never paste a signature from memory.** Every declaration shown anywhere in the site,
  including Book I, is copied from a module source or from a program that was run. A page
  that documents a signature the compiler does not have is worse than a page that omits it.
- Status badges are `<span class="jairs-status absent">absent</span>` for something not built
  yet and `<span class="jairs-status refused">refused</span>` for something the compiler
  deliberately rejects or the design declined. Keeping the two apart matters: calling a
  refusal an absence reads as a gap somebody is going to fill.
- On a book page, frontmatter `title`/`description` values containing `:`, a leading `` ` ``,
  or `#` must be quoted, or the YAML parse fails the build, and only `title`, `description`,
  `sidebar.order` and `sidebar.label` exist — a custom key fails the build. The splash page
  (`index.mdx`) is not a book page and uses Starlight's own splash frontmatter
  (`template: splash`, `hero`) instead.
