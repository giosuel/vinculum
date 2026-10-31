// Where the site is published. This is the ONLY place that knows the base path. Content links are
// root-relative ("/guide/hyprland/"), and plugins/remark-base-links.mjs prefixes BASE at build
// time. Served at the root of its own domain, so BASE is '/'.
export const SITE = 'https://lituus.giosuel.ch';
export const BASE = '/';
