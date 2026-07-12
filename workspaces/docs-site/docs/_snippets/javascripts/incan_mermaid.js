/* Load the vendored Mermaid runtime only on pages that contain diagrams. */
(function () {
  let runtimePromise;
  let configured = false;

  function loadRuntime() {
    if (window.mermaid) return Promise.resolve(window.mermaid);
    if (runtimePromise) return runtimePromise;

    runtimePromise = new Promise((resolve, reject) => {
      const script = document.createElement("script");
      script.src = "/shared/vendor/mermaid.min.js";
      script.addEventListener("load", () => resolve(window.mermaid || globalThis.mermaid), { once: true });
      script.addEventListener("error", () => reject(new Error("Could not load the local diagram runtime")), { once: true });
      document.head.appendChild(script);
    });
    return runtimePromise;
  }

  async function init() {
    const sources = document.querySelectorAll("pre.inc-diagram:not([data-processed])");
    if (sources.length === 0) return;

    const nodes = Array.from(sources, (source) => {
      const host = document.createElement("div");
      host.className = "inc-diagram";
      host.textContent = source.querySelector("code")?.textContent || source.textContent;
      source.replaceWith(host);
      return host;
    });

    try {
      const mermaid = await loadRuntime();
      if (!configured) {
        mermaid.initialize({
          startOnLoad: false,
          securityLevel: "strict",
          theme: "base",
          themeVariables: {
            background: "#06070a",
            primaryColor: "#150f0a",
            primaryTextColor: "#e4ebf2",
            primaryBorderColor: "#ffc15a",
            secondaryColor: "#08171a",
            secondaryTextColor: "#e4ebf2",
            secondaryBorderColor: "#48f0ef",
            tertiaryColor: "#1a0910",
            tertiaryTextColor: "#e4ebf2",
            tertiaryBorderColor: "#ff5c69",
            lineColor: "#98a5b3",
            edgeLabelBackground: "#08090c",
            fontFamily: "Inter, system-ui, sans-serif",
          },
        });
        configured = true;
      }
      await mermaid.run({ nodes, suppressErrors: false });
    } catch (error) {
      console.warn("Incan diagram rendering failed", error);
    }
  }

  if (typeof window.document$ !== "undefined" && typeof window.document$.subscribe === "function") {
    window.document$.subscribe(init);
  } else {
    document.addEventListener("DOMContentLoaded", init);
  }
})();
