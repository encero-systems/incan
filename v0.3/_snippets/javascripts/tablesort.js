// Tablesort initialization (MkDocs Material)
// Applies sorting to the RFC index table.
(function () {
  function init() {
    if (typeof window.Tablesort === "undefined") return;

    const tables = document.querySelectorAll('[data-rfc-table="rfcs-index"] table');
    tables.forEach((table) => {
      // Avoid double-initialization across instant navigation
      if (table.dataset.tablesortBound === "true") return;
      table.dataset.tablesortBound = "true";
      new window.Tablesort(table);
    });
  }

  if (typeof window.document$ !== "undefined" && typeof window.document$.subscribe === "function") {
    window.document$.subscribe(init);
  } else {
    document.addEventListener("DOMContentLoaded", init);
  }
})();
