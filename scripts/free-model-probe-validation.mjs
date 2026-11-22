export function zeroPrice(model) {
  return (model.id?.endsWith(":free") || model.id === "openrouter/free") &&
    model.pricing?.prompt !== undefined && model.pricing?.completion !== undefined &&
    Object.values(model.pricing).length > 0 && Object.values(model.pricing).every(v => (typeof v === "number" || typeof v === "string" && v.trim() !== "") && Number.isFinite(Number(v)) && Number(v) === 0);
}
export const priceGuard = () => ({ max_price: { prompt: 0, completion: 0, request: 0, image: 0 }, allow_fallbacks: false, require_parameters: true });
export function observation(value, protocol) {
  const result = value.result ?? value.job?.result;
  if (!value.ok || value.job?.status !== "succeeded" || result?.status !== "succeeded")
    return { state: value.job?.status === "running" ? "pending_unknown" : "failed", error: value.error ?? value.job?.error ?? { message: result?.message ?? "No successful business result" }, job_id: value.job?.id };
  const output = result.output;
  const first = output?.choices?.[0];
  const content = first?.message?.content;
  const text = typeof content === "string" ? content : Array.isArray(content) ? content.map(p => p.text ?? "").join("") : "";
  const vector = output?.data?.[0]?.embedding;
  return { state: protocol === "embeddings" ? Array.isArray(vector) && vector.length > 0 && vector.every(Number.isFinite) ? "passed" : "invalid_vector" : text.trim() && first.finish_reason !== "length" ? "passed" : first?.message?.tool_calls?.length ? "tool_proposed" : "empty_or_incomplete", job_id: value.job.id, actual_model: output?.model, text: text.slice(0, 160), finish_reason: first?.finish_reason, vector_dimensions: Array.isArray(vector) ? vector.length : undefined, reported_cost: output?.usage?.cost ?? null, usage: output?.usage ? { prompt_tokens: output.usage.prompt_tokens, completion_tokens: output.usage.completion_tokens, total_tokens: output.usage.total_tokens } : null, calls: first?.message?.tool_calls?.map(c => ({ id: c.id, name: c.function?.name, arguments: c.function?.arguments })) ?? [] };
}
