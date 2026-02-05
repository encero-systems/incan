// Enable MathJax rendering for Markdown math produced via pymdownx.arithmatex.
//
// This loads MathJax v3 from a CDN at runtime. MkDocs builds do not fetch this file;
// it is just referenced by the generated HTML.
//
// Note: although this file lives under `_snippets/`, it is loaded via `mkdocs.yml`
// `extra_javascript` like any other static asset.

window.MathJax = {
  tex: {
    inlineMath: [["\\(", "\\)"]],
    displayMath: [["\\[", "\\]"]],
  },
  chtml: {
    // Left-align display equations so they don't "float" awkwardly in the center
    // of the page's content column.
    displayAlign: "left",
    displayIndent: "0em",
  },
};

(() => {
  const script = document.createElement("script");
  script.src = "https://cdn.jsdelivr.net/npm/mathjax@3/es5/tex-mml-chtml.js";
  script.async = true;
  document.head.appendChild(script);
})();

