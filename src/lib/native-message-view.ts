import type { NativeItem } from "./conversations";

// Presentation follows the SDK's block structure, never a text-prefix guess.
// Every extra block stays explicitly reachable. It may be user input or SDK
// background; this view does not promote either into trusted instructions.
export function nativeMessageView(item: NativeItem) {
  const raw = Array.isArray(item.content) ? item.content : [];
  const blocks = raw.filter((block) => block && typeof block === "object");
  const invalid =
    (item.content != null && !Array.isArray(item.content)) ||
    blocks.length !== raw.length ||
    (item.text != null && typeof item.text !== "string");
  const primary = typeof item.text === "string" ? item.text : undefined;
  let texts = blocks
    .filter((b) => typeof b.text === "string")
    .map((b) => b.text!);
  const original = item.homebase_input_blocks;
  if (
    Array.isArray(original) &&
    original.length > 1 &&
    original.every((t) => typeof t === "string") &&
    original.join("") === texts.join("") &&
    (primary === undefined ||
      primary === texts.join("") ||
      primary === texts.join("\n"))
  )
    texts = original;
  const structured =
    primary === undefined ||
    [texts[0], texts.join(""), texts.join("\n"), texts.join("\n\n")].includes(
      primary,
    );
  const body =
    item.type === "userMessage" && structured
      ? (texts[0] ?? primary ?? "")
      : (primary ?? texts.join("\n\n"));
  const additional =
    item.type !== "userMessage" ? [] : structured ? texts.slice(1) : texts;
  return {
    body,
    invalid,
    additional,
    nonText: blocks
      .filter((b) => typeof b.text !== "string")
      .map((b) => b.type ?? "unknown"),
  };
}

export function nativeMessageText(item: NativeItem) {
  const view = nativeMessageView(item);
  return [view.body, ...view.additional]
    .filter((part) => part.length > 0)
    .join("\n\n");
}
