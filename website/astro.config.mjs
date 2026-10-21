// Vinculum website (Astro + Starlight). See README.md in this directory.
import { defineConfig } from 'astro/config';
import starlight from '@astrojs/starlight';
import { remarkBaseLinks } from './plugins/remark-base-links.mjs';
import { BASE, SITE } from './site.config.mjs';

export default defineConfig({
  // Set in site.config.mjs (the only place that knows the base path).
  site: SITE,
  base: BASE,
  markdown: {
    remarkPlugins: [remarkBaseLinks(BASE)],
  },
  integrations: [
    starlight({
      title: 'Vinculum',
      description:
        'A network manager for Hyprland, Sway and other desktops without one: Wi-Fi, wired and WireGuard on NetworkManager, written in Rust.',
      logo: { src: './src/assets/mark.svg' },
      favicon: '/favicon.svg',
      customCss: ['./src/styles/custom.css'],
      components: {
        ThemeProvider: './src/components/ThemeProvider.astro',
        ThemeSelect: './src/components/ThemeSelect.astro',
      },
      social: [{ icon: 'github', label: 'GitHub', href: 'https://github.com/giosuel/vinculum' }],
      editLink: {
        baseUrl: 'https://github.com/giosuel/vinculum/edit/main/website/',
      },
      lastUpdated: false,
      sidebar: [
        { label: 'Installing', link: '/install/' },
        { label: 'Guide', items: [{ autogenerate: { directory: 'guide' } }] },
        { label: 'Development', link: '/development/' },
      ],
    }),
  ],
});
