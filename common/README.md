# Shared package scaffolding

Sourced by every qingshan kpm package. Copied into each `.kpkg` at pack
time (`common/` plus `waf/waf-base.*` and `$PKG_ID/scripts/pkg-lib.sh`).

- `pkg-lib.sh` — Kindle-side install/launch/register/uninstall helpers.
  POSIX `sh` (Kindle ash). Each package has a `pkg.env` of IDs/paths.
- `load.sh` — locates `pkg-lib.sh` + `pkg.env` from a package-root script.
- `waf-base.js` / `waf-base.css` — ES5/CSS2 chrome (Kindle object, LIPC
  JSON cmd, `status.json` poll, chromebar). Loaded before each WAF's own
  `script.js` / `style.css`.
