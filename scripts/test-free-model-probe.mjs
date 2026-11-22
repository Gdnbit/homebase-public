import { test } from "node:test";
import assert from "node:assert/strict";
import { zeroPrice, priceGuard, observation } from "./free-model-probe-validation.mjs";
test("free-only guard rejects ambiguous, paid, missing and changed pricing", () => {
  const m = { id: "fixture/model:free", pricing: { prompt: "0", completion: "0" } };
  assert.equal(zeroPrice(m), true);
  for (const other of [{ ...m, id: "fixture/paid" }, { ...m, pricing: { prompt: "0" } }, { ...m, pricing: { prompt: "0", completion: "0.1" } }, { ...m, pricing: { prompt: "0", completion: "0", request: "0.01" } }, { ...m, pricing: { prompt: "0", completion: "" } }]) assert.equal(zeroPrice(other), false);
  assert.deepEqual(priceGuard().max_price, { prompt: 0, completion: 0, request: 0, image: 0 }); assert.equal(priceGuard().allow_fallbacks, false);
});
test("empty/truncated/failed output never passes, valid vectors do", () => {
  const wrap = output => ({ ok: true, job: { id: "fixture", status: "succeeded" }, result: { status: "succeeded", output } });
  assert.equal(observation(wrap({ choices: [{ message: { content: "" } }] }), "chat").state, "empty_or_incomplete");
  assert.equal(observation(wrap({ choices: [{ finish_reason: "length", message: { content: "partial" } }] }), "chat").state, "empty_or_incomplete");
  assert.equal(observation({ ok: false, job: { status: "failed" } }, "chat").state, "failed");
  assert.equal(observation(wrap({ data: [{ embedding: [0.1, -0.1] }] }), "embeddings").state, "passed");
  assert.equal(observation(wrap({ data: [] }), "embeddings").state, "invalid_vector");
});
