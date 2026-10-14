// Progressive enhancement for the release journal. Markdown retains every
// highlight and release-note link when JavaScript is unavailable.
(function () {
  const descriptions = {
    language: "Syntax, types, the standard library, and native interop.",
    toolchain: "The compiler, Oven, packages, and developer tooling.",
  };

  function setup() {
    const journal = document.querySelector("[data-inc-news]");
    if (!journal || journal.dataset.newsBound) return;
    journal.dataset.newsBound = "true";

    const releases = Array.from(journal.querySelectorAll("[data-news-version]"));
    const entries = Array.from(journal.querySelectorAll("[data-news-area]"));
    const versions = releases.map((release) => release.dataset.newsVersion);
    const defaultRelease = journal.querySelector("[data-news-default]")?.dataset.newsVersion || versions[0];
    const kinds = Array.from(journal.querySelectorAll("[data-news-type]"))
      .map((button) => button.dataset.newsType);
    let state;

    function readLocation() {
      const params = new URLSearchParams(window.location.search);
      const target = document.getElementById(window.location.hash.slice(1));
      const fragmentRelease = target?.closest("[data-news-version]");
      const fragmentEntry = target?.closest("[data-news-area]");
      const version = fragmentRelease?.dataset.newsVersion || params.get("release");
      return {
        release: versions.includes(version) ? version : defaultRelease,
        view: fragmentEntry?.dataset.newsArea || (Object.hasOwn(descriptions, params.get("view")) ? params.get("view") : "language"),
        type: fragmentEntry ? "all" : (kinds.includes(params.get("type")) ? params.get("type") : "all"),
      };
    }

    function render() {
      let count = 0;
      releases.forEach((release) => {
        release.hidden = release.dataset.newsVersion !== state.release;
      });
      entries.forEach((entry) => {
        const matches = entry.dataset.newsArea === state.view
          && (state.type === "all" || entry.dataset.newsKind === state.type);
        entry.hidden = !matches;
        if (matches && entry.closest("[data-news-version]").dataset.newsVersion === state.release) count += 1;
      });
      journal.querySelectorAll("[data-news-release]").forEach((link) => {
        if (link.dataset.newsRelease === state.release) link.setAttribute("aria-current", "true");
        else link.removeAttribute("aria-current");
      });
      ["view", "type"].forEach((key) => {
        journal.querySelectorAll(`[data-news-${key}]`).forEach((button) => {
          button.setAttribute("aria-pressed", String(button.dataset[key === "view" ? "newsView" : "newsType"] === state[key]));
        });
      });
      journal.querySelector("[data-news-description]").textContent = descriptions[state.view];
      journal.querySelector("[data-news-empty]").hidden = count !== 0;
      const selectedRelease = releases.find((release) => release.dataset.newsVersion === state.release);
      const availability = selectedRelease.querySelector(".inc-news__development-note")
        ? "In development; not released." : "Released.";
      const filter = state.type === "all" ? "all change types" : state.type;
      journal.querySelector("[data-news-status]").textContent = `${count} ${state.view} highlight${count === 1 ? "" : "s"} · ${filter} · Incan ${state.release}. ${availability}`;
    }

    function update(next) {
      state = { ...state, ...next };
      const url = new URL(window.location.href);
      url.searchParams.set("release", state.release);
      url.searchParams.set("view", state.view);
      url.searchParams.set("type", state.type);
      // Old section fragments must not scroll a filtered-out release into view.
      if (document.getElementById(url.hash.slice(1))?.closest("[data-news-version]")) url.hash = "";
      window.history.pushState(null, "", url);
      render();
    }

    journal.addEventListener("click", (event) => {
      const control = event.target.closest("[data-news-release], [data-news-view], [data-news-type], [data-news-reset]");
      if (!control || !journal.contains(control)) return;
      if (event.ctrlKey || event.metaKey || event.shiftKey || event.altKey || event.button !== 0) return;
      event.preventDefault();
      if (control.dataset.newsRelease) update({ release: control.dataset.newsRelease, type: "all" });
      else if (control.dataset.newsView) update({ view: control.dataset.newsView });
      else if (control.dataset.newsType) update({ type: control.dataset.newsType });
      else {
        update({ type: "all" });
        journal.querySelector('[data-news-type="all"]').focus();
      }
    });

    // Material's instant navigation replaces the journal element. The global
    // history handler dispatches only to the current, connected journal.
    journal.addEventListener("inc-news-history", () => {
      state = readLocation();
      render();
    });
    state = readLocation();
    journal.querySelector("[data-news-controls]").hidden = false;
    render();
  }

  function restoreSelection() {
    document.querySelector("[data-inc-news]")?.dispatchEvent(new Event("inc-news-history"));
  }
  window.addEventListener("popstate", restoreSelection);
  window.addEventListener("hashchange", restoreSelection);
  if (typeof window.document$?.subscribe === "function") window.document$.subscribe(setup);
  else if (document.readyState === "loading") document.addEventListener("DOMContentLoaded", setup);
  else setup();
})();
