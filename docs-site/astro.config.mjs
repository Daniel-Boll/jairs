// @ts-check
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';

import jairsGrammar from './src/grammars/jairs.tmLanguage.json' with { type: 'json' };

// https://astro.build/config
export default defineConfig({
  site: 'https://jairs.example/',
  markdown: {
    // Starlight uses Expressive Code; but plain markdown code fences also need the language.
  },
  integrations: [
    starlight({
      title: 'Jairs',
      description:
        'A Jai-inspired systems language with compile-time execution, explicit allocators, and no GC, RAII or exceptions.',
      tagline: 'A Jai-inspired systems language, compiled by a hand-written Rust compiler.',
      logo: {
        light: './src/assets/logo-light.svg',
        dark: './src/assets/logo-dark.svg',
        replacesTitle: false,
      },
      social: [
        {
          icon: 'seti:rust',
          label: 'Compiler source (Rust)',
          href: 'https://jairs.example/',
        },
      ],
      customCss: ['./src/styles/jairs.css'],
      expressiveCode: {
        themes: ['github-dark', 'github-light'],
        shiki: {
          langs: [
            // Registered as `jairs`, with `jr` and `jai` as aliases so fenced
            // ```jr blocks highlight with the same grammar.
            /** @type {any} */ (jairsGrammar),
          ],
        },
      },
      sidebar: [
        {
          label: 'Start here',
          items: [
            { label: 'Welcome', link: '/' },
            { label: 'The three books', link: '/start/three-books/' },
            { label: 'Installing & running', link: '/start/installing/' },
          ],
        },
        {
          label: 'Book I · The Jairs Language',
          collapsed: false,
          items: [{ autogenerate: { directory: 'language' } }],
        },
        {
          label: 'Book II · Jairs by Example',
          collapsed: true,
          items: [{ autogenerate: { directory: 'by-example' } }],
        },
        {
          label: 'Book III · Jairs in Practice',
          collapsed: true,
          items: [{ autogenerate: { directory: 'in-practice' } }],
        },
      ],
    }),
  ],
});
