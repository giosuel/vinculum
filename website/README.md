# Vinculum website

The public site at https://vinculum.giosuel.ch: landing page, installing, guides and developer
docs. Built with [Astro](https://astro.build) and [Starlight](https://starlight.astro.build), set
up like Imaginarium's website.

```sh
cd website
npm ci                # exact versions from package-lock.json, no install scripts (.npmrc)
npm run dev           # http://localhost:4321/
npm run build         # static site in dist/
npm run check-links   # broken internal links in dist/ (CI runs this)
```

- `src/content/docs/`: the pages (Markdown/MDX). `index.mdx` is the landing page.
- `src/styles/custom.css`: the app's colours and the landing page's look.
- `public/hero.svg`: the window illustration on the landing page.
- `site.config.mjs`: the only place that knows the URL and base path. Write internal links
  root-relative (`/guide/hyprland/`).

Dependency policy: exact versions, and new versions only once they have been public for at least
two weeks (`npm install --before=<date>`). Install scripts are disabled.
