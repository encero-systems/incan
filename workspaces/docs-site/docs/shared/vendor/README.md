# Vendored browser dependencies

## Mermaid

- Package: [`mermaid`](https://www.npmjs.com/package/mermaid)
- Version: `11.16.0`
- Source file: `dist/mermaid.min.js`
- License: MIT; see [`LICENSE.mermaid`](LICENSE.mermaid)
- SHA-256: `74d7c46dabca328c2294733910a8aa1ed0c37451776e8d5295da38a2b758fb9b`

The docs load Mermaid only on pages containing a Mermaid diagram. Keeping the runtime local makes diagram rendering independent of a public CDN and lets the site use a strict Mermaid security level.

To refresh the pinned bundle, download the chosen package archive with `npm pack mermaid@<version>`, extract `package/dist/mermaid.min.js`, replace the vendored file, and update the version and checksum above. Run `make docs-build` from this workspace and visually verify at least one diagram before committing.
