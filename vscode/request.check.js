const test = require("node:test");
const assert = require("node:assert");
const { endpoint, body, answer } = require("./request");

test("only loopback, valid ports", () => {
  assert.strictEqual(endpoint(7340), "http://127.0.0.1:7340/v1/chat/completions");
  assert.throws(() => endpoint("x"));
  assert.throws(() => endpoint(70000));
});

test("body carries the code and the ask", () => {
  const b = body({ kind: "explain", code: "fn a() {}", language: "rust", fileName: "a.rs" });
  assert.match(b.messages[0].content, /Explain/);
  assert.match(b.messages[0].content, /```rust\nfn a\(\) \{\}/);
  assert.throws(() => body({ kind: "ask", question: "  ", code: "" }));
});

test("answer reads the reply or says it can't", () => {
  assert.strictEqual(answer({ choices: [{ message: { content: "hi" } }] }), "hi");
  assert.throws(() => answer({}));
});
