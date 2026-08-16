import assert from "node:assert/strict";
import fs from "node:fs";
import vm from "node:vm";

const sourcePath = new URL("../docs/_snippets/javascripts/incan_mermaid.js", import.meta.url);
const runtimeSource = fs.readFileSync(sourcePath, "utf8");

function diagramSource(text) {
  return {
    textContent: text,
    querySelector(selector) {
      return selector === "code" ? { textContent: text } : null;
    },
    replaceWith(host) {
      this.replacement = host;
    },
  };
}

const scripts = [];
const warnings = [];
let subscribed;
let sources = [diagramSource("flowchart LR; A-->B")];

const document = {
  head: {
    appendChild(script) {
      scripts.push(script);
    },
  },
  createElement(tagName) {
    if (tagName === "script") {
      const listeners = new Map();
      return {
        addEventListener(type, listener) {
          listeners.set(type, listener);
        },
        dispatch(type) {
          listeners.get(type)?.();
        },
        remove() {
          this.removed = true;
        },
      };
    }
    return { className: "", textContent: "" };
  },
  querySelectorAll() {
    return sources.filter((source) => !source.replacement);
  },
};

const window = {
  document$: {
    subscribe(handler) {
      subscribed = handler;
    },
  },
};

const context = vm.createContext({
  console: { warn: (...args) => warnings.push(args) },
  document,
  window,
});
vm.runInContext(runtimeSource, context, { filename: sourcePath.pathname });

assert.equal(typeof subscribed, "function", "Material navigation should register the diagram initializer");

const failedLoad = subscribed();
assert.equal(sources[0].replacement, undefined, "source must remain visible until the runtime loads");
assert.equal(scripts.length, 1, "the first navigation should request the vendored runtime once");
scripts[0].dispatch("error");
await failedLoad;
assert.equal(sources[0].replacement, undefined, "a failed runtime load must preserve the source diagram");
assert.equal(scripts[0].removed, true, "a failed script element should be removed before retry");

let initializeCalls = 0;
let runCalls = 0;
const mermaid = {
  initialize() {
    initializeCalls += 1;
  },
  async run({ nodes }) {
    runCalls += 1;
    assert.ok(nodes.length > 0, "Mermaid should receive the replacement diagram nodes");
  },
};

const retry = subscribed();
assert.equal(scripts.length, 2, "a later navigation should retry a transient runtime failure");
window.mermaid = mermaid;
context.mermaid = mermaid;
scripts[1].dispatch("load");
await retry;
assert.ok(sources[0].replacement, "a successful retry should replace the source with a rendered host");
assert.equal(initializeCalls, 1, "the shared runtime should be configured once");
assert.equal(runCalls, 1, "the recovered diagram should render once");

sources = [diagramSource("sequenceDiagram; A->>B: next page")];
await subscribed();
assert.ok(sources[0].replacement, "instant navigation should render newly discovered diagrams");
assert.equal(initializeCalls, 1, "instant navigation should reuse the configured runtime");
assert.equal(runCalls, 2, "the next page should render through the existing runtime");
assert.equal(warnings.length, 1, "only the simulated transient failure should emit a warning");

console.log("Incan Mermaid runtime contract passed (failure recovery and instant navigation)");
